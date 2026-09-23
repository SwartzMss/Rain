use std::collections::HashSet;

use sqlx::SqlitePool;

use crate::error::AppError;

/// Read the file IDs visible at one point in time. `visible_files` is a
/// recursive view, so directory deletion semantics stay in one place.
pub async fn snapshot_file_ids(
    pool: &SqlitePool,
    bundle_id: &str,
) -> Result<HashSet<i64>, AppError> {
    let rows: Vec<(i64,)> = sqlx::query_as("SELECT id FROM visible_files WHERE bundle_id = ?")
        .bind(bundle_id)
        .fetch_all(pool)
        .await
        .map_err(AppError::Database)?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}
