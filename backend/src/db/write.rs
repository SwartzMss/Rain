//! Short, replayable SQLite write transactions. Never do filesystem work here.
use std::{
    collections::HashMap,
    ops::{Deref, DerefMut},
    path::PathBuf,
    sync::{Arc, Mutex as StdMutex, Weak},
    time::{Duration, Instant},
};

use futures_util::future::BoxFuture;
use once_cell::sync::Lazy;
use sqlx::{SqliteConnection, SqlitePool, pool::PoolConnection};
use tokio::sync::{Mutex, OwnedMutexGuard};

use crate::{error::AppError, ingest::metrics::micros};

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
    execute: F,
) -> Result<T, AppError>
where
    F: for<'c> FnMut(&'c mut SqliteConnection, &'c I) -> BoxFuture<'c, Result<T, AppError>>,
{
    execute_with_retry(pool, operation, input, execute).await
}

/// Starts a transaction that reserves SQLite's writer slot before any reads.
/// The caller owns admission and must commit or roll back before dropping it.
pub struct ImmediateConnection {
    connection: Option<PoolConnection<sqlx::Sqlite>>,
    transaction_active: bool,
}

impl ImmediateConnection {
    fn new(connection: PoolConnection<sqlx::Sqlite>) -> Self {
        Self {
            connection: Some(connection),
            transaction_active: true,
        }
    }

    pub async fn commit(&mut self) -> Result<(), sqlx::Error> {
        let result = sqlx::query("COMMIT").execute(&mut **self).await;
        if result.is_ok() {
            self.transaction_active = false;
        }
        result.map(|_| ())
    }

    pub async fn rollback(&mut self) -> Result<(), sqlx::Error> {
        let result = sqlx::query("ROLLBACK").execute(&mut **self).await;
        if result.is_ok() {
            self.transaction_active = false;
        }
        result.map(|_| ())
    }
}

impl Deref for ImmediateConnection {
    type Target = SqliteConnection;

    fn deref(&self) -> &Self::Target {
        self.connection
            .as_deref()
            .expect("immediate transaction connection is present")
    }
}

impl DerefMut for ImmediateConnection {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.connection
            .as_deref_mut()
            .expect("immediate transaction connection is present")
    }
}

impl Drop for ImmediateConnection {
    fn drop(&mut self) {
        if self.transaction_active {
            // PoolConnection::Drop returns a live connection to the pool. A raw
            // transaction cannot be rolled back asynchronously from Drop, so
            // detach and drop the underlying connection instead of poisoning
            // the pool if this future is cancelled or cleanup failed.
            if let Some(connection) = self.connection.take() {
                drop(connection.detach());
            }
        }
    }
}

pub async fn begin_immediate(pool: &SqlitePool) -> Result<ImmediateConnection, AppError> {
    let mut connection =
        ImmediateConnection::new(pool.acquire().await.map_err(AppError::Database)?);
    if let Err(error) = sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut *connection)
        .await
    {
        // SQLite may leave a connection in a transaction after a busy
        // response from BEGIN IMMEDIATE. Clear that state before the pooled
        // connection can be reused by a bounded retry.
        if let Err(rollback_error) = connection.rollback().await {
            tracing::warn!(%rollback_error, "failed to clear SQLite transaction after BEGIN IMMEDIATE failed");
        }
        return Err(AppError::Database(error));
    }
    Ok(connection)
}

/// Executes a replayable multi-statement write transaction through the same
/// admission, transaction policy, telemetry, and retry path as [`run`].
pub async fn transaction<T, F>(
    pool: &SqlitePool,
    operation: &str,
    mut execute: F,
) -> Result<T, AppError>
where
    F: for<'c> FnMut(&'c mut SqliteConnection) -> BoxFuture<'c, Result<T, AppError>>,
{
    let unit = ();
    execute_with_retry(pool, operation, &unit, |connection, _| execute(connection)).await
}

async fn execute_with_retry<I: Sync + ?Sized, T, F>(
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
        let queue_elapsed = queued.elapsed();
        let queue_ms = queue_elapsed.as_millis() as u64;
        let queue_us = micros(queue_elapsed);
        let started = Instant::now();
        let begin_result = begin_immediate(pool).await;
        let begin_us = micros(started.elapsed());
        let mut execute_us = 0;
        let mut finish_us = 0;
        let mut rollback_failed = false;
        let mut commit_attempted = false;
        let result = match begin_result {
            Ok(mut connection) => {
                let execute_started = Instant::now();
                let executed = execute(&mut connection, input).await;
                execute_us = micros(execute_started.elapsed());
                let finish_started = Instant::now();
                let result = match executed {
                    Ok(value) => {
                        commit_attempted = true;
                        match connection.commit().await {
                            Ok(_) => Ok(value),
                            Err(commit_error) => {
                                if let Err(rollback_error) = connection.rollback().await {
                                    rollback_failed = true;
                                    tracing::warn!(
                                        %rollback_error,
                                        "failed to roll back SQLite transaction after COMMIT failed"
                                    );
                                }
                                Err(AppError::Database(commit_error))
                            }
                        }
                    }
                    Err(error) => match connection.rollback().await {
                        Ok(_) => Err(error),
                        Err(rollback_error) => {
                            rollback_failed = true;
                            Err(AppError::Database(rollback_error))
                        }
                    },
                };
                finish_us = micros(finish_started.elapsed());
                result
            }
            Err(error) => Err(error),
        };
        drop(permit);
        let elapsed_ms = started.elapsed().as_millis() as u64;
        match result {
            Ok(value) => {
                if elapsed_ms >= 1_000 || queue_ms >= 1_000 {
                    tracing::warn!(
                        metric = "sqlite_write",
                        operation,
                        attempt,
                        queue_us,
                        begin_us,
                        execute_us,
                        finish_us,
                        queue_ms,
                        elapsed_ms,
                        "slow SQLite write transaction completed"
                    );
                } else {
                    tracing::debug!(
                        metric = "sqlite_write",
                        operation,
                        attempt,
                        queue_us,
                        begin_us,
                        execute_us,
                        finish_us,
                        queue_ms,
                        elapsed_ms,
                        "SQLite write transaction completed"
                    );
                }
                return Ok(value);
            }
            Err(error) => {
                // Once COMMIT has been attempted, the transaction outcome is
                // uncertain. Never replay the logical write, even if SQLite
                // reports SQLITE_BUSY for the COMMIT itself.
                let retry =
                    !commit_attempted && !rollback_failed && attempt < MAX_ATTEMPTS && busy(&error);
                tracing::warn!(metric = "sqlite_write", operation, attempt, queue_us, begin_us, execute_us, finish_us, queue_ms, elapsed_ms, retry, %error, "SQLite write transaction failed");
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
    use sqlx::{
        SqlitePool,
        sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Notify;

    async fn fixture() -> (PathBuf, SqlitePool, SqlitePool) {
        fixture_with_max_connections(2).await
    }

    async fn fixture_with_max_connections(
        max_connections: u32,
    ) -> (PathBuf, SqlitePool, SqlitePool) {
        let root = std::env::temp_dir().join(format!("rain-write-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let options = SqliteConnectOptions::new()
            .filename(root.join("test.db"))
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_millis(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(max_connections)
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
    async fn cancelled_writer_drops_active_connection_without_poisoning_pool() {
        let (root, pool, external) = fixture_with_max_connections(1).await;
        let entered = Arc::new(Notify::new());
        let entered_for_run = entered.clone();
        let run_pool = pool.clone();
        let task = tokio::spawn(async move {
            run(&run_pool, "cancelled-writer", &(), move |_, _| {
                let entered = entered_for_run.clone();
                Box::pin(async move {
                    entered.notify_one();
                    std::future::pending::<()>().await;
                    Ok(())
                })
            })
            .await
        });
        entered.notified().await;
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());

        tokio::time::timeout(
            Duration::from_secs(1),
            run(&pool, "after-cancel", &(), |conn, _| {
                Box::pin(async move {
                    sqlx::query("UPDATE counter SET value=value+1")
                        .execute(conn)
                        .await?;
                    Ok(())
                })
            }),
        )
        .await
        .unwrap()
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
    async fn run_uses_begin_immediate_before_closure_reads() {
        let (root, pool, external) = fixture().await;
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let attempts = Arc::new(AtomicUsize::new(0));
        let entered_for_run = entered.clone();
        let release_for_run = release.clone();
        let attempts_for_run = attempts.clone();
        let run_pool = pool.clone();
        let task = tokio::spawn(async move {
            run(&run_pool, "immediate-test", &(), move |conn, _| {
                let entered = entered_for_run.clone();
                let release = release_for_run.clone();
                let attempts = attempts_for_run.clone();
                Box::pin(async move {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    sqlx::query_scalar::<_, i64>("SELECT value FROM counter")
                        .fetch_one(&mut *conn)
                        .await?;
                    entered.notify_one();
                    release.notified().await;
                    sqlx::query("UPDATE counter SET value=value+1")
                        .execute(conn)
                        .await?;
                    Ok(())
                })
            })
            .await
        });
        entered.notified().await;
        let external_error = sqlx::query("UPDATE counter SET value=value+1")
            .execute(&external)
            .await
            .expect_err("BEGIN IMMEDIATE must reserve the writer before the SELECT");
        assert!(matches!(external_error, sqlx::Error::Database(_)));
        let _ = sqlx::query("ROLLBACK").execute(&external).await;
        release.notify_one();
        task.await.unwrap().unwrap();
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT value FROM counter")
                .fetch_one(&external)
                .await
                .unwrap(),
            1
        );
        cleanup_fixture(root, pool, external).await;
    }

    #[tokio::test]
    async fn private_memory_pools_do_not_share_admission() {
        let first = SqlitePool::connect_lazy("sqlite::memory:").unwrap();
        let first_clone = first.clone();
        let second = SqlitePool::connect_lazy("sqlite::memory:").unwrap();
        let permit = acquire(&first).await;

        let mut same_pool = Box::pin(acquire(&first_clone));
        assert!(
            tokio::time::timeout(Duration::from_millis(20), &mut same_pool)
                .await
                .is_err()
        );
        tokio::time::timeout(Duration::from_secs(1), acquire(&second))
            .await
            .unwrap();
        drop(permit);
        drop(
            tokio::time::timeout(Duration::from_secs(1), same_pool)
                .await
                .unwrap(),
        );
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
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            0,
            "BEGIN IMMEDIATE rejects the persistent lock before the closure runs"
        );
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
