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
    sync::{Arc, Mutex, atomic::AtomicU64},
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

pub struct TempResultRuntime {
    pub permits: Arc<Semaphore>,
    pub capacity_lock: Arc<AsyncMutex<()>>,
    pub staging: Arc<Mutex<HashSet<String>>>,
    pub ip_limits: Arc<Mutex<HashMap<String, AuthRateLimitBucket>>>,
}

pub struct AuthRuntime {
    pub config: AuthConfig,
    pub hash_permits: Arc<Semaphore>,
    pub rate_limits: Arc<Mutex<AuthRateLimits>>,
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
) where
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
    });
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
        let processing_permits =
            Arc::new(Semaphore::new(limits.upload.concurrent_processing_tasks));
        let receive_permits = Arc::new(Semaphore::new(limits.upload.concurrent_receive_tasks));
        let temp_result_permits = Arc::new(Semaphore::new(
            limits.temp_results.concurrent_materializations,
        ));
        let auth_hash_permits = Arc::new(Semaphore::new(auth.argon2_concurrency));
        Self {
            db: DatabaseContext { pool },
            storage: StorageContext {
                data_root,
                blob_store,
            },
            upload: UploadRuntime {
                processing_permits,
                receive_permits,
                tmp_bytes: Arc::new(AtomicU64::new(0)),
                temp_cleanup_queue: crate::upload::job::TempCleanupQueue::default(),
            },
            temp_results: TempResultRuntime {
                permits: temp_result_permits,
                capacity_lock: Arc::new(AsyncMutex::new(())),
                staging: Arc::new(Mutex::new(HashSet::new())),
                ip_limits: Arc::new(Mutex::new(HashMap::new())),
            },
            auth_runtime: AuthRuntime {
                config: auth,
                hash_permits: auth_hash_permits,
                rate_limits: Arc::new(Mutex::new(AuthRateLimits::default())),
            },
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
}
pub mod blob_store;
