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
    pub temp_result_ip: HashMap<String, AuthRateLimitBucket>,
}

pub struct AppState {
    pub pool: SqlitePool,
    pub data_root: PathBuf,
    pub limits: AppLimits,
    pub auth: AuthConfig,
    pub processing_permits: Arc<Semaphore>,
    pub receive_permits: Arc<Semaphore>,
    pub tmp_bytes: Arc<AtomicU64>,
    pub temp_result_permits: Arc<Semaphore>,
    pub temp_result_capacity_lock: Arc<AsyncMutex<()>>,
    pub auth_hash_permits: Arc<Semaphore>,
    pub auth_rate_limits: Arc<Mutex<AuthRateLimits>>,
    pub blob_store: Arc<dyn BlobStore>,
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
            pool,
            data_root,
            limits,
            auth,
            processing_permits,
            receive_permits,
            tmp_bytes: Arc::new(AtomicU64::new(0)),
            temp_result_permits,
            temp_result_capacity_lock: Arc::new(AsyncMutex::new(())),
            auth_hash_permits,
            auth_rate_limits: Arc::new(Mutex::new(AuthRateLimits::default())),
            blob_store,
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

        assert_eq!(state.processing_permits.available_permits(), 7);
    }
}
pub mod blob_store;
