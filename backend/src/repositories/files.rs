use std::path::PathBuf;

use serde_json::json;
use sqlx::FromRow;

use crate::{
    blob_store::BlobStore,
    error::AppError,
    file_classification::{PreviewKind, effective_mime_type, preview_kind_from_metadata},
    models::files::FileNode,
};

#[derive(FromRow)]
pub struct FileRow {
    pub id: i64,
    pub parent_id: Option<i64>,
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size_bytes: Option<i64>,
    pub line_count: Option<i64>,
    pub mime_type: Option<String>,
    pub status: Option<String>,
    pub meta: Option<String>,
    pub blob_id: Option<i64>,
    pub storage_backend: Option<String>,
    pub storage_key: Option<String>,
    pub blob_state: Option<String>,
}

pub async fn fetch_file(
    pool: &sqlx::SqlitePool,
    bundle_id: &str,
    file_id: i64,
) -> Result<FileRow, AppError> {
    sqlx::query_as::<_, FileRow>(
        r#"
        SELECT f.id, f.parent_id, f.name, f.path, f.is_dir, f.size_bytes, f.line_count, f.mime_type,
               f.status, f.meta, f.blob_id, b.storage_backend, b.storage_key, b.state AS blob_state
        FROM files f LEFT JOIN blobs b ON b.id = f.blob_id
        WHERE f.bundle_id = ? AND f.id = ?
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
            SELECT f.id, f.parent_id, f.name, f.path, f.is_dir, f.size_bytes, f.line_count, f.mime_type,
                   f.status, f.meta, f.blob_id, b.storage_backend, b.storage_key, b.state AS blob_state
            FROM files f LEFT JOIN blobs b ON b.id = f.blob_id
            WHERE f.bundle_id = ? AND f.parent_id = ?
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
            SELECT f.id, f.parent_id, f.name, f.path, f.is_dir, f.size_bytes, f.line_count, f.mime_type,
                   f.status, f.meta, f.blob_id, b.storage_backend, b.storage_key, b.state AS blob_state
            FROM files f LEFT JOIN blobs b ON b.id = f.blob_id
            WHERE f.bundle_id = ? AND f.parent_id IS NULL
            ORDER BY is_dir DESC, name ASC
            "#,
        )
        .bind(bundle_id)
        .fetch_all(pool)
        .await
    }
    .map_err(AppError::Database)
}

pub async fn nearest_line_offset(
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

pub fn to_file_node(record: FileRow) -> FileNode {
    let preview_kind = preview_kind_for_record(&record);
    let mime_type = effective_mime_type(&record.name, record.mime_type.as_deref());
    let meta = record
        .meta
        .as_deref()
        .and_then(|value| serde_json::from_str(value).ok());
    FileNode {
        id: record.id.to_string(),
        parent_id: record.parent_id.map(|id| id.to_string()),
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

pub async fn resolve_file_path(
    record: &FileRow,
    blob_store: &dyn BlobStore,
) -> Result<PathBuf, AppError> {
    if let Some(storage_key) = record.storage_key.as_deref() {
        let storage_backend = record.storage_backend.as_deref().ok_or_else(|| {
            AppError::Config("blob storage key is missing its storage backend".into())
        })?;
        if storage_backend != blob_store.backend_name() {
            return Err(AppError::Config(format!(
                "blob uses storage backend {storage_backend}, but {} is active",
                blob_store.backend_name()
            )));
        }
        if record.blob_state.as_deref() != Some("READY") {
            return Err(AppError::Conflict(format!(
                "blob is not readable in state {}",
                record.blob_state.as_deref().unwrap_or("UNKNOWN")
            )));
        }
        return blob_store.materialize(storage_key).await;
    }
    Err(AppError::Config(format!(
        "file {} has no content-addressed blob",
        record.id
    )))
}

#[cfg(test)]
mod tests {
    use crate::blob_store::LocalCasBlobStore;

    use super::{FileRow, resolve_file_path};

    fn row(path: &str, meta: Option<&str>) -> FileRow {
        FileRow {
            id: 1,
            parent_id: None,
            name: "app.log".into(),
            path: path.into(),
            is_dir: false,
            size_bytes: Some(1),
            line_count: Some(1),
            mime_type: Some("text/plain".into()),
            status: None,
            meta: meta.map(str::to_string),
            blob_id: None,
            storage_backend: None,
            storage_key: None,
            blob_state: None,
        }
    }

    #[tokio::test]
    async fn blob_path_rejects_the_wrong_active_backend() {
        let root = std::env::temp_dir().join(format!(
            "rain-backend-mismatch-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let mut record = row("/bundle/app.log", None);
        record.blob_id = Some(1);
        record.storage_backend = Some("s3".into());
        record.storage_key = Some("blobs/aa/anything".into());
        record.blob_state = Some("READY".into());
        let store = LocalCasBlobStore::new(root.clone());

        let error = resolve_file_path(&record, &store).await.unwrap_err();
        assert!(error.to_string().contains("storage backend s3"));
        let _ = std::fs::remove_dir_all(root);
    }
}
