use std::path::{Path, PathBuf};

use actix_files::NamedFile;
use actix_web::{HttpResponse, delete, get, http::header, post, web};
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use tokio::{
    fs::File,
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
};
use uuid::Uuid;

use crate::{AppState, error::AppError, log_expression};

use super::{
    files::{fetch_file, resolve_file_path},
    helpers::{data_root, ensure_bundle_ready, load_bundle},
    issues::normalize_issue_code,
};

const RETENTION_DAYS: i64 = 7;

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

#[derive(Serialize)]
struct TempLine {
    line_number: i64,
    content: String,
}

#[derive(Serialize)]
struct TempPreview {
    total: i64,
    lines: Vec<PreviewLine>,
}

#[derive(Serialize)]
struct PreviewLine {
    bundle_hash: Option<String>,
    file_id: Option<String>,
    path: String,
    line_number: i64,
    content: String,
}

#[post("/temp-results/preview")]
pub async fn preview_temp_result(
    payload: web::Json<PreviewTempResultRequest>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    cleanup_expired(&state).await?;
    let expression_text = payload.expression.trim();
    let expression = log_expression::parse(expression_text).map_err(|error| {
        AppError::BadRequest(format!(
            "invalid expression at {}: {}",
            error.offset, error.message
        ))
    })?;
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
    let mut matched = 0_i64;
    let mut lines = Vec::new();
    for source in sources {
        let file = File::open(&source.path).await.map_err(AppError::Io)?;
        let mut reader = BufReader::new(file);
        let mut bytes = Vec::new();
        let mut source_line = 0_i64;
        loop {
            bytes.clear();
            if reader
                .read_until(b'\n', &mut bytes)
                .await
                .map_err(AppError::Io)?
                == 0
            {
                break;
            }
            let line = String::from_utf8_lossy(&bytes);
            let content = line.trim_end_matches(['\r', '\n']);
            if expression.matches(content) {
                if matched >= from && matched < from + size {
                    lines.push(PreviewLine {
                        bundle_hash: source.bundle_hash.clone(),
                        file_id: source.file_id.clone(),
                        path: source.label.clone(),
                        line_number: source_line,
                        content: content.to_string(),
                    });
                }
                matched += 1;
            }
            source_line += 1;
        }
    }
    Ok(HttpResponse::Ok().json(TempPreview {
        total: matched,
        lines,
    }))
}

#[post("/temp-results")]
pub async fn create_temp_result(
    payload: web::Json<CreateTempResultRequest>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    cleanup_expired(&state).await?;
    let expression_text = payload.expression.trim();
    let expression = log_expression::parse(expression_text).map_err(|error| {
        AppError::BadRequest(format!(
            "invalid expression at {}: {}",
            error.offset, error.message
        ))
    })?;

    let sources = resolve_sources(&payload, &state).await?;
    let source_label = if sources.len() == 1 {
        sources[0].label.clone()
    } else {
        format!("{} 个源文件", sources.len())
    };
    let id = Uuid::new_v4().simple().to_string();
    let directory = data_root(&state).join("temp-results");
    tokio::fs::create_dir_all(&directory)
        .await
        .map_err(AppError::Io)?;
    let output_path = directory.join(format!("{id}.log"));
    let mut output = File::create(&output_path).await.map_err(AppError::Io)?;
    let mut matching_lines = 0_i64;
    for source in sources {
        let file = File::open(&source.path).await.map_err(AppError::Io)?;
        let mut reader = BufReader::new(file);
        let mut bytes = Vec::new();
        loop {
            bytes.clear();
            if reader
                .read_until(b'\n', &mut bytes)
                .await
                .map_err(AppError::Io)?
                == 0
            {
                break;
            }
            let line = String::from_utf8_lossy(&bytes);
            if expression.matches(line.trim_end_matches(['\r', '\n'])) {
                output.write_all(&bytes).await.map_err(AppError::Io)?;
                if !bytes.ends_with(b"\n") {
                    output.write_all(b"\n").await.map_err(AppError::Io)?;
                }
                matching_lines += 1;
            }
        }
    }
    output.flush().await.map_err(AppError::Io)?;

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
    let limit = query.limit.unwrap_or(1000).clamp(1, 3000);
    let file = File::open(checked_temp_path(&state, &result.storage_path)?)
        .await
        .map_err(AppError::Io)?;
    let mut reader = BufReader::new(file);
    let mut content = String::new();
    let mut current = 0_i64;
    let mut lines = Vec::new();
    while reader.read_line(&mut content).await.map_err(AppError::Io)? > 0 {
        if current >= start && current < start + limit {
            lines.push(TempLine {
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
    if tokio::fs::metadata(&path).await.is_ok() {
        tokio::fs::remove_file(path).await.map_err(AppError::Io)?;
    }
    Ok(HttpResponse::NoContent().finish())
}

struct Source {
    path: PathBuf,
    label: String,
    bundle_hash: Option<String>,
    file_id: Option<String>,
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
) -> Result<Vec<Source>, AppError> {
    if let Some(source_id) = payload.source_temp_id.as_deref() {
        let source = load_and_renew(state, source_id).await?;
        return Ok(vec![Source {
            path: checked_temp_path(state, &source.storage_path)?,
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
            let file = super::files::FileRow {
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
            sources.push(Source {
                path: resolve_file_path(&file, &data_root(state))?,
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
    if file.is_dir {
        return Err(AppError::BadRequest("cannot filter a directory".into()));
    }
    let path = resolve_file_path(&file, &data_root(state))?;
    Ok(vec![Source {
        path,
        label: file.name,
        bundle_hash: Some(bundle.hash),
        file_id: Some(file.id.to_string()),
    }])
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
            let _ = tokio::fs::remove_file(path).await;
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
    use super::preview_page_size;

    #[test]
    fn preview_supports_log_viewer_page_sizes() {
        assert_eq!(preview_page_size(Some(1_000)), 1_000);
        assert_eq!(preview_page_size(Some(3_000)), 3_000);
        assert_eq!(preview_page_size(Some(9_000)), 3_000);
    }
}
