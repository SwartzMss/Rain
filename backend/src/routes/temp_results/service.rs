use super::lifecycle::load_and_renew;
use super::storage::checked_temp_path;
use super::storage::invalid_sidecar;
use super::*;

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
        .fetch_all(&state.pool)
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
                path: resolve_file_path(&file, state.blob_store.as_ref()).await?,
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
    let bundle = load_bundle(&state.pool, bundle_hash).await?;
    ensure_bundle_ready(&bundle)?;
    let file = fetch_file(&state.pool, &bundle.id, file_id).await?;
    ensure_text_preview(&file)?;
    let path = resolve_file_path(&file, state.blob_store.as_ref()).await?;
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
