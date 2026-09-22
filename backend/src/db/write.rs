//! Short, replayable SQLite write transactions. Never do filesystem work here.
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex as StdMutex, Weak},
    time::{Duration, Instant},
};

use futures_util::future::BoxFuture;
use once_cell::sync::Lazy;
use sqlx::{SqliteConnection, SqlitePool};
use tokio::sync::{Mutex, OwnedMutexGuard};

use crate::error::AppError;

static WRITERS: Lazy<StdMutex<HashMap<PathBuf, Weak<Mutex<()>>>>> = Lazy::new(Default::default);
const MAX_ATTEMPTS: u32 = 3;

#[cfg(test)]
pub(crate) fn retryable_fixture_cleanup_error(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::DirectoryNotEmpty
    ) || error.raw_os_error() == Some(32)
}

#[cfg(test)]
pub(crate) async fn remove_fixture_dir(root: PathBuf) {
    for attempt in 0..50 {
        match std::fs::remove_dir_all(&root) {
            Ok(()) => return,
            Err(error) if attempt < 49 && retryable_fixture_cleanup_error(&error) => {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Err(error) => panic!("failed to remove test fixture: {error}"),
        }
    }
}

/// FIFO admission shared by pools pointing at the same database. Acquire before
/// borrowing a connection, and release after each transaction (not each job).
pub async fn acquire(pool: &SqlitePool) -> OwnedMutexGuard<()> {
    let filename = pool
        .connect_options()
        .as_ref()
        .clone()
        .get_filename()
        .into_owned();
    let key = std::fs::canonicalize(&filename).unwrap_or_else(|_| {
        if filename.is_absolute() {
            filename.clone()
        } else {
            std::env::current_dir().unwrap_or_default().join(&filename)
        }
    });
    let writer = {
        let mut writers = WRITERS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        writers.retain(|_, writer| writer.strong_count() > 0);
        if let Some(writer) = writers.get(&key).and_then(Weak::upgrade) {
            writer
        } else {
            let writer = Arc::new(Mutex::new(()));
            writers.insert(key, Arc::downgrade(&writer));
            writer
        }
    };
    writer.lock_owned().await
}

fn busy(error: &AppError) -> bool {
    matches!(error, AppError::Database(sqlx::Error::Database(error))
        if error.code().and_then(|code| code.parse::<i32>().ok()).is_some_and(|code| code & 0xff == 5))
}

/// `operation` must only use the supplied connection and input. It can be
/// replayed after rollback; external side effects and nested `run` are forbidden.
/// SQLITE_BUSY (including BUSY_SNAPSHOT) is retried, not arbitrary SQL errors.
pub async fn run<I: Sync + ?Sized, T, F>(
    pool: &SqlitePool,
    operation: &str,
    input: &I,
    mut execute: F,
) -> Result<T, AppError>
where
    F: for<'c> FnMut(&'c mut SqliteConnection, &'c I) -> BoxFuture<'c, Result<T, AppError>>,
{
    for attempt in 1..=MAX_ATTEMPTS {
        let queued = Instant::now();
        let permit = acquire(pool).await;
        let queue_ms = queued.elapsed().as_millis() as u64;
        let started = Instant::now();
        let result = match pool.begin().await {
            Ok(mut tx) => match execute(&mut tx, input).await {
                Ok(value) => tx
                    .commit()
                    .await
                    .map(|()| value)
                    .map_err(AppError::Database),
                Err(error) => {
                    // Do not replay if rollback itself fails.
                    tx.rollback().await.map_err(AppError::Database)?;
                    Err(error)
                }
            },
            Err(error) => Err(AppError::Database(error)),
        };
        drop(permit);
        let elapsed_ms = started.elapsed().as_millis() as u64;
        match result {
            Ok(value) => {
                if elapsed_ms >= 1_000 || queue_ms >= 1_000 {
                    tracing::warn!(
                        operation,
                        attempt,
                        queue_ms,
                        elapsed_ms,
                        "slow SQLite write transaction completed"
                    );
                } else {
                    tracing::debug!(
                        operation,
                        attempt,
                        queue_ms,
                        elapsed_ms,
                        "SQLite write transaction completed"
                    );
                }
                return Ok(value);
            }
            Err(error) => {
                let retry = attempt < MAX_ATTEMPTS && busy(&error);
                tracing::warn!(operation, attempt, queue_ms, elapsed_ms, retry, %error, "SQLite write transaction failed");
                if !retry {
                    return Err(error);
                }
                // Release admission between attempts so another writer can progress.
                let delay = 100 * (1_u64 << (attempt - 1)) + u64::from(rand::random::<u8>() % 100);
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
        }
    }
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use std::sync::atomic::{AtomicUsize, Ordering};

    async fn fixture() -> (PathBuf, SqlitePool, SqlitePool) {
        let root = std::env::temp_dir().join(format!("rain-write-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let options = SqliteConnectOptions::new()
            .filename(root.join("test.db"))
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_millis(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(2)
            .connect_with(options.clone())
            .await
            .unwrap();
        sqlx::query("CREATE TABLE counter (value INTEGER NOT NULL)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO counter VALUES (0)")
            .execute(&pool)
            .await
            .unwrap();
        let external = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        (root, pool, external)
    }

    async fn cleanup_fixture(root: PathBuf, pool: SqlitePool, external: SqlitePool) {
        pool.close().await;
        external.close().await;
        drop(pool);
        drop(external);
        remove_fixture_dir(root).await;
    }

    #[test]
    fn windows_sharing_violations_are_retryable_fixture_cleanup_errors() {
        let error = std::io::Error::from_raw_os_error(32);
        assert!(super::retryable_fixture_cleanup_error(&error));
    }

    #[tokio::test]
    async fn busy_replays_whole_transaction_without_duplicate_writes() {
        let (root, pool, external) = fixture().await;
        // Obtain a real SQLITE_BUSY from a separate connection, then inject it
        // after a partial write to verify rollback before replay.
        let mut tx = external.begin().await.unwrap();
        sqlx::query("UPDATE counter SET value=value+1")
            .execute(&mut *tx)
            .await
            .unwrap();
        let mut blocked = pool.begin().await.unwrap();
        let error = sqlx::query("UPDATE counter SET value=value+1")
            .execute(&mut *blocked)
            .await
            .unwrap_err();
        blocked.rollback().await.unwrap();
        tx.rollback().await.unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT value FROM counter")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0,
            "baseline after lock error"
        );
        let injected = StdMutex::new(Some(AppError::Database(error)));
        let attempts = AtomicUsize::new(0);
        run(
            &pool,
            "test-replay",
            &(&injected, &attempts),
            |conn, &(injected, attempts)| {
                Box::pin(async move {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    assert_eq!(
                        sqlx::query_scalar::<_, i64>("SELECT value FROM counter")
                            .fetch_one(&mut *conn)
                            .await?,
                        0,
                        "rollback before replay"
                    );
                    sqlx::query("UPDATE counter SET value=value+1")
                        .execute(conn)
                        .await?;
                    if let Some(error) = injected.lock().unwrap().take() {
                        return Err(error);
                    }
                    Ok(())
                })
            },
        )
        .await
        .unwrap();
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT value FROM counter")
                .fetch_one(&pool)
                .await
                .unwrap(),
            1
        );
        cleanup_fixture(root, pool, external).await;
    }

    #[tokio::test]
    async fn concurrent_pools_share_admission_without_borrowing_connections() {
        let (root, pool, other) = fixture().await;
        let permit = acquire(&pool).await;
        let mut waiting = Box::pin(acquire(&other));
        assert!(
            tokio::time::timeout(Duration::from_millis(20), &mut waiting)
                .await
                .is_err()
        );
        // Writer waiters must not consume the other pool's only connection.
        tokio::time::timeout(
            Duration::from_secs(1),
            sqlx::query("SELECT 1").execute(&other),
        )
        .await
        .unwrap()
        .unwrap();
        drop(permit);
        drop(
            tokio::time::timeout(Duration::from_secs(1), waiting)
                .await
                .unwrap(),
        );
        let mut tasks = Vec::new();
        for index in 0..20 {
            let pool = if index % 2 == 0 {
                pool.clone()
            } else {
                other.clone()
            };
            tasks.push(tokio::spawn(async move {
                run(&pool, "increment", &(), |conn, _| {
                    Box::pin(async move {
                        sqlx::query("UPDATE counter SET value=value+1")
                            .execute(conn)
                            .await?;
                        Ok(())
                    })
                })
                .await
                .unwrap();
            }));
        }
        for task in tasks {
            task.await.unwrap();
        }
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT value FROM counter")
                .fetch_one(&pool)
                .await
                .unwrap(),
            20
        );
        cleanup_fixture(root, pool, other).await;
    }

    #[tokio::test]
    async fn constraint_failure_rolls_back_without_retry() {
        let (root, pool, external) = fixture().await;
        let attempts = AtomicUsize::new(0);
        let result = run(&pool, "constraint", &attempts, |conn, attempts| {
            Box::pin(async move {
                attempts.fetch_add(1, Ordering::SeqCst);
                sqlx::query("UPDATE counter SET value=value+1")
                    .execute(&mut *conn)
                    .await?;
                sqlx::query("INSERT INTO counter VALUES (NULL)")
                    .execute(conn)
                    .await?;
                Ok(())
            })
        })
        .await;
        assert!(result.is_err());
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT value FROM counter")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
        cleanup_fixture(root, pool, external).await;
    }

    #[tokio::test]
    async fn persistent_external_lock_exhausts_bounded_retries_and_releases_admission() {
        let (root, pool, external) = fixture().await;
        let mut blocker = external.begin().await.unwrap();
        sqlx::query("UPDATE counter SET value=value+1")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let attempts = AtomicUsize::new(0);
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            run(&pool, "external-lock", &attempts, |conn, attempts| {
                Box::pin(async move {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    sqlx::query("UPDATE counter SET value=value+1")
                        .execute(conn)
                        .await?;
                    Ok(())
                })
            }),
        )
        .await
        .unwrap();
        assert!(busy(&result.unwrap_err()));
        assert_eq!(attempts.load(Ordering::SeqCst), MAX_ATTEMPTS as usize);
        blocker.rollback().await.unwrap();
        run(&pool, "after-exhaustion", &(), |conn, _| {
            Box::pin(async move {
                sqlx::query("UPDATE counter SET value=value+1")
                    .execute(conn)
                    .await?;
                Ok(())
            })
        })
        .await
        .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT value FROM counter")
                .fetch_one(&pool)
                .await
                .unwrap(),
            1
        );
        cleanup_fixture(root, pool, external).await;
    }
}
