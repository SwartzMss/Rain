pub mod config;
pub mod db;
pub mod error;
pub mod file_classification;
pub mod ingest;
pub mod log_expression;
pub mod models;
pub mod routes;

use std::path::PathBuf;

use sqlx::SqlitePool;

pub struct AppState {
    pub pool: SqlitePool,
    pub data_root: PathBuf,
}
