use std::{
    path::Path,
    str::FromStr,
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, Instant},
};

use once_cell::sync::Lazy;

use sqlx::{
    FromRow, SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};
use tokio::sync::{Semaphore, SemaphorePermit};

use crate::error::AppError;

pub const CLEANUP_BATCH_SIZE: u64 = 1_000;
const LARGE_CLEANUP_CHECKPOINT_ROWS: u64 = 10_000;
const LOG_SEGMENT_BACKFILL_BATCH_SIZE: i64 = 500;
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

    Ok(SqlitePoolOptions::new()
        .max_connections(5)
        .connect_lazy_with(options))
}

pub async fn prepare_schema(pool: &SqlitePool, reset: bool) -> Result<(), AppError> {
    if reset {
        reset_schema(pool).await?;
    }
    create_schema(pool).await?;
    Ok(())
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
            "DELETE FROM files WHERE rowid IN (SELECT rowid FROM files WHERE bundle_id = ? LIMIT ?)",
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
        let affected = sqlx::query(statement)
            .bind(bundle_id)
            .bind(batch_size as i64)
            .execute(pool)
            .await
            .map_err(AppError::Database)?
            .rows_affected();
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

pub async fn cleanup_expired_bundles(
    pool: &SqlitePool,
    retention_days: u64,
) -> Result<u64, AppError> {
    let cutoff = format!("-{retention_days} days");
    let bundles = sqlx::query_as::<_, ExpiredBundle>(
        r#"
        SELECT id
        FROM bundles
        WHERE deleted_at IS NULL
          AND status IN ('READY', 'FAILED')
          AND datetime(created_at) < datetime('now', ?)
        "#,
    )
    .bind(cutoff)
    .fetch_all(pool)
    .await
    .map_err(AppError::Database)?;

    if bundles.is_empty() {
        return Ok(0);
    }

    for bundle in &bundles {
        sqlx::query(
            "UPDATE bundles SET status = 'DELETING', deleted_at = CURRENT_TIMESTAMP WHERE id = ?",
        )
        .bind(&bundle.id)
        .execute(pool)
        .await
        .map_err(AppError::Database)?;
        finish_bundle_deletion(pool, &bundle.id).await?;
    }

    Ok(bundles.len() as u64)
}

pub async fn finish_bundle_deletion(pool: &SqlitePool, bundle_id: &str) -> Result<(), AppError> {
    let _cleanup_permit = acquire_heavy_cleanup_writer(bundle_id, None).await?;
    cleanup_bundle_content_batched_inner(pool, bundle_id, CLEANUP_BATCH_SIZE, None).await?;
    sqlx::query("UPDATE bundles SET status = 'DELETED', content_size_bytes = 0 WHERE id = ? AND status = 'DELETING'")
        .bind(bundle_id)
        .execute(pool)
        .await
        .map_err(AppError::Database)?;
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
    require_inactive_issue_lease(pool, issue_code, lease_token, lease_seconds).await?;
    sqlx::query("UPDATE bundles SET status = 'DELETED', content_size_bytes = 0 WHERE id = ? AND status = 'DELETING'")
        .bind(bundle_id)
        .execute(pool)
        .await
        .map_err(AppError::Database)?;
    require_inactive_issue_lease(pool, issue_code, lease_token, lease_seconds).await?;
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
    let modifier = format!("+{lease_seconds} seconds");
    let renewed = sqlx::query("UPDATE issues SET deletion_lease_until=datetime('now', ?) WHERE code=? AND status='DELETING' AND deletion_reason IN ('INACTIVE', 'MANUAL') AND deletion_lease_token=? AND datetime(deletion_lease_until) > datetime('now')")
        .bind(modifier)
        .bind(issue_code)
        .bind(lease_token)
        .execute(pool)
        .await
        .map_err(AppError::Database)?
        .rows_affected();
    Ok(renewed == 1)
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
    let result = sqlx::query(
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
    .execute(pool)
    .await
    .map_err(AppError::Database)?;

    Ok(result.rows_affected())
}

pub async fn fail_stale_processing_bundles_before(
    pool: &SqlitePool,
    created_before: &str,
) -> Result<u64, AppError> {
    let result = sqlx::query(
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
    .execute(pool)
    .await
    .map_err(AppError::Database)?;

    Ok(result.rows_affected())
}

#[derive(FromRow)]
struct ExpiredBundle {
    id: String,
}

async fn reset_schema(pool: &SqlitePool) -> Result<(), AppError> {
    let statements = [
        "DROP TABLE IF EXISTS log_segments_fts",
        "DROP TABLE IF EXISTS skill_run_steps",
        "DROP TABLE IF EXISTS skill_runs",
        "DROP TABLE IF EXISTS skill_reviews",
        "DROP TABLE IF EXISTS user_skills",
        "DROP TABLE IF EXISTS ai_provider_settings",
        "DROP TABLE IF EXISTS admin_audit_logs",
        "DROP TABLE IF EXISTS system_settings",
        "DROP TABLE IF EXISTS saved_searches",
        "DROP TABLE IF EXISTS user_sessions",
        "DROP TABLE IF EXISTS users",
        "DROP TABLE IF EXISTS temp_results",
        "DROP TABLE IF EXISTS rain_ready_probe",
        "DROP TABLE IF EXISTS log_line_offsets",
        "DROP TABLE IF EXISTS log_segments",
        "DROP TABLE IF EXISTS files",
        "DROP TABLE IF EXISTS blobs",
        "DROP TABLE IF EXISTS bundles",
        "DROP TABLE IF EXISTS issues",
    ];

    for statement in statements {
        sqlx::query(statement)
            .execute(pool)
            .await
            .map_err(AppError::Database)?;
    }

    Ok(())
}

async fn create_schema(pool: &SqlitePool) -> Result<(), AppError> {
    let statements = [
        r#"
        CREATE TABLE IF NOT EXISTS system_settings (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            allow_registration INTEGER NOT NULL CHECK (allow_registration IN (0, 1)),
            updated_by_user_id TEXT REFERENCES users(id) ON DELETE SET NULL,
            updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            ,login_ip_limit_per_minute INTEGER NOT NULL DEFAULT 20 CHECK (login_ip_limit_per_minute BETWEEN 1 AND 1000)
            ,login_username_failure_limit_per_5_minutes INTEGER NOT NULL DEFAULT 10 CHECK (login_username_failure_limit_per_5_minutes BETWEEN 1 AND 100)
            ,issue_inactive_days INTEGER NOT NULL DEFAULT 0 CHECK (issue_inactive_days = 0 OR issue_inactive_days BETWEEN 7 AND 30)
        )
        "#,
        r#"
        CREATE TABLE IF NOT EXISTS users (
            id TEXT PRIMARY KEY,
            username TEXT NOT NULL,
            username_normalized TEXT NOT NULL UNIQUE,
            password_hash TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'ACTIVE' CHECK (status IN ('ACTIVE', 'DISABLED')),
            role TEXT NOT NULL DEFAULT 'USER' CHECK (role IN ('USER', 'ADMIN')),
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            last_login_at TEXT,
            password_changed_at TEXT,
            CHECK (role != 'ADMIN' OR status = 'ACTIVE')
        )
        "#,
        r#"
        CREATE TABLE IF NOT EXISTS admin_audit_logs (
            id TEXT PRIMARY KEY,
            actor_type TEXT NOT NULL CHECK (actor_type IN ('USER', 'SYSTEM')),
            actor_user_id TEXT REFERENCES users(id) ON DELETE SET NULL,
            target_user_id TEXT REFERENCES users(id) ON DELETE SET NULL,
            action TEXT NOT NULL,
            old_value TEXT,
            new_value TEXT,
            client_ip TEXT,
            user_agent TEXT,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        )
        "#,
        r#"
        CREATE TABLE IF NOT EXISTS user_sessions (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            token_hash TEXT NOT NULL UNIQUE,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            last_seen_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            expires_at TEXT NOT NULL,
            revoked_at TEXT,
            user_agent TEXT,
            client_ip TEXT
        )
        "#,
        r#"
        CREATE TABLE IF NOT EXISTS saved_searches (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            name TEXT COLLATE NOCASE NOT NULL,
            search_type TEXT NOT NULL CHECK (search_type IN ('FILENAME', 'DETAIL')),
            query_text TEXT NOT NULL,
            scope_type TEXT NOT NULL DEFAULT 'GLOBAL' CHECK (scope_type IN ('GLOBAL', 'ISSUE')),
            scope_key TEXT,
            options_json TEXT NOT NULL DEFAULT '{}',
            is_pinned INTEGER NOT NULL DEFAULT 0,
            sort_order INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            last_used_at TEXT,
            UNIQUE(user_id, name),
            CHECK (
                (scope_type = 'GLOBAL' AND scope_key IS NULL)
                OR (scope_type = 'ISSUE' AND scope_key IS NOT NULL)
            )
        )
        "#,
        r#"
        CREATE TABLE IF NOT EXISTS issues (
            code TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            description TEXT,
            owner_user_id TEXT REFERENCES users(id) ON DELETE SET NULL,
            status TEXT NOT NULL DEFAULT 'ACTIVE',
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            last_activity_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            deletion_reason TEXT CHECK (deletion_reason IS NULL OR deletion_reason IN ('MANUAL', 'INACTIVE')),
            inactive_claim_days INTEGER CHECK (inactive_claim_days BETWEEN 1 AND 30),
            deletion_lease_token TEXT,
            deletion_lease_until TEXT,
            deletion_retry_at TEXT,
            deletion_attempts INTEGER NOT NULL DEFAULT 0 CHECK (deletion_attempts >= 0)
        )
        "#,
        r#"
        CREATE TABLE IF NOT EXISTS bundles (
            id TEXT PRIMARY KEY,
            issue_code TEXT NOT NULL REFERENCES issues(code) ON DELETE CASCADE,
            hash TEXT NOT NULL UNIQUE,
            name TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'PENDING',
            process_stage TEXT NOT NULL DEFAULT 'PENDING',
            failure_stage TEXT,
            failure_code TEXT,
            failure_reason TEXT,
            retryable INTEGER,
            deleted_at TEXT,
            uploader_user_id TEXT REFERENCES users(id) ON DELETE SET NULL,
            size_bytes INTEGER,
            content_size_bytes INTEGER NOT NULL DEFAULT 0 CHECK (content_size_bytes >= 0),
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        )
        "#,
        r#"
        CREATE TABLE IF NOT EXISTS blobs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            content_hash TEXT NOT NULL UNIQUE,
            size_bytes INTEGER NOT NULL CHECK (size_bytes >= 0),
            storage_backend TEXT NOT NULL,
            storage_key TEXT NOT NULL UNIQUE,
            state TEXT NOT NULL,
            last_attempt_at TEXT,
            unreferenced_at TEXT,
            verified_at TEXT
        )
        "#,
        r#"
        CREATE TABLE IF NOT EXISTS files (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            bundle_id TEXT NOT NULL REFERENCES bundles(id) ON DELETE CASCADE,
            parent_id INTEGER REFERENCES files(id) ON DELETE CASCADE,
            blob_id INTEGER REFERENCES blobs(id),
            name TEXT NOT NULL,
            path TEXT NOT NULL,
            is_dir INTEGER NOT NULL,
            size_bytes INTEGER,
            line_count INTEGER,
            mime_type TEXT,
            status TEXT,
            meta TEXT,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            CONSTRAINT files_bundle_path UNIQUE (bundle_id, path)
        )
        "#,
        r#"
        CREATE TABLE IF NOT EXISTS log_segments (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            bundle_id TEXT NOT NULL REFERENCES bundles(id) ON DELETE CASCADE,
            file_id INTEGER REFERENCES files(id) ON DELETE CASCADE,
            timeline TEXT,
            content TEXT NOT NULL,
            line_offset INTEGER,
            line_end INTEGER,
            chunk_index INTEGER,
            event_time_start_ms INTEGER,
            event_time_end_ms INTEGER,
            event_time_indexed INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        )
        "#,
        r#"
        CREATE TABLE IF NOT EXISTS log_line_offsets (
            file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
            line_number INTEGER NOT NULL,
            byte_offset INTEGER NOT NULL,
            PRIMARY KEY (file_id, line_number)
        )
        "#,
        r#"
        CREATE TABLE IF NOT EXISTS temp_results (
            id TEXT PRIMARY KEY,
            status TEXT NOT NULL DEFAULT 'ACTIVE' CHECK (status IN ('STAGING', 'ACTIVE', 'DELETING')),
            name TEXT NOT NULL,
            expression TEXT NOT NULL,
            source_label TEXT NOT NULL,
            storage_path TEXT NOT NULL,
            line_count INTEGER NOT NULL,
            size_bytes INTEGER NOT NULL,
            created_at TEXT NOT NULL,
            expires_at TEXT NOT NULL
        )
        "#,
        r#"
        CREATE TABLE IF NOT EXISTS user_skills (
            id TEXT PRIMARY KEY,
            owner_user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            name TEXT COLLATE NOCASE NOT NULL,
            description TEXT,
            skill_markdown TEXT NOT NULL,
            content_hash TEXT NOT NULL,
            version INTEGER NOT NULL DEFAULT 1 CHECK (version > 0),
            enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            UNIQUE(owner_user_id, name)
        )
        "#,
        r#"
        CREATE TABLE IF NOT EXISTS skill_reviews (
            skill_id TEXT PRIMARY KEY REFERENCES user_skills(id) ON DELETE CASCADE,
            skill_version INTEGER NOT NULL CHECK (skill_version > 0),
            skill_content_hash TEXT NOT NULL,
            reviewer_model TEXT NOT NULL,
            rubric_version TEXT NOT NULL,
            overall_score INTEGER NOT NULL CHECK (overall_score BETWEEN 0 AND 100),
            grade TEXT NOT NULL,
            dimension_scores_json TEXT NOT NULL,
            findings_json TEXT NOT NULL,
            evaluated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        )
        "#,
        r#"
        CREATE TABLE IF NOT EXISTS ai_provider_settings (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            base_url TEXT NOT NULL,
            encrypted_api_key TEXT NOT NULL,
            model TEXT NOT NULL,
            request_timeout_seconds INTEGER NOT NULL CHECK (request_timeout_seconds BETWEEN 1 AND 300),
            updated_by_user_id TEXT REFERENCES users(id) ON DELETE SET NULL,
            updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        )
        "#,
        r#"
        CREATE TABLE IF NOT EXISTS skill_runs (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            issue_code TEXT NOT NULL REFERENCES issues(code) ON DELETE CASCADE,
            skill_id TEXT NOT NULL,
            skill_version INTEGER NOT NULL CHECK (skill_version > 0),
            skill_name TEXT NOT NULL,
            skill_snapshot_markdown TEXT NOT NULL,
            status TEXT NOT NULL CHECK (status IN ('QUEUED', 'RUNNING', 'SUCCEEDED', 'FAILED', 'CANCELLED')),
            iteration_count INTEGER NOT NULL DEFAULT 0 CHECK (iteration_count >= 0),
            tool_call_count INTEGER NOT NULL DEFAULT 0 CHECK (tool_call_count >= 0),
            cancel_requested INTEGER NOT NULL DEFAULT 0 CHECK (cancel_requested IN (0, 1)),
            result_json TEXT,
            error_code TEXT,
            error_message TEXT,
            started_at TEXT,
            completed_at TEXT,
            analysis_start_time TEXT,
            analysis_end_time TEXT,
            analysis_start_ms INTEGER,
            analysis_end_ms INTEGER,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        )
        "#,
        r#"
        CREATE TABLE IF NOT EXISTS skill_run_steps (
            id TEXT PRIMARY KEY,
            run_id TEXT NOT NULL REFERENCES skill_runs(id) ON DELETE CASCADE,
            sequence INTEGER NOT NULL CHECK (sequence >= 0),
            iteration INTEGER NOT NULL CHECK (iteration >= 0),
            tool_name TEXT,
            arguments_summary TEXT,
            hit_count INTEGER,
            evidence_json TEXT,
            elapsed_ms INTEGER NOT NULL DEFAULT 0 CHECK (elapsed_ms >= 0),
            status TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            UNIQUE(run_id, sequence)
        )
        "#,
        r#"
        CREATE TABLE IF NOT EXISTS rain_ready_probe (
            id TEXT PRIMARY KEY,
            value INTEGER NOT NULL
        )
        "#,
        r#"
        CREATE VIRTUAL TABLE IF NOT EXISTS log_segments_fts USING fts5(
            content,
            content='log_segments',
            content_rowid='id',
            tokenize='trigram'
        )
        "#,
        r#"
        CREATE TRIGGER IF NOT EXISTS log_segments_fts_ai AFTER INSERT ON log_segments BEGIN
            INSERT INTO log_segments_fts(rowid, content) VALUES (new.id, new.content);
        END
        "#,
        r#"
        CREATE TRIGGER IF NOT EXISTS log_segments_fts_ad AFTER DELETE ON log_segments BEGIN
            INSERT INTO log_segments_fts(log_segments_fts, rowid, content)
            VALUES ('delete', old.id, old.content);
        END
        "#,
        r#"
        CREATE TRIGGER IF NOT EXISTS log_segments_fts_au AFTER UPDATE OF content ON log_segments BEGIN
            INSERT INTO log_segments_fts(log_segments_fts, rowid, content)
            VALUES ('delete', old.id, old.content);
            INSERT INTO log_segments_fts(rowid, content) VALUES (new.id, new.content);
        END
        "#,
    ];

    for statement in statements {
        sqlx::query(statement)
            .execute(pool)
            .await
            .map_err(AppError::Database)?;
    }

    ensure_skill_run_optional_columns(pool).await?;
    ensure_log_segment_optional_columns(pool).await?;
    backfill_log_segment_event_times(pool).await?;

    let index_statements = [
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_skill_runs_one_active_per_user ON skill_runs(user_id) WHERE status IN ('QUEUED', 'RUNNING')",
        "CREATE INDEX IF NOT EXISTS idx_skill_runs_terminal_cleanup ON skill_runs(status, completed_at)",
        "CREATE INDEX IF NOT EXISTS idx_bundles_issue ON bundles (issue_code, created_at DESC)",
        "CREATE INDEX IF NOT EXISTS idx_issues_activity ON issues (status, last_activity_at)",
        "CREATE INDEX IF NOT EXISTS idx_files_parent ON files (parent_id)",
        "CREATE INDEX IF NOT EXISTS idx_files_bundle ON files (bundle_id)",
        "CREATE INDEX IF NOT EXISTS idx_files_path ON files (path)",
        "CREATE INDEX IF NOT EXISTS idx_logs_bundle_timeline ON log_segments (bundle_id, timeline)",
        "CREATE INDEX IF NOT EXISTS idx_logs_file_chunk ON log_segments (file_id, chunk_index)",
        "CREATE INDEX IF NOT EXISTS idx_logs_file_event_time ON log_segments (file_id, event_time_start_ms, event_time_end_ms)",
        "CREATE INDEX IF NOT EXISTS idx_logs_event_time_indexed ON log_segments (event_time_indexed, id)",
        "CREATE INDEX IF NOT EXISTS idx_line_offsets_file_line ON log_line_offsets (file_id, line_number)",
        "CREATE INDEX IF NOT EXISTS idx_temp_results_expiry ON temp_results (expires_at)",
        "CREATE INDEX IF NOT EXISTS idx_user_sessions_user ON user_sessions (user_id)",
        "CREATE INDEX IF NOT EXISTS idx_user_sessions_expiry ON user_sessions (expires_at)",
        "CREATE INDEX IF NOT EXISTS idx_users_role_status ON users (role, status, created_at, id)",
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_users_single_admin ON users (role) WHERE role = 'ADMIN'",
        "CREATE INDEX IF NOT EXISTS idx_admin_audit_created ON admin_audit_logs (created_at DESC, id DESC)",
        "CREATE INDEX IF NOT EXISTS idx_admin_audit_target ON admin_audit_logs (target_user_id, created_at DESC)",
        "CREATE INDEX IF NOT EXISTS idx_saved_searches_user ON saved_searches (user_id, is_pinned DESC, sort_order, updated_at DESC)",
    ];
    for statement in index_statements {
        sqlx::query(statement)
            .execute(pool)
            .await
            .map_err(AppError::Database)?;
    }

    sqlx::query("UPDATE saved_searches SET scope_type = 'GLOBAL', scope_key = NULL WHERE scope_type != 'GLOBAL' OR scope_key IS NOT NULL")
        .execute(pool)
        .await
        .map_err(AppError::Database)?;

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_files_blob ON files (blob_id)")
        .execute(pool)
        .await
        .map_err(AppError::Database)?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_bundles_deleted ON bundles (deleted_at)")
        .execute(pool)
        .await
        .map_err(AppError::Database)?;
    Ok(())
}

async fn ensure_skill_run_optional_columns(pool: &SqlitePool) -> Result<(), AppError> {
    let existing: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('skill_runs')")
            .fetch_all(pool)
            .await
            .map_err(AppError::Database)?;
    let columns = [
        (
            "analysis_start_time",
            "ALTER TABLE skill_runs ADD COLUMN analysis_start_time TEXT",
        ),
        (
            "analysis_end_time",
            "ALTER TABLE skill_runs ADD COLUMN analysis_end_time TEXT",
        ),
        (
            "analysis_start_ms",
            "ALTER TABLE skill_runs ADD COLUMN analysis_start_ms INTEGER",
        ),
        (
            "analysis_end_ms",
            "ALTER TABLE skill_runs ADD COLUMN analysis_end_ms INTEGER",
        ),
    ];
    for (column, statement) in columns {
        if !existing.iter().any(|name| name == column) {
            sqlx::query(statement)
                .execute(pool)
                .await
                .map_err(AppError::Database)?;
        }
    }
    Ok(())
}

async fn ensure_log_segment_optional_columns(pool: &SqlitePool) -> Result<(), AppError> {
    let existing: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('log_segments')")
            .fetch_all(pool)
            .await
            .map_err(AppError::Database)?;
    let columns = [
        (
            "event_time_start_ms",
            "ALTER TABLE log_segments ADD COLUMN event_time_start_ms INTEGER",
        ),
        (
            "event_time_end_ms",
            "ALTER TABLE log_segments ADD COLUMN event_time_end_ms INTEGER",
        ),
        (
            "event_time_indexed",
            "ALTER TABLE log_segments ADD COLUMN event_time_indexed INTEGER NOT NULL DEFAULT 0",
        ),
    ];
    for (column, statement) in columns {
        if !existing.iter().any(|name| name == column) {
            sqlx::query(statement)
                .execute(pool)
                .await
                .map_err(AppError::Database)?;
        }
    }
    Ok(())
}

async fn backfill_log_segment_event_times(pool: &SqlitePool) -> Result<(), AppError> {
    let mut last_id = 0_i64;
    loop {
        let mut tx = pool.begin().await.map_err(AppError::Database)?;
        let segments: Vec<(i64, String)> = sqlx::query_as(
            "SELECT id, content FROM log_segments WHERE id > ? AND event_time_indexed = 0 ORDER BY id LIMIT ?",
        )
        .bind(last_id)
        .bind(LOG_SEGMENT_BACKFILL_BATCH_SIZE)
        .fetch_all(&mut *tx)
        .await
        .map_err(AppError::Database)?;
        if segments.is_empty() {
            tx.commit().await.map_err(AppError::Database)?;
            break;
        }

        let batch_last_id = segments
            .last()
            .map(|(id, _)| *id)
            .expect("non-empty segment batch has a last id");
        for (id, content) in segments {
            let (Some(start_ms), Some(end_ms)) = crate::ingest::event_time_range(&content) else {
                sqlx::query(
                    "UPDATE log_segments SET event_time_indexed = 1 WHERE id = ? AND event_time_indexed = 0",
                )
                .bind(id)
                .execute(&mut *tx)
                .await
                .map_err(AppError::Database)?;
                continue;
            };
            sqlx::query(
                "UPDATE log_segments SET event_time_start_ms = COALESCE(event_time_start_ms, ?), event_time_end_ms = COALESCE(event_time_end_ms, ?), event_time_indexed = 1 WHERE id = ? AND event_time_indexed = 0",
            )
            .bind(start_ms)
            .bind(end_ms)
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(AppError::Database)?;
        }
        tx.commit().await.map_err(AppError::Database)?;
        last_id = batch_last_id;
    }
    Ok(())
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
    async fn prepare_schema_upgrades_legacy_skill_run_tables_idempotently() {
        let pool = super::init_pool("sqlite::memory:").expect("init pool");
        sqlx::query(
            r#"
            CREATE TABLE skill_runs (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                issue_code TEXT NOT NULL,
                skill_id TEXT NOT NULL,
                skill_version INTEGER NOT NULL,
                skill_name TEXT NOT NULL,
                skill_snapshot_markdown TEXT NOT NULL,
                status TEXT NOT NULL,
                iteration_count INTEGER NOT NULL DEFAULT 0,
                tool_call_count INTEGER NOT NULL DEFAULT 0,
                cancel_requested INTEGER NOT NULL DEFAULT 0,
                result_json TEXT,
                error_code TEXT,
                error_message TEXT,
                started_at TEXT,
                completed_at TEXT,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            )
            "#,
        )
        .execute(&pool)
        .await
        .expect("create legacy skill_runs");
        sqlx::query(
            r#"
            CREATE TABLE log_segments (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                bundle_id TEXT,
                file_id INTEGER,
                timeline TEXT,
                content TEXT NOT NULL,
                line_offset INTEGER,
                line_end INTEGER,
                chunk_index INTEGER,
                event_time_start_ms INTEGER,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            )
            "#,
        )
        .execute(&pool)
        .await
        .expect("create legacy log_segments");

        sqlx::query("INSERT INTO log_segments (content) VALUES (?)")
            .bind("2026-08-14T09:32:15Z first\nnoise\n2026-08-14T09:33:15Z second")
            .execute(&pool)
            .await
            .expect("insert backfillable segment");
        sqlx::query("INSERT INTO log_segments (content) VALUES (?)")
            .bind("not a dated log line")
            .execute(&pool)
            .await
            .expect("insert unparseable segment");
        sqlx::query("INSERT INTO log_segments (content, event_time_start_ms) VALUES (?, ?)")
            .bind("2026-08-14T09:34:15Z partial")
            .bind(111_i64)
            .execute(&pool)
            .await
            .expect("insert partially bounded segment");

        super::prepare_schema(&pool, false)
            .await
            .expect("upgrade schema");

        let names: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_table_info('skill_runs')")
                .fetch_all(&pool)
                .await
                .expect("inspect upgraded columns");
        for column in [
            "analysis_start_time",
            "analysis_end_time",
            "analysis_start_ms",
            "analysis_end_ms",
        ] {
            assert!(
                names.iter().any(|name| name == column),
                "skill_runs.{column}"
            );
        }

        let log_segment_columns: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_table_info('log_segments')")
                .fetch_all(&pool)
                .await
                .expect("inspect upgraded log segment columns");
        for column in [
            "event_time_start_ms",
            "event_time_end_ms",
            "event_time_indexed",
        ] {
            assert!(
                log_segment_columns.iter().any(|name| name == column),
                "log_segments.{column}"
            );
        }

        let bounds: (Option<i64>, Option<i64>) = sqlx::query_as(
            "SELECT event_time_start_ms, event_time_end_ms FROM log_segments WHERE content LIKE '2026-%'",
        )
        .fetch_one(&pool)
        .await
        .expect("inspect backfilled segment");
        assert_eq!(bounds, (Some(1_786_699_935_000), Some(1_786_699_995_000)));

        let partial_bounds: (Option<i64>, Option<i64>) = sqlx::query_as(
            "SELECT event_time_start_ms, event_time_end_ms FROM log_segments WHERE content LIKE '%partial'",
        )
        .fetch_one(&pool)
        .await
        .expect("inspect partially bounded segment");
        assert_eq!(partial_bounds, (Some(111), Some(1_786_700_055_000)));

        sqlx::query(
            "UPDATE log_segments SET event_time_start_ms = 111, event_time_end_ms = 222 WHERE content LIKE '2026-%'",
        )
        .execute(&pool)
        .await
        .expect("mark populated segment");
        super::prepare_schema(&pool, false)
            .await
            .expect("repeat backfill schema");
        let preserved_bounds: (Option<i64>, Option<i64>) = sqlx::query_as(
            "SELECT event_time_start_ms, event_time_end_ms FROM log_segments WHERE content LIKE '2026-%'",
        )
        .fetch_one(&pool)
        .await
        .expect("inspect preserved segment");
        assert_eq!(preserved_bounds, (Some(111), Some(222)));

        sqlx::query("INSERT INTO log_segments (content) VALUES (?)")
            .bind("2026-08-14T09:35:15Z added after upgrade")
            .execute(&pool)
            .await
            .expect("insert segment after upgrade");
        super::prepare_schema(&pool, false)
            .await
            .expect("do not repeat completed backfill");
        let late_bounds: (Option<i64>, Option<i64>) = sqlx::query_as(
            "SELECT event_time_start_ms, event_time_end_ms FROM log_segments WHERE content LIKE '%added after upgrade'",
        )
        .fetch_one(&pool)
        .await
        .expect("inspect late segment");
        assert_eq!(
            late_bounds,
            (Some(1_786_700_115_000), Some(1_786_700_115_000))
        );

        let null_bounds: (Option<i64>, Option<i64>) = sqlx::query_as(
            "SELECT event_time_start_ms, event_time_end_ms FROM log_segments WHERE content = 'not a dated log line'",
        )
        .fetch_one(&pool)
        .await
        .expect("inspect unparseable segment");
        assert_eq!(null_bounds, (None, None));

        let unparseable_indexed: i64 = sqlx::query_scalar(
            "SELECT event_time_indexed FROM log_segments WHERE content = 'not a dated log line'",
        )
        .fetch_one(&pool)
        .await
        .expect("inspect unparseable indexed state");
        assert_eq!(unparseable_indexed, 1);

        sqlx::query(
            "UPDATE log_segments SET content = '2026-08-14T09:36:15Z now parseable' WHERE content = 'not a dated log line'",
        )
        .execute(&pool)
        .await
        .expect("change completed unparseable segment");
        super::prepare_schema(&pool, false)
            .await
            .expect("skip completed event time indexing");
        let skipped_bounds: (Option<i64>, Option<i64>, i64) = sqlx::query_as(
            "SELECT event_time_start_ms, event_time_end_ms, event_time_indexed FROM log_segments WHERE content = '2026-08-14T09:36:15Z now parseable'",
        )
        .fetch_one(&pool)
        .await
        .expect("inspect skipped completed segment");
        assert_eq!(skipped_bounds, (None, None, 1));

        sqlx::query(
            "INSERT INTO log_segments (content, event_time_start_ms, event_time_end_ms) VALUES (?, ?, ?)",
        )
        .bind("2026-08-14T09:37:15Z interrupted")
        .bind(111_i64)
        .bind(222_i64)
        .execute(&pool)
        .await
        .expect("insert interrupted segment");
        let pending_indexed: i64 = sqlx::query_scalar(
            "SELECT event_time_indexed FROM log_segments WHERE content LIKE '%interrupted%'",
        )
        .fetch_one(&pool)
        .await
        .expect("inspect pending interrupted segment");
        assert_eq!(pending_indexed, 0);
        super::backfill_log_segment_event_times(&pool)
            .await
            .expect("resume interrupted event time indexing");
        let resumed: (Option<i64>, Option<i64>, i64) = sqlx::query_as(
            "SELECT event_time_start_ms, event_time_end_ms, event_time_indexed FROM log_segments WHERE content LIKE '%interrupted%'",
        )
        .fetch_one(&pool)
        .await
        .expect("inspect resumed segment");
        assert_eq!(resumed, (Some(111), Some(222), 1));

        let index_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'index' AND name = 'idx_logs_file_event_time')",
        )
        .fetch_one(&pool)
        .await
        .expect("inspect event time index");
        assert!(index_exists);

        let backfill_index_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'index' AND name = 'idx_logs_event_time_indexed')",
        )
        .fetch_one(&pool)
        .await
        .expect("inspect event time backfill index");
        assert!(backfill_index_exists);
        let backfill_index_columns: Vec<(i64, String)> = sqlx::query_as(
            "SELECT seqno, name FROM pragma_index_info('idx_logs_event_time_indexed') ORDER BY seqno",
        )
        .fetch_all(&pool)
        .await
        .expect("inspect event time backfill index columns");
        assert_eq!(
            backfill_index_columns,
            vec![(0, "event_time_indexed".to_owned()), (1, "id".to_owned())]
        );
    }

    #[tokio::test]
    async fn event_time_backfill_preserves_fts_content() {
        let pool = super::init_pool("sqlite::memory:").expect("init pool");
        super::prepare_schema(&pool, true)
            .await
            .expect("prepare schema");
        sqlx::query("INSERT INTO issues (code, name) VALUES ('BACKFILLFTS', 'Backfill FTS')")
            .execute(&pool)
            .await
            .expect("insert issue");
        sqlx::query(
            "INSERT INTO bundles (id, issue_code, hash, name, status) VALUES ('backfill-fts-bundle', 'BACKFILLFTS', 'hash', 'Backfill FTS', 'READY')",
        )
        .execute(&pool)
        .await
        .expect("insert bundle");
        let file_id: i64 = sqlx::query_scalar(
            "INSERT INTO files (bundle_id, name, path, is_dir) VALUES ('backfill-fts-bundle', 'app.log', '/app.log', 0) RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .expect("insert file");
        sqlx::query(
            "INSERT INTO log_segments (bundle_id, file_id, content, event_time_start_ms, event_time_end_ms) VALUES (?, ?, ?, NULL, NULL)",
        )
        .bind("backfill-fts-bundle")
        .bind(file_id)
        .bind("2026-08-14T09:32:15Z requestId=backfill123")
        .execute(&pool)
        .await
        .expect("insert segment");

        let before: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM log_segments_fts WHERE log_segments_fts MATCH 'backfill123'",
        )
        .fetch_one(&pool)
        .await
        .expect("search FTS before backfill");
        super::backfill_log_segment_event_times(&pool)
            .await
            .expect("backfill event time");
        let after: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM log_segments_fts WHERE log_segments_fts MATCH 'backfill123'",
        )
        .fetch_one(&pool)
        .await
        .expect("search FTS after backfill");

        assert_eq!(before, 1);
        assert_eq!(after, before);
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
