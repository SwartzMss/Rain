pub mod ai_provider;
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
pub mod services;
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
use tokio::sync::{Mutex as AsyncMutex, Semaphore, broadcast};
use tokio_util::sync::CancellationToken;

use crate::blob_store::{BlobStore, LocalCasBlobStore};
use crate::config::{AiProviderEnv, AppLimits, AuthConfig};

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
    pub temp_cleanup_queue: crate::upload::job::TempCleanupQueue,
}

impl UploadRuntime {
    pub fn new(processing: usize, receiving: usize) -> Self {
        Self {
            processing_permits: Arc::new(Semaphore::new(processing)),
            receive_permits: Arc::new(Semaphore::new(receiving)),
            tmp_bytes: Arc::new(AtomicU64::new(0)),
            temp_cleanup_queue: crate::upload::job::TempCleanupQueue::default(),
        }
    }
}

pub struct TempResultRuntime {
    pub permits: Arc<Semaphore>,
    pub capacity_lock: Arc<AsyncMutex<()>>,
    pub staging: Arc<Mutex<HashSet<String>>>,
    pub ip_limits: Arc<Mutex<HashMap<String, AuthRateLimitBucket>>>,
}

impl TempResultRuntime {
    pub fn new(materializations: usize) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(materializations)),
            capacity_lock: Arc::new(AsyncMutex::new(())),
            staging: Arc::new(Mutex::new(HashSet::new())),
            ip_limits: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

pub struct AuthRuntime {
    pub config: AuthConfig,
    pub allow_registration: AtomicBool,
    pub login_ip_limit_per_minute: AtomicUsize,
    pub login_username_failure_limit_per_5_minutes: AtomicUsize,
    pub registration_settings_lock: Arc<AsyncMutex<()>>,
    pub hash_permits: Arc<Semaphore>,
    pub rate_limits: Arc<Mutex<AuthRateLimits>>,
    pub admin_username_normalized: Arc<OnceLock<String>>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SkillRunEvent {
    pub event: String,
    pub data: serde_json::Value,
}

#[derive(Clone)]
struct SkillRunHandle {
    cancellation: CancellationToken,
    events: broadcast::Sender<SkillRunEvent>,
}

#[derive(Default)]
pub struct SkillRunRuntime {
    runs: Mutex<HashMap<String, SkillRunHandle>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillReviewAdmissionError {
    AlreadyRunning,
    RateLimited,
}

#[derive(Debug, Default)]
struct SkillReviewUserState {
    in_flight: HashSet<String>,
    attempts: HashMap<String, VecDeque<Instant>>,
}

pub struct SkillReviewRuntime {
    pub permits: Arc<Semaphore>,
    users: Arc<Mutex<SkillReviewUserState>>,
    per_user_limit: usize,
    window: Duration,
}

#[derive(Debug)]
pub struct SkillReviewGuard {
    user_id: String,
    users: Arc<Mutex<SkillReviewUserState>>,
}

impl Drop for SkillReviewGuard {
    fn drop(&mut self) {
        if let Ok(mut users) = self.users.lock() {
            users.in_flight.remove(&self.user_id);
        }
    }
}

impl SkillReviewRuntime {
    pub fn new(global_concurrency: usize, per_user_limit: usize, window: Duration) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(global_concurrency)),
            users: Arc::new(Mutex::new(SkillReviewUserState::default())),
            per_user_limit,
            window,
        }
    }

    pub fn admit(
        &self,
        user_id: &str,
        now: Instant,
    ) -> Result<SkillReviewGuard, SkillReviewAdmissionError> {
        let mut users = self
            .users
            .lock()
            .map_err(|_| SkillReviewAdmissionError::RateLimited)?;
        if users.in_flight.contains(user_id) {
            return Err(SkillReviewAdmissionError::AlreadyRunning);
        }
        for attempts in users.attempts.values_mut() {
            while attempts
                .front()
                .is_some_and(|timestamp| now.duration_since(*timestamp) >= self.window)
            {
                attempts.pop_front();
            }
        }
        users.attempts.retain(|_, attempts| !attempts.is_empty());
        let attempts = users.attempts.entry(user_id.to_owned()).or_default();
        if attempts.len() >= self.per_user_limit {
            return Err(SkillReviewAdmissionError::RateLimited);
        }
        attempts.push_back(now);
        users.in_flight.insert(user_id.to_owned());
        Ok(SkillReviewGuard {
            user_id: user_id.to_owned(),
            users: self.users.clone(),
        })
    }
}

impl SkillRunRuntime {
    pub fn register(
        &self,
        run_id: &str,
    ) -> (CancellationToken, broadcast::Receiver<SkillRunEvent>) {
        let (events, receiver) = broadcast::channel(64);
        let cancellation = CancellationToken::new();
        self.runs.lock().expect("Skill run runtime lock").insert(
            run_id.to_owned(),
            SkillRunHandle {
                cancellation: cancellation.clone(),
                events,
            },
        );
        (cancellation, receiver)
    }

    pub fn subscribe(&self, run_id: &str) -> Option<broadcast::Receiver<SkillRunEvent>> {
        self.runs
            .lock()
            .ok()?
            .get(run_id)
            .map(|handle| handle.events.subscribe())
    }

    pub fn cancel(&self, run_id: &str) {
        if let Ok(runs) = self.runs.lock()
            && let Some(handle) = runs.get(run_id)
        {
            handle.cancellation.cancel();
        }
    }

    pub fn emit(&self, run_id: &str, event: SkillRunEvent) {
        if let Ok(runs) = self.runs.lock()
            && let Some(handle) = runs.get(run_id)
        {
            let _ = handle.events.send(event);
        }
    }

    pub fn remove(&self, run_id: &str) {
        if let Ok(mut runs) = self.runs.lock() {
            runs.remove(run_id);
        }
    }
}

impl AuthRuntime {
    pub fn new(config: AuthConfig) -> Self {
        let allow_registration = config.allow_registration;
        let ip_limit = config.login_ip_limit_per_minute;
        let username_limit = config.login_username_failure_limit_per_5_minutes;
        Self {
            hash_permits: Arc::new(Semaphore::new(config.argon2_concurrency)),
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

pub struct AppState {
    pub db: DatabaseContext,
    pub storage: StorageContext,
    pub upload: UploadRuntime,
    pub temp_results: TempResultRuntime,
    pub auth_runtime: AuthRuntime,
    pub ai_provider: AiProviderEnv,
    pub skill_runs: SkillRunRuntime,
    pub skill_reviews: SkillReviewRuntime,
    pub issue_inactive_days: AtomicUsize,
    pub limits: AppLimits,
}

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

    pub fn new_with_ai(
        pool: SqlitePool,
        data_root: PathBuf,
        limits: AppLimits,
        ai_provider: AiProviderEnv,
    ) -> Self {
        let blob_store = Arc::new(LocalCasBlobStore::new(data_root.clone()));
        Self::with_blob_store_auth_and_ai(
            pool,
            data_root,
            limits,
            AuthConfig::default(),
            ai_provider,
            blob_store,
        )
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
        Self::with_blob_store_auth_and_ai(
            pool,
            data_root,
            limits,
            auth,
            AiProviderEnv::default(),
            blob_store,
        )
    }

    pub fn with_blob_store_auth_and_ai(
        pool: SqlitePool,
        data_root: PathBuf,
        limits: AppLimits,
        auth: AuthConfig,
        ai_provider: AiProviderEnv,
        blob_store: Arc<dyn BlobStore>,
    ) -> Self {
        let upload = UploadRuntime::new(
            limits.upload.concurrent_processing_tasks,
            limits.upload.concurrent_receive_tasks,
        );
        let temp_results = TempResultRuntime::new(limits.temp_results.concurrent_materializations);
        let auth_runtime = AuthRuntime::new(auth);
        Self {
            db: DatabaseContext { pool },
            storage: StorageContext {
                data_root,
                blob_store,
            },
            upload,
            temp_results,
            auth_runtime,
            ai_provider,
            skill_runs: SkillRunRuntime::default(),
            skill_reviews: SkillReviewRuntime::new(2, 5, Duration::from_secs(60 * 60)),
            issue_inactive_days: AtomicUsize::new(0),
            limits,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use sqlx::sqlite::SqlitePoolOptions;

    use crate::config::AppLimits;

    use super::AppState;

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

        let reviews = super::SkillReviewRuntime::new(2, 2, std::time::Duration::from_secs(60));
        let first = reviews.admit("user", std::time::Instant::now()).unwrap();
        assert_eq!(
            reviews
                .admit("user", std::time::Instant::now())
                .unwrap_err(),
            super::SkillReviewAdmissionError::AlreadyRunning
        );
        drop(first);
        drop(reviews.admit("user", std::time::Instant::now()).unwrap());
        assert_eq!(
            reviews
                .admit("user", std::time::Instant::now())
                .unwrap_err(),
            super::SkillReviewAdmissionError::RateLimited
        );
    }
}
pub mod blob_store;
