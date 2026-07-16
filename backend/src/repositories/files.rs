use std::path::PathBuf;

use serde_json::json;
use sqlx::FromRow;

use crate::{
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

        if let Some(row) = row {
            paths.push(validated_storage_path(&row, data_root)?);
        }
    }

    paths.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    paths.dedup();
    Ok(paths)
}

pub async fn delete_index_rows_for_file(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    file_id: i64,
) -> Result<(), AppError> {
    for statement in [
        "DELETE FROM log_line_offsets WHERE file_id = ?",
        "DELETE FROM log_segments_fts WHERE file_id = ?",
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

pub fn resolve_file_path(
    record: &FileRow,
    data_root: &std::path::Path,
) -> Result<PathBuf, AppError> {
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
    let candidate = storage_path_candidate(record, data_root);

    let canonical_data_root = std::fs::canonicalize(data_root).map_err(AppError::Io)?;
    let canonical_candidate = std::fs::canonicalize(&candidate).map_err(AppError::Io)?;

    if !canonical_candidate.starts_with(&canonical_data_root) {
        return Err(AppError::BadRequest(
            "file path is outside data root".into(),
        ));
    }

    Ok(canonical_candidate)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{FileRow, storage_path_candidate};

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
}
