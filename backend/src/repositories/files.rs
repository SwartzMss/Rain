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
        SELECT f.id, f.name, f.path, f.is_dir, f.size_bytes, f.line_count, f.mime_type,
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
            SELECT f.id, f.name, f.path, f.is_dir, f.size_bytes, f.line_count, f.mime_type,
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
            SELECT f.id, f.name, f.path, f.is_dir, f.size_bytes, f.line_count, f.mime_type,
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

pub async fn fetch_subtree_ids(
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

pub async fn fetch_extracted_child_ids(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    bundle_id: &str,
    parent_id: i64,
) -> Result<Vec<i64>, AppError> {
    let rows = sqlx::query_as::<_, FileRow>(
        r#"
        SELECT f.id, f.name, f.path, f.is_dir, f.size_bytes, f.line_count, f.mime_type,
               f.status, f.meta, f.blob_id, b.storage_backend, b.storage_key, b.state AS blob_state
        FROM files f LEFT JOIN blobs b ON b.id = f.blob_id
        WHERE f.bundle_id = ? AND f.parent_id = ?
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

pub async fn fetch_storage_paths_for_ids(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    data_root: &std::path::Path,
    bundle_id: &str,
    file_ids: &[i64],
) -> Result<Vec<PathBuf>, AppError> {
    let mut paths = Vec::new();
    for file_id in file_ids {
        let row = sqlx::query_as::<_, FileRow>(
            r#"
            SELECT f.id, f.name, f.path, f.is_dir, f.size_bytes, f.line_count, f.mime_type,
                   f.status, f.meta, f.blob_id, b.storage_backend, b.storage_key, b.state AS blob_state
            FROM files f LEFT JOIN blobs b ON b.id = f.blob_id
            WHERE f.bundle_id = ? AND f.id = ? AND f.blob_id IS NULL
            LIMIT 1
            "#,
        )
        .bind(bundle_id)
        .bind(file_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(AppError::Database)?;

        if let Some(row) = row {
            paths.push(validated_storage_path(&row, data_root)?);
        }
    }

    paths.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    paths.dedup();
    Ok(paths)
}

pub async fn fetch_legacy_bundle_paths(
    pool: &sqlx::SqlitePool,
    data_root: &std::path::Path,
    bundle_id: &str,
) -> Result<Vec<PathBuf>, AppError> {
    let rows = sqlx::query_as::<_, FileRow>(
        r#"
        SELECT f.id, f.name, f.path, f.is_dir, f.size_bytes, f.line_count, f.mime_type,
               f.status, f.meta, f.blob_id, NULL AS storage_backend,
               NULL AS storage_key, NULL AS blob_state
        FROM files f
        WHERE f.bundle_id = ? AND f.blob_id IS NULL
        "#,
    )
    .bind(bundle_id)
    .fetch_all(pool)
    .await
    .map_err(AppError::Database)?;
    let mut paths = rows
        .iter()
        .map(|row| validated_storage_path(row, data_root))
        .collect::<Result<Vec<_>, _>>()?;
    if !rows.is_empty() {
        let bundle_hash: String = sqlx::query_scalar("SELECT hash FROM bundles WHERE id = ?")
            .bind(bundle_id)
            .fetch_one(pool)
            .await
            .map_err(AppError::Database)?;
        let hash_path = std::path::Path::new(&bundle_hash);
        if hash_path.is_absolute()
            || hash_path.components().count() != 1
            || hash_path.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir | std::path::Component::Prefix(_)
                )
            })
        {
            return Err(AppError::BadRequest(
                "invalid legacy bundle hash path".into(),
            ));
        }
        let bundle_root = data_root.join(hash_path);
        let canonical_data_root = std::fs::canonicalize(data_root).map_err(AppError::Io)?;
        let boundary = canonicalize_existing_ancestor(&bundle_root)?;
        if !boundary.starts_with(&canonical_data_root) {
            return Err(AppError::BadRequest(
                "legacy bundle path is outside data root".into(),
            ));
        }
        paths.push(bundle_root);
    }
    paths.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    paths.dedup();
    Ok(paths)
}

pub async fn remove_legacy_paths(paths: Vec<PathBuf>) -> Result<(), AppError> {
    for path in paths {
        match tokio::fs::metadata(&path).await {
            Ok(metadata) if metadata.is_dir() => {
                tokio::fs::remove_dir_all(path)
                    .await
                    .map_err(AppError::Io)?;
            }
            Ok(_) => {
                tokio::fs::remove_file(path).await.map_err(AppError::Io)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(AppError::Io(error)),
        }
    }
    Ok(())
}

pub async fn delete_index_rows_for_file(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    file_id: i64,
) -> Result<(), AppError> {
    for statement in [
        "DELETE FROM log_line_offsets WHERE file_id = ?",
        "DELETE FROM log_segments WHERE file_id = ?",
    ] {
        sqlx::query(statement)
            .bind(file_id)
            .execute(&mut **tx)
            .await
            .map_err(AppError::Database)?;
    }
    Ok(())
}

pub async fn delete_file_row(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    bundle_id: &str,
    file_id: i64,
) -> Result<(), AppError> {
    sqlx::query("DELETE FROM files WHERE bundle_id = ? AND id = ?")
        .bind(bundle_id)
        .bind(file_id)
        .execute(&mut **tx)
        .await
        .map_err(AppError::Database)?;
    Ok(())
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
    data_root: &std::path::Path,
) -> Result<PathBuf, AppError> {
    if let Some(storage_key) = record.storage_key.as_deref() {
        if record.blob_state.as_deref() != Some("READY") {
            return Err(AppError::Conflict(format!(
                "blob is not readable in state {}",
                record.blob_state.as_deref().unwrap_or("UNKNOWN")
            )));
        }
        return blob_store.materialize(storage_key).await;
    }
    validated_storage_path(record, data_root)
}

fn storage_path_candidate(record: &FileRow, data_root: &std::path::Path) -> PathBuf {
    storage_path_from_meta(&record.meta)
        .unwrap_or_else(|| data_root.join(record.path.trim_start_matches('/')))
}

fn validated_storage_path(
    record: &FileRow,
    data_root: &std::path::Path,
) -> Result<PathBuf, AppError> {
    let candidate = match (
        record.storage_backend.as_deref(),
        record.storage_key.as_deref(),
    ) {
        (Some(_), Some(_)) => {
            return Err(AppError::Config(
                "blob path must be resolved through BlobStore".into(),
            ));
        }
        (Some(backend), _) => {
            return Err(AppError::BadRequest(format!(
                "unsupported blob storage backend: {backend}"
            )));
        }
        _ => storage_path_candidate(record, data_root),
    };

    let canonical_data_root = std::fs::canonicalize(data_root).map_err(AppError::Io)?;
    let (resolved_path, canonical_boundary) = match std::fs::canonicalize(&candidate) {
        Ok(path) => (path.clone(), path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (
            candidate.clone(),
            canonicalize_existing_ancestor(&candidate)?,
        ),
        Err(error) => return Err(AppError::Io(error)),
    };

    if !canonical_boundary.starts_with(&canonical_data_root) {
        return Err(AppError::BadRequest(
            "file path is outside data root".into(),
        ));
    }

    Ok(resolved_path)
}

fn canonicalize_existing_ancestor(path: &std::path::Path) -> Result<PathBuf, AppError> {
    let mut ancestor = path.to_path_buf();
    loop {
        match std::fs::canonicalize(&ancestor) {
            Ok(path) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if !ancestor.pop() {
                    return Err(AppError::Io(error));
                }
            }
            Err(error) => return Err(AppError::Io(error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{FileRow, storage_path_candidate, validated_storage_path};

    fn row(path: &str, meta: Option<&str>) -> FileRow {
        FileRow {
            id: 1,
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

    #[test]
    fn stable_file_path_is_used_when_metadata_has_no_storage_path() {
        let record = row("/bundle/logs/app.log", Some(r#"{"kind":"extracted_file"}"#));

        assert_eq!(
            storage_path_candidate(&record, Path::new("/data")),
            PathBuf::from("/data/bundle/logs/app.log")
        );
    }

    #[test]
    fn legacy_storage_path_remains_preferred() {
        let record = row(
            "/bundle/logs/app.log",
            Some(r#"{"storage_path":"/legacy/bundle/app.log"}"#),
        );

        assert_eq!(
            storage_path_candidate(&record, Path::new("/data")),
            PathBuf::from("/legacy/bundle/app.log")
        );
    }

    #[test]
    fn missing_file_path_can_still_be_selected_for_database_cleanup() {
        let root = std::env::temp_dir().join(format!(
            "rain-missing-storage-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(root.join("bundle")).unwrap();
        let record = row("/bundle/missing.log", None);

        let selected = validated_storage_path(&record, &root).unwrap();

        assert_eq!(selected, root.join("bundle/missing.log"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn existing_legacy_path_outside_data_root_is_rejected() {
        let root = std::env::temp_dir().join(format!(
            "rain-storage-root-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let outside = std::env::temp_dir().join(format!(
            "rain-storage-outside-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&outside, "outside").unwrap();
        let meta = serde_json::json!({ "storage_path": outside }).to_string();
        let record = row("/bundle/app.log", Some(&meta));

        let error = validated_storage_path(&record, &root).unwrap_err();

        assert!(error.to_string().contains("outside data root"));
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_file(outside);
    }
}
