use actix_web::{HttpResponse, get, web};
use serde::Deserialize;
use sqlx::FromRow;

use crate::{
    AppState,
    error::AppError,
    models::logs::{LogSearchHit, LogSearchResponse},
};

use super::issues::normalize_issue_code;

use super::helpers::{ensure_bundle_ready, load_bundle};

#[derive(Deserialize)]
struct LogQuery {
    q: String,
    timeline: Option<String>,
    path_like: Option<String>,
    file_id: Option<i64>,
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
    ensure_bundle_ready(&bundle)?;
    let fts_query = build_fts_query(search_term);
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
    let file_id = term.file_id;
    let from = term.from.unwrap_or(0).max(0);
    let size = term
        .size
        .unwrap_or(state.limits.api.default_search_results)
        .clamp(1, state.limits.api.max_search_results);
    let path_pattern = path_like.as_ref().map(|value| format!("%{}%", value));
    let short_pattern = format!("%{}%", escape_like_pattern(search_term));

    let total: i64 = if search_term.chars().count() < 3 {
        sqlx::query_scalar(
            r#"
        SELECT COUNT(*) FROM log_segments ls
        JOIN files f ON f.id = ls.file_id
        WHERE ls.content LIKE ? ESCAPE '\' COLLATE NOCASE
          AND ls.bundle_id = ?
          AND (? IS NULL OR ls.timeline = ?)
          AND (? IS NULL OR f.path LIKE ?)
          AND (? IS NULL OR ls.file_id = ?)
        "#,
        )
        .bind(&short_pattern)
        .bind(&bundle.id)
        .bind(&timeline)
        .bind(&timeline)
        .bind(&path_pattern)
        .bind(&path_pattern)
        .bind(file_id)
        .bind(file_id)
        .fetch_one(&state.pool)
        .await
        .map_err(AppError::Database)?
    } else {
        sqlx::query_scalar(
            r#"
        SELECT COUNT(*) FROM log_segments ls
        JOIN log_segments_fts ON log_segments_fts.rowid = ls.id
        JOIN files f ON f.id = ls.file_id
        WHERE log_segments_fts MATCH ?
          AND ls.bundle_id = ?
          AND (? IS NULL OR ls.timeline = ?)
          AND (? IS NULL OR f.path LIKE ?)
          AND (? IS NULL OR ls.file_id = ?)
        "#,
        )
        .bind(&fts_query)
        .bind(&bundle.id)
        .bind(&timeline)
        .bind(&timeline)
        .bind(&path_pattern)
        .bind(&path_pattern)
        .bind(file_id)
        .bind(file_id)
        .fetch_one(&state.pool)
        .await
        .map_err(AppError::Database)?
    };

    let rows = if search_term.chars().count() < 3 {
        sqlx::query_as::<_, LogRow>(
            r#"
        SELECT ls.file_id, f.path, ls.timeline, ls.line_offset AS offset,
               ls.line_end, ls.chunk_index, ls.content
        FROM log_segments ls
        JOIN files f ON f.id = ls.file_id
        WHERE ls.content LIKE ? ESCAPE '\' COLLATE NOCASE
          AND ls.bundle_id = ?
          AND (? IS NULL OR ls.timeline = ?)
          AND (? IS NULL OR f.path LIKE ?)
          AND (? IS NULL OR ls.file_id = ?)
        ORDER BY ls.line_offset NULLS FIRST, ls.id
        LIMIT ? OFFSET ?
        "#,
        )
        .bind(&short_pattern)
        .bind(&bundle.id)
        .bind(&timeline)
        .bind(&timeline)
        .bind(&path_pattern)
        .bind(&path_pattern)
        .bind(file_id)
        .bind(file_id)
        .bind(size)
        .bind(from)
        .fetch_all(&state.pool)
        .await
        .map_err(AppError::Database)?
    } else {
        sqlx::query_as::<_, LogRow>(
            r#"
        SELECT ls.file_id,
               f.path,
               ls.timeline,
               ls.line_offset AS offset,
               ls.line_end,
               ls.chunk_index,
               ls.content AS content
        FROM log_segments ls
        JOIN log_segments_fts ON log_segments_fts.rowid = ls.id
        JOIN files f ON f.id = ls.file_id
        WHERE log_segments_fts MATCH ?
          AND ls.bundle_id = ?
          AND (? IS NULL OR ls.timeline = ?)
          AND (? IS NULL OR f.path LIKE ?)
          AND (? IS NULL OR ls.file_id = ?)
        ORDER BY ls.line_offset NULLS FIRST, ls.id
        LIMIT ? OFFSET ?
        "#,
        )
        .bind(&fts_query)
        .bind(&bundle.id)
        .bind(&timeline)
        .bind(&timeline)
        .bind(&path_pattern)
        .bind(&path_pattern)
        .bind(file_id)
        .bind(file_id)
        .bind(size)
        .bind(from)
        .fetch_all(&state.pool)
        .await
        .map_err(AppError::Database)?
    };

    let hits = rows
        .into_iter()
        .map(|row| LogSearchHit {
            file_id: row.file_id.to_string(),
            path: row.path,
            bundle_hash: Some(bundle.hash.clone()),
            snippet: literal_snippet(&row.content, search_term),
            timeline: row.timeline,
            offset: row.offset,
            line_end: row.line_end,
            line_number: row.offset,
            chunk_index: row.chunk_index,
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
    #[serde(default)]
    mode: IssueSearchMode,
    path_like: Option<String>,
    from: Option<i64>,
    size: Option<i64>,
}

#[derive(Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
enum IssueSearchMode {
    Filename,
    #[default]
    Content,
}

#[get("/issues/{issue_code}/search")]
pub async fn search_issue_logs(
    path: web::Path<String>,
    query: web::Query<IssueLogQuery>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    let issue_code = normalize_issue_code(&path.into_inner())?;
    let term = query.into_inner();
    let search_term = term.q.trim();
    if search_term.is_empty() {
        return Err(AppError::BadRequest("query parameter q is required".into()));
    }

    if matches!(term.mode, IssueSearchMode::Filename) {
        return search_issue_files(
            &state.pool,
            &state.limits.api,
            &issue_code,
            search_term,
            term.from,
            term.size,
        )
        .await;
    }

    let fts_query = build_fts_query(search_term);
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
        .unwrap_or(state.limits.api.default_search_results)
        .clamp(1, state.limits.api.max_search_results);
    let path_pattern = path_like.as_ref().map(|value| format!("%{}%", value));
    let short_pattern = format!("%{}%", escape_like_pattern(search_term));

    let total: i64 = if search_term.chars().count() < 3 {
        sqlx::query_scalar(
            r#"
        SELECT COUNT(*) FROM log_segments ls
        JOIN bundles b ON b.id = ls.bundle_id
        JOIN files f ON f.id = ls.file_id
        WHERE ls.content LIKE ? ESCAPE '\' COLLATE NOCASE
          AND b.issue_code = ?
          AND b.status = 'READY'
          AND (? IS NULL OR f.path LIKE ?)
        "#,
        )
        .bind(&short_pattern)
        .bind(&issue_code)
        .bind(&path_pattern)
        .bind(&path_pattern)
        .fetch_one(&state.pool)
        .await
        .map_err(AppError::Database)?
    } else {
        sqlx::query_scalar(
            r#"
        SELECT COUNT(*) FROM log_segments ls
        JOIN log_segments_fts ON log_segments_fts.rowid = ls.id
        JOIN bundles b ON b.id = ls.bundle_id
        JOIN files f ON f.id = ls.file_id
        WHERE log_segments_fts MATCH ?
          AND b.issue_code = ?
          AND b.status = 'READY'
          AND (? IS NULL OR f.path LIKE ?)
        "#,
        )
        .bind(&fts_query)
        .bind(&issue_code)
        .bind(&path_pattern)
        .bind(&path_pattern)
        .fetch_one(&state.pool)
        .await
        .map_err(AppError::Database)?
    };

    let rows = if search_term.chars().count() < 3 {
        sqlx::query_as::<_, IssueLogRow>(
            r#"
        SELECT ls.file_id, f.path, ls.line_offset AS offset, ls.line_end,
               ls.chunk_index, ls.content, b.hash AS bundle_hash
        FROM log_segments ls
        JOIN bundles b ON b.id = ls.bundle_id
        JOIN files f ON f.id = ls.file_id
        WHERE ls.content LIKE ? ESCAPE '\' COLLATE NOCASE
          AND b.issue_code = ?
          AND b.status = 'READY'
          AND (? IS NULL OR f.path LIKE ?)
        ORDER BY ls.line_offset NULLS FIRST, ls.id
        LIMIT ? OFFSET ?
        "#,
        )
        .bind(&short_pattern)
        .bind(&issue_code)
        .bind(&path_pattern)
        .bind(&path_pattern)
        .bind(size)
        .bind(from)
        .fetch_all(&state.pool)
        .await
        .map_err(AppError::Database)?
    } else {
        sqlx::query_as::<_, IssueLogRow>(
            r#"
        SELECT ls.file_id,
               f.path,
               ls.line_offset AS offset,
               ls.line_end,
               ls.chunk_index,
               ls.content AS content,
               b.hash as bundle_hash
        FROM log_segments ls
        JOIN log_segments_fts ON log_segments_fts.rowid = ls.id
        JOIN bundles b ON b.id = ls.bundle_id
        JOIN files f ON f.id = ls.file_id
        WHERE log_segments_fts MATCH ?
          AND b.issue_code = ?
          AND b.status = 'READY'
          AND (? IS NULL OR f.path LIKE ?)
        ORDER BY ls.line_offset NULLS FIRST, ls.id
        LIMIT ? OFFSET ?
        "#,
        )
        .bind(&fts_query)
        .bind(&issue_code)
        .bind(&path_pattern)
        .bind(&path_pattern)
        .bind(size)
        .bind(from)
        .fetch_all(&state.pool)
        .await
        .map_err(AppError::Database)?
    };

    let hits = rows
        .into_iter()
        .map(|row| LogSearchHit {
            file_id: row.file_id.to_string(),
            path: row.path,
            bundle_hash: Some(row.bundle_hash),
            snippet: literal_snippet(&row.content, search_term),
            timeline: None,
            offset: row.offset,
            line_end: row.line_end,
            line_number: row.offset,
            chunk_index: row.chunk_index,
        })
        .collect();

    Ok(HttpResponse::Ok().json(LogSearchResponse {
        total: total.max(0) as u64,
        hits,
    }))
}

async fn search_issue_files(
    pool: &sqlx::SqlitePool,
    api: &crate::config::ApiConfig,
    issue_code: &str,
    search_term: &str,
    from: Option<i64>,
    size: Option<i64>,
) -> Result<HttpResponse, AppError> {
    let from = from.unwrap_or(0).max(0);
    let size = size
        .unwrap_or(api.default_search_results)
        .clamp(1, api.max_search_results);
    let pattern = format!("%{}%", escape_like_pattern(search_term));

    let total: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM files f
        JOIN bundles b ON b.id = f.bundle_id
        WHERE b.issue_code = ?
          AND b.status = 'READY'
          AND f.is_dir = 0
          AND (
            f.name LIKE ? ESCAPE '\' COLLATE NOCASE
            OR f.path LIKE ? ESCAPE '\' COLLATE NOCASE
          )
        "#,
    )
    .bind(issue_code)
    .bind(&pattern)
    .bind(&pattern)
    .fetch_one(pool)
    .await
    .map_err(AppError::Database)?;

    let rows = sqlx::query_as::<_, IssueFileSearchRow>(
        r#"
        SELECT f.id AS file_id,
               f.name,
               CASE WHEN f.parent_id IS NULL THEN f.name ELSE f.path END AS path,
               b.hash AS bundle_hash
        FROM files f
        JOIN bundles b ON b.id = f.bundle_id
        WHERE b.issue_code = ?
          AND b.status = 'READY'
          AND f.is_dir = 0
          AND (
            f.name LIKE ? ESCAPE '\' COLLATE NOCASE
            OR f.path LIKE ? ESCAPE '\' COLLATE NOCASE
          )
        ORDER BY CASE WHEN f.name = ? COLLATE NOCASE THEN 0 ELSE 1 END,
                 f.name COLLATE NOCASE,
                 f.path COLLATE NOCASE
        LIMIT ? OFFSET ?
        "#,
    )
    .bind(issue_code)
    .bind(&pattern)
    .bind(&pattern)
    .bind(search_term)
    .bind(size)
    .bind(from)
    .fetch_all(pool)
    .await
    .map_err(AppError::Database)?;

    let hits = rows
        .into_iter()
        .map(|row| LogSearchHit {
            file_id: row.file_id.to_string(),
            path: row.path,
            bundle_hash: Some(row.bundle_hash),
            snippet: row.name,
            timeline: None,
            offset: None,
            line_end: None,
            line_number: None,
            chunk_index: None,
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
    line_end: Option<i64>,
    chunk_index: Option<i64>,
    content: String,
}

#[derive(FromRow)]
struct IssueLogRow {
    file_id: i64,
    path: String,
    offset: Option<i64>,
    line_end: Option<i64>,
    chunk_index: Option<i64>,
    content: String,
    bundle_hash: String,
}

#[derive(FromRow)]
struct IssueFileSearchRow {
    file_id: i64,
    name: String,
    path: String,
    bundle_hash: String,
}

fn escape_like_pattern(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn build_fts_query(search_term: &str) -> String {
    format!("\"{}\"", search_term.replace('"', "\"\""))
}

fn literal_snippet(content: &str, search_term: &str) -> String {
    const MAX_CHARS: usize = 400;
    const CONTEXT_BEFORE: usize = 120;
    let content_chars: Vec<char> = content.chars().collect();
    if content_chars.len() <= MAX_CHARS {
        return content.to_string();
    }
    let term_chars: Vec<char> = search_term.chars().collect();
    let match_start = if term_chars.is_empty() {
        0
    } else {
        content_chars
            .windows(term_chars.len())
            .position(|window| {
                window
                    .iter()
                    .zip(&term_chars)
                    .all(|(left, right)| left.eq_ignore_ascii_case(right))
            })
            .unwrap_or(0)
    };
    let start = match_start.saturating_sub(CONTEXT_BEFORE);
    let end = (start + MAX_CHARS).min(content_chars.len());
    let mut snippet: String = content_chars[start..end].iter().collect();
    if start > 0 {
        snippet.insert_str(0, "... ");
    }
    if end < content_chars.len() {
        snippet.push_str(" ...");
    }
    snippet
}
