use super::common::checked_page_end;
use super::lifecycle::{
    abort_staging_result, acquire_materialization_lease, acquire_materialization_lease_for_client,
};
use super::lifecycle::{
    acquire_active_result, finish_deleting_temp_result, load_active_unexpired_record, load_record,
};
use super::lifecycle::{check_temp_result_rate_limit, preview_page_size, to_response};
use super::repository::{
    TransitionResult, claim_active_for_delete, delete_deleting_record, ensure_temp_result_budget,
    insert_staging_temp_result_with_retention, publish_temp_result_with_retention,
};
use super::search_plan::{
    IndexedSource, PreviewSearchPlan, build_source_search_plans_with_context,
};
use super::storage::checked_temp_path;
use super::storage::invalid_sidecar;
use super::storage::{
    read_indexed_lines_bounded, remove_result_files, result_storage_size, staging_path,
    temp_result_too_large,
};
use super::*;
use crate::auth::extractor::OptionalUser;
use crate::services::search_execution::{
    SearchExecutionContext, SearchExecutionRegistry, StopReason, TerminalStatus,
};
use crate::services::temp_results::SourceSearchPlan;
use crate::services::temp_results::scan_timeout;
use futures_util::TryStreamExt;

enum MaterializeMode {
    Full,
    Preview,
}

impl MaterializeMode {
    fn retention(&self) -> Duration {
        match self {
            Self::Full => Duration::days(RETENTION_DAYS),
            Self::Preview => Duration::minutes(30),
        }
    }
}

struct MaterializeOutcome {
    id: String,
    total: i64,
}

struct SearchExecutionGuard {
    registry: SearchExecutionRegistry,
    search_id: String,
    token: crate::services::search_execution::CancellationToken,
    finished: bool,
}

struct SearchExecutionHandlerGuard {
    registry: SearchExecutionRegistry,
    search_id: String,
    token: crate::services::search_execution::CancellationToken,
}

impl SearchExecutionHandlerGuard {
    fn new(registry: SearchExecutionRegistry, context: &SearchExecutionContext) -> Self {
        Self {
            registry,
            search_id: context.search_id.clone(),
            token: context.cancellation_token(),
        }
    }
}

impl Drop for SearchExecutionHandlerGuard {
    fn drop(&mut self) {
        self.token.cancel();
        self.registry.cancel_from_handler(&self.search_id);
    }
}

impl SearchExecutionGuard {
    fn new(registry: SearchExecutionRegistry, context: SearchExecutionContext) -> Self {
        let search_id = context.search_id.clone();
        let token = context.cancellation_token();
        Self {
            registry,
            search_id,
            token,
            finished: false,
        }
    }

    fn finish(&mut self, requested: TerminalStatus) -> TerminalStatus {
        if self.finished {
            return requested;
        }
        self.finished = true;
        if !matches!(requested, TerminalStatus::Completed) {
            self.token.cancel();
        }
        self.registry
            .finish(&self.search_id, requested)
            .unwrap_or(requested)
    }

    fn finish_result<T>(&mut self, result: &Result<T, AppError>) -> TerminalStatus {
        let status = match result {
            Ok(_) => TerminalStatus::Completed,
            Err(error) if is_search_cancelled(error) => TerminalStatus::Cancelled,
            Err(error) if is_scan_timeout(error) => TerminalStatus::TimedOut,
            Err(_) => TerminalStatus::Failed,
        };
        self.finish(status)
    }
}

impl Drop for SearchExecutionGuard {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let status = if self.token.is_cancelled() {
            TerminalStatus::Cancelled
        } else {
            TerminalStatus::Failed
        };
        self.finish(status);
    }
}

pub(crate) struct ResolvedSources {
    sources: Vec<TempSource>,
    indexed_sources: Vec<Option<IndexedSource>>,
    deferred_files: Option<Vec<FileRow>>,
    _source_lease: Option<TempResultReadLease>,
}

async fn materialize_result(
    state: &web::Data<AppState>,
    expression_text: &str,
    expression: &log_expression::Expression,
    sources: &[TempSource],
    source_label: &str,
    mode: MaterializeMode,
    preview_plan: Option<&PreviewSearchPlan>,
) -> Result<MaterializeOutcome, AppError> {
    materialize_result_with_context(
        state,
        expression_text,
        expression,
        sources,
        source_label,
        mode,
        preview_plan,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn materialize_result_with_context(
    state: &web::Data<AppState>,
    expression_text: &str,
    expression: &log_expression::Expression,
    sources: &[TempSource],
    source_label: &str,
    mode: MaterializeMode,
    preview_plan: Option<&PreviewSearchPlan>,
    context: Option<&SearchExecutionContext>,
) -> Result<MaterializeOutcome, AppError> {
    let settings = state.settings.snapshot().await;
    materialize_result_with_timeout_and_context(
        state,
        expression_text,
        expression,
        sources,
        source_label,
        mode,
        preview_plan,
        std::time::Duration::from_secs(settings.effective.temp_results_max_scan_duration_seconds),
        context,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
#[allow(dead_code)]
async fn materialize_result_with_timeout(
    state: &web::Data<AppState>,
    expression_text: &str,
    expression: &log_expression::Expression,
    sources: &[TempSource],
    source_label: &str,
    mode: MaterializeMode,
    preview_plan: Option<&PreviewSearchPlan>,
    timeout: std::time::Duration,
) -> Result<MaterializeOutcome, AppError> {
    materialize_result_with_timeout_and_context(
        state,
        expression_text,
        expression,
        sources,
        source_label,
        mode,
        preview_plan,
        timeout,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn materialize_result_with_timeout_and_context(
    state: &web::Data<AppState>,
    expression_text: &str,
    expression: &log_expression::Expression,
    sources: &[TempSource],
    source_label: &str,
    mode: MaterializeMode,
    preview_plan: Option<&PreviewSearchPlan>,
    timeout: std::time::Duration,
    context: Option<&SearchExecutionContext>,
) -> Result<MaterializeOutcome, AppError> {
    let settings = state.settings.snapshot().await;
    checkpoint(context)?;
    let id = Uuid::new_v4().simple().to_string();
    let directory = data_root(state).join("temp-results");
    tokio::fs::create_dir_all(&directory)
        .await
        .map_err(AppError::Io)?;
    let output_path = directory.join(format!("{id}.log"));
    let meta_path = output_path.with_extension("meta");
    let index_path = output_path.with_extension("idx");
    let staging_output_path = staging_path(&output_path);
    let staging_meta_path = staging_path(&meta_path);
    let staging_index_path = staging_path(&index_path);
    let _staging_lease = register_staging_lease(state, &id);
    let retention = mode.retention();
    insert_staging_temp_result_with_retention(
        state,
        &id,
        expression_text,
        source_label,
        &output_path,
        retention,
    )
    .await?;
    let result = async {
        let mut output = File::create(&staging_output_path)
            .await
            .map_err(AppError::Io)?;
        let mut metadata = File::create(&staging_meta_path)
            .await
            .map_err(AppError::Io)?;
        let mut index = File::create(&staging_index_path)
            .await
            .map_err(AppError::Io)?;
        let effective_timeout = context.map_or(timeout, |context| timeout.min(context.remaining()));
        let total = tokio::time::timeout(effective_timeout, async {
            checkpoint(context)?;
            if let Some(plan) = preview_plan {
                let preview = TempResultExecutor::materialize_preview_with_plans_and_context(
                    sources,
                    &plan.source_plans,
                    expression,
                    0,
                    0,
                    settings.effective.temp_results_max_result_size,
                    &mut output,
                    &mut metadata,
                    &mut index,
                    context,
                )
                .await?;
                tracing::info!(
                    metric = "temp_result_preview",
                    search_backend = plan.backend_label(),
                    candidate_count = plan.candidate_count,
                    candidate_strategy = plan.candidate_strategy,
                    candidate_term_count = plan.candidate_term_count,
                    boolean_node_count = plan.boolean_node_count,
                    verified_match_count = preview.total,
                    query_elapsed_ms = plan.query_elapsed_ms.min(u64::MAX as u128) as u64,
                    fallback_reasons = ?plan.fallback_reasons,
                    "completed temporary result preview"
                );
                Ok::<i64, AppError>(preview.total)
            } else {
                TempResultExecutor::write_matches_with_context(
                    sources,
                    expression,
                    &mut output,
                    &mut metadata,
                    &mut index,
                    settings.effective.temp_results_max_result_size,
                    context,
                )
                .await
            }
        })
        .await
        .map_err(|_| stop_or_timeout(context))??;
        checkpoint(context)?;
        drop(output);
        drop(metadata);
        drop(index);
        let size_bytes = result_storage_size(
            &staging_output_path,
            &staging_meta_path,
            &staging_index_path,
        )
        .await
        .map_err(AppError::Io)?;
        if let Some(context) = context {
            state
                .temp_results
                .search_executions
                .mark_committing(&context.search_id, context)
                .map_err(StopReason::into_error)?;
        }
        tokio::fs::rename(&staging_output_path, &output_path)
            .await
            .map_err(AppError::Io)?;
        tokio::fs::rename(&staging_meta_path, &meta_path)
            .await
            .map_err(AppError::Io)?;
        tokio::fs::rename(&staging_index_path, &index_path)
            .await
            .map_err(AppError::Io)?;
        let transition = publish_temp_result_with_retention(
            state,
            &id,
            expression_text,
            source_label,
            &output_path,
            total,
            i64::try_from(size_bytes).map_err(|_| temp_result_too_large())?,
            retention,
        )
        .await?;
        if !matches!(transition, TransitionResult::Applied(())) {
            return Err(AppError::NotFound(format!("temporary result {id}")));
        }
        Ok::<i64, AppError>(total)
    }
    .await;
    match result {
        Ok(total) => Ok(MaterializeOutcome { id, total }),
        Err(error) => {
            abort_staging_result(state, &id, &output_path).await;
            Err(error)
        }
    }
}

#[derive(FromRow)]
struct IssueSourceRow {
    bundle_id: String,
    id: i64,
    name: String,
    path: String,
    size_bytes: Option<i64>,
    line_count: Option<i64>,
    mime_type: Option<String>,
    status: Option<String>,
    meta: Option<String>,
    blob_id: Option<i64>,
    storage_backend: Option<String>,
    storage_key: Option<String>,
    blob_state: Option<String>,
    bundle_hash: String,
}

pub(crate) async fn resolve_sources(
    payload: &CreateTempResultRequest,
    state: &web::Data<AppState>,
) -> Result<ResolvedSources, AppError> {
    let mut resolved = resolve_sources_with_context(payload, state, None).await?;
    resolved.resolve_deferred_paths(state, None, None).await?;
    Ok(resolved)
}

pub(crate) async fn resolve_sources_with_context(
    payload: &CreateTempResultRequest,
    state: &web::Data<AppState>,
    context: Option<&SearchExecutionContext>,
) -> Result<ResolvedSources, AppError> {
    let settings = state.settings.snapshot().await;
    checkpoint(context)?;
    tokio::time::timeout(
        std::time::Duration::from_secs(settings.effective.temp_results_max_scan_duration_seconds),
        resolve_sources_inner(payload, state, context),
    )
    .await
    .map_err(|_| stop_or_timeout(context))?
}

async fn resolve_sources_inner(
    payload: &CreateTempResultRequest,
    state: &web::Data<AppState>,
    context: Option<&SearchExecutionContext>,
) -> Result<ResolvedSources, AppError> {
    checkpoint(context)?;
    if let Some(source_id) = payload.source_temp_id.as_deref() {
        let (source, source_lease) = acquire_active_result(state, source_id).await?;
        let path = checked_temp_path(state, &source.storage_path)?;
        let meta_path = path.with_extension("meta");
        let index_path = path.with_extension("idx");
        let has_meta = tokio::fs::try_exists(&meta_path)
            .await
            .map_err(AppError::Io)?;
        let has_index = tokio::fs::try_exists(&index_path)
            .await
            .map_err(AppError::Io)?;
        if !has_meta || !has_index {
            return Err(invalid_sidecar(
                "temporary result metadata or index is missing",
            ));
        }
        return Ok(ResolvedSources {
            sources: vec![TempSource {
                path,
                metadata_path: Some(meta_path),
                label: source.name,
                bundle_hash: None,
                file_id: None,
            }],
            indexed_sources: vec![None],
            deferred_files: None,
            _source_lease: Some(source_lease),
        });
    }
    if let Some(issue_code) = payload.issue_code.as_deref() {
        let issue_code = normalize_issue_code(issue_code)?;
        let mut rows = sqlx::query_as::<_, IssueSourceRow>(
            r#"
            SELECT b.id AS bundle_id, f.id, f.name, f.path, f.size_bytes, f.line_count, f.mime_type,
                   f.status, f.meta, f.blob_id, bl.storage_backend, bl.storage_key,
                   bl.state AS blob_state,
                   b.hash AS bundle_hash
            FROM visible_files f
            JOIN bundles b ON b.id = f.bundle_id
            JOIN issues i ON i.code = b.issue_code
            LEFT JOIN blobs bl ON bl.id = f.blob_id
            WHERE b.issue_code = ? AND i.status = 'ACTIVE' AND b.status = 'READY' AND f.is_dir = 0
              AND EXISTS (SELECT 1 FROM log_segments ls WHERE ls.file_id = f.id)
            ORDER BY b.created_at, f.path
            "#,
        )
        .bind(&issue_code)
        .fetch(&state.db.pool);
        let mut sources = Vec::new();
        let mut indexed_sources = Vec::new();
        let mut deferred_files = Vec::new();
        while let Some(row) = rows.try_next().await.map_err(AppError::Database)? {
            checkpoint(context)?;
            let file = FileRow {
                id: row.id,
                parent_id: None,
                name: row.name,
                path: row.path,
                is_dir: false,
                size_bytes: row.size_bytes,
                line_count: row.line_count,
                mime_type: row.mime_type,
                status: row.status,
                meta: row.meta,
                blob_id: row.blob_id,
                storage_backend: row.storage_backend,
                storage_key: row.storage_key,
                blob_state: row.blob_state,
            };
            sources.push(TempSource {
                path: std::path::PathBuf::new(),
                metadata_path: None,
                label: file.name.clone(),
                bundle_hash: Some(row.bundle_hash),
                file_id: Some(file.id.to_string()),
            });
            let file_id = file.id;
            deferred_files.push(file);
            indexed_sources.push(Some(IndexedSource {
                bundle_id: row.bundle_id,
                file_id,
            }));
        }
        if sources.is_empty() {
            return Err(AppError::NotFound(format!(
                "ready log files for issue {issue_code}"
            )));
        }
        return Ok(ResolvedSources {
            sources,
            indexed_sources,
            deferred_files: Some(deferred_files),
            _source_lease: None,
        });
    }
    let bundle_hash = payload
        .bundle_hash
        .as_deref()
        .ok_or_else(|| AppError::BadRequest("bundle_hash is required".into()))?;
    let file_id = payload
        .file_id
        .as_deref()
        .ok_or_else(|| AppError::BadRequest("file_id is required".into()))?
        .parse::<i64>()
        .map_err(|_| AppError::BadRequest("invalid file_id".into()))?;
    checkpoint(context)?;
    let bundle = load_bundle(&state.db.pool, bundle_hash).await?;
    ensure_bundle_ready(&bundle)?;
    let file = fetch_file(&state.db.pool, &bundle.id, file_id).await?;
    ensure_text_preview(&file)?;
    let path = resolve_file_path(&file, state.storage.blob_store.as_ref()).await?;
    Ok(ResolvedSources {
        sources: vec![TempSource {
            path,
            metadata_path: None,
            label: file.name,
            bundle_hash: Some(bundle.hash),
            file_id: Some(file.id.to_string()),
        }],
        indexed_sources: vec![Some(IndexedSource {
            bundle_id: bundle.id,
            file_id,
        })],
        deferred_files: None,
        _source_lease: None,
    })
}

impl ResolvedSources {
    async fn resolve_deferred_paths(
        &mut self,
        state: &web::Data<AppState>,
        plan: Option<&PreviewSearchPlan>,
        context: Option<&SearchExecutionContext>,
    ) -> Result<(), AppError> {
        let Some(files) = self.deferred_files.take() else {
            return Ok(());
        };
        for (index, file) in files.into_iter().enumerate() {
            checkpoint(context)?;
            let should_resolve = plan.is_none_or(|plan| match plan.source_plans.get(index) {
                Some(SourceSearchPlan::Tantivy(candidate)) => !candidate.ranges.is_empty(),
                Some(SourceSearchPlan::Raw { .. }) | None => true,
            });
            if should_resolve {
                self.sources[index].path =
                    resolve_file_path(&file, state.storage.blob_store.as_ref()).await?;
            }
        }
        Ok(())
    }
}

pub(crate) fn source_label(sources: &[TempSource]) -> String {
    if sources.len() == 1 {
        sources[0].label.clone()
    } else {
        format!("{} 个源文件", sources.len())
    }
}

async fn read_result_page(
    state: &web::Data<AppState>,
    result: &TempResultRecord,
    start: i64,
    limit: i64,
) -> Result<(Vec<TempLine>, Option<i64>), AppError> {
    let settings = state.settings.snapshot().await;
    let result_path = checked_temp_path(state, &result.storage_path)?;
    let meta_path = result_path.with_extension("meta");
    let index_path = result_path.with_extension("idx");
    let has_meta = tokio::fs::try_exists(&meta_path)
        .await
        .map_err(AppError::Io)?;
    let has_index = tokio::fs::try_exists(&index_path)
        .await
        .map_err(AppError::Io)?;
    if !has_meta || !has_index {
        return Err(invalid_sidecar(
            "temporary result metadata or index is missing",
        ));
    }
    let lines = read_indexed_lines_bounded(
        &result_path,
        &meta_path,
        &index_path,
        start,
        limit,
        result.line_count,
        settings.effective.api_max_line_page_bytes,
    )
    .await?;
    let next_start = if start
        .checked_add(lines.len() as i64)
        .is_some_and(|end| end < result.line_count)
    {
        Some(checked_page_end(start, lines.len() as i64)?)
    } else {
        None
    };
    Ok((lines, next_start))
}

async fn cleanup_published_result_after_preview_failure(
    state: &web::Data<AppState>,
    result: &TempResultRecord,
) {
    match claim_active_for_delete(state, &result.id).await {
        Ok(TransitionResult::Applied(())) => finish_deleting_temp_result(state, result).await,
        Ok(TransitionResult::NotFound | TransitionResult::StateMismatch) => {}
        Err(error) => tracing::warn!(
            result_id = %result.id,
            %error,
            "published temporary result could not be claimed after preview failure"
        ),
    }
}

pub(crate) async fn create_preview_result(
    user: OptionalUser,
    request: HttpRequest,
    payload: web::Json<PreviewTempResultRequest>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    let settings = state.settings.snapshot().await;
    let client_key = request_client_key(&request);
    let payload = payload.into_inner();
    let context = if let Some(search_id) = payload.search_id.as_deref() {
        let Some(cancel_token) = request
            .headers()
            .get(crate::routes::search_requests::CANCEL_TOKEN_HEADER)
            .and_then(|value| value.to_str().ok())
            .filter(|value| !value.is_empty())
        else {
            return Err(AppError::api(
                StatusCode::CONFLICT,
                "SEARCH_REQUEST_UNAVAILABLE",
                "搜索请求不可用",
            ));
        };
        state
            .temp_results
            .search_executions
            .start_with_timeout(
                search_id,
                cancel_token,
                user.0.as_ref().map(|user| user.id.as_str()),
                std::time::Duration::from_secs(
                    settings.effective.temp_results_max_scan_duration_seconds,
                ),
            )
            .map(Some)
            .map_err(|_| {
                AppError::api(
                    StatusCode::CONFLICT,
                    "SEARCH_REQUEST_UNAVAILABLE",
                    "搜索请求不可用",
                )
            })?
    } else {
        check_temp_result_rate_limit(&state, &request)?;
        None
    };
    let Some(context) = context else {
        return create_preview_result_inner(&client_key, &payload, &state, None)
            .await
            .map(|response| HttpResponse::Ok().json(response));
    };

    let worker_registry = state.temp_results.search_executions.clone();
    let _handler_guard = SearchExecutionHandlerGuard::new(worker_registry.clone(), &context);
    let worker_state = state.clone();
    let worker_client_key = client_key.clone();
    let worker = tokio::spawn(async move {
        let mut execution_guard =
            SearchExecutionGuard::new(worker_registry.clone(), context.clone());
        let result = create_preview_result_inner(
            &worker_client_key,
            &payload,
            &worker_state,
            Some(&context),
        )
        .await;
        execution_guard.finish_result(&result);
        result
    });
    match worker.await {
        Ok(Ok(response)) => Ok(HttpResponse::Ok().json(response)),
        Ok(Err(error)) => Err(error),
        Err(error) => {
            tracing::error!(%error, "interactive search worker terminated unexpectedly");
            Err(AppError::Config(
                "interactive search worker terminated".into(),
            ))
        }
    }
}

async fn create_preview_result_inner(
    client_key: &str,
    payload: &PreviewTempResultRequest,
    state: &web::Data<AppState>,
    context: Option<&SearchExecutionContext>,
) -> Result<MaterializedPreviewResponse, AppError> {
    let settings = state.settings.snapshot().await;
    let _client_lease = acquire_materialization_lease_for_client(state, client_key)?;
    let _permit = state
        .temp_results
        .permits
        .clone()
        .try_acquire_owned()
        .map_err(|_| {
            AppError::api(
                StatusCode::TOO_MANY_REQUESTS,
                "TEMP_RESULT_BUSY",
                "临时结果生成任务过多，请稍后重试",
            )
        })?;
    ensure_temp_result_budget(state).await?;
    let expression_text = payload.expression.trim();
    let expression = log_expression::parse(expression_text)
        .map_err(|error| invalid_expression(expression_text, error))?;
    let start = payload.from.unwrap_or(0).max(0);
    let limit = preview_page_size(
        payload.size,
        settings.effective.api_default_line_page_size,
        settings.effective.api_max_line_page_size,
    );
    checked_page_end(start, limit)?;
    let request = CreateTempResultRequest {
        expression: expression_text.to_string(),
        bundle_hash: payload.bundle_hash.clone(),
        file_id: payload.file_id.clone(),
        issue_code: payload.issue_code.clone(),
        source_temp_id: payload.source_temp_id.clone(),
    };
    let mut resolved = resolve_sources_with_context(&request, state, context).await?;
    let preview_plan = build_source_search_plans_with_context(
        state,
        &expression,
        &resolved.indexed_sources,
        context,
    )
    .await?;
    resolved
        .resolve_deferred_paths(state, Some(&preview_plan), context)
        .await?;
    let source_label = source_label(&resolved.sources);
    let outcome = materialize_result_with_context(
        state,
        expression_text,
        &expression,
        &resolved.sources,
        &source_label,
        MaterializeMode::Preview,
        Some(&preview_plan),
        context,
    )
    .await?;
    let (result, read_lease) = acquire_active_result(state, &outcome.id).await?;
    let page = read_result_page(state, &result, start, limit).await;
    drop(read_lease);
    let (lines, next_start) = match page {
        Ok(page) => page,
        Err(error) => {
            cleanup_published_result_after_preview_failure(state, &result).await;
            return Err(error);
        }
    };
    Ok(MaterializedPreviewResponse {
        result_id: outcome.id,
        total: outcome.total,
        next_start,
        lines,
    })
}

pub(crate) async fn create_full_result(
    request: HttpRequest,
    payload: web::Json<CreateTempResultRequest>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    check_temp_result_rate_limit(&state, &request)?;
    let _client_lease = acquire_materialization_lease(&state, &request)?;
    let _permit = state
        .temp_results
        .permits
        .clone()
        .try_acquire_owned()
        .map_err(|_| {
            AppError::api(
                StatusCode::TOO_MANY_REQUESTS,
                "TEMP_RESULT_BUSY",
                "临时结果生成任务过多，请稍后重试",
            )
        })?;
    ensure_temp_result_budget(&state).await?;
    let expression_text = payload.expression.trim();
    let expression = log_expression::parse(expression_text)
        .map_err(|error| invalid_expression(expression_text, error))?;
    let resolved = resolve_sources(&payload, &state).await?;
    let source_label = source_label(&resolved.sources);
    let outcome = materialize_result(
        &state,
        expression_text,
        &expression,
        &resolved.sources,
        &source_label,
        MaterializeMode::Full,
        None,
    )
    .await?;
    let result = load_active_unexpired_record(&state, &outcome.id).await?;
    Ok(HttpResponse::Created().json(to_response(result)))
}

pub(crate) async fn get_result(
    id: web::Path<String>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    let result = load_active_unexpired_record(&state, &id).await?;
    Ok(HttpResponse::Ok().json(to_response(result)))
}

pub(crate) async fn get_result_lines(
    request: HttpRequest,
    id: web::Path<String>,
    query: web::Query<LinesQuery>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    let settings = state.settings.snapshot().await;
    let _line_read = state.acquire_line_read(&request_client_key(&request))?;
    let (result, _read_lease) = acquire_active_result(&state, &id).await?;
    let start = query.start.unwrap_or(0).max(0);
    let limit = query
        .limit
        .unwrap_or(settings.effective.api_default_line_page_size)
        .clamp(1, settings.effective.api_max_line_page_size);
    let (lines, next_start) = read_result_page(&state, &result, start, limit).await?;
    Ok(HttpResponse::Ok().json(TempResultLines {
        start,
        limit,
        line_count: result.line_count,
        next_start,
        lines,
    }))
}

pub(crate) async fn open_result_download(
    _user: RequireUser,
    id: web::Path<String>,
    state: web::Data<AppState>,
) -> Result<NamedFile, AppError> {
    let (result, _read_lease) = acquire_active_result(&state, &id).await?;
    let file = NamedFile::open_async(checked_temp_path(&state, &result.storage_path)?)
        .await
        .map_err(AppError::Io)?
        .set_content_disposition(header::ContentDisposition {
            disposition: header::DispositionType::Attachment,
            parameters: vec![header::DispositionParam::Filename(result.name)],
        });
    Ok(file)
}

pub(crate) async fn delete_result(
    _user: RequireBusinessUser,
    id: web::Path<String>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    let result = load_record(&state, &id).await?;
    match claim_active_for_delete(&state, &result.id).await? {
        TransitionResult::Applied(()) => {}
        TransitionResult::NotFound | TransitionResult::StateMismatch => {
            return Err(AppError::NotFound(format!("temporary result {id}")));
        }
    }
    if super::storage::is_read_lease_active(&state, &result.id) {
        return Ok(HttpResponse::NoContent().finish());
    }
    let path = checked_temp_path(&state, &result.storage_path)?;
    remove_result_files(&path).await?;
    match delete_deleting_record(&state, &result.id).await? {
        TransitionResult::Applied(()) => {}
        TransitionResult::NotFound | TransitionResult::StateMismatch => {}
    }
    Ok(HttpResponse::NoContent().finish())
}

fn checkpoint(context: Option<&SearchExecutionContext>) -> Result<(), AppError> {
    if let Some(context) = context {
        context.checkpoint().map_err(StopReason::into_error)?;
    }
    Ok(())
}

fn stop_or_timeout(context: Option<&SearchExecutionContext>) -> AppError {
    context
        .and_then(|context| context.checkpoint().err())
        .map(StopReason::into_error)
        .unwrap_or_else(scan_timeout)
}

fn is_search_cancelled(error: &AppError) -> bool {
    matches!(
        error,
        AppError::Api {
            code: "SEARCH_CANCELLED",
            ..
        }
    )
}

fn is_scan_timeout(error: &AppError) -> bool {
    matches!(
        error,
        AppError::PublicApi {
            code: "TEMP_RESULT_SCAN_TIMEOUT",
            ..
        }
    )
}

#[cfg(test)]
mod tests {
    use std::{
        path::{Path, PathBuf},
        time::Duration,
    };

    use actix_web::web;
    use sqlx::sqlite::SqlitePoolOptions;
    use uuid::Uuid;

    use super::{
        MaterializeMode, materialize_result, materialize_result_with_timeout, resolve_sources,
    };
    use crate::{
        AppState,
        config::AppLimits,
        db,
        error::AppError,
        log_expression,
        routes::temp_results::CreateTempResultRequest,
        services::{search_execution::TerminalStatus, temp_results::TempSource},
    };

    fn test_path(suffix: &str) -> PathBuf {
        std::env::temp_dir().join(format!("rain-temp-materialize-{}-{suffix}", Uuid::new_v4()))
    }

    async fn test_state(root: &Path, limits: AppLimits) -> web::Data<AppState> {
        let pool = SqlitePoolOptions::new()
            .max_connections(2)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        db::prepare_schema(&pool, false).await.unwrap();
        web::Data::new(AppState::new(pool, root.to_path_buf(), limits))
    }

    async fn assert_failed_materialization_is_clean(
        state: &web::Data<AppState>,
        source: TempSource,
        expected: &'static str,
        timeout: Option<Duration>,
    ) {
        let expression = log_expression::parse("ERROR").unwrap();
        let result = match timeout {
            Some(timeout) => {
                materialize_result_with_timeout(
                    state,
                    "ERROR",
                    &expression,
                    &[source],
                    "app.log",
                    MaterializeMode::Full,
                    None,
                    timeout,
                )
                .await
            }
            None => {
                materialize_result(
                    state,
                    "ERROR",
                    &expression,
                    &[source],
                    "app.log",
                    MaterializeMode::Full,
                    None,
                )
                .await
            }
        };
        let error = match result {
            Ok(_) => panic!("materialization should fail with {expected}"),
            Err(error) => error,
        };
        assert!(matches!(
            error,
            AppError::PublicApi { code, .. } if code == expected
        ));
        let staging_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM temp_results WHERE status = 'STAGING'")
                .fetch_one(&state.db.pool)
                .await
                .unwrap();
        assert_eq!(staging_count, 0);
        assert!(state.temp_results.staging.lock().unwrap().is_empty());
        let mut entries = tokio::fs::read_dir(super::data_root(state).join("temp-results"))
            .await
            .unwrap();
        assert!(entries.next_entry().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn output_limit_cleans_staging_record_and_part_files() {
        let root = test_path("scan-limit");
        let source_path = root.join("source.log");
        tokio::fs::create_dir_all(&root).await.unwrap();
        tokio::fs::write(&source_path, "ERROR ".repeat(1024))
            .await
            .unwrap();
        let mut limits = AppLimits::default();
        limits.temp_results.max_result_size = 32;
        let state = test_state(&root, limits).await;

        assert_failed_materialization_is_clean(
            &state,
            TempSource {
                path: source_path,
                metadata_path: None,
                label: "app.log".into(),
                bundle_hash: None,
                file_id: None,
            },
            "TEMP_RESULT_TOO_LARGE",
            None,
        )
        .await;
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn timeout_cleans_staging_record_and_part_files() {
        let root = test_path("timeout");
        let source_path = root.join("source.log");
        tokio::fs::create_dir_all(&root).await.unwrap();
        tokio::fs::write(&source_path, "INFO\n".repeat((32 * 1024 * 1024) / 5))
            .await
            .unwrap();
        let state = test_state(&root, AppLimits::default()).await;

        assert_failed_materialization_is_clean(
            &state,
            TempSource {
                path: source_path,
                metadata_path: None,
                label: "app.log".into(),
                bundle_hash: None,
                file_id: None,
            },
            "TEMP_RESULT_SCAN_TIMEOUT",
            Some(Duration::from_millis(1)),
        )
        .await;
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn context_materialization_commits_the_registered_search_execution() {
        let root = test_path("registered-search");
        let source_path = root.join("source.log");
        tokio::fs::create_dir_all(&root).await.unwrap();
        tokio::fs::write(&source_path, "ERROR registered\n")
            .await
            .unwrap();
        let state = test_state(&root, AppLimits::default()).await;
        let reservation = state
            .temp_results
            .search_executions
            .reserve("search-execution", None, "test-peer")
            .unwrap();
        let context = state
            .temp_results
            .search_executions
            .start_with_timeout(
                &reservation.search_id,
                &reservation.cancel_token,
                None,
                Duration::from_secs(5),
            )
            .unwrap();
        let expression = log_expression::parse("ERROR").unwrap();

        let outcome = super::materialize_result_with_timeout_and_context(
            &state,
            "ERROR",
            &expression,
            &[TempSource {
                path: source_path,
                metadata_path: None,
                label: "app.log".into(),
                bundle_hash: None,
                file_id: None,
            }],
            "app.log",
            MaterializeMode::Full,
            None,
            Duration::from_secs(5),
            Some(&context),
        )
        .await;

        assert!(outcome.is_ok(), "registered preview should publish");
        assert_eq!(state.temp_results.search_executions.active_count(), 1);
        state
            .temp_results
            .search_executions
            .finish(&context.search_id, TerminalStatus::Completed);
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[test]
    fn dropped_preview_execution_guard_finishes_the_registered_execution() {
        let state = crate::services::search_execution::SearchExecutionRegistry::new(
            8,
            8,
            Duration::from_secs(60),
        );
        let reservation = state.reserve("dropped-search", None, "test-peer").unwrap();
        let context = state
            .start(&reservation.search_id, &reservation.cancel_token, None)
            .unwrap();
        let token = context.cancellation_token();

        {
            let _guard = super::SearchExecutionGuard::new(state.clone(), context);
        }

        assert!(token.is_cancelled());
        assert_eq!(state.active_count(), 0);
    }

    #[test]
    fn dropped_preview_handler_guard_marks_the_execution_cancelling() {
        let state = crate::services::search_execution::SearchExecutionRegistry::new(
            8,
            8,
            Duration::from_secs(60),
        );
        let reservation = state
            .reserve("handler-drop-search", None, "test-peer")
            .unwrap();
        let context = state
            .start(&reservation.search_id, &reservation.cancel_token, None)
            .unwrap();
        let token = context.cancellation_token();

        {
            let _guard = super::SearchExecutionHandlerGuard::new(state.clone(), &context);
        }

        assert!(token.is_cancelled());
        assert_eq!(
            state.finish("handler-drop-search", TerminalStatus::Completed),
            Some(TerminalStatus::Cancelled)
        );
    }

    #[tokio::test]
    async fn completed_preview_execution_guard_does_not_cancel_after_finish() {
        let state = crate::services::search_execution::SearchExecutionRegistry::new(
            8,
            8,
            Duration::from_secs(60),
        );
        let reservation = state
            .reserve("completed-search", None, "test-peer")
            .unwrap();
        let context = state
            .start(&reservation.search_id, &reservation.cancel_token, None)
            .unwrap();
        let token = context.cancellation_token();

        let mut guard = super::SearchExecutionGuard::new(state.clone(), context);
        guard.finish(TerminalStatus::Completed);
        drop(guard);

        assert!(!token.is_cancelled());
        assert_eq!(
            state
                .wait_for_terminal("completed-search", Duration::from_secs(1))
                .await,
            Some(TerminalStatus::Completed)
        );
    }

    #[tokio::test]
    async fn issue_source_resolution_accepts_more_than_ten_thousand_files() {
        let root = test_path("source-limit");
        let state = test_state(&root, AppLimits::default()).await;
        sqlx::query("INSERT INTO issues(code, name) VALUES('ISSUE', 'Issue')")
            .execute(&state.db.pool)
            .await
            .unwrap();
        for (bundle_id, file_name) in [("bundle-a", "a.log"), ("bundle-b", "b.log")] {
            sqlx::query(
                "INSERT INTO bundles(id, issue_code, hash, name, status) VALUES(?, 'ISSUE', ?, ?, 'READY')",
            )
            .bind(bundle_id)
            .bind(format!("hash-{bundle_id}"))
            .bind(file_name)
            .execute(&state.db.pool)
            .await
            .unwrap();
            let file_id: i64 = sqlx::query_scalar(
                "INSERT INTO files(bundle_id, name, path, is_dir) VALUES(?, ?, ?, 0) RETURNING id",
            )
            .bind(bundle_id)
            .bind(file_name)
            .bind(format!("/{file_name}"))
            .fetch_one(&state.db.pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO log_segments(bundle_id, file_id, content) VALUES(?, ?, 'ERROR')",
            )
            .bind(bundle_id)
            .bind(file_id)
            .execute(&state.db.pool)
            .await
            .unwrap();
        }

        sqlx::query("WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM n WHERE x < 10001) INSERT INTO files(bundle_id, name, path, is_dir) SELECT 'bundle-a', 'extra-' || x || '.log', '/extra-' || x || '.log', 0 FROM n")
            .execute(&state.db.pool).await.unwrap();
        sqlx::query("INSERT INTO log_segments(bundle_id, file_id, content) SELECT bundle_id, id, 'ERROR' FROM files WHERE name LIKE 'extra-%'")
            .execute(&state.db.pool).await.unwrap();

        tokio::fs::create_dir_all(&root).await.unwrap();
        let source_path = root.join("shared.log");
        tokio::fs::write(&source_path, "ERROR\n").await.unwrap();
        let blob_id = crate::blob_store::persist_blob(
            &state.db.pool,
            state.storage.blob_store.as_ref(),
            &source_path,
        )
        .await
        .unwrap();
        crate::blob_store::mark_blob_ready(
            &state.db.pool,
            state.storage.blob_store.as_ref(),
            blob_id,
        )
        .await
        .unwrap();
        sqlx::query("UPDATE files SET blob_id = ?")
            .bind(blob_id)
            .execute(&state.db.pool)
            .await
            .unwrap();

        let payload = CreateTempResultRequest {
            expression: "ERROR".into(),
            bundle_hash: None,
            file_id: None,
            issue_code: Some("ISSUE".into()),
            source_temp_id: None,
        };
        let resolved = resolve_sources(&payload, &state).await.unwrap();
        assert_eq!(resolved.sources.len(), 10_003);
        let expression = log_expression::parse("ERROR").unwrap();
        let outcome = materialize_result(
            &state,
            "ERROR",
            &expression,
            &resolved.sources,
            "all files",
            MaterializeMode::Preview,
            None,
        )
        .await
        .unwrap();
        assert_eq!(outcome.total, 10_003);
        let record = super::load_active_unexpired_record(&state, &outcome.id)
            .await
            .unwrap();
        let (lines, next) = super::read_result_page(&state, &record, 10_000, 10)
            .await
            .unwrap();
        assert_eq!(lines.len(), 3);
        assert_eq!(next, None);

        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn preview_failure_cleanup_removes_published_result() {
        let root = test_path("preview-failure-cleanup");
        let result_dir = root.join("temp-results");
        tokio::fs::create_dir_all(&result_dir).await.unwrap();
        let state = test_state(&root, AppLimits::default()).await;
        let result_path = result_dir.join("preview-failure.log");
        let meta_path = result_path.with_extension("meta");
        let index_path = result_path.with_extension("idx");
        let timestamp = chrono::Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO temp_results (id, status, name, expression, source_label, storage_path, line_count, size_bytes, created_at, expires_at) VALUES ('preview-failure', 'ACTIVE', 'preview-failure.log', 'ERROR', 'app.log', ?, 1, 10, ?, ?)",
        )
        .bind(result_path.to_string_lossy().to_string())
        .bind(&timestamp)
        .bind(&timestamp)
        .execute(&state.db.pool)
        .await
        .unwrap();
        for path in [&result_path, &meta_path, &index_path] {
            tokio::fs::write(path, b"artifact").await.unwrap();
        }

        let result = super::load_record(&state, "preview-failure").await.unwrap();
        super::cleanup_published_result_after_preview_failure(&state, &result).await;

        let status: Option<String> =
            sqlx::query_scalar("SELECT status FROM temp_results WHERE id = 'preview-failure'")
                .fetch_optional(&state.db.pool)
                .await
                .unwrap();
        assert!(status.is_none());
        for path in [result_path, meta_path, index_path] {
            assert!(!path.exists());
        }
        let _ = tokio::fs::remove_dir_all(root).await;
    }
}
