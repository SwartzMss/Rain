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
    let input = (
        id.clone(),
        user_id.to_owned(),
        payload.name.trim().to_owned(),
        payload.search_type.clone(),
        payload.query_text.clone(),
        payload.options.to_string(),
        payload.is_pinned,
    );
    crate::db::write::run(pool, "create saved search", &input, |conn, input| {
        Box::pin(async move {
            sqlx::query("INSERT INTO saved_searches (id, user_id, name, search_type, query_text, scope_type, scope_key, options_json, is_pinned, sort_order) VALUES (?, ?, ?, ?, ?, 'GLOBAL', NULL, ?, ?, 0)")
                .bind(&input.0)
                .bind(&input.1)
                .bind(&input.2)
                .bind(&input.3)
                .bind(&input.4)
                .bind(&input.5)
                .bind(input.6)
                .execute(conn)
                .await
                .map(|_| ())
                .map_err(AppError::Database)
        })
    })
    .await?;
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
    let input = (
        payload.name.trim().to_owned(),
        payload.search_type.clone(),
        payload.query_text.clone(),
        payload.options.to_string(),
        payload.is_pinned,
        id.to_owned(),
        user_id.to_owned(),
    );
    let changed = crate::db::write::run(pool, "update saved search", &input, |conn, input| {
        Box::pin(async move {
            Ok(sqlx::query("UPDATE saved_searches SET name = ?, search_type = ?, query_text = ?, scope_type = 'GLOBAL', scope_key = NULL, options_json = ?, is_pinned = ?, sort_order = 0, updated_at = CURRENT_TIMESTAMP WHERE id = ? AND user_id = ?")
                .bind(&input.0)
                .bind(&input.1)
                .bind(&input.2)
                .bind(&input.3)
                .bind(input.4)
                .bind(&input.5)
                .bind(&input.6)
                .execute(conn)
                .await
                .map_err(AppError::Database)?
                .rows_affected())
        })
    })
    .await?;
    if changed == 0 {
        return Ok(None);
    }
    find_owned(pool, user_id, id).await
}

pub async fn delete(pool: &SqlitePool, user_id: &str, id: &str) -> Result<bool, AppError> {
    let input = (user_id.to_owned(), id.to_owned());
    crate::db::write::run(pool, "delete saved search", &input, |conn, input| {
        Box::pin(async move {
            Ok(
                sqlx::query("DELETE FROM saved_searches WHERE id = ? AND user_id = ?")
                    .bind(&input.1)
                    .bind(&input.0)
                    .execute(conn)
                    .await
                    .map_err(AppError::Database)?
                    .rows_affected()
                    > 0,
            )
        })
    })
    .await
}

pub async fn mark_used(pool: &SqlitePool, user_id: &str, id: &str) -> Result<bool, AppError> {
    let input = (user_id.to_owned(), id.to_owned());
    crate::db::write::run(pool, "mark saved search used", &input, |conn, input| {
        Box::pin(async move {
            Ok(sqlx::query("UPDATE saved_searches SET last_used_at = CURRENT_TIMESTAMP WHERE id = ? AND user_id = ?")
                .bind(&input.1)
                .bind(&input.0)
                .execute(conn)
                .await
                .map_err(AppError::Database)?
                .rows_affected() > 0)
        })
    })
    .await
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
