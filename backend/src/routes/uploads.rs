use actix_multipart::{Field, Multipart};
use actix_web::{HttpResponse, get, http::StatusCode, post, web};
use futures_util::TryStreamExt;
use serde::Serialize;
use std::{
    future::Future,
    io,
    path::{Path, PathBuf},
};
use tokio::{fs, io::AsyncWriteExt};
use tracing::{error, warn};
use uuid::Uuid;

use crate::{
    AppState,
    error::AppError,
    ingest::{ArchiveBudget, ProcessFileOptions, process_uploaded_file},
    models::issues::{UploadStage, UploadStatus},
};

use super::issues::{normalize_issue_code, require_issue_exists};

const WINDOWS_MOVE_RETRY_DELAYS_MS: [u64; 7] = [100, 200, 400, 800, 1600, 3200, 5000];
const DEFAULT_MOVE_RETRY_DELAYS_MS: [u64; 3] = [150, 300, 600];

async fn move_bundle_directory_with_retry(
    source: &Path,
    destination: &Path,
) -> Result<(), AppError> {
    let retry_delays = if cfg!(windows) {
        WINDOWS_MOVE_RETRY_DELAYS_MS.as_slice()
    } else {
        DEFAULT_MOVE_RETRY_DELAYS_MS.as_slice()
    };
    move_bundle_directory_with_retry_using(
        source,
        destination,
        retry_delays,
        cfg!(windows),
        fs::rename,
    )
    .await
}

async fn move_bundle_directory_with_retry_using<R, Fut>(
    source: &Path,
    destination: &Path,
    retry_delays_ms: &[u64],
    windows: bool,
    mut rename: R,
) -> Result<(), AppError>
where
    R: FnMut(PathBuf, PathBuf) -> Fut + Send,
    Fut: Future<Output = io::Result<()>> + Send,
{
    let source = absolute_diagnostic_path(source);
    let destination = absolute_diagnostic_path(destination);
    let max_attempts = retry_delays_ms.len() + 1;

    for attempt in 1..=max_attempts {
        match rename(source.clone(), destination.clone()).await {
            Ok(()) => return Ok(()),
            Err(error) => {
                let error_kind = error.kind();
                let os_error = error.raw_os_error();
                let retryable = is_retryable_bundle_move_error(&error, windows);
                if retryable && attempt < max_attempts {
                    let delay_ms = retry_delays_ms[attempt - 1];
                    warn!(
                        attempt,
                        max_attempts,
                        error_kind = ?error_kind,
                        os_error,
                        source = %source.display(),
                        destination = %destination.display(),
                        next_retry_ms = delay_ms,
                        "transient bundle directory move failure; retrying"
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                    continue;
                }

                error!(
                    attempt,
                    max_attempts,
                    error_kind = ?error_kind,
                    os_error,
                    retryable,
                    source = %source.display(),
                    destination = %destination.display(),
                    "bundle directory move failed"
                );
                return Err(AppError::Io(io::Error::new(
                    error_kind,
                    format!(
                        "move processed bundle {} -> {} failed on attempt {attempt}/{max_attempts} (kind: {error_kind:?}, os error: {os_error:?}): {error}",
                        source.display(),
                        destination.display()
                    ),
                )));
            }
        }
    }

    unreachable!("bundle move retry loop always returns")
}

fn is_retryable_bundle_move_error(error: &io::Error, windows: bool) -> bool {
    error.kind() == io::ErrorKind::PermissionDenied
        || (windows && matches!(error.raw_os_error(), Some(5 | 32 | 33)))
}

fn absolute_diagnostic_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    std::env::current_dir()
        .map(|current| current.join(path))
        .unwrap_or_else(|_| path.to_path_buf())
}

// scoped under /api in routes::register, so use relative path
#[post("/issues/{issue_code}/uploads")]
pub async fn upload_logs(
    state: web::Data<AppState>,
    path: web::Path<String>,
    mut payload: Multipart,
) -> Result<HttpResponse, AppError> {
    let issue_code = normalize_issue_code(&path.into_inner())?;
    require_issue_exists(&state.pool, &issue_code).await?;

    let mut files: Vec<UploadedFile> = Vec::new();
    let upload_id = Uuid::new_v4().simple().to_string();
    let temp_dir = state.data_root.join(".tmp").join(&upload_id);
    fs::create_dir_all(&temp_dir).await.map_err(AppError::Io)?;
    let mut total_bytes: u64 = 0;

    while let Some(mut field) = payload
        .try_next()
        .await
        .map_err(|err| AppError::BadRequest(format!("multipart error: {err}")))?
    {
        let content_disposition = field.content_disposition().clone();
        let field_name = content_disposition.get_name().unwrap_or("").to_string();

        match field_name.as_str() {
            "issue_code" => {
                collect_text_field(&mut field, state.limits.upload.max_text_field_size).await?;
            }
            "files" => {
                let filename = content_disposition
                    .get_filename()
                    .map(|name| name.to_string())
                    .unwrap_or_else(|| "upload.log".into());

                let content_type = field.content_type().map(|mime| mime.to_string());
                let display_name = sanitize_filename(&filename);
                let storage_name = unique_storage_name(&filename);
                let temp_name = format!("{}-{storage_name}", files.len());
                let temp_path = temp_dir.join(temp_name);
                let size_bytes = match collect_file_field(
                    &mut field,
                    &temp_path,
                    state.limits.upload.max_file_size,
                    &filename,
                )
                .await
                {
                    Ok(size_bytes) => size_bytes,
                    Err(error) => {
                        let _ = fs::remove_dir_all(&temp_dir).await;
                        return Err(error);
                    }
                };

                if size_bytes > 0 {
                    if files.len() >= state.limits.upload.max_files {
                        let _ = fs::remove_dir_all(&temp_dir).await;
                        return Err(AppError::BadRequest(format!(
                            "too many files; max {} files per upload",
                            state.limits.upload.max_files
                        )));
                    }
                    total_bytes = total_bytes.saturating_add(size_bytes);
                    if total_bytes > state.limits.upload.max_total_size {
                        let _ = fs::remove_dir_all(&temp_dir).await;
                        return Err(AppError::BadRequest(format!(
                            "upload is too large; max total size is {}",
                            format_bytes(state.limits.upload.max_total_size)
                        )));
                    }
                    files.push(UploadedFile {
                        original_name: filename,
                        display_name,
                        storage_name,
                        temp_path,
                        size_bytes,
                        content_type,
                    });
                } else {
                    let _ = fs::remove_file(&temp_path).await;
                }
            }
            _ => {
                // Ignore unknown fields
                collect_binary_field(
                    &mut field,
                    state.limits.upload.max_text_field_size,
                    &field_name,
                )
                .await?;
            }
        }
    }

    if files.is_empty() {
        let _ = fs::remove_dir_all(&temp_dir).await;
        return Err(AppError::BadRequest("no files provided".into()));
    }

    let bundle_hash = Uuid::new_v4().simple().to_string();
    let bundle_name = if files.len() == 1 {
        files[0].display_name.clone()
    } else {
        format!("{} 等 {} 个文件", files[0].display_name, files.len())
    };

    let bundle_id = Uuid::new_v4().simple().to_string();

    let insert_bundle_result = sqlx::query(
        r#"
        INSERT INTO bundles (id, issue_code, hash, name, status, process_stage, size_bytes)
        VALUES (?, ?, ?, ?, 'PROCESSING', 'PENDING', ?)
        "#,
    )
    .bind(&bundle_id)
    .bind(&issue_code)
    .bind(&bundle_hash)
    .bind(&bundle_name)
    .bind(Some(total_bytes as i64))
    .execute(&state.pool)
    .await
    .map_err(AppError::Database);
    if let Err(error) = insert_bundle_result {
        let _ = fs::remove_dir_all(&temp_dir).await;
        return Err(error);
    }

    let file_count = files.len() as u64;
    let pool = state.pool.clone();
    let data_root = state.data_root.clone();
    let staging_root = temp_dir.join("staging");
    let processing_permits = state.processing_permits.clone();
    let archive_config = state.limits.archive.clone();
    let indexing_config = state.limits.indexing.clone();
    let task_bundle_id = bundle_id.clone();
    let task_bundle_hash = bundle_hash.clone();
    tokio::spawn(async move {
        let _permit = match processing_permits.acquire_owned().await {
            Ok(permit) => permit,
            Err(error) => {
                error!(
                    bundle_id = %task_bundle_id,
                    bundle_hash = %task_bundle_hash,
                    error = %error,
                    "failed to acquire upload processing permit"
                );
                let _ = update_bundle_status(&pool, &task_bundle_id, "FAILED", "FAILED").await;
                let _ = fs::remove_dir_all(&temp_dir).await;
                return;
            }
        };

        let process_result = async {
            let archive_budget = ArchiveBudget::new(archive_config);
            for uploaded in &files {
                process_uploaded_file(ProcessFileOptions {
                    pool: &pool,
                    bundle_id: &task_bundle_id,
                    bundle_hash: &task_bundle_hash,
                    data_root: &staging_root,
                    storage_name: &uploaded.storage_name,
                    original_name: &uploaded.original_name,
                    display_name: &uploaded.display_name,
                    content_type: uploaded.content_type.as_deref(),
                    source_path: &uploaded.temp_path,
                    size_bytes: uploaded.size_bytes,
                    archive_budget: archive_budget.clone(),
                    indexing: &indexing_config,
                })
                .await?;
            }

            let staging_bundle_dir = staging_root.join(&task_bundle_hash);
            let final_bundle_dir = data_root.join(&task_bundle_hash);
            if fs::metadata(&final_bundle_dir).await.is_ok() {
                return Err(AppError::BadRequest(format!(
                    "bundle directory already exists: {}",
                    final_bundle_dir.display()
                )));
            }
            if let Some(parent) = final_bundle_dir.parent() {
                fs::create_dir_all(parent).await.map_err(AppError::Io)?;
            }
            move_bundle_directory_with_retry(&staging_bundle_dir, &final_bundle_dir).await?;
            finalize_bundle_ready_with_retry(
                &pool,
                &task_bundle_id,
                &staging_bundle_dir,
                &final_bundle_dir,
            )
            .await?;
            Ok::<(), AppError>(())
        }
        .await;

        if let Err(error) = process_result {
            error!(
                bundle_id = %task_bundle_id,
                bundle_hash = %task_bundle_hash,
                error = %error,
                "failed to process uploaded log bundle"
            );
            let _ = cleanup_failed_bundle_artifacts(
                &pool,
                &task_bundle_id,
                &data_root,
                &staging_root,
                &task_bundle_hash,
            )
            .await;
            let _ = update_bundle_status(&pool, &task_bundle_id, "FAILED", "FAILED").await;
        }

        let _ = fs::remove_dir_all(&temp_dir).await;
    });

    Ok(
        HttpResponse::build(StatusCode::ACCEPTED).json(UploadResponse {
            task_id: bundle_hash.clone(),
            issue_code,
            bundle_hash,
            status: UploadStatus::Processing,
            stage: UploadStage::Pending,
            file_count,
            total_bytes,
        }),
    )
}

#[derive(Serialize)]
struct UploadResponse {
    task_id: String,
    issue_code: String,
    bundle_hash: String,
    status: UploadStatus,
    stage: UploadStage,
    file_count: u64,
    total_bytes: u64,
}

#[get("/uploads/{task_id}")]
pub async fn get_upload_task(
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> Result<HttpResponse, AppError> {
    let task_id = path.into_inner();
    let row = sqlx::query_as::<_, UploadTaskRow>(
        r#"
        SELECT issue_code, hash, status, process_stage, size_bytes
        FROM bundles
        WHERE hash = ?
        LIMIT 1
        "#,
    )
    .bind(&task_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(AppError::Database)?
    .ok_or_else(|| AppError::NotFound(format!("upload task {task_id}")))?;

    let status = UploadStatus::from_db_value(&row.status);
    let progress_percent = match status {
        UploadStatus::Ready | UploadStatus::Failed => 100,
        UploadStatus::Processing => 0,
        UploadStatus::Pending => 0,
    };

    Ok(HttpResponse::Ok().json(UploadTaskResponse {
        task_id: row.hash.clone(),
        issue_code: row.issue_code,
        bundle_hash: row.hash,
        status,
        stage: UploadStage::from_db_value(&row.process_stage),
        progress_percent,
        total_bytes: row.size_bytes.unwrap_or(0).max(0) as u64,
    }))
}

#[derive(sqlx::FromRow)]
struct UploadTaskRow {
    issue_code: String,
    hash: String,
    status: String,
    process_stage: String,
    size_bytes: Option<i64>,
}

#[derive(Serialize)]
struct UploadTaskResponse {
    task_id: String,
    issue_code: String,
    bundle_hash: String,
    status: UploadStatus,
    stage: UploadStage,
    progress_percent: u8,
    total_bytes: u64,
}

struct UploadedFile {
    original_name: String,
    display_name: String,
    storage_name: String,
    temp_path: std::path::PathBuf,
    size_bytes: u64,
    content_type: Option<String>,
}

async fn collect_text_field(field: &mut Field, limit: u64) -> Result<String, AppError> {
    let bytes = collect_binary_field(field, limit, "text field").await?;
    let value = String::from_utf8(bytes)
        .map_err(|_| AppError::BadRequest("field is not valid UTF-8".into()))?;
    Ok(value.trim().to_string())
}

async fn collect_binary_field(
    field: &mut Field,
    limit: u64,
    label: &str,
) -> Result<Vec<u8>, AppError> {
    let mut data = Vec::new();
    while let Some(chunk) = field
        .try_next()
        .await
        .map_err(|err| AppError::BadRequest(format!("failed to read field: {err}")))?
    {
        if (data.len() as u64).saturating_add(chunk.len() as u64) > limit {
            return Err(AppError::BadRequest(format!(
                "{label} is too large; max size is {}",
                format_bytes(limit)
            )));
        }
        data.extend_from_slice(&chunk);
    }
    Ok(data)
}

async fn collect_file_field(
    field: &mut Field,
    path: &std::path::Path,
    limit: u64,
    label: &str,
) -> Result<u64, AppError> {
    let mut file = fs::File::create(path).await.map_err(AppError::Io)?;
    let mut written = 0u64;
    while let Some(chunk) = field
        .try_next()
        .await
        .map_err(|err| AppError::BadRequest(format!("failed to read field: {err}")))?
    {
        written = written.saturating_add(chunk.len() as u64);
        if written > limit {
            return Err(AppError::BadRequest(format!(
                "{label} is too large; max size is {}",
                format_bytes(limit)
            )));
        }
        file.write_all(&chunk).await.map_err(AppError::Io)?;
    }
    file.flush().await.map_err(AppError::Io)?;
    Ok(written)
}

async fn update_bundle_status(
    pool: &sqlx::SqlitePool,
    bundle_id: &str,
    status: &str,
    stage: &str,
) -> Result<(), AppError> {
    sqlx::query("UPDATE bundles SET status = ?, process_stage = ? WHERE id = ?")
        .bind(status)
        .bind(stage)
        .bind(bundle_id)
        .execute(pool)
        .await
        .map_err(AppError::Database)?;
    Ok(())
}

async fn finalize_bundle_ready_with_retry(
    pool: &sqlx::SqlitePool,
    bundle_id: &str,
    staging_bundle_dir: &std::path::Path,
    final_bundle_dir: &std::path::Path,
) -> Result<(), AppError> {
    let mut last_error: Option<AppError> = None;
    for attempt in 1..=3 {
        match finalize_bundle_ready(pool, bundle_id, staging_bundle_dir, final_bundle_dir).await {
            Ok(()) => return Ok(()),
            Err(error) => {
                error!(
                    bundle_id = %bundle_id,
                    attempt,
                    error = %error,
                    "failed to finalize uploaded log bundle"
                );
                last_error = Some(error);
                tokio::time::sleep(std::time::Duration::from_millis(100 * attempt)).await;
            }
        }
    }

    let _ = update_bundle_status(pool, bundle_id, "FAILED", "FAILED").await;
    Err(last_error.unwrap_or_else(|| AppError::Database(sqlx::Error::RowNotFound)))
}

async fn finalize_bundle_ready(
    pool: &sqlx::SqlitePool,
    bundle_id: &str,
    staging_bundle_dir: &std::path::Path,
    final_bundle_dir: &std::path::Path,
) -> Result<(), AppError> {
    let mut tx = pool.begin().await.map_err(AppError::Database)?;
    let rows = sqlx::query_as::<_, FileMetaRow>(
        r#"
        SELECT id, meta
        FROM files
        WHERE bundle_id = ?
        "#,
    )
    .bind(bundle_id)
    .fetch_all(&mut *tx)
    .await
    .map_err(AppError::Database)?;

    for row in rows {
        let Some(meta_text) = row.meta else {
            continue;
        };
        let mut meta: serde_json::Value = serde_json::from_str(&meta_text)
            .map_err(|err| AppError::BadRequest(format!("invalid file metadata: {err}")))?;
        let Some(storage_path) = meta.get("storage_path").and_then(|value| value.as_str()) else {
            continue;
        };
        let current_path = std::path::PathBuf::from(storage_path);
        if !current_path.starts_with(staging_bundle_dir) {
            continue;
        }
        let relative = current_path
            .strip_prefix(staging_bundle_dir)
            .map_err(|err| AppError::BadRequest(format!("invalid staging path: {err}")))?;
        let final_path = final_bundle_dir.join(relative);
        if let Some(object) = meta.as_object_mut() {
            object.insert(
                "storage_path".to_string(),
                serde_json::Value::String(final_path.to_string_lossy().to_string()),
            );
        }
        sqlx::query("UPDATE files SET meta = ? WHERE id = ?")
            .bind(meta.to_string())
            .bind(row.id)
            .execute(&mut *tx)
            .await
            .map_err(AppError::Database)?;
    }
    sqlx::query("UPDATE bundles SET status = 'READY', process_stage = 'READY' WHERE id = ?")
        .bind(bundle_id)
        .execute(&mut *tx)
        .await
        .map_err(AppError::Database)?;

    tx.commit().await.map_err(AppError::Database)?;
    Ok(())
}

async fn cleanup_failed_bundle_artifacts(
    pool: &sqlx::SqlitePool,
    bundle_id: &str,
    data_root: &std::path::Path,
    staging_root: &std::path::Path,
    bundle_hash: &str,
) -> Result<(), AppError> {
    let mut tx = pool.begin().await.map_err(AppError::Database)?;

    sqlx::query(
        "DELETE FROM log_line_offsets WHERE file_id IN (SELECT id FROM files WHERE bundle_id = ?)",
    )
    .bind(bundle_id)
    .execute(&mut *tx)
    .await
    .map_err(AppError::Database)?;

    sqlx::query("DELETE FROM log_events WHERE bundle_id = ?")
        .bind(bundle_id)
        .execute(&mut *tx)
        .await
        .map_err(AppError::Database)?;

    sqlx::query("DELETE FROM log_segments_fts WHERE bundle_id = ?")
        .bind(bundle_id)
        .execute(&mut *tx)
        .await
        .map_err(AppError::Database)?;

    sqlx::query("DELETE FROM log_segments WHERE bundle_id = ?")
        .bind(bundle_id)
        .execute(&mut *tx)
        .await
        .map_err(AppError::Database)?;

    sqlx::query("DELETE FROM files WHERE bundle_id = ?")
        .bind(bundle_id)
        .execute(&mut *tx)
        .await
        .map_err(AppError::Database)?;

    tx.commit().await.map_err(AppError::Database)?;

    let staging_bundle_dir = staging_root.join(bundle_hash);
    if fs::metadata(&staging_bundle_dir).await.is_ok() {
        let _ = fs::remove_dir_all(&staging_bundle_dir).await;
    }

    let final_bundle_dir = data_root.join(bundle_hash);
    if fs::metadata(&final_bundle_dir).await.is_ok() {
        let _ = fs::remove_dir_all(&final_bundle_dir).await;
    }

    Ok(())
}

#[derive(sqlx::FromRow)]
struct FileMetaRow {
    id: i64,
    meta: Option<String>,
}

pub(crate) fn format_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{} GiB", bytes / 1024 / 1024 / 1024)
    } else if bytes >= 1024 * 1024 {
        format!("{} MiB", bytes / 1024 / 1024)
    } else if bytes >= 1024 {
        format!("{} KiB", bytes / 1024)
    } else {
        format!("{bytes} B")
    }
}

fn sanitize_filename(name: &str) -> String {
    use std::path::Path;
    let fallback = "upload.log";
    let file_name = Path::new(name)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(fallback);
    let sanitized: String = file_name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        fallback.into()
    } else {
        sanitized
    }
}

fn unique_storage_name(original_name: &str) -> String {
    use std::path::Path;

    let suffix = Path::new(original_name)
        .extension()
        .and_then(|value| value.to_str())
        .map(sanitize_extension)
        .filter(|value| !value.is_empty())
        .map(|value| format!(".{value}"))
        .unwrap_or_default();
    format!("{}{}", Uuid::new_v4().simple(), suffix)
}

fn sanitize_extension(extension: &str) -> String {
    extension
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .take(16)
        .collect()
}

#[cfg(test)]
mod tests {
    use std::{
        future::ready,
        io,
        path::Path,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };

    use super::move_bundle_directory_with_retry_using;

    #[tokio::test]
    async fn retries_windows_sharing_violations_until_move_succeeds() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let rename_attempts = attempts.clone();

        move_bundle_directory_with_retry_using(
            Path::new("staging/bundle"),
            Path::new("uploads/bundle"),
            &[0, 0, 0],
            true,
            move |_, _| {
                let attempt = rename_attempts.fetch_add(1, Ordering::SeqCst);
                ready(if attempt < 3 {
                    Err(io::Error::from_raw_os_error(32))
                } else {
                    Ok(())
                })
            },
        )
        .await
        .expect("transient Windows sharing violation should recover");

        assert_eq!(attempts.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn fails_after_windows_lock_retry_window_is_exhausted() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let rename_attempts = attempts.clone();

        let error = move_bundle_directory_with_retry_using(
            Path::new("staging/bundle"),
            Path::new("uploads/bundle"),
            &[0, 0],
            true,
            move |_, _| {
                rename_attempts.fetch_add(1, Ordering::SeqCst);
                ready(Err(io::Error::from_raw_os_error(33)))
            },
        )
        .await
        .expect_err("persistent Windows lock violation should fail");

        assert_eq!(attempts.load(Ordering::SeqCst), 3);
        assert!(error.to_string().contains("attempt 3/3"));
        assert!(error.to_string().contains("staging"));
        assert!(error.to_string().contains("uploads"));
    }

    #[tokio::test]
    async fn does_not_retry_non_recoverable_move_errors() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let rename_attempts = attempts.clone();

        move_bundle_directory_with_retry_using(
            Path::new("staging/bundle"),
            Path::new("uploads/bundle"),
            &[0, 0, 0],
            true,
            move |_, _| {
                rename_attempts.fetch_add(1, Ordering::SeqCst);
                ready(Err(io::Error::new(io::ErrorKind::NotFound, "missing")))
            },
        )
        .await
        .expect_err("non-recoverable move error should fail immediately");

        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }
}
