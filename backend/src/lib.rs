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

use std::{path::PathBuf, sync::Arc};

use sqlx::SqlitePool;
use tokio::sync::Semaphore;

use crate::blob_store::{BlobStore, LocalCasBlobStore};
use crate::config::AppLimits;

pub struct AppState {
    pub pool: SqlitePool,
    pub data_root: PathBuf,
    pub limits: AppLimits,
    pub processing_permits: Arc<Semaphore>,
    pub blob_store: Arc<dyn BlobStore>,
}

impl AppState {
    pub fn new(pool: SqlitePool, data_root: PathBuf, limits: AppLimits) -> Self {
        let blob_store = Arc::new(LocalCasBlobStore::new(data_root.clone()));
        Self::with_blob_store(pool, data_root, limits, blob_store)
    }

    pub fn with_blob_store(
        pool: SqlitePool,
        data_root: PathBuf,
        limits: AppLimits,
        blob_store: Arc<dyn BlobStore>,
    ) -> Self {
        let processing_permits =
            Arc::new(Semaphore::new(limits.upload.concurrent_processing_tasks));
        Self {
            pool,
            data_root,
            limits,
            processing_permits,
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
