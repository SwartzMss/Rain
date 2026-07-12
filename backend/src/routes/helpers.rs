use sqlx::FromRow;

use crate::{AppState, error::AppError};

#[derive(FromRow)]
pub struct BundleRow {
    pub id: String,
    pub hash: String,
    pub name: String,
}

pub async fn load_bundle(pool: &sqlx::SqlitePool, hash: &str) -> Result<BundleRow, AppError> {
    sqlx::query_as::<_, BundleRow>("SELECT id, hash, name FROM bundles WHERE hash = ? LIMIT 1")
        .bind(hash)
        .fetch_optional(pool)
        .await
        .map_err(AppError::Database)?
        .ok_or_else(|| AppError::NotFound(format!("bundle {hash}")))
}

pub fn data_root(state: &actix_web::web::Data<AppState>) -> std::path::PathBuf {
    state.data_root.clone()
}
