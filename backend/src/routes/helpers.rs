use sqlx::FromRow;

use crate::{AppState, error::AppError};

#[derive(FromRow)]
pub struct BundleRow {
    pub id: String,
    pub hash: String,
    pub name: String,
    pub status: String,
}

pub async fn load_bundle(pool: &sqlx::SqlitePool, hash: &str) -> Result<BundleRow, AppError> {
    sqlx::query_as::<_, BundleRow>(
        "SELECT id, hash, name, status FROM bundles WHERE hash = ? LIMIT 1",
    )
    .bind(hash)
    .fetch_optional(pool)
    .await
    .map_err(AppError::Database)?
    .ok_or_else(|| AppError::NotFound(format!("bundle {hash}")))
}

pub fn ensure_bundle_ready(bundle: &BundleRow) -> Result<(), AppError> {
    if !bundle.status.eq_ignore_ascii_case("READY") {
        return Err(AppError::Conflict(
            "bundle is not ready; please wait for processing to finish".into(),
        ));
    }
    Ok(())
}

pub fn data_root(state: &actix_web::web::Data<AppState>) -> std::path::PathBuf {
    state.data_root.clone()
}
