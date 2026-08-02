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

use std::{
    collections::{HashMap, HashSet, VecDeque},
    future::Future,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use sqlx::SqlitePool;
use tokio::sync::{Mutex as AsyncMutex, Semaphore};

use crate::blob_store::{BlobStore, LocalCasBlobStore};
use crate::config::{AppLimits, AuthConfig};

pub struct AuthRateLimitBucket {
    window: Duration,
    events: VecDeque<Instant>,
}

impl AuthRateLimitBucket {
    pub fn new(window: Duration) -> Self {
        Self {
            window,
            events: VecDeque::new(),
        }
    }

    pub fn prune(&mut self, now: Instant) {
        while self
            .events
            .front()
            .is_some_and(|timestamp| now.duration_since(*timestamp) >= self.window)
        {
            self.events.pop_front();
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
    pub hash_permits: Arc<Semaphore>,
    pub rate_limits: Arc<Mutex<AuthRateLimits>>,
}

impl AuthRuntime {
    pub fn new(config: AuthConfig) -> Self {
        let allow_registration = config.allow_registration;
        Self {
            hash_permits: Arc::new(Semaphore::new(config.argon2_concurrency)),
            config,
            allow_registration: AtomicBool::new(allow_registration),
            rate_limits: Arc::new(Mutex::new(AuthRateLimits::default())),
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
    }
}
pub mod blob_store;
