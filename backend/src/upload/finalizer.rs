use std::{
    future::Future,
    io,
    path::{Path, PathBuf},
};

use tokio::fs;
use tracing::{error, warn};

use crate::{
    db::{CLEANUP_BATCH_SIZE, cleanup_bundle_content_batched},
    error::AppError,
};

use super::lifecycle::{update_bundle_status, user_facing_failure_reason};

const WINDOWS_MOVE_RETRY_DELAYS_MS: [u64; 7] = [100, 200, 400, 800, 1600, 3200, 5000];
const DEFAULT_MOVE_RETRY_DELAYS_MS: [u64; 3] = [150, 300, 600];
const FAILURE_STATUS_RETRY_DELAYS_MS: [u64; 2] = [100, 250];

pub(crate) async fn move_bundle_directory_with_retry(
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

pub async fn finalize_bundle_failed(
    pool: &sqlx::SqlitePool,
    bundle_id: &str,
    data_root: &Path,
    staging_root: &Path,
    bundle_hash: &str,
    failure: &AppError,
) {
    let reason = user_facing_failure_reason(failure);
    let max_attempts = FAILURE_STATUS_RETRY_DELAYS_MS.len() + 1;
    let mut terminal_state_persisted = false;

    for attempt in 1..=max_attempts {
        match update_bundle_status(pool, bundle_id, "FAILED", "FAILED", Some(&reason)).await {
            Ok(()) => {
                terminal_state_persisted = true;
                break;
            }
            Err(error) => {
                error!(
                    bundle_id,
                    bundle_hash,
                    attempt,
                    max_attempts,
                    error = %error,
                    "failed to persist terminal upload state"
                );
                if attempt < max_attempts {
                    tokio::time::sleep(std::time::Duration::from_millis(
                        FAILURE_STATUS_RETRY_DELAYS_MS[attempt - 1],
                    ))
                    .await;
                }
            }
        }
    }

    if !terminal_state_persisted {
        error!(
            bundle_id,
            bundle_hash,
            "upload terminal state could not be persisted; startup recovery will retry"
        );
    }

    if let Err(error) = cleanup_failed_bundle_database_artifacts(pool, bundle_id).await {
        error!(
            bundle_id,
            bundle_hash,
            error = %error,
            "failed to clean database artifacts for failed upload"
        );
    }

    for path in [staging_root.join(bundle_hash), data_root.join(bundle_hash)] {
        match fs::remove_dir_all(&path).await {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => error!(
                bundle_id,
                bundle_hash,
                path = %path.display(),
                error = %error,
                "failed to clean filesystem artifacts for failed upload"
            ),
        }
    }
}

pub async fn finalize_bundle_ready_with_retry(
    pool: &sqlx::SqlitePool,
    bundle_id: &str,
    staging_bundle_dir: &Path,
    final_bundle_dir: &Path,
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

    Err(last_error.unwrap_or_else(|| AppError::Database(sqlx::Error::RowNotFound)))
}

async fn finalize_bundle_ready(
    pool: &sqlx::SqlitePool,
    bundle_id: &str,
    staging_bundle_dir: &Path,
    final_bundle_dir: &Path,
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
        let current_path = PathBuf::from(storage_path);
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
    sqlx::query(
        "UPDATE bundles SET status = 'READY', process_stage = 'READY', failure_reason = NULL WHERE id = ?",
    )
        .bind(bundle_id)
        .execute(&mut *tx)
        .await
        .map_err(AppError::Database)?;

    tx.commit().await.map_err(AppError::Database)?;
    Ok(())
}

async fn cleanup_failed_bundle_database_artifacts(
    pool: &sqlx::SqlitePool,
    bundle_id: &str,
) -> Result<(), AppError> {
    cleanup_bundle_content_batched(pool, bundle_id, CLEANUP_BATCH_SIZE).await?;
    Ok(())
}

#[derive(sqlx::FromRow)]
struct FileMetaRow {
    id: i64,
    meta: Option<String>,
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
