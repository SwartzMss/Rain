use std::path::{Path, PathBuf};

use actix_files::NamedFile;
use actix_web::{HttpResponse, delete, get, http::header, post, web};
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use tokio::{
    fs::File,
    io::{AsyncBufReadExt, AsyncSeekExt, BufReader, SeekFrom},
};
use uuid::Uuid;

use crate::{
    AppState,
    error::AppError,
    log_expression,
    repositories::files::{FileRow, ensure_text_preview, fetch_file, resolve_file_path},
    services::temp_results::{
        MatchMetadata, PreviewLine, SparseCheckpoint, TempResultExecutor, TempSource,
        select_checkpoint,
    },
};

use super::{
    helpers::{data_root, ensure_bundle_ready, load_bundle},
    issues::normalize_issue_code,
};

const RETENTION_DAYS: i64 = 7;

fn invalid_expression(error: log_expression::ParseError) -> AppError {
    AppError::BadRequest(format!(
        "搜索条件无效，请检查 AND/OR/NOT 前后是否都有关键词（位置 {}：{}）",
        error.offset, error.message
    ))
}

#[derive(Deserialize)]
pub struct CreateTempResultRequest {
    expression: String,
    bundle_hash: Option<String>,
    file_id: Option<String>,
    issue_code: Option<String>,
    source_temp_id: Option<String>,
}

#[derive(Deserialize)]
pub struct PreviewTempResultRequest {
    expression: String,
    bundle_hash: Option<String>,
    file_id: Option<String>,
    issue_code: Option<String>,
    source_temp_id: Option<String>,
    from: Option<i64>,
    size: Option<i64>,
}

#[derive(Serialize, FromRow)]
pub struct TempResult {
    id: String,
    name: String,
    expression: String,
    source_label: String,
    line_count: i64,
    size_bytes: i64,
    created_at: String,
    expires_at: String,
}

#[derive(FromRow)]
struct TempResultRecord {
    id: String,
    name: String,
    expression: String,
    source_label: String,
    storage_path: String,
    line_count: i64,
    size_bytes: i64,
    created_at: String,
    expires_at: String,
}

#[derive(Deserialize)]
struct LinesQuery {
    start: Option<i64>,
    limit: Option<i64>,
}

#[derive(Serialize)]
struct TempResultLines {
    start: i64,
    limit: i64,
    line_count: i64,
    next_start: Option<i64>,
    lines: Vec<TempLine>,
}

#[derive(Debug, Serialize)]
struct TempLine {
    bundle_hash: Option<String>,
    file_id: Option<String>,
    path: Option<String>,
    line_number: i64,
    content: String,
}

#[derive(Serialize)]
struct MaterializedPreviewResponse {
    result_id: String,
    total: i64,
    lines: Vec<PreviewLine>,
}

#[post("/temp-results/preview")]
pub async fn preview_temp_result(
    payload: web::Json<PreviewTempResultRequest>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    cleanup_expired(&state).await?;
    let expression_text = payload.expression.trim();
    let expression = log_expression::parse(expression_text).map_err(invalid_expression)?;
    let source_request = CreateTempResultRequest {
        expression: expression_text.to_string(),
        bundle_hash: payload.bundle_hash.clone(),
        file_id: payload.file_id.clone(),
        issue_code: payload.issue_code.clone(),
        source_temp_id: payload.source_temp_id.clone(),
    };
    let sources = resolve_sources(&source_request, &state).await?;
    let from = payload.from.unwrap_or(0).max(0);
    let size = preview_page_size(payload.size);
    let source_label = source_label(&sources);
    let id = Uuid::new_v4().simple().to_string();
    let directory = data_root(&state).join("temp-results");
    tokio::fs::create_dir_all(&directory)
        .await
        .map_err(AppError::Io)?;
    let output_path = directory.join(format!("{id}.log"));
    let meta_path = output_path.with_extension("meta");
    let index_path = output_path.with_extension("idx");
    let staging_output_path = staging_path(&output_path);
    let staging_meta_path = staging_path(&meta_path);
    let staging_index_path = staging_path(&index_path);
    let materialized = async {
        let mut output = File::create(&staging_output_path)
            .await
            .map_err(AppError::Io)?;
        let mut metadata = File::create(&staging_meta_path)
            .await
            .map_err(AppError::Io)?;
        let mut index = File::create(&staging_index_path)
            .await
            .map_err(AppError::Io)?;
        TempResultExecutor::materialize_preview(
            &sources,
            &expression,
            from,
            size,
            &mut output,
            &mut metadata,
            &mut index,
        )
        .await
    }
    .await;
    let preview = match materialized {
        Ok(preview) => preview,
        Err(error) => {
            remove_preview_artifacts(&output_path).await?;
            return Err(error);
        }
    };
    let metadata = match tokio::fs::metadata(&staging_output_path).await {
        Ok(metadata) => metadata,
        Err(error) => {
            remove_preview_artifacts(&output_path).await?;
            return Err(AppError::Io(error));
        }
    };
    let publish_result = async {
        tokio::fs::rename(&staging_output_path, &output_path)
            .await
            .map_err(AppError::Io)?;
        tokio::fs::rename(&staging_meta_path, &meta_path)
            .await
            .map_err(AppError::Io)?;
        tokio::fs::rename(&staging_index_path, &index_path)
            .await
            .map_err(AppError::Io)?;
        Ok::<(), AppError>(())
    }
    .await;
    if let Err(error) = publish_result {
        remove_preview_artifacts(&output_path).await?;
        return Err(error);
    }
    if let Err(error) = insert_temp_result(
        &state,
        &id,
        expression_text,
        &source_label,
        &output_path,
        preview.total,
        metadata.len() as i64,
    )
    .await
    {
        remove_preview_artifacts(&output_path).await?;
        return Err(error);
    }
    Ok(HttpResponse::Ok().json(MaterializedPreviewResponse {
        result_id: id,
        total: preview.total,
        lines: preview.lines,
    }))
}

#[post("/temp-results")]
pub async fn create_temp_result(
    payload: web::Json<CreateTempResultRequest>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    cleanup_expired(&state).await?;
    let expression_text = payload.expression.trim();
    let expression = log_expression::parse(expression_text).map_err(invalid_expression)?;

    let sources = resolve_sources(&payload, &state).await?;
    let source_label = source_label(&sources);
    let id = Uuid::new_v4().simple().to_string();
    let directory = data_root(&state).join("temp-results");
    tokio::fs::create_dir_all(&directory)
        .await
        .map_err(AppError::Io)?;
    let output_path = directory.join(format!("{id}.log"));
    let mut output = File::create(&output_path).await.map_err(AppError::Io)?;
    let matching_lines =
        TempResultExecutor::write_matches(&sources, &expression, &mut output).await?;

    let metadata = tokio::fs::metadata(&output_path)
        .await
        .map_err(AppError::Io)?;
    let created_at = Utc::now();
    let expires_at = created_at + Duration::days(RETENTION_DAYS);
    let name = format!("filtered-{}.log", &id[..8]);
    let storage_path = output_path.to_string_lossy().to_string();
    let insert_result = sqlx::query(
        r#"
        INSERT INTO temp_results
            (id, name, expression, source_label, storage_path, line_count, size_bytes, created_at, expires_at)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(&id)
    .bind(&name)
    .bind(expression_text)
    .bind(&source_label)
    .bind(storage_path)
    .bind(matching_lines)
    .bind(metadata.len() as i64)
    .bind(created_at.to_rfc3339())
    .bind(expires_at.to_rfc3339())
    .execute(&state.pool)
    .await;
    if let Err(error) = insert_result {
        let _ = tokio::fs::remove_file(&output_path).await;
        return Err(AppError::Database(error));
    }

    let result = load_and_renew(&state, &id).await?;
    Ok(HttpResponse::Created().json(to_response(result)))
}

#[get("/temp-results/{id}")]
pub async fn get_temp_result(
    id: web::Path<String>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    cleanup_expired(&state).await?;
    let result = load_and_renew(&state, &id).await?;
    Ok(HttpResponse::Ok().json(to_response(result)))
}

#[get("/temp-results/{id}/lines")]
pub async fn get_temp_result_lines(
    id: web::Path<String>,
    query: web::Query<LinesQuery>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    cleanup_expired(&state).await?;
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
    if has_meta != has_index {
        return Err(invalid_sidecar(
            "temporary result metadata and index must either both exist or both be absent",
        ));
    }
    if has_meta {
        let lines = read_indexed_lines(
            &result_path,
            &meta_path,
            &index_path,
            start,
            limit,
            result.line_count,
        )
        .await?;
        let next_start =
            (start + (lines.len() as i64) < result.line_count).then_some(start + limit);
        return Ok(HttpResponse::Ok().json(TempResultLines {
            start,
            limit,
            line_count: result.line_count,
            next_start,
            lines,
        }));
    }
    let file = File::open(&result_path).await.map_err(AppError::Io)?;
    let mut reader = BufReader::new(file);
    let mut content = String::new();
    let mut current = 0_i64;
    let mut lines = Vec::new();
    while reader.read_line(&mut content).await.map_err(AppError::Io)? > 0 {
        if current >= start && current < start + limit {
            lines.push(TempLine {
                bundle_hash: None,
                file_id: None,
                path: None,
                line_number: current,
                content: content.trim_end_matches(['\r', '\n']).to_string(),
            });
        }
        current += 1;
        content.clear();
        if current >= start + limit {
            break;
        }
    }
    let next_start = (start + (lines.len() as i64) < result.line_count).then_some(start + limit);
    Ok(HttpResponse::Ok().json(TempResultLines {
        start,
        limit,
        line_count: result.line_count,
        next_start,
        lines,
    }))
}

#[get("/temp-results/{id}/download")]
pub async fn download_temp_result(
    id: web::Path<String>,
    state: web::Data<AppState>,
) -> Result<NamedFile, AppError> {
    cleanup_expired(&state).await?;
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

#[delete("/temp-results/{id}")]
pub async fn delete_temp_result(
    id: web::Path<String>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    let result = load_record(&state, &id).await?;
    sqlx::query("DELETE FROM temp_results WHERE id = ?")
        .bind(&result.id)
        .execute(&state.pool)
        .await
        .map_err(AppError::Database)?;
    let path = checked_temp_path(&state, &result.storage_path)?;
    remove_result_files(&path).await?;
    Ok(HttpResponse::NoContent().finish())
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
    bundle_hash: String,
}

async fn resolve_sources(
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
        if has_meta != has_index {
            return Err(invalid_sidecar(
                "temporary result metadata and index must either both exist or both be absent",
            ));
        }
        return Ok(vec![TempSource {
            path,
            metadata_path: has_meta.then_some(meta_path),
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
                   f.status, f.meta, b.hash AS bundle_hash
            FROM files f
            JOIN bundles b ON b.id = f.bundle_id
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
                name: row.name,
                path: row.path,
                is_dir: false,
                size_bytes: row.size_bytes,
                line_count: row.line_count,
                mime_type: row.mime_type,
                status: row.status,
                meta: row.meta,
            };
            sources.push(TempSource {
                path: resolve_file_path(&file, &data_root(state))?,
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
    let path = resolve_file_path(&file, &data_root(state))?;
    Ok(vec![TempSource {
        path,
        metadata_path: None,
        label: file.name,
        bundle_hash: Some(bundle.hash),
        file_id: Some(file.id.to_string()),
    }])
}

fn source_label(sources: &[TempSource]) -> String {
    if sources.len() == 1 {
        sources[0].label.clone()
    } else {
        format!("{} 个源文件", sources.len())
    }
}

async fn insert_temp_result(
    state: &web::Data<AppState>,
    id: &str,
    expression: &str,
    source_label: &str,
    output_path: &Path,
    line_count: i64,
    size_bytes: i64,
) -> Result<(), AppError> {
    let created_at = Utc::now();
    let expires_at = created_at + Duration::days(RETENTION_DAYS);
    let name = format!("filtered-{}.log", &id[..8]);
    sqlx::query(
        r#"
        INSERT INTO temp_results
            (id, name, expression, source_label, storage_path, line_count, size_bytes, created_at, expires_at)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(id)
    .bind(name)
    .bind(expression)
    .bind(source_label)
    .bind(output_path.to_string_lossy().to_string())
    .bind(line_count)
    .bind(size_bytes)
    .bind(created_at.to_rfc3339())
    .bind(expires_at.to_rfc3339())
    .execute(&state.pool)
    .await
    .map_err(AppError::Database)?;
    Ok(())
}

async fn read_indexed_lines(
    result_path: &Path,
    meta_path: &Path,
    index_path: &Path,
    start: i64,
    limit: i64,
    line_count: i64,
) -> Result<Vec<TempLine>, AppError> {
    if start >= line_count {
        return Ok(Vec::new());
    }
    let index_content = tokio::fs::read_to_string(index_path)
        .await
        .map_err(AppError::Io)?;
    if index_content.is_empty() {
        return Err(invalid_sidecar(
            "temporary result index is empty for a nonempty result",
        ));
    }
    let checkpoints = index_content
        .lines()
        .map(|line| decode_sidecar::<SparseCheckpoint>(line))
        .collect::<Result<Vec<_>, _>>()?;
    let checkpoint = select_checkpoint(&checkpoints, start).ok_or_else(|| {
        AppError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "temporary result index has no checkpoint for requested line",
        ))
    })?;
    let mut log_reader = BufReader::new(File::open(result_path).await.map_err(AppError::Io)?);
    let mut meta_reader = BufReader::new(File::open(meta_path).await.map_err(AppError::Io)?);
    log_reader
        .seek(SeekFrom::Start(checkpoint.log_offset))
        .await
        .map_err(AppError::Io)?;
    meta_reader
        .seek(SeekFrom::Start(checkpoint.meta_offset))
        .await
        .map_err(AppError::Io)?;

    let mut current = checkpoint.result_line;
    let mut content = String::new();
    let mut metadata_line = String::new();
    let mut lines = Vec::new();
    let expected_end = (start + limit).min(line_count);
    while lines.len() < limit as usize {
        content.clear();
        metadata_line.clear();
        let content_bytes = log_reader
            .read_line(&mut content)
            .await
            .map_err(AppError::Io)?;
        let metadata_bytes = meta_reader
            .read_line(&mut metadata_line)
            .await
            .map_err(AppError::Io)?;
        if content_bytes == 0 && metadata_bytes == 0 {
            if current < expected_end {
                return Err(invalid_sidecar(
                    "temporary result ended before expected line count",
                ));
            }
            break;
        }
        if content_bytes == 0 || metadata_bytes == 0 {
            return Err(AppError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "temporary result content and metadata are out of sync",
            )));
        }
        let metadata = decode_sidecar::<MatchMetadata>(metadata_line.trim_end())?;
        if current >= start {
            lines.push(TempLine {
                bundle_hash: metadata.bundle_hash,
                file_id: metadata.file_id,
                path: Some(metadata.path),
                line_number: metadata.line_number,
                content: content.trim_end_matches(['\r', '\n']).to_string(),
            });
        }
        current += 1;
    }
    Ok(lines)
}

fn decode_sidecar<T: serde::de::DeserializeOwned>(line: &str) -> Result<T, AppError> {
    serde_json::from_str(line)
        .map_err(|error| AppError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, error)))
}

fn invalid_sidecar(message: &str) -> AppError {
    AppError::Io(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        message,
    ))
}

async fn load_record(state: &web::Data<AppState>, id: &str) -> Result<TempResultRecord, AppError> {
    sqlx::query_as::<_, TempResultRecord>(
        r#"
        SELECT id, name, expression, source_label, storage_path, line_count,
               size_bytes, created_at, expires_at
        FROM temp_results WHERE id = ? LIMIT 1
        "#,
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await
    .map_err(AppError::Database)?
    .ok_or_else(|| AppError::NotFound(format!("temporary result {id}")))
}

async fn load_and_renew(
    state: &web::Data<AppState>,
    id: &str,
) -> Result<TempResultRecord, AppError> {
    let expires_at = (Utc::now() + Duration::days(RETENTION_DAYS)).to_rfc3339();
    sqlx::query("UPDATE temp_results SET expires_at = ? WHERE id = ?")
        .bind(&expires_at)
        .bind(id)
        .execute(&state.pool)
        .await
        .map_err(AppError::Database)?;
    load_record(state, id).await
}

async fn cleanup_expired(state: &web::Data<AppState>) -> Result<(), AppError> {
    let records = sqlx::query_as::<_, TempResultRecord>(
        r#"
        SELECT id, name, expression, source_label, storage_path, line_count,
               size_bytes, created_at, expires_at
        FROM temp_results WHERE datetime(expires_at) < datetime('now')
        "#,
    )
    .fetch_all(&state.pool)
    .await
    .map_err(AppError::Database)?;
    for record in records {
        sqlx::query("DELETE FROM temp_results WHERE id = ?")
            .bind(&record.id)
            .execute(&state.pool)
            .await
            .map_err(AppError::Database)?;
        if let Ok(path) = checked_temp_path(state, &record.storage_path) {
            remove_result_files(&path).await?;
        }
    }
    Ok(())
}

async fn remove_result_files(log_path: &Path) -> Result<(), AppError> {
    for path in [
        log_path.to_path_buf(),
        log_path.with_extension("meta"),
        log_path.with_extension("idx"),
    ] {
        match tokio::fs::remove_file(path).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(AppError::Io(error)),
        }
    }
    Ok(())
}

fn staging_path(path: &Path) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(".part");
    PathBuf::from(value)
}

async fn remove_preview_artifacts(log_path: &Path) -> Result<(), AppError> {
    let paths = [
        log_path.to_path_buf(),
        log_path.with_extension("meta"),
        log_path.with_extension("idx"),
    ];
    for path in paths {
        for candidate in [path.clone(), staging_path(&path)] {
            match tokio::fs::remove_file(candidate).await {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(AppError::Io(error)),
            }
        }
    }
    Ok(())
}

fn checked_temp_path(state: &web::Data<AppState>, stored_path: &str) -> Result<PathBuf, AppError> {
    let root = data_root(state).join("temp-results");
    let path = Path::new(stored_path);
    if !path.starts_with(&root) {
        return Err(AppError::BadRequest(
            "temporary result path is invalid".into(),
        ));
    }
    Ok(path.to_path_buf())
}

fn preview_page_size(requested: Option<i64>) -> i64 {
    requested.unwrap_or(1_000).clamp(1, 3_000)
}

fn to_response(record: TempResultRecord) -> TempResult {
    TempResult {
        id: record.id,
        name: record.name,
        expression: record.expression,
        source_label: record.source_label,
        line_count: record.line_count,
        size_bytes: record.size_bytes,
        created_at: record.created_at,
        expires_at: record.expires_at,
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::{preview_page_size, read_indexed_lines, staging_path};

    #[test]
    fn preview_supports_log_viewer_page_sizes() {
        assert_eq!(preview_page_size(Some(1_000)), 1_000);
        assert_eq!(preview_page_size(Some(3_000)), 3_000);
        assert_eq!(preview_page_size(Some(9_000)), 3_000);
    }

    #[test]
    fn preview_uses_distinct_staging_paths_before_publication() {
        let final_path = std::path::Path::new("temp-results/result.log");

        assert_eq!(
            staging_path(final_path),
            std::path::PathBuf::from("temp-results/result.log.part")
        );
    }

    #[tokio::test]
    async fn indexed_reader_seeks_to_deep_pages_and_preserves_metadata() {
        let root = std::env::temp_dir().join(format!("rain-index-read-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(&root).await.unwrap();
        let log = root.join("result.log");
        let meta = root.join("result.meta");
        let index = root.join("result.idx");
        let mut log_content = String::new();
        let mut meta_content = String::new();
        for line in 0..1_005 {
            log_content.push_str(&format!("line {line}\n"));
            meta_content.push_str(&format!(
                "{{\"bundle_hash\":\"bundle\",\"file_id\":\"42\",\"path\":\"app.log\",\"line_number\":{line}}}\n"
            ));
        }
        tokio::fs::write(&log, log_content).await.unwrap();
        tokio::fs::write(&meta, meta_content).await.unwrap();
        let log_offset = (0..1_000)
            .map(|line| format!("line {line}\n").len())
            .sum::<usize>();
        let meta_offset = (0..1_000)
            .map(|line| {
                format!(
                    "{{\"bundle_hash\":\"bundle\",\"file_id\":\"42\",\"path\":\"app.log\",\"line_number\":{line}}}\n"
                )
                .len()
            })
            .sum::<usize>();
        tokio::fs::write(
            &index,
            format!(
                "{{\"result_line\":0,\"log_offset\":0,\"meta_offset\":0}}\n{{\"result_line\":1000,\"log_offset\":{log_offset},\"meta_offset\":{meta_offset}}}\n"
            ),
        )
        .await
        .unwrap();

        let lines = read_indexed_lines(&log, &meta, &index, 1_002, 2, 1_005)
            .await
            .unwrap();

        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].content, "line 1002");
        assert_eq!(lines[0].line_number, 1_002);
        assert_eq!(lines[0].path.as_deref(), Some("app.log"));
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn indexed_reader_rejects_jointly_truncated_content_and_metadata() {
        let root = std::env::temp_dir().join(format!("rain-index-corrupt-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(&root).await.unwrap();
        let log = root.join("result.log");
        let meta = root.join("result.meta");
        let index = root.join("result.idx");
        tokio::fs::write(&log, "only one\n").await.unwrap();
        tokio::fs::write(
            &meta,
            "{\"bundle_hash\":null,\"file_id\":null,\"path\":\"app.log\",\"line_number\":0}\n",
        )
        .await
        .unwrap();
        tokio::fs::write(
            &index,
            "{\"result_line\":0,\"log_offset\":0,\"meta_offset\":0}\n",
        )
        .await
        .unwrap();

        let error = read_indexed_lines(&log, &meta, &index, 0, 2, 2)
            .await
            .expect_err("truncated sidecars must fail");

        assert!(error.to_string().contains("ended before expected line"));
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn indexed_reader_rejects_empty_index_for_nonempty_result() {
        let root = std::env::temp_dir().join(format!("rain-index-empty-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(&root).await.unwrap();
        let log = root.join("result.log");
        let meta = root.join("result.meta");
        let index = root.join("result.idx");
        tokio::fs::write(&log, "line\n").await.unwrap();
        tokio::fs::write(
            &meta,
            "{\"bundle_hash\":null,\"file_id\":null,\"path\":\"app.log\",\"line_number\":0}\n",
        )
        .await
        .unwrap();
        tokio::fs::write(&index, "").await.unwrap();

        let error = read_indexed_lines(&log, &meta, &index, 0, 1, 1)
            .await
            .expect_err("nonempty results require checkpoint zero");

        assert!(error.to_string().contains("index is empty"));
        let _ = tokio::fs::remove_dir_all(root).await;
    }
}
