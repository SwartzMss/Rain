use std::{
    cmp::Ordering,
    collections::{BinaryHeap, HashMap},
    path::PathBuf,
    sync::Arc,
};

use actix_web::{HttpResponse, get, web};
use futures_util::StreamExt;
use serde::Deserialize;

use crate::{
    AppState,
    error::AppError,
    models::logs::{LogSearchHit, LogSearchResponse},
    search::generation_lease::GenerationLeaseRegistry,
    search::{
        ContentSearchRequest, ContentSearchResult, ContentSearchScope, SearchIndex, SearchWindow,
        hydrate_tantivy_bundle_rows,
        parallel::stream_issue_bundle_searches,
        publication::{
            acquire_generation_lease_with_registry, artifact_relative_path,
            can_skip_visibility_snapshot,
        },
        search_tantivy_bundle_visible_with_lease_and_permit,
        search_tantivy_bundle_with_lease_and_permit,
        sqlite::SqliteFtsSearchIndex,
        validate_search_window,
        visibility::snapshot_file_ids,
    },
};

use super::issues::{ensure_issue_active, normalize_issue_code};

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

type PublicationRow = (String, String, i64, Option<i64>, Option<i64>, i64, i64);
type IssueBundleSearchRow = (
    String,
    String,
    String,
    String,
    i64,
    i64,
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
    let settings = state.settings.snapshot().await;
    let mut runtime_limits = state.limits.clone();
    let mut runtime_auth = state.auth_runtime.config.clone();
    settings
        .effective
        .apply_to_config(&mut runtime_limits, &mut runtime_auth);
    let search_term = term.q.trim();
    if search_term.is_empty() {
        return Err(AppError::BadRequest("query parameter q is required".into()));
    }
    let window = normalize_search_window(&runtime_limits.api, term.from, term.size)?;

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
        include_content: true,
    };
    let publication: Option<PublicationRow> = sqlx::query_as(
        "SELECT backend, state, generation, schema_version, tokenizer_version, visibility_revision, compacted_revision FROM bundle_search_indexes WHERE bundle_id = ?",
    )
    .bind(&bundle.id)
    .fetch_optional(&state.db.pool)
    .await
    .map_err(AppError::Database)?;
    let result = match publication {
        Some((
            backend,
            state_name,
            generation,
            schema_version,
            tokenizer_version,
            visibility_revision,
            compacted_revision,
        )) if backend == "tantivy" => {
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
            let complete_visibility =
                can_skip_visibility_snapshot(&state_name, visibility_revision, compacted_revision);
            let permit = state
                .search
                .query_permits
                .clone()
                .acquire_owned()
                .await
                .map_err(|_| {
                    AppError::Conflict("Tantivy query admission is shutting down".into())
                })?;
            let lease = acquire_generation_lease_with_registry(
                &state.search.generation_leases,
                &state.db.pool,
                &bundle.id,
                generation,
            )
            .await?;
            let result = if complete_visibility {
                search_tantivy_bundle_with_lease_and_permit(
                    state.storage.data_root.join(artifact),
                    request,
                    bundle.id.clone(),
                    generation,
                    lease,
                    permit,
                )
                .await
            } else {
                let visible_file_ids = snapshot_file_ids(&state.db.pool, &bundle.id).await?;
                search_tantivy_bundle_visible_with_lease_and_permit(
                    state.storage.data_root.join(artifact),
                    request,
                    visible_file_ids,
                    bundle.id.clone(),
                    generation,
                    lease,
                    permit,
                )
                .await
            };
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

    Ok(HttpResponse::Ok().json(LogSearchResponse {
        total: total.max(0) as u64,
        hits,
        truncated,
        max_search_window: runtime_limits.api.max_search_window as u64,
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
    let settings = state.settings.snapshot().await;
    let mut runtime_limits = state.limits.clone();
    let mut runtime_auth = state.auth_runtime.config.clone();
    settings
        .effective
        .apply_to_config(&mut runtime_limits, &mut runtime_auth);
    let search_term = term.q.trim();
    if search_term.is_empty() {
        return Err(AppError::BadRequest("query parameter q is required".into()));
    }
    let window = normalize_search_window(&runtime_limits.api, term.from, term.size)?;
    ensure_issue_active(&state.db.pool, &issue_code).await?;

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
        state.search.query_permits.clone(),
        ContentSearchRequest {
            scope: ContentSearchScope::Issue {
                issue_code: issue_code.clone(),
            },
            query: search_term.to_owned(),
            path_like,
            from,
            size,
            include_content: true,
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

    Ok(HttpResponse::Ok().json(LogSearchResponse {
        total: total.max(0) as u64,
        hits,
        truncated,
        max_search_window: runtime_limits.api.max_search_window as u64,
    }))
}

#[derive(Debug)]
struct RankedIssueRow {
    key: (i64, String, i64, i64),
    row: crate::search::ContentSearchRow,
    tantivy_context: Option<TantivyIssueRowContext>,
}

#[derive(Debug, Clone)]
struct TantivyIssueRowContext {
    bundle_id: String,
    generation: i64,
    artifact_path: PathBuf,
}

type TantivyHydrationEntries = Vec<(usize, (u32, u32))>;
type TantivyHydrationGroup = (PathBuf, TantivyHydrationEntries);

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
    tantivy_context: Option<TantivyIssueRowContext>,
) {
    if limit == 0 {
        return;
    }
    let ranked = RankedIssueRow {
        key: issue_row_key(&row),
        row,
        tantivy_context,
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

async fn hydrate_issue_rows(
    rows: &mut [RankedIssueRow],
    pool: &sqlx::SqlitePool,
    registry: &GenerationLeaseRegistry,
    query_permits: &Arc<tokio::sync::Semaphore>,
) -> Result<(), AppError> {
    let mut grouped: HashMap<(String, i64), TantivyHydrationGroup> = HashMap::new();
    for (index, ranked) in rows.iter().enumerate() {
        let Some(context) = ranked.tantivy_context.as_ref() else {
            continue;
        };
        let Some(address) = ranked.row.tantivy_doc_address else {
            return Err(AppError::Config(
                "Tantivy result is missing its document address".into(),
            ));
        };
        grouped
            .entry((context.bundle_id.clone(), context.generation))
            .or_insert_with(|| (context.artifact_path.clone(), Vec::new()))
            .1
            .push((index, address));
    }

    for ((bundle_id, generation), (artifact_path, entries)) in grouped {
        let permit =
            query_permits.clone().acquire_owned().await.map_err(|_| {
                AppError::Conflict("Tantivy query admission is shutting down".into())
            })?;
        let lease =
            acquire_generation_lease_with_registry(registry, pool, &bundle_id, generation).await?;
        let addresses = entries
            .iter()
            .map(|(_, address)| *address)
            .collect::<Vec<_>>();
        let contents = hydrate_tantivy_bundle_rows(
            artifact_path,
            bundle_id,
            generation,
            lease,
            permit,
            addresses,
        )
        .await?;
        if contents.len() != entries.len() {
            return Err(AppError::Config(
                "Tantivy hydration returned an unexpected row count".into(),
            ));
        }
        for ((index, _), content) in entries.into_iter().zip(contents) {
            rows[index].row.content = content;
            rows[index].row.tantivy_doc_address = None;
        }
    }
    Ok(())
}

async fn search_issue_content_mixed(
    pool: &sqlx::SqlitePool,
    data_root: &std::path::Path,
    registry: &GenerationLeaseRegistry,
    query_permits: Arc<tokio::sync::Semaphore>,
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
        retain_issue_row(&mut retained, row, window.limit, None);
    }
    let mut total = sqlite_result.total;
    let bundles: Vec<IssueBundleSearchRow> = sqlx::query_as(
        "SELECT b.id, b.hash, COALESCE(si.backend, 'sqlite_fts'), COALESCE(si.state, 'LEGACY'), COALESCE(si.generation, 0), COALESCE(si.visibility_revision, 0), COALESCE(si.compacted_revision, 0), si.schema_version, si.tokenizer_version FROM bundles b LEFT JOIN bundle_search_indexes si ON si.bundle_id = b.id WHERE b.issue_code = ? AND b.status = 'READY' ORDER BY b.id",
    )
    .bind(issue_code)
    .fetch_all(pool)
    .await
    .map_err(AppError::Database)?;
    for (bundle_id, _, backend, state, _, _, _, schema_version, tokenizer_version) in &bundles {
        if backend != "tantivy" {
            continue;
        }
        if !matches!(state.as_str(), "READY" | "NEEDS_REBUILD") {
            return Err(AppError::Conflict(format!(
                "Bundle {bundle_id} search index is not ready"
            )));
        }
        if *schema_version != Some(crate::search::publication::TANTIVY_SCHEMA_VERSION)
            || *tokenizer_version != Some(crate::search::publication::TANTIVY_TOKENIZER_VERSION)
        {
            return Err(AppError::Conflict(
                "Bundle search index version is unsupported; rebuild is required".into(),
            ));
        }
    }
    let jobs = bundles
        .into_iter()
        .filter(|bundle| bundle.2 == "tantivy")
        .collect::<Vec<_>>();
    let pool_for_jobs = pool.clone();
    let data_root = data_root.to_path_buf();
    let registry = registry.clone();
    let registry_for_jobs = registry.clone();
    let request_for_jobs = request.clone();
    let mut results =
        stream_issue_bundle_searches(jobs, query_permits.clone(), move |bundle, permit| {
            let pool = pool_for_jobs.clone();
            let data_root = data_root.clone();
            let registry = registry_for_jobs.clone();
            let request = request_for_jobs.clone();
            async move {
                let (
                    bundle_id,
                    bundle_hash,
                    _,
                    state,
                    generation,
                    visibility_revision,
                    compacted_revision,
                    schema_version,
                    tokenizer_version,
                ) = bundle;
                debug_assert!(matches!(state.as_str(), "READY" | "NEEDS_REBUILD"));
                debug_assert_eq!(
                    schema_version,
                    Some(crate::search::publication::TANTIVY_SCHEMA_VERSION)
                );
                debug_assert_eq!(
                    tokenizer_version,
                    Some(crate::search::publication::TANTIVY_TOKENIZER_VERSION)
                );
                let artifact = artifact_relative_path(&bundle_id, generation)?;
                let artifact_path = data_root.join(&artifact);
                let lease = acquire_generation_lease_with_registry(
                    &registry, &pool, &bundle_id, generation,
                )
                .await?;
                let search_request = ContentSearchRequest {
                    scope: ContentSearchScope::Bundle {
                        bundle_id: bundle_id.clone(),
                        timeline: None,
                        file_id: None,
                    },
                    query: request.query,
                    path_like: request.path_like,
                    from: 0,
                    size: candidate_limit as i64,
                    include_content: false,
                };
                let result = if can_skip_visibility_snapshot(
                    &state,
                    visibility_revision,
                    compacted_revision,
                ) {
                    search_tantivy_bundle_with_lease_and_permit(
                        artifact_path.clone(),
                        search_request,
                        bundle_id.clone(),
                        generation,
                        lease,
                        permit,
                    )
                    .await?
                } else {
                    let visible_file_ids = snapshot_file_ids(&pool, &bundle_id).await?;
                    search_tantivy_bundle_visible_with_lease_and_permit(
                        artifact_path.clone(),
                        search_request,
                        visible_file_ids,
                        bundle_id.clone(),
                        generation,
                        lease,
                        permit,
                    )
                    .await?
                };
                Ok((bundle_id, bundle_hash, generation, artifact_path, result))
            }
        });
    while let Some((_index, result)) = results.next().await {
        let (bundle_id, bundle_hash, generation, artifact_path, result) = result?;
        total = total.saturating_add(result.total);
        for mut row in result.rows {
            row.bundle_hash = Some(bundle_hash.clone());
            retain_issue_row(
                &mut retained,
                row,
                window.limit,
                Some(TantivyIssueRowContext {
                    bundle_id: bundle_id.clone(),
                    generation,
                    artifact_path: artifact_path.clone(),
                }),
            );
        }
    }
    let mut ranked_rows: Vec<_> = retained.into_iter().collect();
    ranked_rows.sort_by_key(|ranked| ranked.key.clone());
    let mut page: Vec<RankedIssueRow> = ranked_rows
        .into_iter()
        .skip(window.from)
        .take(window.size)
        .collect();
    hydrate_issue_rows(&mut page, pool, &registry, &query_permits).await?;
    let rows = page.into_iter().map(|ranked| ranked.row).collect();
    Ok(ContentSearchResult {
        total,
        rows,
        truncated: false,
    })
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
