use super::common::checked_page_end;
use super::lifecycle::abort_staging_result;
use super::lifecycle::load_and_renew;
use super::lifecycle::load_record;
use super::lifecycle::{check_temp_result_rate_limit, preview_page_size, to_response};
use super::repository::{
    TransitionResult, claim_active_for_delete, delete_deleting_record, ensure_temp_result_budget,
    insert_staging_temp_result, publish_temp_result,
};
use super::storage::checked_temp_path;
use super::storage::invalid_sidecar;
use super::storage::{
    read_indexed_lines, remove_result_files, result_storage_size, staging_path,
    temp_result_too_large,
};
use super::*;
use crate::services::temp_results::MaterializedPreview;

enum MaterializeMode {
    Full,
    Preview { from: i64, size: i64 },
}

struct MaterializeOutcome {
    id: String,
    total: i64,
    lines: Vec<PreviewLine>,
}

async fn materialize_result(
    state: &web::Data<AppState>,
    expression_text: &str,
    expression: &log_expression::Expression,
    sources: &[TempSource],
    source_label: &str,
    mode: MaterializeMode,
) -> Result<MaterializeOutcome, AppError> {
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
    insert_staging_temp_result(state, &id, expression_text, source_label, &output_path).await?;
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
        let materialized = match mode {
            MaterializeMode::Full => TempResultExecutor::write_matches(
                sources,
                expression,
                &mut output,
                &mut metadata,
                &mut index,
                state.limits.temp_results.max_result_size,
            )
            .await
            .map(|total| MaterializedPreview {
                total,
                lines: Vec::new(),
            }),
            MaterializeMode::Preview { from, size } => {
                TempResultExecutor::materialize_preview(
                    sources,
                    expression,
                    from,
                    size,
                    state.limits.temp_results.max_result_size,
                    &mut output,
                    &mut metadata,
                    &mut index,
                )
                .await
            }
        }?;
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
        tokio::fs::rename(&staging_output_path, &output_path)
            .await
            .map_err(AppError::Io)?;
        tokio::fs::rename(&staging_meta_path, &meta_path)
            .await
            .map_err(AppError::Io)?;
        tokio::fs::rename(&staging_index_path, &index_path)
            .await
            .map_err(AppError::Io)?;
        let transition = publish_temp_result(
            state,
            &id,
            expression_text,
            source_label,
            &output_path,
            materialized.total,
            i64::try_from(size_bytes).map_err(|_| temp_result_too_large())?,
        )
        .await?;
        if !matches!(transition, TransitionResult::Applied(())) {
            return Err(AppError::NotFound(format!("temporary result {id}")));
        }
        Ok::<MaterializedPreview, AppError>(materialized)
    }
    .await;
    match result {
        Ok(materialized) => Ok(MaterializeOutcome {
            id,
            total: materialized.total,
            lines: materialized.lines,
        }),
        Err(error) => {
            abort_staging_result(state, &id, &output_path).await;
            Err(error)
        }
    }
}

#[derive(FromRow)]
struct IssueSourceRow {
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
) -> Result<Vec<TempSource>, AppError> {
    if let Some(source_id) = payload.source_temp_id.as_deref() {
        let source = load_and_renew(state, source_id).await?;
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
        return Ok(vec![TempSource {
            path,
            metadata_path: Some(meta_path),
            label: source.name,
            bundle_hash: None,
            file_id: None,
        }]);
    }
    if let Some(issue_code) = payload.issue_code.as_deref() {
        let issue_code = normalize_issue_code(issue_code)?;
        let rows = sqlx::query_as::<_, IssueSourceRow>(
            r#"
            SELECT f.id, f.name, f.path, f.size_bytes, f.line_count, f.mime_type,
                   f.status, f.meta, f.blob_id, bl.storage_backend, bl.storage_key,
                   bl.state AS blob_state,
                   b.hash AS bundle_hash
            FROM files f
            JOIN bundles b ON b.id = f.bundle_id
            LEFT JOIN blobs bl ON bl.id = f.blob_id
            WHERE b.issue_code = ? AND b.status = 'READY' AND f.is_dir = 0
              AND EXISTS (SELECT 1 FROM log_segments ls WHERE ls.file_id = f.id)
            ORDER BY b.created_at, f.path
            "#,
        )
        .bind(&issue_code)
        .fetch_all(&state.db.pool)
        .await
        .map_err(AppError::Database)?;
        let mut sources = Vec::new();
        for row in rows {
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
                path: resolve_file_path(&file, state.storage.blob_store.as_ref()).await?,
                metadata_path: None,
                label: file.name.clone(),
                bundle_hash: Some(row.bundle_hash),
                file_id: Some(file.id.to_string()),
            });
        }
        if sources.is_empty() {
            return Err(AppError::NotFound(format!(
                "ready log files for issue {issue_code}"
            )));
        }
        return Ok(sources);
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
    let bundle = load_bundle(&state.db.pool, bundle_hash).await?;
    ensure_bundle_ready(&bundle)?;
    let file = fetch_file(&state.db.pool, &bundle.id, file_id).await?;
    ensure_text_preview(&file)?;
    let path = resolve_file_path(&file, state.storage.blob_store.as_ref()).await?;
    Ok(vec![TempSource {
        path,
        metadata_path: None,
        label: file.name,
        bundle_hash: Some(bundle.hash),
        file_id: Some(file.id.to_string()),
    }])
}

pub(crate) fn source_label(sources: &[TempSource]) -> String {
    if sources.len() == 1 {
        sources[0].label.clone()
    } else {
        format!("{} 个源文件", sources.len())
    }
}

pub(crate) async fn create_preview_result(
    request: HttpRequest,
    payload: web::Json<PreviewTempResultRequest>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    check_temp_result_rate_limit(&state, &request)?;
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
    let expression = log_expression::parse(expression_text).map_err(invalid_expression)?;
    let request = CreateTempResultRequest {
        expression: expression_text.to_string(),
        bundle_hash: payload.bundle_hash.clone(),
        file_id: payload.file_id.clone(),
        issue_code: payload.issue_code.clone(),
        source_temp_id: payload.source_temp_id.clone(),
    };
    let sources = resolve_sources(&request, &state).await?;
    let source_label = source_label(&sources);
    let outcome = materialize_result(
        &state,
        expression_text,
        &expression,
        &sources,
        &source_label,
        MaterializeMode::Preview {
            from: payload.from.unwrap_or(0).max(0),
            size: preview_page_size(payload.size),
        },
    )
    .await?;
    if let Some(issue_code) = payload.issue_code.as_deref() {
        touch_issue_activity(&state.db.pool, &normalize_issue_code(issue_code)?).await?;
    }
    Ok(HttpResponse::Ok().json(MaterializedPreviewResponse {
        result_id: outcome.id,
        total: outcome.total,
        lines: outcome.lines,
    }))
}

pub(crate) async fn create_full_result(
    request: HttpRequest,
    payload: web::Json<CreateTempResultRequest>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    check_temp_result_rate_limit(&state, &request)?;
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
    let expression = log_expression::parse(expression_text).map_err(invalid_expression)?;
    let sources = resolve_sources(&payload, &state).await?;
    let source_label = source_label(&sources);
    let outcome = materialize_result(
        &state,
        expression_text,
        &expression,
        &sources,
        &source_label,
        MaterializeMode::Full,
    )
    .await?;
    if let Some(issue_code) = payload.issue_code.as_deref() {
        touch_issue_activity(&state.db.pool, &normalize_issue_code(issue_code)?).await?;
    }
    let result = load_and_renew(&state, &outcome.id).await?;
    Ok(HttpResponse::Created().json(to_response(result)))
}

pub(crate) async fn get_result(
    id: web::Path<String>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    let result = load_and_renew(&state, &id).await?;
    Ok(HttpResponse::Ok().json(to_response(result)))
}

pub(crate) async fn get_result_lines(
    id: web::Path<String>,
    query: web::Query<LinesQuery>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    let result = load_and_renew(&state, &id).await?;
    let start = query.start.unwrap_or(0).max(0);
    let limit = query
        .limit
        .unwrap_or(state.limits.api.default_line_page_size)
        .clamp(1, state.limits.api.max_line_page_size);
    let result_path = checked_temp_path(&state, &result.storage_path)?;
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
    let lines = read_indexed_lines(
        &result_path,
        &meta_path,
        &index_path,
        start,
        limit,
        result.line_count,
    )
    .await?;
    let next_start = if start
        .checked_add(lines.len() as i64)
        .is_some_and(|end| end < result.line_count)
    {
        Some(checked_page_end(start, limit)?)
    } else {
        None
    };
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
    let result = load_and_renew(&state, &id).await?;
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
    let path = checked_temp_path(&state, &result.storage_path)?;
    remove_result_files(&path).await?;
    match delete_deleting_record(&state, &result.id).await? {
        TransitionResult::Applied(()) => {}
        TransitionResult::NotFound | TransitionResult::StateMismatch => {}
    }
    Ok(HttpResponse::NoContent().finish())
}
