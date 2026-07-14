use std::path::PathBuf;

use actix_files::NamedFile;
use actix_web::{
    HttpResponse, delete, get,
    http::header::{ContentDisposition, DispositionParam, DispositionType},
    web,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::FromRow;
use tokio::{
    fs::File,
    io::{AsyncReadExt, AsyncSeekExt, BufReader},
};

use crate::{
    AppState,
    error::AppError,
    file_classification::{PreviewKind, effective_mime_type, preview_kind_from_metadata},
    ingest::{MAX_LINE_BYTES, decode_log_line, read_line_bytes_limited},
    models::files::{FileNode, FileNodeResponse},
};

use super::helpers::{data_root, ensure_bundle_ready, load_bundle};

const MAX_FILE_PREVIEW_BYTES: u64 = 64 * 1024;

#[derive(Deserialize)]
struct FilePath {
    bundle_id: String,
    file_id: String,
}

#[derive(Deserialize)]
struct LinesQuery {
    start: Option<i64>,
    limit: Option<i64>,
}

#[derive(Serialize)]
struct FileLinesResponse {
    path: String,
    size_bytes: Option<i64>,
    line_count: Option<i64>,
    start: i64,
    limit: i64,
    next_start: Option<i64>,
    lines: Vec<FileLine>,
}

#[derive(Serialize)]
struct FileLine {
    line_number: i64,
    content: String,
    truncated: bool,
}

// scoped under /api in routes::register
#[get("/files/v1/{bundle_id}/files/{file_id}")]
pub async fn get_file_node(
    params: web::Path<FilePath>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    let FilePath { bundle_id, file_id } = params.into_inner();
    let bundle = load_bundle(&state.pool, &bundle_id).await?;
    ensure_bundle_ready(&bundle)?;
    let is_root = file_id.eq_ignore_ascii_case("root");

    let node = if is_root {
        FileNode {
            id: "root".into(),
            name: format!("{}_root", bundle.hash),
            path: format!("/{}", bundle.hash),
            is_dir: true,
            preview_kind: PreviewKind::Directory,
            size_bytes: Some(0),
            mime_type: None,
            status: Some("READY".into()),
            meta: Some(json!({
                "bundle_hash": bundle.hash,
                "bundle_name": bundle.name,
                "storage_root": data_root(&state).display().to_string()
            })),
        }
    } else {
        let parsed_id = file_id
            .parse::<i64>()
            .map_err(|_| AppError::BadRequest(format!("invalid file id: {file_id}")))?;
        let record = fetch_file(&state.pool, &bundle.id, parsed_id).await?;
        to_file_node(record)
    };

    let parent_id = if is_root {
        None
    } else {
        Some(
            file_id
                .parse::<i64>()
                .map_err(|_| AppError::BadRequest(format!("invalid file id: {file_id}")))?,
        )
    };
    let children_records = fetch_children(&state.pool, &bundle.id, parent_id).await?;
    let children = children_records.into_iter().map(to_file_node).collect();

    Ok(HttpResponse::Ok().json(FileNodeResponse { node, children }))
}

#[get("/files/v1/{bundle_id}/files/{file_id}/content")]
pub async fn get_file_content(
    params: web::Path<FilePath>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    let FilePath { bundle_id, file_id } = params.into_inner();
    let bundle = load_bundle(&state.pool, &bundle_id).await?;
    ensure_bundle_ready(&bundle)?;
    let parsed_id = file_id
        .parse::<i64>()
        .map_err(|_| AppError::BadRequest(format!("invalid file id: {file_id}")))?;
    let record = fetch_file(&state.pool, &bundle.id, parsed_id).await?;
    if record.is_dir {
        return Err(AppError::BadRequest("cannot read directory content".into()));
    }
    ensure_text_preview(&record)?;

    let disk_path = resolve_file_path(&record, &data_root(&state))?;
    let metadata = tokio::fs::metadata(&disk_path)
        .await
        .map_err(AppError::Io)?;
    let size_bytes = metadata.len();
    let file = File::open(&disk_path).await.map_err(AppError::Io)?;
    let mut buffer = Vec::new();
    let mut limited = file.take(MAX_FILE_PREVIEW_BYTES);
    limited
        .read_to_end(&mut buffer)
        .await
        .map_err(AppError::Io)?;

    let preview = String::from_utf8_lossy(&buffer).to_string();
    let truncated = size_bytes > MAX_FILE_PREVIEW_BYTES;

    Ok(HttpResponse::Ok().json(json!({
        "path": record.path,
        "size_bytes": record.size_bytes.unwrap_or(size_bytes as i64),
        "mime_type": record.mime_type,
        "preview": preview,
        "truncated": truncated,
    })))
}

#[get("/files/v1/{bundle_id}/files/{file_id}/lines")]
pub async fn get_file_lines(
    params: web::Path<FilePath>,
    query: web::Query<LinesQuery>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    let FilePath { bundle_id, file_id } = params.into_inner();
    let bundle = load_bundle(&state.pool, &bundle_id).await?;
    ensure_bundle_ready(&bundle)?;
    let parsed_id = file_id
        .parse::<i64>()
        .map_err(|_| AppError::BadRequest(format!("invalid file id: {file_id}")))?;
    let record = fetch_file(&state.pool, &bundle.id, parsed_id).await?;
    if record.is_dir {
        return Err(AppError::BadRequest("cannot read directory content".into()));
    }
    ensure_text_preview(&record)?;

    let start = query.start.unwrap_or(0).max(0);
    let limit = query.limit.unwrap_or(1000).clamp(1, 3000);
    let (base_line, byte_offset) = nearest_line_offset(&state.pool, record.id, start).await?;
    let disk_path = resolve_file_path(&record, &data_root(&state))?;

    let mut file = File::open(&disk_path).await.map_err(AppError::Io)?;
    file.seek(std::io::SeekFrom::Start(byte_offset as u64))
        .await
        .map_err(AppError::Io)?;
    let mut reader = BufReader::new(file);
    let mut current_line = base_line;
    let end_line = start.saturating_add(limit);
    let mut lines = Vec::new();
    let mut buffer = Vec::new();

    while current_line < end_line {
        let Some((_read, truncated)) =
            read_line_bytes_limited(&mut reader, &mut buffer, MAX_LINE_BYTES)
                .await
                .map_err(AppError::Io)?
        else {
            break;
        };

        if current_line >= start {
            lines.push(FileLine {
                line_number: current_line,
                content: decode_log_line(&buffer, truncated),
                truncated,
            });
        }
        current_line += 1;
    }

    let next_start = if lines.len() as i64 == limit {
        Some(start + limit)
    } else {
        None
    };

    Ok(HttpResponse::Ok().json(FileLinesResponse {
        path: record.path,
        size_bytes: record.size_bytes,
        line_count: record.line_count,
        start,
        limit,
        next_start,
        lines,
    }))
}

#[get("/files/v1/{bundle_id}/files/{file_id}/download")]
pub async fn download_file(
    params: web::Path<FilePath>,
    state: web::Data<AppState>,
) -> Result<NamedFile, AppError> {
    let FilePath { bundle_id, file_id } = params.into_inner();
    let bundle = load_bundle(&state.pool, &bundle_id).await?;
    ensure_bundle_ready(&bundle)?;
    let parsed_id = file_id
        .parse::<i64>()
        .map_err(|_| AppError::BadRequest(format!("invalid file id: {file_id}")))?;
    let record = fetch_file(&state.pool, &bundle.id, parsed_id).await?;
    if record.is_dir {
        return Err(AppError::BadRequest("cannot download directory".into()));
    }

    let disk_path = resolve_file_path(&record, &data_root(&state))?;
    let named = NamedFile::open_async(disk_path)
        .await
        .map_err(AppError::Io)?
        .set_content_disposition(ContentDisposition {
            disposition: DispositionType::Attachment,
            parameters: vec![DispositionParam::Filename(record.name)],
        });
    Ok(named)
}

#[delete("/files/v1/{bundle_id}/files/{file_id}")]
pub async fn delete_file_node(
    params: web::Path<FilePath>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    let FilePath { bundle_id, file_id } = params.into_inner();
    let bundle = load_bundle(&state.pool, &bundle_id).await?;
    ensure_bundle_ready(&bundle)?;
    let parsed_id = file_id
        .parse::<i64>()
        .map_err(|_| AppError::BadRequest(format!("invalid file id: {file_id}")))?;
    let _record = fetch_file(&state.pool, &bundle.id, parsed_id).await?;

    let mut tx = state.pool.begin().await.map_err(AppError::Database)?;

    let mut file_ids = fetch_subtree_ids(&mut tx, &bundle.id, parsed_id).await?;
    let extracted_root_ids = fetch_extracted_child_ids(&mut tx, &bundle.id, parsed_id).await?;
    for extracted_root_id in extracted_root_ids {
        for id in fetch_subtree_ids(&mut tx, &bundle.id, extracted_root_id).await? {
            if !file_ids.contains(&id) {
                file_ids.push(id);
            }
        }
    }

    let disk_paths = fetch_storage_paths_for_ids(&mut tx, &bundle.id, &file_ids).await?;

    for file_id in &file_ids {
        sqlx::query("DELETE FROM log_line_offsets WHERE file_id = ?")
            .bind(file_id)
            .execute(&mut *tx)
            .await
            .map_err(AppError::Database)?;

        sqlx::query("DELETE FROM log_events WHERE file_id = ?")
            .bind(file_id)
            .execute(&mut *tx)
            .await
            .map_err(AppError::Database)?;

        sqlx::query("DELETE FROM log_segments_fts WHERE file_id = ?")
            .bind(file_id)
            .execute(&mut *tx)
            .await
            .map_err(AppError::Database)?;

        sqlx::query("DELETE FROM log_segments WHERE file_id = ?")
            .bind(file_id)
            .execute(&mut *tx)
            .await
            .map_err(AppError::Database)?;
    }

    for file_id in &file_ids {
        sqlx::query("DELETE FROM files WHERE bundle_id = ? AND id = ?")
            .bind(&bundle.id)
            .bind(file_id)
            .execute(&mut *tx)
            .await
            .map_err(AppError::Database)?;
    }

    tx.commit().await.map_err(AppError::Database)?;

    for disk_path in disk_paths {
        if tokio::fs::metadata(&disk_path).await.is_ok() {
            if disk_path.is_dir() {
                let _ = tokio::fs::remove_dir_all(&disk_path).await;
            } else {
                let _ = tokio::fs::remove_file(&disk_path).await;
            }
        }
    }

    Ok(HttpResponse::NoContent().finish())
}

async fn fetch_subtree_ids(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    bundle_id: &str,
    root_id: i64,
) -> Result<Vec<i64>, AppError> {
    sqlx::query_scalar(
        r#"
        WITH RECURSIVE subtree(id) AS (
            SELECT id
            FROM files
            WHERE bundle_id = ? AND id = ?

            UNION ALL

            SELECT f.id
            FROM files f
            JOIN subtree s ON f.parent_id = s.id
            WHERE f.bundle_id = ?
        )
        SELECT id FROM subtree
        "#,
    )
    .bind(bundle_id)
    .bind(root_id)
    .bind(bundle_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(AppError::Database)
}

async fn fetch_extracted_child_ids(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    bundle_id: &str,
    parent_id: i64,
) -> Result<Vec<i64>, AppError> {
    let rows = sqlx::query_as::<_, FileRow>(
        r#"
        SELECT id, name, path, is_dir, size_bytes, line_count, mime_type, status, meta
        FROM files
        WHERE bundle_id = ? AND parent_id = ?
        "#,
    )
    .bind(bundle_id)
    .bind(parent_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(AppError::Database)?;

    Ok(rows
        .into_iter()
        .filter(|row| row.is_dir && meta_kind(&row.meta).as_deref() == Some("extracted_dir"))
        .map(|row| row.id)
        .collect())
}

async fn fetch_storage_paths_for_ids(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    bundle_id: &str,
    file_ids: &[i64],
) -> Result<Vec<PathBuf>, AppError> {
    let mut paths = Vec::new();
    for file_id in file_ids {
        let row = sqlx::query_as::<_, FileRow>(
            r#"
            SELECT id, name, path, is_dir, size_bytes, line_count, mime_type, status, meta
            FROM files
            WHERE bundle_id = ? AND id = ?
            LIMIT 1
            "#,
        )
        .bind(bundle_id)
        .bind(file_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(AppError::Database)?;

        if let Some(row) = row
            && let Some(path) = storage_path_from_meta(&row.meta)
        {
            paths.push(path);
        }
    }

    paths.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    paths.dedup();
    Ok(paths)
}

fn meta_kind(meta: &Option<String>) -> Option<String> {
    meta.as_deref()
        .and_then(|value| serde_json::from_str::<serde_json::Value>(value).ok())
        .and_then(|value| {
            value
                .get("kind")
                .and_then(|kind| kind.as_str())
                .map(str::to_string)
        })
}

fn storage_path_from_meta(meta: &Option<String>) -> Option<PathBuf> {
    meta.as_deref()
        .and_then(|value| serde_json::from_str::<serde_json::Value>(value).ok())
        .and_then(|value| {
            value
                .get("storage_path")
                .and_then(|path| path.as_str())
                .map(PathBuf::from)
        })
}

#[derive(FromRow)]
pub struct FileRow {
    pub id: i64,
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size_bytes: Option<i64>,
    pub line_count: Option<i64>,
    pub mime_type: Option<String>,
    pub status: Option<String>,
    pub meta: Option<String>,
}

pub async fn fetch_file(
    pool: &sqlx::SqlitePool,
    bundle_id: &str,
    file_id: i64,
) -> Result<FileRow, AppError> {
    sqlx::query_as::<_, FileRow>(
        r#"
        SELECT id, name, path, is_dir, size_bytes, line_count, mime_type, status, meta
        FROM files
        WHERE bundle_id = ? AND id = ?
        LIMIT 1
        "#,
    )
    .bind(bundle_id)
    .bind(file_id)
    .fetch_optional(pool)
    .await
    .map_err(AppError::Database)?
    .ok_or_else(|| AppError::NotFound(format!("file {file_id}")))
}

pub async fn fetch_children(
    pool: &sqlx::SqlitePool,
    bundle_id: &str,
    parent_id: Option<i64>,
) -> Result<Vec<FileRow>, AppError> {
    if let Some(parent) = parent_id {
        sqlx::query_as::<_, FileRow>(
            r#"
            SELECT id, name, path, is_dir, size_bytes, line_count, mime_type, status, meta
            FROM files
            WHERE bundle_id = ? AND parent_id = ?
            ORDER BY is_dir DESC, name ASC
            "#,
        )
        .bind(bundle_id)
        .bind(parent)
        .fetch_all(pool)
        .await
    } else {
        sqlx::query_as::<_, FileRow>(
            r#"
            SELECT id, name, path, is_dir, size_bytes, line_count, mime_type, status, meta
            FROM files
            WHERE bundle_id = ? AND parent_id IS NULL
            ORDER BY is_dir DESC, name ASC
            "#,
        )
        .bind(bundle_id)
        .fetch_all(pool)
        .await
    }
    .map_err(AppError::Database)
}

pub fn to_file_node(record: FileRow) -> FileNode {
    let preview_kind = preview_kind_for_record(&record);
    let mime_type = effective_mime_type(&record.name, record.mime_type.as_deref());
    let meta = record
        .meta
        .as_deref()
        .and_then(|value| serde_json::from_str(value).ok());
    FileNode {
        id: record.id.to_string(),
        name: record.name,
        path: record.path,
        is_dir: record.is_dir,
        preview_kind,
        size_bytes: record.size_bytes.map(|value| value as u64),
        mime_type,
        status: record.status,
        meta: append_line_count_meta(meta, record.line_count),
    }
}

pub fn preview_kind_for_record(record: &FileRow) -> PreviewKind {
    preview_kind_from_metadata(
        &record.name,
        record.mime_type.as_deref(),
        record.is_dir,
        record.line_count,
        record.meta.as_deref(),
    )
}

pub fn ensure_text_preview(record: &FileRow) -> Result<(), AppError> {
    let preview_kind = preview_kind_for_record(record);
    if preview_kind == PreviewKind::Text {
        return Ok(());
    }
    Err(AppError::BadRequest(format!(
        "text preview is not supported for {} file: {}",
        preview_kind.as_str(),
        record.name
    )))
}

async fn nearest_line_offset(
    pool: &sqlx::SqlitePool,
    file_id: i64,
    start: i64,
) -> Result<(i64, i64), AppError> {
    let row = sqlx::query_as::<_, LineOffsetRow>(
        r#"
        SELECT line_number, byte_offset
        FROM log_line_offsets
        WHERE file_id = ? AND line_number <= ?
        ORDER BY line_number DESC
        LIMIT 1
        "#,
    )
    .bind(file_id)
    .bind(start)
    .fetch_optional(pool)
    .await
    .map_err(AppError::Database)?;

    Ok(row
        .map(|row| (row.line_number, row.byte_offset))
        .unwrap_or((0, 0)))
}

#[derive(FromRow)]
struct LineOffsetRow {
    line_number: i64,
    byte_offset: i64,
}

fn append_line_count_meta(
    meta: Option<serde_json::Value>,
    line_count: Option<i64>,
) -> Option<serde_json::Value> {
    let Some(line_count) = line_count else {
        return meta;
    };
    let mut value = meta.unwrap_or_else(|| json!({}));
    if let Some(object) = value.as_object_mut() {
        object.insert("line_count".to_string(), json!(line_count));
    }
    Some(value)
}

pub fn resolve_file_path(
    record: &FileRow,
    data_root: &std::path::Path,
) -> Result<PathBuf, AppError> {
    let meta_path = record
        .meta
        .as_deref()
        .and_then(|value| serde_json::from_str::<serde_json::Value>(value).ok())
        .as_ref()
        .and_then(|meta| meta.get("storage_path"))
        .and_then(|value| value.as_str())
        .map(std::path::PathBuf::from);

    let fallback_path = data_root.join(record.path.trim_start_matches('/'));
    let candidate = meta_path.unwrap_or(fallback_path);

    let canonical_data_root = std::fs::canonicalize(data_root).map_err(AppError::Io)?;
    let canonical_candidate = std::fs::canonicalize(&candidate).map_err(AppError::Io)?;

    if !canonical_candidate.starts_with(&canonical_data_root) {
        return Err(AppError::BadRequest(
            "file path is outside data root".into(),
        ));
    }

    Ok(canonical_candidate)
}
