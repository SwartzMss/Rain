use sqlx::FromRow;
use uuid::Uuid;

use crate::{AppState, error::AppError};

#[derive(FromRow)]
pub struct BundleRow {
    pub id: Uuid,
    pub hash: String,
    pub name: String,
}

pub async fn load_bundle(pool: &sqlx::PgPool, hash: &str) -> Result<BundleRow, AppError> {
    sqlx::query_as::<_, BundleRow>("SELECT id, hash, name FROM bundles WHERE hash = $1 LIMIT 1")
        .bind(hash)
        .fetch_optional(pool)
        .await
        .map_err(AppError::Database)?
        .ok_or_else(|| AppError::NotFound(format!("bundle {hash}")))
}

pub fn data_root(state: &actix_web::web::Data<AppState>) -> std::path::PathBuf {
    state.data_root.clone()
}
