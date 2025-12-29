use std::path::PathBuf;

use actix_web::{HttpResponse, get, web};
use serde::Deserialize;
use serde_json::json;
use sqlx::FromRow;
use tokio::{fs::File, io::AsyncReadExt};

use crate::{
    AppState,
    error::AppError,
    models::files::{FileNode, FileNodeResponse},
};

use super::helpers::{data_root, load_bundle};

const MAX_FILE_PREVIEW_BYTES: u64 = 64 * 1024;

#[derive(Deserialize)]
struct FilePath {
    bundle_id: String,
    file_id: String,
}

#[get("/api/files/v1/{bundle_id}/files/{file_id}")]
pub async fn get_file_node(
    params: web::Path<FilePath>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    let FilePath { bundle_id, file_id } = params.into_inner();
    let bundle = load_bundle(&state.pool, &bundle_id).await?;
    let is_root = file_id.eq_ignore_ascii_case("root");

    let node = if is_root {
        FileNode {
            id: "root".into(),
            name: format!("{}_root", bundle.hash),
            path: format!("/{}", bundle.hash),
            is_dir: true,
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
        let record = fetch_file(&state.pool, bundle.id, parsed_id).await?;
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
    let children_records = fetch_children(&state.pool, bundle.id, parent_id).await?;
    let children = children_records.into_iter().map(to_file_node).collect();

    Ok(HttpResponse::Ok().json(FileNodeResponse { node, children }))
}

#[get("/api/files/v1/{bundle_id}/files/{file_id}/content")]
pub async fn get_file_content(
    params: web::Path<FilePath>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    let FilePath { bundle_id, file_id } = params.into_inner();
    let bundle = load_bundle(&state.pool, &bundle_id).await?;
    let parsed_id = file_id
        .parse::<i64>()
        .map_err(|_| AppError::BadRequest(format!("invalid file id: {file_id}")))?;
    let record = fetch_file(&state.pool, bundle.id, parsed_id).await?;
    if record.is_dir {
        return Err(AppError::BadRequest("cannot read directory content".into()));
    }

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

#[derive(FromRow)]
pub struct FileRow {
    pub id: i64,
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size_bytes: Option<i64>,
    pub mime_type: Option<String>,
    pub status: Option<String>,
    pub meta: Option<serde_json::Value>,
}

pub async fn fetch_file(
    pool: &sqlx::PgPool,
    bundle_id: uuid::Uuid,
    file_id: i64,
) -> Result<FileRow, AppError> {
    sqlx::query_as::<_, FileRow>(
        r#"
        SELECT id, name, path, is_dir, size_bytes, mime_type, status, meta
        FROM files
        WHERE bundle_id = $1 AND id = $2
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
    pool: &sqlx::PgPool,
    bundle_id: uuid::Uuid,
    parent_id: Option<i64>,
) -> Result<Vec<FileRow>, AppError> {
    if let Some(parent) = parent_id {
        sqlx::query_as::<_, FileRow>(
            r#"
            SELECT id, name, path, is_dir, size_bytes, mime_type, status, meta
            FROM files
            WHERE bundle_id = $1 AND parent_id = $2
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
            SELECT id, name, path, is_dir, size_bytes, mime_type, status, meta
            FROM files
            WHERE bundle_id = $1 AND parent_id IS NULL
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
    FileNode {
        id: record.id.to_string(),
        name: record.name,
        path: record.path,
        is_dir: record.is_dir,
        size_bytes: record.size_bytes.map(|value| value as u64),
        mime_type: record.mime_type,
        status: record.status,
        meta: record.meta,
    }
}

pub fn resolve_file_path(
    record: &FileRow,
    data_root: &std::path::Path,
) -> Result<PathBuf, AppError> {
    let meta_path = record
        .meta
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
