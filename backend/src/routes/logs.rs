use std::{cmp::Ordering, collections::BinaryHeap};

use actix_web::{HttpResponse, get, web};
use serde::Deserialize;

use crate::{
    AppState,
    error::AppError,
    models::logs::{LogSearchHit, LogSearchResponse},
    search::generation_lease::GenerationLeaseRegistry,
    search::{
        ContentSearchRequest, ContentSearchResult, ContentSearchScope, FilenameSearchRequest,
        SearchIndex, SearchWindow,
        publication::{acquire_generation_lease_with_registry, artifact_relative_path},
        search_tantivy_bundle_visible_with_lease,
        sqlite::SqliteFtsSearchIndex,
        validate_search_window,
        visibility::snapshot_file_ids,
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

type PublicationRow = (String, String, i64, Option<i64>, Option<i64>);
type IssueBundleSearchRow = (
    String,
    String,
    String,
    String,
    i64,
    Option<i64>,
    Option<i64>,
);

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
    let window = normalize_search_window(&state.limits.api, term.from, term.size)?;

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
    let from = window.from as i64;
    let size = window.size as i64;
    if search_term.chars().count() < 3 {
        return Err(AppError::BadRequest("搜索关键词至少需要 3 个字符".into()));
    }
    let request = ContentSearchRequest {
        scope: ContentSearchScope::Bundle {
            bundle_id: bundle.id.clone(),
            timeline,
            file_id,
        },
        query: search_term.to_owned(),
        path_like,
        from,
        size,
    };
    let publication: Option<PublicationRow> = sqlx::query_as(
        "SELECT backend, state, generation, schema_version, tokenizer_version FROM bundle_search_indexes WHERE bundle_id = ?",
    )
    .bind(&bundle.id)
    .fetch_optional(&state.db.pool)
    .await
    .map_err(AppError::Database)?;
    let result = match publication {
        Some((backend, state_name, generation, schema_version, tokenizer_version))
            if backend == "tantivy" =>
        {
            if !matches!(state_name.as_str(), "READY" | "NEEDS_REBUILD") {
                return Err(AppError::Conflict(
                    "Bundle search index is not ready".into(),
                ));
            }
            if schema_version != Some(crate::search::publication::TANTIVY_SCHEMA_VERSION)
                || tokenizer_version != Some(crate::search::publication::TANTIVY_TOKENIZER_VERSION)
            {
                return Err(AppError::Conflict(
                    "Bundle search index version is unsupported; rebuild is required".into(),
                ));
            }
            let artifact = artifact_relative_path(&bundle.id, generation)?;
            let visible_file_ids = snapshot_file_ids(&state.db.pool, &bundle.id).await?;
            let lease = acquire_generation_lease_with_registry(
                &state.search.generation_leases,
                &state.db.pool,
                &bundle.id,
                generation,
            )
            .await?;
            let result = search_tantivy_bundle_visible_with_lease(
                state.storage.data_root.join(artifact),
                request,
                visible_file_ids,
                lease,
            )
            .await;
            result?
        }
        _ => {
            SqliteFtsSearchIndex::new(state.db.pool.clone())
                .search_content(request)
                .await?
        }
    };
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
        max_search_window: state.limits.api.max_search_window as u64,
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
    let term = query.into_inner();
    let search_term = term.q.trim();
    if search_term.is_empty() {
        return Err(AppError::BadRequest("query parameter q is required".into()));
    }
    let window = normalize_search_window(&state.limits.api, term.from, term.size)?;
    ensure_issue_active(&state.db.pool, &issue_code).await?;

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
    let from = window.from as i64;
    let size = window.size as i64;
    if search_term.chars().count() < 3 {
        return Err(AppError::BadRequest("搜索关键词至少需要 3 个字符".into()));
    }
    let result = search_issue_content_mixed(
        &state.db.pool,
        &state.storage.data_root,
        &state.search.generation_leases,
        ContentSearchRequest {
            scope: ContentSearchScope::Issue {
                issue_code: issue_code.clone(),
            },
            query: search_term.to_owned(),
            path_like,
            from,
            size,
        },
    )
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
        max_search_window: state.limits.api.max_search_window as u64,
    }))
}

#[derive(Debug)]
struct RankedIssueRow {
    key: (i64, String, i64, i64),
    row: crate::search::ContentSearchRow,
}

impl PartialEq for RankedIssueRow {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

impl Eq for RankedIssueRow {}

impl PartialOrd for RankedIssueRow {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for RankedIssueRow {
    fn cmp(&self, other: &Self) -> Ordering {
        self.key.cmp(&other.key)
    }
}

fn issue_row_key(row: &crate::search::ContentSearchRow) -> (i64, String, i64, i64) {
    (
        row.offset.unwrap_or(i64::MIN),
        row.bundle_hash.clone().unwrap_or_default(),
        row.file_id,
        row.chunk_index.unwrap_or(i64::MIN),
    )
}

fn retain_issue_row(
    retained: &mut BinaryHeap<RankedIssueRow>,
    row: crate::search::ContentSearchRow,
    limit: usize,
) {
    if limit == 0 {
        return;
    }
    let ranked = RankedIssueRow {
        key: issue_row_key(&row),
        row,
    };
    if retained.len() < limit {
        retained.push(ranked);
    } else if retained
        .peek()
        .is_some_and(|current| ranked.key < current.key)
    {
        retained.pop();
        retained.push(ranked);
    }
}

async fn search_issue_content_mixed(
    pool: &sqlx::SqlitePool,
    data_root: &std::path::Path,
    registry: &GenerationLeaseRegistry,
    request: ContentSearchRequest,
) -> Result<ContentSearchResult, AppError> {
    let ContentSearchScope::Issue { issue_code } = &request.scope else {
        return Err(AppError::Config(
            "mixed Issue search requires an Issue scope".into(),
        ));
    };
    let window = validate_search_window(
        request.from,
        request.size,
        crate::search::HARD_MAX_SEARCH_WINDOW as i64,
    )?;
    let candidate_limit = window.limit.max(1);
    let sqlite_result = SqliteFtsSearchIndex::new(pool.clone())
        .search_content(ContentSearchRequest {
            from: 0,
            size: candidate_limit as i64,
            ..request.clone()
        })
        .await?;
    let mut retained = BinaryHeap::new();
    for row in sqlite_result.rows {
        retain_issue_row(&mut retained, row, window.limit);
    }
    let mut total = sqlite_result.total;
    let bundles: Vec<IssueBundleSearchRow> = sqlx::query_as(
        "SELECT b.id, b.hash, COALESCE(si.backend, 'sqlite_fts'), COALESCE(si.state, 'LEGACY'), COALESCE(si.generation, 0), si.schema_version, si.tokenizer_version FROM bundles b LEFT JOIN bundle_search_indexes si ON si.bundle_id = b.id WHERE b.issue_code = ? AND b.status = 'READY' ORDER BY b.id",
    )
    .bind(issue_code)
    .fetch_all(pool)
    .await
    .map_err(AppError::Database)?;
    for (bundle_id, bundle_hash, backend, state, generation, schema_version, tokenizer_version) in
        bundles
    {
        if backend != "tantivy" {
            continue;
        }
        if !matches!(state.as_str(), "READY" | "NEEDS_REBUILD") {
            return Err(AppError::Conflict(format!(
                "Bundle {bundle_id} search index is not ready"
            )));
        }
        if schema_version != Some(crate::search::publication::TANTIVY_SCHEMA_VERSION)
            || tokenizer_version != Some(crate::search::publication::TANTIVY_TOKENIZER_VERSION)
        {
            return Err(AppError::Conflict(
                "Bundle search index version is unsupported; rebuild is required".into(),
            ));
        }
        let artifact = artifact_relative_path(&bundle_id, generation)?;
        let visible_file_ids = snapshot_file_ids(pool, &bundle_id).await?;
        let lease =
            acquire_generation_lease_with_registry(registry, pool, &bundle_id, generation).await?;
        let result = search_tantivy_bundle_visible_with_lease(
            data_root.join(artifact),
            ContentSearchRequest {
                scope: ContentSearchScope::Bundle {
                    bundle_id: bundle_id.clone(),
                    timeline: None,
                    file_id: None,
                },
                query: request.query.clone(),
                path_like: request.path_like.clone(),
                from: 0,
                size: candidate_limit as i64,
            },
            visible_file_ids,
            lease,
        )
        .await;
        let result = result?;
        total = total.saturating_add(result.total);
        for mut row in result.rows {
            row.bundle_hash = Some(bundle_hash.clone());
            retain_issue_row(&mut retained, row, window.limit);
        }
    }
    let mut rows: Vec<_> = retained.into_iter().map(|ranked| ranked.row).collect();
    rows.sort_by_key(issue_row_key);
    let rows = rows
        .into_iter()
        .skip(window.from)
        .take(window.size)
        .collect();
    Ok(ContentSearchResult {
        total,
        rows,
        truncated: false,
    })
}

async fn search_issue_files(
    pool: &sqlx::SqlitePool,
    api: &crate::config::ApiConfig,
    issue_code: &str,
    search_term: &str,
    from: Option<i64>,
    size: Option<i64>,
) -> Result<HttpResponse, AppError> {
    let window = normalize_search_window(api, from, size)?;
    let from = window.from as i64;
    let size = window.size as i64;
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
        max_search_window: api.max_search_window as u64,
    }))
}

fn normalize_search_window(
    api: &crate::config::ApiConfig,
    from: Option<i64>,
    size: Option<i64>,
) -> Result<SearchWindow, AppError> {
    let size = size
        .unwrap_or(api.default_search_results)
        .clamp(1, api.max_search_results);
    validate_search_window(from.unwrap_or(0), size, api.max_search_window)
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
