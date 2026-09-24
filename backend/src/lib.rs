pub mod auth;
pub mod config;
pub mod db;
pub mod error;
pub mod file_classification;
pub mod ingest;
pub mod log_expression;
pub mod models;
pub mod repositories;
pub mod routes;
pub mod search;
pub mod services;
pub mod settings;
pub mod upload;

use chrono::{DateTime, Utc};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    future::Future,
    path::PathBuf,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use sqlx::SqlitePool;
use tokio::sync::OwnedSemaphorePermit;
use tokio::sync::{Mutex as AsyncMutex, Semaphore};

use crate::blob_store::{BlobStore, LocalCasBlobStore};
use crate::config::{AppLimits, AuthConfig};
use crate::error::AppError;
use crate::search::resource::SearchResourceBudget;
use crate::services::issue_cleanup_policy::IssueCleanupPolicy;
use crate::settings::SettingsService;

#[derive(Debug, Clone)]
pub struct RequestLogId(pub String);

pub struct AuthRateLimitBucket {
    window: Duration,
    pub events: VecDeque<Instant>,
    pub event_times: VecDeque<DateTime<Utc>>,
}

impl AuthRateLimitBucket {
    pub fn new(window: Duration) -> Self {
        Self {
            window,
            events: VecDeque::new(),
            event_times: VecDeque::new(),
        }
    }

    pub fn prune(&mut self, now: Instant) {
        while self
            .events
            .front()
            .is_some_and(|timestamp| now.duration_since(*timestamp) >= self.window)
        {
            self.events.pop_front();
            self.event_times.pop_front();
        }
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn push(&mut self, timestamp: Instant) {
        self.events.push_back(timestamp);
        self.event_times.push_back(Utc::now());
    }

    pub fn set_window(&mut self, window: Duration) {
        self.window = window;
    }
}

#[derive(Default)]
pub struct AuthRateLimits {
    pub login_ip: HashMap<String, AuthRateLimitBucket>,
    pub login_username_failure: HashMap<String, AuthRateLimitBucket>,
    pub register_ip: HashMap<String, AuthRateLimitBucket>,
    pub change_password_user_attempt: HashMap<String, AuthRateLimitBucket>,
    pub change_password_in_flight: HashSet<String>,
}

pub struct DatabaseContext {
    pub pool: SqlitePool,
}

pub struct StorageContext {
    pub data_root: PathBuf,
    pub blob_store: Arc<dyn BlobStore>,
}

pub struct UploadRuntime {
    pub processing_permits: Arc<Semaphore>,
    pub receive_permits: Arc<Semaphore>,
    pub tmp_bytes: Arc<AtomicU64>,
    pub tmp_max_bytes: Arc<AtomicU64>,
    pub temp_cleanup_queue: crate::upload::job::TempCleanupQueue,
    pub session_locks: Arc<Mutex<HashMap<String, Arc<AsyncMutex<()>>>>>,
}

pub struct SearchRuntime {
    pub tantivy_budget: SearchResourceBudget,
    pub query_permits: Arc<Semaphore>,
    pub(crate) generation_leases: crate::search::generation_lease::GenerationLeaseRegistry,
}

impl SearchRuntime {
    pub fn new(max_writers: usize, writer_heap_size_bytes: u64) -> Self {
        Self {
            tantivy_budget: SearchResourceBudget::new(max_writers, writer_heap_size_bytes)
                .expect("validated Tantivy resource budget"),
            query_permits: Arc::new(Semaphore::new(
                crate::search::resource::MAX_CONCURRENT_TANTIVY_QUERIES,
            )),
            generation_leases: crate::search::generation_lease::GenerationLeaseRegistry::shared(),
        }
    }
}

impl UploadRuntime {
    pub fn new(processing: usize, receiving: usize) -> Self {
        Self {
            processing_permits: Arc::new(Semaphore::new(processing)),
            receive_permits: Arc::new(Semaphore::new(receiving)),
            tmp_bytes: Arc::new(AtomicU64::new(0)),
            tmp_max_bytes: Arc::new(AtomicU64::new(u64::MAX)),
            temp_cleanup_queue: crate::upload::job::TempCleanupQueue::default(),
            session_locks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn session_lock(&self, session_id: &str) -> Arc<AsyncMutex<()>> {
        let mut locks = self
            .session_locks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        locks
            .entry(session_id.to_owned())
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone()
    }
}

pub struct TempResultRuntime {
    pub permits: Arc<Semaphore>,
    pub capacity_lock: Arc<AsyncMutex<()>>,
    pub staging: Arc<Mutex<HashSet<String>>>,
    pub reads: Arc<Mutex<HashMap<String, usize>>>,
    pub materializations: Arc<Mutex<HashMap<String, usize>>>,
    pub ip_limits: Arc<Mutex<HashMap<String, AuthRateLimitBucket>>>,
}

impl TempResultRuntime {
    pub fn new(materializations: usize) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(materializations)),
            capacity_lock: Arc::new(AsyncMutex::new(())),
            staging: Arc::new(Mutex::new(HashSet::new())),
            reads: Arc::new(Mutex::new(HashMap::new())),
            materializations: Arc::new(Mutex::new(HashMap::new())),
            ip_limits: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

pub struct AuthRuntime {
    pub config: AuthConfig,
    pub session_ttl_seconds: AtomicU64,
    pub register_ip_limit_per_hour: AtomicUsize,
    pub allow_registration: AtomicBool,
    pub login_ip_limit_per_minute: AtomicUsize,
    pub login_username_failure_limit_per_5_minutes: AtomicUsize,
    pub registration_settings_lock: Arc<AsyncMutex<()>>,
    pub hash_permits: Arc<Semaphore>,
    pub rate_limits: Arc<Mutex<AuthRateLimits>>,
    pub admin_username_normalized: Arc<OnceLock<String>>,
}

#[derive(Default)]
pub struct RecoveryRuntime {
    stale_processing_bundles_ready: AtomicBool,
}

impl RecoveryRuntime {
    pub fn ready() -> Self {
        Self {
            stale_processing_bundles_ready: AtomicBool::new(true),
        }
    }

    pub fn invariant_recovery_ready(&self) -> bool {
        self.stale_processing_bundles_ready.load(Ordering::Acquire)
    }

    pub fn stale_processing_bundles_ready(&self) -> bool {
        self.stale_processing_bundles_ready.load(Ordering::Acquire)
    }

    pub fn mark_stale_processing_bundles_ready(&self) {
        self.stale_processing_bundles_ready
            .store(true, Ordering::Release);
    }
}

impl AuthRuntime {
    pub fn new(config: AuthConfig) -> Self {
        let allow_registration = config.allow_registration;
        let ip_limit = config.login_ip_limit_per_minute;
        let username_limit = config.login_username_failure_limit_per_5_minutes;
        Self {
            hash_permits: Arc::new(Semaphore::new(config.argon2_concurrency)),
            session_ttl_seconds: AtomicU64::new(config.session_ttl_seconds),
            register_ip_limit_per_hour: AtomicUsize::new(config.register_ip_limit_per_hour),
            config,
            allow_registration: AtomicBool::new(allow_registration),
            login_ip_limit_per_minute: AtomicUsize::new(ip_limit),
            login_username_failure_limit_per_5_minutes: AtomicUsize::new(username_limit),
            registration_settings_lock: Arc::new(AsyncMutex::new(())),
            rate_limits: Arc::new(Mutex::new(AuthRateLimits::default())),
            admin_username_normalized: Arc::new(OnceLock::new()),
        }
    }

    pub fn registration_allowed(&self) -> bool {
        self.allow_registration.load(Ordering::Acquire)
    }
    pub fn set_registration_allowed(&self, value: bool) {
        self.allow_registration.store(value, Ordering::Release);
    }
}

#[derive(Clone, Copy)]
pub(crate) struct ReadinessSnapshot {
    pub(crate) checked_at: Instant,
    pub(crate) database_ok: bool,
    pub(crate) storage_ok: bool,
}

#[derive(Default)]
pub(crate) struct ReadinessCache(pub(crate) AsyncMutex<Option<ReadinessSnapshot>>);

pub struct AppState {
    pub db: DatabaseContext,
    pub storage: StorageContext,
    pub upload: UploadRuntime,
    pub search: SearchRuntime,
    pub temp_results: TempResultRuntime,
    pub line_read_permits: Arc<Semaphore>,
    pub line_read_per_client: AtomicUsize,
    pub line_read_clients: Arc<Mutex<HashMap<String, usize>>>,
    pub auth_runtime: AuthRuntime,
    pub recovery: Arc<RecoveryRuntime>,
    pub(crate) readiness_cache: ReadinessCache,
    pub issue_inactive_days: AtomicUsize,
    pub issue_cleanup_policy: Arc<IssueCleanupPolicy>,
    issue_cleanup_policy_override: Arc<Mutex<Option<IssueCleanupPolicy>>>,
    pub settings: SettingsService,
    pub limits: AppLimits,
    pub search_backend: crate::search::publication::SearchBackendKind,
}

const MAX_LINE_READ_CLIENTS: usize = 1024;

/// Start a resilient Tokio periodic job. A failed iteration is logged and does
/// not terminate the worker, so unrelated maintenance jobs keep running.
pub fn spawn_periodic_job<F, Fut>(
    name: &'static str,
    initial_delay: Duration,
    interval_duration: Duration,
    mut job: F,
) -> tokio::task::JoinHandle<()>
where
    F: FnMut() -> Fut + Send + 'static,
    Fut: Future<Output = Result<(), String>> + Send + 'static,
{
    tokio::spawn(async move {
        tokio::time::sleep(initial_delay).await;
        let mut interval = tokio::time::interval(interval_duration);
        loop {
            interval.tick().await;
            let started = Instant::now();
            match job().await {
                Ok(()) => tracing::debug!(
                    job = name,
                    elapsed_ms = started.elapsed().as_millis(),
                    "periodic job completed"
                ),
                Err(error) => {
                    tracing::warn!(job = name, elapsed_ms = started.elapsed().as_millis(), %error, "periodic job failed; will retry")
                }
            }
        }
    })
}

impl AppState {
    pub fn new(pool: SqlitePool, data_root: PathBuf, limits: AppLimits) -> Self {
        let blob_store = Arc::new(LocalCasBlobStore::new(data_root.clone()));
        Self::with_blob_store_and_auth(pool, data_root, limits, AuthConfig::default(), blob_store)
    }

    pub fn with_blob_store(
        pool: SqlitePool,
        data_root: PathBuf,
        limits: AppLimits,
        blob_store: Arc<dyn BlobStore>,
    ) -> Self {
        Self::with_blob_store_and_auth(pool, data_root, limits, AuthConfig::default(), blob_store)
    }

    pub fn with_blob_store_and_auth(
        pool: SqlitePool,
        data_root: PathBuf,
        limits: AppLimits,
        auth: AuthConfig,
        blob_store: Arc<dyn BlobStore>,
    ) -> Self {
        Self::with_blob_store_auth(pool, data_root, limits, auth, blob_store)
    }

    fn with_blob_store_auth(
        pool: SqlitePool,
        data_root: PathBuf,
        limits: AppLimits,
        auth: AuthConfig,
        blob_store: Arc<dyn BlobStore>,
    ) -> Self {
        let upload = UploadRuntime::new(
            limits.upload.concurrent_processing_tasks,
            limits.upload.concurrent_receive_tasks,
        );
        upload
            .tmp_max_bytes
            .store(limits.upload.max_tmp_bytes, Ordering::Release);
        let search = SearchRuntime::new(
            limits.search.tantivy_max_writers,
            limits.search.tantivy_writer_heap_size,
        );
        let temp_results = TempResultRuntime::new(limits.temp_results.concurrent_materializations);
        let line_read_permits = Arc::new(Semaphore::new(limits.api.concurrent_line_reads));
        let line_read_clients = Arc::new(Mutex::new(HashMap::new()));
        let settings = SettingsService::new_with_config(pool.clone(), &limits, &auth);
        let auth_runtime = AuthRuntime::new(auth);
        Self {
            db: DatabaseContext { pool },
            storage: StorageContext {
                data_root,
                blob_store,
            },
            upload,
            search,
            temp_results,
            line_read_permits,
            line_read_per_client: AtomicUsize::new(limits.api.concurrent_line_reads_per_client),
            line_read_clients,
            auth_runtime,
            recovery: Arc::new(RecoveryRuntime::ready()),
            readiness_cache: ReadinessCache::default(),
            issue_inactive_days: AtomicUsize::new(0),
            issue_cleanup_policy: Arc::new(IssueCleanupPolicy::default()),
            issue_cleanup_policy_override: Arc::new(Mutex::new(None)),
            settings,
            limits,
            search_backend: crate::search::publication::SearchBackendKind::SqliteFts,
        }
    }

    pub fn acquire_line_read(&self, client_key: &str) -> Result<LineReadLease, AppError> {
        let permit = self
            .line_read_permits
            .clone()
            .try_acquire_owned()
            .map_err(|_| {
                AppError::api(
                    actix_web::http::StatusCode::TOO_MANY_REQUESTS,
                    "LINE_READ_BUSY",
                    "行读取任务过多，请稍后重试",
                )
            })?;
        let mut clients = self.line_read_clients.lock().map_err(|_| {
            AppError::api(
                actix_web::http::StatusCode::SERVICE_UNAVAILABLE,
                "LINE_READ_UNAVAILABLE",
                "行读取服务暂时不可用",
            )
        })?;
        if !clients.contains_key(client_key) && clients.len() >= MAX_LINE_READ_CLIENTS {
            drop(permit);
            return Err(AppError::api(
                actix_web::http::StatusCode::TOO_MANY_REQUESTS,
                "LINE_READ_RATE_LIMITED",
                "行读取客户端数量超过限制，请稍后重试",
            ));
        }
        let count = clients.entry(client_key.to_owned()).or_insert(0);
        if *count >= self.line_read_per_client.load(Ordering::Acquire) {
            drop(permit);
            return Err(AppError::api(
                actix_web::http::StatusCode::TOO_MANY_REQUESTS,
                "LINE_READ_RATE_LIMITED",
                "单个客户端的行读取任务过多，请稍后重试",
            ));
        }
        *count = count.saturating_add(1);
        Ok(LineReadLease {
            client_key: client_key.to_owned(),
            clients: self.line_read_clients.clone(),
            _permit: permit,
        })
    }

    pub fn current_cleanup_policy(&self) -> IssueCleanupPolicy {
        self.issue_cleanup_policy_override
            .lock()
            .ok()
            .and_then(|policy| policy.clone())
            .unwrap_or_else(|| (*self.issue_cleanup_policy).clone())
    }

    pub fn set_cleanup_policy(&self, policy: IssueCleanupPolicy) {
        if let Ok(mut current) = self.issue_cleanup_policy_override.lock() {
            *current = Some(policy);
        }
    }
}

pub struct LineReadLease {
    client_key: String,
    clients: Arc<Mutex<HashMap<String, usize>>>,
    _permit: OwnedSemaphorePermit,
}

impl Drop for LineReadLease {
    fn drop(&mut self) {
        if let Ok(mut clients) = self.clients.lock()
            && let Some(count) = clients.get_mut(&self.client_key)
        {
            *count = count.saturating_sub(1);
            if *count == 0 {
                clients.remove(&self.client_key);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use std::path::PathBuf;

    use sqlx::sqlite::SqlitePoolOptions;

    use crate::config::AppLimits;

    use super::{AppState, RecoveryRuntime};

    #[tokio::test]
    async fn state_uses_configured_processing_concurrency() {
        let pool = SqlitePoolOptions::new()
            .connect_lazy("sqlite::memory:")
            .unwrap();
        let mut limits = AppLimits::default();
        limits.upload.concurrent_processing_tasks = 7;

        let state = AppState::new(pool, PathBuf::from("data"), limits);

        assert_eq!(state.upload.processing_permits.available_permits(), 7);
    }

    #[tokio::test]
    async fn state_uses_configured_tantivy_writer_admission() {
        let pool = SqlitePoolOptions::new()
            .connect_lazy("sqlite::memory:")
            .unwrap();
        let mut limits = AppLimits::default();
        limits.search.tantivy_max_writers = 2;
        limits.search.tantivy_writer_heap_size = 8 * 1024 * 1024;

        let state = AppState::new(pool, PathBuf::from("data"), limits);

        assert_eq!(state.search.tantivy_budget.available_writers(), 2);
        assert_eq!(
            state.search.tantivy_budget.writer_heap_size_bytes(),
            8 * 1024 * 1024
        );
    }

    #[tokio::test]
    async fn state_limits_concurrent_tantivy_queries() {
        let limits = AppLimits::default();
        let pool = sqlx::SqlitePool::connect_lazy("sqlite::memory:").unwrap();
        let state = AppState::new(pool, std::env::temp_dir(), limits);
        let permits = state.search.query_permits.clone();
        let mut held = Vec::new();
        for _ in 0..crate::search::resource::MAX_CONCURRENT_TANTIVY_QUERIES {
            held.push(permits.clone().acquire_owned().await.unwrap());
        }
        let blocked =
            tokio::time::timeout(Duration::from_millis(25), permits.clone().acquire_owned()).await;
        assert!(blocked.is_err(), "query admission must be globally bounded");
        drop(held);
        assert!(
            tokio::time::timeout(Duration::from_secs(1), permits.acquire_owned())
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn line_read_leases_are_bounded_per_client_and_released() {
        let pool = SqlitePoolOptions::new()
            .connect_lazy("sqlite::memory:")
            .unwrap();
        let mut limits = AppLimits::default();
        limits.api.concurrent_line_reads = 2;
        limits.api.concurrent_line_reads_per_client = 1;
        let state = AppState::new(pool, PathBuf::from("data"), limits);

        let first = state.acquire_line_read("client-a").unwrap();
        assert!(state.acquire_line_read("client-a").is_err());
        let second = state.acquire_line_read("client-b").unwrap();
        assert!(state.acquire_line_read("client-c").is_err());

        drop(first);
        let replacement = state.acquire_line_read("client-c").unwrap();
        drop(second);
        drop(replacement);
        assert!(state.line_read_clients.lock().unwrap().is_empty());
    }

    #[test]
    fn domain_runtimes_can_be_constructed_independently() {
        let upload = super::UploadRuntime::new(3, 2);
        assert_eq!(upload.processing_permits.available_permits(), 3);
        assert_eq!(upload.receive_permits.available_permits(), 2);

        let temp_results = super::TempResultRuntime::new(4);
        assert_eq!(temp_results.permits.available_permits(), 4);
        assert!(temp_results.ip_limits.lock().unwrap().is_empty());

        let auth = super::AuthRuntime::new(crate::config::AuthConfig::default());
        assert_eq!(
            auth.hash_permits.available_permits(),
            crate::config::AuthConfig::default().argon2_concurrency
        );
    }

    #[test]
    fn recovery_runtime_requires_both_invariants() {
        let recovery = RecoveryRuntime::default();
        assert!(!recovery.invariant_recovery_ready());

        recovery.mark_stale_processing_bundles_ready();
        assert!(recovery.stale_processing_bundles_ready());
        assert!(recovery.invariant_recovery_ready());
    }
}
pub mod blob_store;
