use std::{
    path::Path,
    str::FromStr,
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, Instant},
};

use once_cell::sync::Lazy;

use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};
use tokio::sync::{Semaphore, SemaphorePermit};

use crate::error::AppError;

mod migrations;
pub mod write;

pub const CLEANUP_BATCH_SIZE: u64 = 100;
const LARGE_CLEANUP_CHECKPOINT_ROWS: u64 = 10_000;
static HEAVY_CLEANUP_WRITER: Lazy<Semaphore> = Lazy::new(|| Semaphore::new(1));
static QUEUED_CLEANUPS: AtomicUsize = AtomicUsize::new(0);
static ACTIVE_CLEANUPS: AtomicUsize = AtomicUsize::new(0);

pub async fn capture_recovery_cutoff(pool: &SqlitePool) -> Result<String, AppError> {
    sqlx::query_scalar("SELECT CURRENT_TIMESTAMP")
        .fetch_one(pool)
        .await
        .map_err(AppError::Database)
}

struct CleanupQueueGuard {
    queued: bool,
}

impl CleanupQueueGuard {
    fn enter() -> (Self, usize) {
        let queue_depth = QUEUED_CLEANUPS.fetch_add(1, Ordering::AcqRel) + 1;
        (Self { queued: true }, queue_depth)
    }

    fn leave(&mut self) -> usize {
        self.queued = false;
        QUEUED_CLEANUPS.fetch_sub(1, Ordering::AcqRel) - 1
    }
}

impl Drop for CleanupQueueGuard {
    fn drop(&mut self) {
        if self.queued {
            QUEUED_CLEANUPS.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

struct HeavyCleanupPermit {
    _permit: SemaphorePermit<'static>,
    bundle_id: String,
    started: Instant,
}

impl Drop for HeavyCleanupPermit {
    fn drop(&mut self) {
        let active_cleanup_count = ACTIVE_CLEANUPS.fetch_sub(1, Ordering::AcqRel) - 1;
        tracing::info!(
            bundle_id = %self.bundle_id,
            active_cleanup_count,
            queue_depth = QUEUED_CLEANUPS.load(Ordering::Acquire),
            total_elapsed_ms = self.started.elapsed().as_millis() as u64,
            "bundle cleanup writer released"
        );
    }
}

async fn acquire_heavy_cleanup_writer(
    bundle_id: &str,
    inactive_lease: Option<(&SqlitePool, InactiveCleanupLease<'_>)>,
) -> Result<HeavyCleanupPermit, AppError> {
    let wait_started = Instant::now();
    let (mut queue_guard, queue_depth) = CleanupQueueGuard::enter();
    tracing::info!(bundle_id, queue_depth, "bundle cleanup queued");
    let acquire = HEAVY_CLEANUP_WRITER.acquire();
    tokio::pin!(acquire);
    let permit = if let Some((pool, lease)) = inactive_lease {
        require_inactive_issue_lease(pool, lease.issue_code, lease.token, lease.seconds).await?;
        let renew_interval_ms = lease
            .seconds
            .saturating_mul(1_000)
            .saturating_div(3)
            .clamp(50, 60_000);
        let mut renew_interval = tokio::time::interval(Duration::from_millis(renew_interval_ms));
        renew_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // Tokio intervals tick immediately once; the lease was renewed just above.
        renew_interval.tick().await;
        loop {
            tokio::select! {
                permit = &mut acquire => {
                    break permit.map_err(|_| AppError::Config("bundle cleanup coordinator is closed".into()))?;
                }
                _ = renew_interval.tick() => {
                    require_inactive_issue_lease(pool, lease.issue_code, lease.token, lease.seconds).await?;
                    tracing::debug!(
                        bundle_id,
                        issue_code = lease.issue_code,
                        queue_depth = QUEUED_CLEANUPS.load(Ordering::Acquire),
                        "bundle cleanup lease renewed while waiting for writer"
                    );
                }
            }
        }
    } else {
        acquire
            .await
            .map_err(|_| AppError::Config("bundle cleanup coordinator is closed".into()))?
    };
    let queue_depth = queue_guard.leave();
    let active_cleanup_count = ACTIVE_CLEANUPS.fetch_add(1, Ordering::AcqRel) + 1;
    tracing::info!(
        bundle_id,
        queue_depth,
        active_cleanup_count,
        queue_wait_ms = wait_started.elapsed().as_millis() as u64,
        "bundle cleanup writer acquired"
    );
    Ok(HeavyCleanupPermit {
        _permit: permit,
        bundle_id: bundle_id.to_owned(),
        started: Instant::now(),
    })
}

#[derive(Debug, Clone, Copy)]
pub struct WalCheckpointStats {
    pub busy: i64,
    pub log_pages: i64,
    pub checkpointed_pages: i64,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct CleanupPhaseStats {
    pub rows: u64,
    pub batches: u64,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct BundleCleanupStats {
    pub line_offsets: CleanupPhaseStats,
    pub fts_segments: CleanupPhaseStats,
    pub segments: CleanupPhaseStats,
    pub files: CleanupPhaseStats,
}

#[derive(Clone, Copy)]
struct InactiveCleanupLease<'a> {
    issue_code: &'a str,
    token: &'a str,
    seconds: u64,
}

impl BundleCleanupStats {
    pub fn total_rows(self) -> u64 {
        self.line_offsets.rows + self.fts_segments.rows + self.segments.rows + self.files.rows
    }
}

pub fn init_pool(database_url: &str) -> Result<SqlitePool, AppError> {
    ensure_sqlite_parent(database_url)?;

    let options = SqliteConnectOptions::from_str(database_url)
        .map_err(AppError::Database)?
        .create_if_missing(true)
        .foreign_keys(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(Duration::from_secs(30));

    // SQLx enables SQLite's shared in-memory cache for `sqlite::memory:`.
    // That makes independently-created test pools share one database, so
    // parallel tests can observe and overwrite each other's rows. Keep an
    // in-memory pool single-connection and private while leaving file-backed
    // production pools unchanged.
    let in_memory = options.clone().get_filename() == Path::new(":memory:");
    let options = if in_memory {
        options.shared_cache(false)
    } else {
        options
    };

    let pool_options = SqlitePoolOptions::new().max_connections(if in_memory { 1 } else { 5 });

    Ok(pool_options.connect_lazy_with(options))
}

pub async fn prepare_schema(pool: &SqlitePool, reset: bool) -> Result<(), AppError> {
    migrations::prepare(pool, reset).await
}

pub async fn checkpoint_wal(pool: &SqlitePool) -> Result<WalCheckpointStats, AppError> {
    let (busy, log_pages, checkpointed_pages): (i64, i64, i64) =
        sqlx::query_as("PRAGMA wal_checkpoint(PASSIVE)")
            .fetch_one(pool)
            .await
            .map_err(AppError::Database)?;
    Ok(WalCheckpointStats {
        busy,
        log_pages,
        checkpointed_pages,
    })
}

pub async fn cleanup_bundle_content_batched(
    pool: &SqlitePool,
    bundle_id: &str,
    batch_size: u64,
) -> Result<BundleCleanupStats, AppError> {
    let _cleanup_permit = acquire_heavy_cleanup_writer(bundle_id, None).await?;
    cleanup_bundle_content_batched_inner(pool, bundle_id, batch_size, None).await
}

async fn cleanup_bundle_content_batched_inner(
    pool: &SqlitePool,
    bundle_id: &str,
    batch_size: u64,
    lease: Option<InactiveCleanupLease<'_>>,
) -> Result<BundleCleanupStats, AppError> {
    if batch_size == 0 {
        return Err(AppError::Config(
            "cleanup batch size must be positive".into(),
        ));
    }

    let stats = BundleCleanupStats {
        line_offsets: delete_bundle_rows_in_batches(
            pool,
            bundle_id,
            batch_size,
            "log_line_offsets",
            "DELETE FROM log_line_offsets WHERE rowid IN (SELECT rowid FROM log_line_offsets WHERE file_id IN (SELECT id FROM files WHERE bundle_id = ?) LIMIT ?)",
            lease,
        )
        .await?,
        // The external-content FTS index is maintained by log_segments triggers.
        fts_segments: CleanupPhaseStats::default(),
        segments: delete_bundle_rows_in_batches(
            pool,
            bundle_id,
            batch_size,
            "log_segments",
            "DELETE FROM log_segments WHERE rowid IN (SELECT rowid FROM log_segments WHERE bundle_id = ? LIMIT ?)",
            lease,
        )
        .await?,
        files: delete_bundle_rows_in_batches(
            pool,
            bundle_id,
            batch_size,
            "files",
            "DELETE FROM files WHERE rowid IN (SELECT f.rowid FROM files f WHERE f.bundle_id = ? AND NOT EXISTS (SELECT 1 FROM files child WHERE child.parent_id = f.id) AND NOT EXISTS (SELECT 1 FROM log_line_offsets offsets WHERE offsets.file_id = f.id) AND NOT EXISTS (SELECT 1 FROM log_segments segments WHERE segments.file_id = f.id) LIMIT ?)",
            lease,
        )
        .await?,
    };

    if let Some(lease) = lease {
        require_inactive_issue_lease(pool, lease.issue_code, lease.token, lease.seconds).await?;
    }

    if stats.total_rows() >= LARGE_CLEANUP_CHECKPOINT_ROWS {
        let started = std::time::Instant::now();
        match checkpoint_wal(pool).await {
            Ok(checkpoint) => tracing::info!(
                bundle_id,
                checkpoint_mode = "PASSIVE",
                busy = checkpoint.busy,
                log_pages = checkpoint.log_pages,
                checkpointed_pages = checkpoint.checkpointed_pages,
                elapsed_ms = started.elapsed().as_millis() as u64,
                "large bundle cleanup WAL checkpoint completed"
            ),
            Err(error) => tracing::warn!(
                bundle_id,
                checkpoint_mode = "PASSIVE",
                error = %error,
                elapsed_ms = started.elapsed().as_millis() as u64,
                "large bundle cleanup WAL checkpoint failed"
            ),
        }
    }

    if let Some(lease) = lease {
        require_inactive_issue_lease(pool, lease.issue_code, lease.token, lease.seconds).await?;
    }

    Ok(stats)
}

async fn delete_bundle_rows_in_batches(
    pool: &SqlitePool,
    bundle_id: &str,
    batch_size: u64,
    phase: &'static str,
    statement: &'static str,
    lease: Option<InactiveCleanupLease<'_>>,
) -> Result<CleanupPhaseStats, AppError> {
    let started = std::time::Instant::now();
    let mut stats = CleanupPhaseStats::default();
    loop {
        if let Some(lease) = lease {
            require_inactive_issue_lease(pool, lease.issue_code, lease.token, lease.seconds)
                .await?;
        }
        let batch_started = Instant::now();
        let affected = write::run(
            pool,
            phase,
            &(statement, bundle_id, batch_size),
            |conn, &(statement, bundle_id, batch_size)| {
                Box::pin(async move {
                    Ok(sqlx::query(statement)
                        .bind(bundle_id)
                        .bind(batch_size as i64)
                        .execute(conn)
                        .await
                        .map_err(AppError::Database)?
                        .rows_affected())
                })
            },
        )
        .await?;
        if affected == 0 {
            break;
        }
        stats.rows += affected;
        stats.batches += 1;
        tracing::debug!(
            bundle_id,
            phase,
            batch = stats.batches,
            batch_rows = affected,
            batch_elapsed_ms = batch_started.elapsed().as_millis() as u64,
            "bundle cleanup batch completed"
        );
        // Leave a small scheduling window between write transactions so foreground
        // requests can acquire SQLite's writer lock before cleanup takes it again.
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    tracing::info!(
        bundle_id,
        phase,
        rows = stats.rows,
        batches = stats.batches,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "bundle cleanup phase completed"
    );
    Ok(stats)
}

pub async fn finish_bundle_deletion(pool: &SqlitePool, bundle_id: &str) -> Result<(), AppError> {
    let _cleanup_permit = acquire_heavy_cleanup_writer(bundle_id, None).await?;
    cleanup_bundle_content_batched_inner(pool, bundle_id, CLEANUP_BATCH_SIZE, None).await?;
    write::run(
        pool,
        "finalize bundle deletion",
        &bundle_id,
        |conn, bundle_id| {
            Box::pin(async move {
                sqlx::query("UPDATE bundles SET status = 'DELETED', content_size_bytes = 0 WHERE id = ? AND status = 'DELETING'")
                    .bind(bundle_id)
                    .execute(conn)
                    .await
                    .map_err(AppError::Database)?;
                Ok(())
            })
        },
    )
    .await?;
    Ok(())
}

pub async fn finish_bundle_deletion_with_inactive_lease(
    pool: &SqlitePool,
    bundle_id: &str,
    issue_code: &str,
    lease_token: &str,
    lease_seconds: u64,
) -> Result<(), AppError> {
    let lease = InactiveCleanupLease {
        issue_code,
        token: lease_token,
        seconds: lease_seconds,
    };
    let _cleanup_permit = acquire_heavy_cleanup_writer(bundle_id, Some((pool, lease))).await?;
    cleanup_bundle_content_batched_inner(pool, bundle_id, CLEANUP_BATCH_SIZE, Some(lease)).await?;
    let input = (bundle_id, issue_code, lease_token, lease_seconds);
    write::run(
        pool,
        "finalize leased bundle deletion",
        &input,
        |conn, &(bundle_id, issue_code, lease_token, lease_seconds)| {
            Box::pin(async move {
                let modifier = format!("+{lease_seconds} seconds");
                let renewed = sqlx::query("UPDATE issues SET deletion_lease_until=datetime('now', ?) WHERE code=? AND status='DELETING' AND deletion_reason IN ('INACTIVE', 'MANUAL') AND deletion_lease_token=? AND datetime(deletion_lease_until) > datetime('now')")
                    .bind(modifier)
                    .bind(issue_code)
                    .bind(lease_token)
                    .execute(&mut *conn)
                    .await
                    .map_err(AppError::Database)?
                    .rows_affected();
                if renewed != 1 {
                    return Err(AppError::Conflict(format!("inactive cleanup lease for issue {issue_code} was lost")));
                }
                sqlx::query("UPDATE bundles SET status = 'DELETED', content_size_bytes = 0 WHERE id = ? AND status = 'DELETING'")
                    .bind(bundle_id)
                    .execute(&mut *conn)
                    .await
                    .map_err(AppError::Database)?;
                Ok(())
            })
        },
    )
    .await?;
    Ok(())
}

pub async fn renew_inactive_issue_lease(
    pool: &SqlitePool,
    issue_code: &str,
    lease_token: &str,
    lease_seconds: u64,
) -> Result<bool, AppError> {
    if lease_seconds == 0 {
        return Err(AppError::Config(
            "inactive cleanup lease must be positive".into(),
        ));
    }
    let input = (issue_code, lease_token, lease_seconds);
    write::run(
        pool,
        "renew inactive cleanup lease",
        &input,
        |conn, &(issue_code, lease_token, lease_seconds)| {
            Box::pin(async move {
                let modifier = format!("+{lease_seconds} seconds");
                Ok(sqlx::query("UPDATE issues SET deletion_lease_until=datetime('now', ?) WHERE code=? AND status='DELETING' AND deletion_reason IN ('INACTIVE', 'MANUAL') AND deletion_lease_token=? AND datetime(deletion_lease_until) > datetime('now')")
                    .bind(modifier)
                    .bind(issue_code)
                    .bind(lease_token)
                    .execute(conn)
                    .await
                    .map_err(AppError::Database)?
                    .rows_affected() == 1)
            })
        },
    )
    .await
}

async fn require_inactive_issue_lease(
    pool: &SqlitePool,
    issue_code: &str,
    lease_token: &str,
    lease_seconds: u64,
) -> Result<(), AppError> {
    if renew_inactive_issue_lease(pool, issue_code, lease_token, lease_seconds).await? {
        Ok(())
    } else {
        Err(AppError::Conflict(format!(
            "inactive cleanup lease for issue {issue_code} was lost"
        )))
    }
}

pub async fn resume_deleting_bundles(pool: &SqlitePool) -> Result<u64, AppError> {
    let bundle_ids: Vec<String> = sqlx::query_scalar(
        "SELECT bundles.id FROM bundles JOIN issues ON issues.code=bundles.issue_code WHERE bundles.status='DELETING' AND NOT (issues.status='DELETING' AND issues.deletion_reason IN ('INACTIVE', 'MANUAL'))",
    )
            .fetch_all(pool)
            .await
            .map_err(AppError::Database)?;
    for bundle_id in &bundle_ids {
        if let Err(error) = finish_bundle_deletion(pool, bundle_id).await {
            tracing::warn!(bundle_id, %error, "deleting bundle recovery failed; will retry later");
        }
    }
    Ok(bundle_ids.len() as u64)
}

pub async fn fail_stale_processing_bundles(pool: &SqlitePool) -> Result<u64, AppError> {
    let result = write::run(pool, "fail stale processing bundles", &(), |conn, _| {
        Box::pin(async move {
            sqlx::query(
                "UPDATE bundle_search_indexes SET state = 'FAILED', last_error_code = 'PROCESS_INTERRUPTED', updated_at = CURRENT_TIMESTAMP WHERE state = 'BUILDING' AND bundle_id IN (SELECT id FROM bundles WHERE status IN ('PENDING', 'PROCESSING'))",
            )
            .execute(&mut *conn)
            .await
            .map_err(AppError::Database)?;
            Ok(sqlx::query(
                r#"
        UPDATE bundles
        SET failure_stage = process_stage,
            failure_code = 'PROCESS_INTERRUPTED',
            retryable = 1,
            status = 'FAILED',
            failure_reason = '服务重启时检测到未完成的上传，请删除后重试'
        WHERE status IN ('PENDING', 'PROCESSING')
        "#,
            )
            .execute(conn)
            .await
            .map_err(AppError::Database)?
            .rows_affected())
        })
    })
    .await?;

    Ok(result)
}

pub async fn fail_stale_processing_bundles_before(
    pool: &SqlitePool,
    created_before: &str,
) -> Result<u64, AppError> {
    let result = write::run(
        pool,
        "fail stale processing bundles before cutoff",
        &created_before,
        |conn, created_before| {
            Box::pin(async move {
                sqlx::query(
                    "UPDATE bundle_search_indexes SET state = 'FAILED', last_error_code = 'PROCESS_INTERRUPTED', updated_at = CURRENT_TIMESTAMP WHERE state = 'BUILDING' AND bundle_id IN (SELECT id FROM bundles WHERE status IN ('PENDING', 'PROCESSING') AND datetime(created_at) <= datetime(?))",
                )
                .bind(created_before)
                .execute(&mut *conn)
                .await
                .map_err(AppError::Database)?;
                Ok(sqlx::query(
                    r#"
        UPDATE bundles
        SET failure_stage = process_stage,
            failure_code = 'PROCESS_INTERRUPTED',
            retryable = 1,
            status = 'FAILED',
            failure_reason = '服务重启时检测到未完成的上传，请删除后重试'
        WHERE status IN ('PENDING', 'PROCESSING')
          AND datetime(created_at) <= datetime(?)
        "#,
                )
                .bind(created_before)
                .execute(conn)
                .await
                .map_err(AppError::Database)?
                .rows_affected())
            })
        },
    )
    .await?;

    Ok(result)
}

fn ensure_sqlite_parent(database_url: &str) -> Result<(), AppError> {
    let Some(path) = database_url.strip_prefix("sqlite://") else {
        return Ok(());
    };
    if path == ":memory:" {
        return Ok(());
    }
    if let Some(parent) = Path::new(path).parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(AppError::Io)?;
    }
    Ok(())
}

pub async fn load_or_initialize_registration_setting(
    pool: &SqlitePool,
    default_value: bool,
) -> Result<bool, AppError> {
    let (value, _, _) = load_or_initialize_auth_settings(pool, default_value, 20, 10).await?;
    Ok(value != 0)
}

pub async fn load_or_initialize_rate_limits(
    pool: &SqlitePool,
    ip: usize,
    username: usize,
) -> Result<(usize, usize), AppError> {
    let (_, ip, username) = load_or_initialize_auth_settings(pool, true, ip, username).await?;
    Ok((ip, username))
}

pub async fn load_or_initialize_auth_settings(
    pool: &SqlitePool,
    allow_registration: bool,
    ip: usize,
    username: usize,
) -> Result<(i64, usize, usize), AppError> {
    let (registration, ip, username, _) =
        load_or_initialize_system_settings(pool, allow_registration, ip, username, 0).await?;
    Ok((registration, ip, username))
}

pub async fn load_or_initialize_system_settings(
    pool: &SqlitePool,
    allow_registration: bool,
    ip: usize,
    username: usize,
    issue_inactive_days: usize,
) -> Result<(i64, usize, usize, usize), AppError> {
    let ip = i64::try_from(ip).map_err(|_| AppError::Config("IP 限流阈值过大".into()))?;
    let username =
        i64::try_from(username).map_err(|_| AppError::Config("用户名限流阈值过大".into()))?;
    let issue_inactive_days = i64::try_from(issue_inactive_days)
        .map_err(|_| AppError::Config("Issue 非活跃天数过大".into()))?;
    sqlx::query("INSERT OR IGNORE INTO system_settings(id, allow_registration, login_ip_limit_per_minute, login_username_failure_limit_per_5_minutes, issue_inactive_days) VALUES(1, ?, ?, ?, ?)")
        .bind(allow_registration as i64).bind(ip).bind(username).bind(issue_inactive_days).execute(pool).await.map_err(AppError::Database)?;
    let row: (i64, i64, i64, i64) = sqlx::query_as("SELECT allow_registration, login_ip_limit_per_minute, login_username_failure_limit_per_5_minutes, issue_inactive_days FROM system_settings WHERE id=1").fetch_one(pool).await.map_err(AppError::Database)?;
    let ip = usize::try_from(row.1)
        .map_err(|_| AppError::Config("数据库中的 IP 限流阈值无效".into()))?;
    let username = usize::try_from(row.2)
        .map_err(|_| AppError::Config("数据库中的用户名限流阈值无效".into()))?;
    let issue_inactive_days = usize::try_from(row.3)
        .map_err(|_| AppError::Config("数据库中的 Issue 非活跃天数无效".into()))?;
    if issue_inactive_days != 0 && !(7..=30).contains(&issue_inactive_days) {
        return Err(AppError::Config(
            "数据库中的 Issue 非活跃天数必须为 0，或 7 到 30".into(),
        ));
    }
    Ok((row.0, ip, username, issue_inactive_days))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{
        ACTIVE_CLEANUPS, QUEUED_CLEANUPS, acquire_heavy_cleanup_writer, checkpoint_wal,
        load_or_initialize_system_settings,
    };

    #[tokio::test]
    async fn checkpoint_returns_sqlite_page_counts() {
        let pool = super::init_pool("sqlite::memory:").expect("init pool");
        super::prepare_schema(&pool, true)
            .await
            .expect("prepare schema");

        let stats = checkpoint_wal(&pool).await.expect("checkpoint wal");
        assert!(stats.busy >= 0);
        assert!(stats.log_pages >= -1);
        assert!(stats.checkpointed_pages >= -1);
    }

    #[tokio::test]
    async fn heavyweight_cleanup_writers_are_serialized_and_queued_leases_are_renewed() {
        let first = acquire_heavy_cleanup_writer("first", None)
            .await
            .expect("acquire first cleanup writer");

        let second = tokio::time::timeout(
            Duration::from_millis(25),
            acquire_heavy_cleanup_writer("second", None),
        )
        .await;
        assert!(second.is_err(), "second cleanup writer must remain queued");
        assert_eq!(
            ACTIVE_CLEANUPS.load(std::sync::atomic::Ordering::Acquire),
            1
        );
        assert_eq!(
            QUEUED_CLEANUPS.load(std::sync::atomic::Ordering::Acquire),
            0
        );

        drop(first);
        let second = tokio::time::timeout(
            Duration::from_secs(1),
            acquire_heavy_cleanup_writer("second", None),
        )
        .await
        .expect("second cleanup writer should be released")
        .expect("acquire second cleanup writer");
        drop(second);

        let pool = super::init_pool("sqlite::memory:").expect("init pool");
        super::prepare_schema(&pool, true).await.expect("schema");
        sqlx::query("INSERT INTO issues(code,name,status,deletion_reason,deletion_lease_token,deletion_lease_until) VALUES('LEASE','Lease','DELETING','MANUAL','token',datetime('now','+2 seconds'))")
            .execute(&pool)
            .await
            .expect("insert leased issue");
        sqlx::query("INSERT INTO bundles(id,issue_code,hash,name,status) VALUES('leased-bundle','LEASE','leased-hash','Leased','DELETING')")
            .execute(&pool)
            .await
            .expect("insert deleting bundle");

        let blocker = acquire_heavy_cleanup_writer("blocker", None)
            .await
            .expect("acquire blocking cleanup writer");
        let queued_pool = pool.clone();
        let queued = tokio::spawn(async move {
            super::finish_bundle_deletion_with_inactive_lease(
                &queued_pool,
                "leased-bundle",
                "LEASE",
                "token",
                2,
            )
            .await
        });
        tokio::time::sleep(Duration::from_millis(3_200)).await;
        let lease_is_current: bool = sqlx::query_scalar(
            "SELECT datetime(deletion_lease_until) > datetime('now') FROM issues WHERE code='LEASE'",
        )
        .fetch_one(&pool)
        .await
        .expect("inspect queued lease");
        assert!(lease_is_current, "queued cleanup must keep its lease alive");

        drop(blocker);
        tokio::time::timeout(Duration::from_secs(2), queued)
            .await
            .expect("queued cleanup should acquire the writer")
            .expect("join queued cleanup")
            .expect("finish queued cleanup");
        let bundle_status: String =
            sqlx::query_scalar("SELECT status FROM bundles WHERE id='leased-bundle'")
                .fetch_one(&pool)
                .await
                .expect("inspect cleaned bundle");
        assert_eq!(bundle_status, "DELETED");
    }

    #[tokio::test]
    async fn issue_inactivity_uses_first_start_default_then_database_value() {
        let pool = super::init_pool("sqlite::memory:").expect("init pool");
        super::prepare_schema(&pool, true).await.expect("schema");
        let (_, _, _, days) = load_or_initialize_system_settings(&pool, true, 20, 10, 15)
            .await
            .unwrap();
        assert_eq!(days, 15);
        let (_, _, _, days) = load_or_initialize_system_settings(&pool, false, 30, 20, 3)
            .await
            .unwrap();
        assert_eq!(days, 15);
    }

    #[tokio::test]
    async fn schema_does_not_create_structured_event_storage() {
        let pool = super::init_pool("sqlite::memory:").expect("init pool");
        super::prepare_schema(&pool, true)
            .await
            .expect("prepare schema");

        for object in [
            "log_events",
            "idx_events_bundle_level",
            "idx_events_file_line",
        ] {
            let exists: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name = ?)")
                    .bind(object)
                    .fetch_one(&pool)
                    .await
                    .expect("inspect schema");
            assert!(!exists, "{object} should not exist");
        }
    }

    #[tokio::test]
    async fn schema_creates_authentication_storage() {
        let pool = super::init_pool("sqlite::memory:").expect("init pool");
        super::prepare_schema(&pool, true)
            .await
            .expect("prepare schema");

        for object in [
            "users",
            "user_sessions",
            "saved_searches",
            "idx_user_sessions_user",
            "idx_user_sessions_expiry",
            "idx_saved_searches_user",
        ] {
            let exists: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name = ?)")
                    .bind(object)
                    .fetch_one(&pool)
                    .await
                    .expect("inspect schema");
            assert!(exists, "{object} should exist");
        }
    }

    #[tokio::test]
    async fn schema_uses_trigram_fts_for_substring_matches() {
        let pool = super::init_pool("sqlite::memory:").expect("init pool");
        super::prepare_schema(&pool, true)
            .await
            .expect("prepare schema");
        let schema: String = sqlx::query_scalar(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'log_segments_fts'",
        )
        .fetch_one(&pool)
        .await
        .expect("load fts schema");
        assert!(schema.contains("tokenize='trigram'"), "{schema}");
        assert!(schema.contains("content='log_segments'"), "{schema}");
        assert!(schema.contains("content_rowid='id'"), "{schema}");

        sqlx::query("INSERT INTO issues (code, name) VALUES ('SEARCH', 'Search')")
            .execute(&pool)
            .await
            .expect("insert issue");
        sqlx::query("INSERT INTO bundles (id, issue_code, hash, name, status) VALUES ('bundle', 'SEARCH', 'hash', 'Search', 'READY')")
        .execute(&pool)
        .await
        .expect("insert bundle");
        let file_id: i64 = sqlx::query_scalar("INSERT INTO files (bundle_id, name, path, is_dir) VALUES ('bundle', 'app.log', '/app.log', 0) RETURNING id")
        .fetch_one(&pool)
        .await
        .expect("insert file");
        sqlx::query("INSERT INTO log_segments (bundle_id, file_id, content) VALUES ('bundle', ?, 'requestId=abcdef123456')")
        .bind(file_id)
        .execute(&pool)
        .await
        .expect("insert segment content");
        let matches: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM log_segments_fts WHERE log_segments_fts MATCH 'def123'",
        )
        .fetch_one(&pool)
        .await
        .expect("search trigram substring");
        assert_eq!(matches, 1);
    }
}
