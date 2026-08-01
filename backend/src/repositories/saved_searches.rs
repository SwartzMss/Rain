use sqlx::SqlitePool;
use uuid::Uuid;

use crate::{
    error::AppError,
    models::saved_searches::{SavedSearchPayload, SavedSearchRecord},
};

const COLUMNS: &str = "id, user_id, name, search_type, query_text, scope_type, scope_key, options_json, is_pinned, sort_order, created_at, updated_at, last_used_at";

pub async fn list(pool: &SqlitePool, user_id: &str) -> Result<Vec<SavedSearchRecord>, AppError> {
    let sql = format!(
        "SELECT {COLUMNS} FROM saved_searches WHERE user_id = ? ORDER BY is_pinned DESC, updated_at DESC"
    );
    sqlx::query_as(&sql)
        .bind(user_id)
        .fetch_all(pool)
        .await
        .map_err(AppError::Database)
}

pub async fn create(
    pool: &SqlitePool,
    user_id: &str,
    payload: &SavedSearchPayload,
) -> Result<SavedSearchRecord, AppError> {
    let id = Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO saved_searches (id, user_id, name, search_type, query_text, scope_type, scope_key, options_json, is_pinned, sort_order) VALUES (?, ?, ?, ?, ?, 'GLOBAL', NULL, ?, ?, 0)")
        .bind(&id).bind(user_id).bind(payload.name.trim()).bind(&payload.search_type)
        .bind(&payload.query_text).bind(payload.options.to_string()).bind(payload.is_pinned)
        .execute(pool).await.map_err(AppError::Database)?;
    find_owned(pool, user_id, &id)
        .await?
        .ok_or_else(|| AppError::Config("created saved search is missing".into()))
}

pub async fn update(
    pool: &SqlitePool,
    user_id: &str,
    id: &str,
    payload: &SavedSearchPayload,
) -> Result<Option<SavedSearchRecord>, AppError> {
    let result = sqlx::query("UPDATE saved_searches SET name = ?, search_type = ?, query_text = ?, scope_type = 'GLOBAL', scope_key = NULL, options_json = ?, is_pinned = ?, sort_order = 0, updated_at = CURRENT_TIMESTAMP WHERE id = ? AND user_id = ?")
        .bind(payload.name.trim()).bind(&payload.search_type).bind(&payload.query_text)
        .bind(payload.options.to_string()).bind(payload.is_pinned).bind(id).bind(user_id)
        .execute(pool).await.map_err(AppError::Database)?;
    if result.rows_affected() == 0 {
        return Ok(None);
    }
    find_owned(pool, user_id, id).await
}

pub async fn delete(pool: &SqlitePool, user_id: &str, id: &str) -> Result<bool, AppError> {
    Ok(
        sqlx::query("DELETE FROM saved_searches WHERE id = ? AND user_id = ?")
            .bind(id)
            .bind(user_id)
            .execute(pool)
            .await
            .map_err(AppError::Database)?
            .rows_affected()
            > 0,
    )
}

pub async fn mark_used(pool: &SqlitePool, user_id: &str, id: &str) -> Result<bool, AppError> {
    Ok(sqlx::query(
        "UPDATE saved_searches SET last_used_at = CURRENT_TIMESTAMP WHERE id = ? AND user_id = ?",
    )
    .bind(id)
    .bind(user_id)
    .execute(pool)
    .await
    .map_err(AppError::Database)?
    .rows_affected()
        > 0)
}

async fn find_owned(
    pool: &SqlitePool,
    user_id: &str,
    id: &str,
) -> Result<Option<SavedSearchRecord>, AppError> {
    let sql = format!("SELECT {COLUMNS} FROM saved_searches WHERE id = ? AND user_id = ?");
    sqlx::query_as(&sql)
        .bind(id)
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .map_err(AppError::Database)
}
