use serde_json::json;
use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
};
use tokio::{fs, task};
use uuid::Uuid;
use walkdir::WalkDir;

use crate::error::AppError;

const MAX_LOG_LINES: usize = 1000;

pub struct ProcessFileOptions<'a> {
    pub pool: &'a sqlx::PgPool,
    pub bundle_id: Uuid,
    pub bundle_hash: &'a str,
    pub data_root: &'a Path,
    pub file_name: &'a str,
    pub original_name: &'a str,
    pub content_type: Option<&'a str>,
    pub bytes: &'a [u8],
}

pub async fn process_uploaded_file(options: ProcessFileOptions<'_>) -> Result<(), AppError> {
    let ProcessFileOptions {
        pool,
        bundle_id,
        bundle_hash,
        data_root,
        file_name,
        original_name,
        content_type,
        bytes,
    } = options;

    let bundle_dir = data_root.join(bundle_hash);
    fs::create_dir_all(&bundle_dir).await.map_err(io_error)?;

    let disk_path = bundle_dir.join(file_name);
    fs::write(&disk_path, bytes).await.map_err(io_error)?;

    let relative_path = format!("/{bundle_hash}/{file_name}");
    let meta = json!({
        "original_name": original_name,
        "storage_path": disk_path.to_string_lossy(),
        "kind": "uploaded_file"
    });

    let file_id = insert_file_record(
        pool,
        bundle_id,
        None,
        file_name,
        &relative_path,
        false,
        Some(bytes.len() as i64),
        content_type,
        Some(meta),
    )
    .await?;

    if is_text_like(file_name, content_type) {
        ingest_text_file(pool, bundle_id, file_id, &disk_path).await?;
    }

    if is_zip_file(file_name) {
        let extracted_dir_name = format!("{file_name}_extracted");
        let extracted_dir = bundle_dir.join(&extracted_dir_name);
        fs::create_dir_all(&extracted_dir).await.map_err(io_error)?;

        extract_zip_archive(&disk_path, &extracted_dir).await?;

        let extracted_relative_path = format!("/{bundle_hash}/{extracted_dir_name}");
        let dir_meta = json!({
            "source": file_name,
            "storage_path": extracted_dir.to_string_lossy(),
            "kind": "extracted_dir"
        });

        let dir_id = insert_file_record(
            pool,
            bundle_id,
            Some(file_id),
            &extracted_dir_name,
            &extracted_relative_path,
            true,
            None,
            None,
            Some(dir_meta),
        )
        .await?;

        ingest_directory(
            pool,
            bundle_id,
            dir_id,
            &extracted_dir,
            &format!("{}/{extracted_dir_name}", bundle_hash),
        )
        .await?;
    }

    Ok(())
}

async fn ingest_directory(
    pool: &sqlx::PgPool,
    bundle_id: Uuid,
    parent_id: i64,
    dir_path: &Path,
    relative_root: &str,
) -> Result<(), AppError> {
    let mut entries = Vec::new();
    for entry in WalkDir::new(dir_path).into_iter().filter_map(Result::ok) {
        let path = entry.path();
        if path == dir_path {
            continue;
        }
        let rel = path.strip_prefix(dir_path).unwrap_or(path).to_path_buf();
        entries.push((rel, path.to_path_buf(), entry.file_type().is_dir()));
    }

    let mut id_map: HashMap<PathBuf, i64> = HashMap::new();
    id_map.insert(PathBuf::new(), parent_id);

    for (relative, disk_path, is_dir) in entries {
        let parent_rel = relative
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(PathBuf::new);
        let parent = *id_map.get(&parent_rel).unwrap_or(&parent_id);
        let name = disk_path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("unknown");
        let rel_string = relative.to_string_lossy().replace('\\', "/");
        let db_path = format!("/{}/{}", relative_root.trim_start_matches('/'), rel_string);
        let metadata = fs::metadata(&disk_path).await.map_err(io_error)?;
        let size_bytes = if metadata.is_file() {
            Some(metadata.len() as i64)
        } else {
            None
        };
        let meta = json!({
            "storage_path": disk_path.to_string_lossy(),
            "kind": if is_dir { "extracted_dir" } else { "extracted_file" }
        });

        let record_id = insert_file_record(
            pool,
            bundle_id,
            Some(parent),
            name,
            &db_path,
            is_dir,
            size_bytes,
            None,
            Some(meta),
        )
        .await?;

        if !is_dir && is_text_like(name, None) {
            ingest_text_file(pool, bundle_id, record_id, &disk_path).await?;
        }

        if is_dir {
            id_map.insert(relative, record_id);
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn insert_file_record(
    pool: &sqlx::PgPool,
    bundle_id: Uuid,
    parent_id: Option<i64>,
    name: &str,
    path: &str,
    is_dir: bool,
    size_bytes: Option<i64>,
    mime_type: Option<&str>,
    meta: Option<serde_json::Value>,
) -> Result<i64, AppError> {
    let record_id = sqlx::query_scalar::<_, i64>(
        r#"
        INSERT INTO files (
            bundle_id, parent_id, name, path, is_dir, size_bytes, mime_type, status, meta
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        RETURNING id
        "#,
    )
    .bind(bundle_id)
    .bind(parent_id)
    .bind(name)
    .bind(path)
    .bind(is_dir)
    .bind(size_bytes)
    .bind(mime_type)
    .bind(Some("READY"))
    .bind(meta)
    .fetch_one(pool)
    .await
    .map_err(AppError::Database)?;

    Ok(record_id)
}

async fn ingest_text_file(
    pool: &sqlx::PgPool,
    bundle_id: Uuid,
    file_id: i64,
    disk_path: &Path,
) -> Result<(), AppError> {
    let bytes = fs::read(disk_path).await.map_err(io_error)?;
    let content = String::from_utf8_lossy(&bytes);
    let mut inserted = 0usize;

    for (index, line) in content.lines().enumerate() {
        if inserted >= MAX_LOG_LINES {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        sqlx::query(
            r#"
            INSERT INTO log_segments (bundle_id, file_id, timeline, content, offset)
            VALUES ($1, $2, $3, $4, $5)
            "#,
        )
        .bind(bundle_id)
        .bind(file_id)
        .bind(Some("all".to_string()))
        .bind(trimmed)
        .bind(Some(index as i64))
        .execute(pool)
        .await
        .map_err(AppError::Database)?;
        inserted += 1;
    }

    Ok(())
}

async fn extract_zip_archive(src: &Path, dest: &Path) -> Result<(), AppError> {
    let src_path = src.to_path_buf();
    let dest_path = dest.to_path_buf();
    task::spawn_blocking(move || -> Result<(), AppError> {
        let file = std::fs::File::open(&src_path).map_err(io_error)?;
        let mut archive =
            zip::ZipArchive::new(file).map_err(|err| AppError::BadRequest(err.to_string()))?;

        for i in 0..archive.len() {
            let mut entry = archive
                .by_index(i)
                .map_err(|err| AppError::BadRequest(err.to_string()))?;
            let entry_path = sanitize_zip_path(entry.name());
            let out_path = dest_path.join(entry_path);

            if entry.is_dir() {
                std::fs::create_dir_all(&out_path).map_err(io_error)?;
            } else {
                if let Some(parent) = out_path.parent() {
                    std::fs::create_dir_all(parent).map_err(io_error)?;
                }
                let mut outfile = std::fs::File::create(&out_path).map_err(io_error)?;
                std::io::copy(&mut entry, &mut outfile).map_err(io_error)?;
            }
        }
        Ok(())
    })
    .await
    .map_err(|err| io_error(io::Error::other(err.to_string())))??;

    Ok(())
}

fn sanitize_zip_path(name: &str) -> PathBuf {
    let mut sanitized = PathBuf::new();
    for component in Path::new(name).components() {
        if let std::path::Component::Normal(os_str) = component
            && let Some(segment) = os_str.to_str()
        {
            let safe = segment
                .chars()
                .map(|ch| {
                    if ch.is_ascii_alphanumeric() || "-_.".contains(ch) {
                        ch
                    } else {
                        '_'
                    }
                })
                .collect::<String>();
            sanitized.push(safe);
        }
    }
    sanitized
}

fn is_text_like(name: &str, content_type: Option<&str>) -> bool {
    if let Some(ct) = content_type
        && ct.starts_with("text/")
    {
        return true;
    }
    matches!(
        Path::new(name)
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_ascii_lowercase())
            .as_deref(),
        Some("log") | Some("txt")
    )
}

fn is_zip_file(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("zip"))
        .unwrap_or(false)
}

fn io_error(err: std::io::Error) -> AppError {
    AppError::Io(err)
}
