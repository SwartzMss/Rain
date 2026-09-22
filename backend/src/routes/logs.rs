use actix_web::{HttpResponse, get, web};
use serde::Deserialize;

use crate::{
    AppState,
    error::AppError,
    models::logs::{LogSearchHit, LogSearchResponse},
    search::{
        ContentSearchRequest, ContentSearchScope, FilenameSearchRequest, SearchIndex,
        sqlite::SqliteFtsSearchIndex,
    },
};

use super::issues::{ensure_issue_active, normalize_issue_code, touch_issue_activity_best_effort};

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
    crate::ingest::metrics::measure("bundle_search", search_logs_inner(path, query, state)).await
}

async fn search_logs_inner(
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

    let bundle = load_bundle(&state.db.pool, &bundle_hash).await?;
    ensure_bundle_ready(&bundle)?;
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
    if search_term.chars().count() < 3 {
        return Err(AppError::BadRequest("搜索关键词至少需要 3 个字符".into()));
    }
    let result = SqliteFtsSearchIndex::new(state.db.pool.clone())
        .search_content(ContentSearchRequest {
            scope: ContentSearchScope::Bundle {
                bundle_id: bundle.id.clone(),
                timeline,
                file_id,
            },
            query: search_term.to_owned(),
            path_like,
            from,
            size,
        })
        .await?;
    let total = result.total;
    let truncated = result.truncated;
    let rows = result.rows;

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

    touch_issue_activity_best_effort(&state.db.pool, &bundle.issue_code, "bundle log search").await;

    Ok(HttpResponse::Ok().json(LogSearchResponse {
        total: total.max(0) as u64,
        hits,
        truncated,
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
    crate::ingest::metrics::measure("issue_search", search_issue_logs_inner(path, query, state))
        .await
}

async fn search_issue_logs_inner(
    path: web::Path<String>,
    query: web::Query<IssueLogQuery>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    let issue_code = normalize_issue_code(&path.into_inner())?;
    ensure_issue_active(&state.db.pool, &issue_code).await?;
    let term = query.into_inner();
    let search_term = term.q.trim();
    if search_term.is_empty() {
        return Err(AppError::BadRequest("query parameter q is required".into()));
    }

    if matches!(term.mode, IssueSearchMode::Filename) {
        let response = search_issue_files(
            &state.db.pool,
            &state.limits.api,
            &issue_code,
            search_term,
            term.from,
            term.size,
        )
        .await?;
        touch_issue_activity_best_effort(&state.db.pool, &issue_code, "issue filename search")
            .await;
        return Ok(response);
    }

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
    if search_term.chars().count() < 3 {
        return Err(AppError::BadRequest("搜索关键词至少需要 3 个字符".into()));
    }
    let result = SqliteFtsSearchIndex::new(state.db.pool.clone())
        .search_content(ContentSearchRequest {
            scope: ContentSearchScope::Issue {
                issue_code: issue_code.clone(),
            },
            query: search_term.to_owned(),
            path_like,
            from,
            size,
        })
        .await?;
    let total = result.total;
    let truncated = result.truncated;
    let rows = result.rows;

    let hits = rows
        .into_iter()
        .map(|row| LogSearchHit {
            file_id: row.file_id.to_string(),
            path: row.path,
            bundle_hash: row.bundle_hash,
            snippet: literal_snippet(&row.content, search_term),
            timeline: None,
            offset: row.offset,
            line_end: row.line_end,
            line_number: row.offset,
            chunk_index: row.chunk_index,
        })
        .collect();

    touch_issue_activity_best_effort(&state.db.pool, &issue_code, "issue log search").await;

    Ok(HttpResponse::Ok().json(LogSearchResponse {
        total: total.max(0) as u64,
        hits,
        truncated,
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
    let result = SqliteFtsSearchIndex::new(pool.clone())
        .search_filenames(FilenameSearchRequest {
            issue_code: issue_code.to_owned(),
            query: search_term.to_owned(),
            from,
            size,
        })
        .await?;
    let total = result.total;
    let rows = result.rows;

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
        truncated: false,
    }))
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
