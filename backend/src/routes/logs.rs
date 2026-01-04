use actix_web::{HttpResponse, get, web};
use serde::Deserialize;
use sqlx::FromRow;

use crate::{
    AppState,
    error::AppError,
    models::logs::{LogSearchHit, LogSearchResponse},
};

use super::helpers::load_bundle;

const MAX_LOG_RESULTS: i64 = 1000;
const LINES_PER_CHUNK: i64 = 200;

#[derive(Deserialize)]
struct LogQuery {
    q: String,
    timeline: Option<String>,
    path_like: Option<String>,
    from: Option<i64>,
    size: Option<i64>,
}

// scoped under /api in routes::register
#[get("/log/v2/{bundle_id}/search")]
pub async fn search_logs(
    path: web::Path<String>,
    query: web::Query<LogQuery>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    let bundle_hash = path.into_inner();
    let term = query.into_inner();
    let search_term = term.q.trim();
    if search_term.is_empty() {
        return Err(AppError::BadRequest("query parameter q is required".into()));
    }

    let bundle = load_bundle(&state.pool, &bundle_hash).await?;
    let like_pattern = format!("%{}%", search_term);
    let timeline = term.timeline.and_then(|value| {
        let trimmed = value.trim().to_string();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    });
    let path_like = term.path_like.and_then(|value| {
        let trimmed = value.trim().to_string();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    });
    let from = term.from.unwrap_or(0).max(0);
    let size = term
        .size
        .unwrap_or(MAX_LOG_RESULTS)
        .clamp(1, MAX_LOG_RESULTS);

    let total: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*) FROM log_segments ls
        WHERE ls.bundle_id = $1
          AND ls.content ILIKE $2
          AND ($3::text IS NULL OR ls.timeline = $3)
          AND ($4::text IS NULL OR f.path ILIKE $4)
        "#,
    )
    .bind(bundle.id)
    .bind(&like_pattern)
    .bind(&timeline)
    .bind(path_like.as_ref().map(|value| format!("%{}%", value)))
    .fetch_one(&state.pool)
    .await
    .map_err(AppError::Database)?;

    let rows = sqlx::query_as::<_, LogRow>(
        r#"
        SELECT ls.file_id, f.path, ls.timeline, ls.line_offset AS offset, ls.content
        FROM log_segments ls
        JOIN files f ON f.id = ls.file_id
        WHERE ls.bundle_id = $1
          AND ls.content ILIKE $2
          AND ($3::text IS NULL OR ls.timeline = $3)
          AND ($4::text IS NULL OR f.path ILIKE $4)
        ORDER BY ls.line_offset NULLS FIRST, ls.id
        LIMIT $5 OFFSET $6
        "#,
    )
    .bind(bundle.id)
    .bind(&like_pattern)
    .bind(&timeline)
    .bind(path_like.as_ref().map(|value| format!("%{}%", value)))
    .bind(size)
    .bind(from)
    .fetch_all(&state.pool)
    .await
    .map_err(AppError::Database)?;

    let hits = rows
        .into_iter()
        .map(|row| LogSearchHit {
            file_id: row.file_id.to_string(),
            path: row.path,
            bundle_hash: Some(bundle.hash.clone()),
            snippet: row.content,
            timeline: row.timeline,
            offset: row.offset,
            line_end: row.offset.map(|offset| offset + LINES_PER_CHUNK - 1),
            line_number: row.offset,
            chunk_index: row.offset.map(|offset| offset / LINES_PER_CHUNK),
        })
        .collect();

    Ok(HttpResponse::Ok().json(LogSearchResponse {
        total: total.max(0) as u64,
        hits,
    }))
}

#[derive(Deserialize)]
struct IssueLogQuery {
    q: String,
    path_like: Option<String>,
    from: Option<i64>,
    size: Option<i64>,
}

#[get("/issues/{issue_code}/search")]
pub async fn search_issue_logs(
    path: web::Path<String>,
    query: web::Query<IssueLogQuery>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    let issue_code = path.into_inner();
    let term = query.into_inner();
    let search_term = term.q.trim();
    if search_term.is_empty() {
        return Err(AppError::BadRequest("query parameter q is required".into()));
    }

    let like_pattern = format!("%{}%", search_term);
    let path_like = term.path_like.and_then(|value| {
        let trimmed = value.trim().to_string();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    });
    let from = term.from.unwrap_or(0).max(0);
    let size = term
        .size
        .unwrap_or(MAX_LOG_RESULTS)
        .clamp(1, MAX_LOG_RESULTS);

    let total: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*) FROM log_segments ls
        JOIN bundles b ON b.id = ls.bundle_id
        JOIN files f ON f.id = ls.file_id
        WHERE b.issue_code = $1
          AND ls.content ILIKE $2
          AND ($3::text IS NULL OR f.path ILIKE $3)
        "#,
    )
    .bind(&issue_code)
    .bind(&like_pattern)
    .bind(path_like.as_ref().map(|value| format!("%{}%", value)))
    .fetch_one(&state.pool)
    .await
    .map_err(AppError::Database)?;

    let rows = sqlx::query_as::<_, IssueLogRow>(
        r#"
        SELECT ls.file_id,
               f.path,
               ls.line_offset AS offset,
               ls.content,
               b.hash as bundle_hash
        FROM log_segments ls
        JOIN bundles b ON b.id = ls.bundle_id
        JOIN files f ON f.id = ls.file_id
        WHERE b.issue_code = $1
          AND ls.content ILIKE $2
          AND ($3::text IS NULL OR f.path ILIKE $3)
        ORDER BY ls.line_offset NULLS FIRST, ls.id
        LIMIT $4 OFFSET $5
        "#,
    )
    .bind(&issue_code)
    .bind(&like_pattern)
    .bind(path_like.as_ref().map(|value| format!("%{}%", value)))
    .bind(size)
    .bind(from)
    .fetch_all(&state.pool)
    .await
    .map_err(AppError::Database)?;

    let hits = rows
        .into_iter()
        .map(|row| LogSearchHit {
            file_id: row.file_id.to_string(),
            path: row.path,
            bundle_hash: Some(row.bundle_hash),
            snippet: row.content,
            timeline: None,
            offset: row.offset,
            line_end: row.offset.map(|offset| offset + LINES_PER_CHUNK - 1),
            line_number: row.offset,
            chunk_index: row.offset.map(|offset| offset / LINES_PER_CHUNK),
        })
        .collect();

    Ok(HttpResponse::Ok().json(LogSearchResponse {
        total: total.max(0) as u64,
        hits,
    }))
}

#[derive(FromRow)]
struct LogRow {
    file_id: i64,
    path: String,
    timeline: Option<String>,
    offset: Option<i64>,
    content: String,
}

#[derive(FromRow)]
struct IssueLogRow {
    file_id: i64,
    path: String,
    offset: Option<i64>,
    content: String,
    bundle_hash: String,
}
