use std::{path::Path, str::FromStr, time::Duration};

use sqlx::{
    FromRow, SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};

use crate::error::AppError;

pub const CLEANUP_BATCH_SIZE: u64 = 10_000;
const LARGE_CLEANUP_CHECKPOINT_ROWS: u64 = 10_000;

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
        sqlx::query_as("PRAGMA wal_checkpoint(TRUNCATE)")
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
        )
        .await?,
        files: delete_bundle_rows_in_batches(
            pool,
            bundle_id,
            batch_size,
            "files",
            "DELETE FROM files WHERE rowid IN (SELECT rowid FROM files WHERE bundle_id = ? LIMIT ?)",
        )
        .await?,
    };

    if stats.total_rows() >= LARGE_CLEANUP_CHECKPOINT_ROWS {
        let started = std::time::Instant::now();
        match checkpoint_wal(pool).await {
            Ok(checkpoint) => tracing::info!(
                bundle_id,
                busy = checkpoint.busy,
                log_pages = checkpoint.log_pages,
                checkpointed_pages = checkpoint.checkpointed_pages,
                elapsed_ms = started.elapsed().as_millis() as u64,
                "large bundle cleanup WAL checkpoint completed"
            ),
            Err(error) => tracing::warn!(
                bundle_id,
                error = %error,
                elapsed_ms = started.elapsed().as_millis() as u64,
                "large bundle cleanup WAL checkpoint failed"
            ),
        }
    }

    Ok(stats)
}

async fn delete_bundle_rows_in_batches(
    pool: &SqlitePool,
    bundle_id: &str,
    batch_size: u64,
    phase: &'static str,
    statement: &'static str,
) -> Result<CleanupPhaseStats, AppError> {
    let started = std::time::Instant::now();
    let mut stats = CleanupPhaseStats::default();
    loop {
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
        WHERE deleted_at IS NULL AND datetime(created_at) < datetime('now', ?)
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
        cleanup_bundle_content_batched(pool, &bundle.id, CLEANUP_BATCH_SIZE).await?;

        sqlx::query(
            "UPDATE bundles SET status = 'DELETED', deleted_at = CURRENT_TIMESTAMP WHERE id = ?",
        )
        .bind(&bundle.id)
        .execute(pool)
        .await
        .map_err(AppError::Database)?;
    }

    Ok(bundles.len() as u64)
}

pub async fn fail_stale_processing_bundles(pool: &SqlitePool) -> Result<u64, AppError> {
    let result = sqlx::query(
        r#"
        UPDATE bundles
        SET failure_stage = process_stage,
            failure_code = 'PROCESS_INTERRUPTED',
            retryable = 1,
            status = 'FAILED',
            process_stage = 'FAILED',
            failure_reason = '服务重启时检测到未完成的上传，请删除后重试'
        WHERE status IN ('PENDING', 'PROCESSING', 'RECEIVING', 'EXTRACTING', 'INDEXING', 'PUBLISHING')
        "#,
    )
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
        "DROP TABLE IF EXISTS temp_results",
        "DROP TABLE IF EXISTS log_events",
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
        CREATE TABLE IF NOT EXISTS issues (
            code TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            description TEXT,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
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
            unreferenced_at TEXT
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
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        )
        "#,
        "DROP TABLE IF EXISTS log_events",
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
        "CREATE INDEX IF NOT EXISTS idx_bundles_issue ON bundles (issue_code, created_at DESC)",
        "CREATE INDEX IF NOT EXISTS idx_files_parent ON files (parent_id)",
        "CREATE INDEX IF NOT EXISTS idx_files_bundle ON files (bundle_id)",
        "CREATE INDEX IF NOT EXISTS idx_files_path ON files (path)",
        "CREATE INDEX IF NOT EXISTS idx_logs_bundle_timeline ON log_segments (bundle_id, timeline)",
        "CREATE INDEX IF NOT EXISTS idx_logs_file_chunk ON log_segments (file_id, chunk_index)",
        "CREATE INDEX IF NOT EXISTS idx_line_offsets_file_line ON log_line_offsets (file_id, line_number)",
        "CREATE INDEX IF NOT EXISTS idx_temp_results_expiry ON temp_results (expires_at)",
    ];

    for statement in statements {
        sqlx::query(statement)
            .execute(pool)
            .await
            .map_err(AppError::Database)?;
    }

    ensure_trigram_fts(pool).await?;

    let has_blob_id: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('files') WHERE name = 'blob_id')",
    )
    .fetch_one(pool)
    .await
    .map_err(AppError::Database)?;
    if !has_blob_id {
        sqlx::query("ALTER TABLE files ADD COLUMN blob_id INTEGER REFERENCES blobs(id)")
            .execute(pool)
            .await
            .map_err(AppError::Database)?;
    }
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_files_blob ON files (blob_id)")
        .execute(pool)
        .await
        .map_err(AppError::Database)?;

    let has_failure_reason: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('bundles') WHERE name = 'failure_reason')",
    )
    .fetch_one(pool)
    .await
    .map_err(AppError::Database)?;
    if !has_failure_reason {
        sqlx::query("ALTER TABLE bundles ADD COLUMN failure_reason TEXT")
            .execute(pool)
            .await
            .map_err(AppError::Database)?;
    }
    for (column, definition) in [
        ("failure_stage", "TEXT"),
        ("failure_code", "TEXT"),
        ("retryable", "INTEGER"),
        ("deleted_at", "TEXT"),
    ] {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('bundles') WHERE name = ?)",
        )
        .bind(column)
        .fetch_one(pool)
        .await
        .map_err(AppError::Database)?;
        if !exists {
            sqlx::query(&format!(
                "ALTER TABLE bundles ADD COLUMN {column} {definition}"
            ))
            .execute(pool)
            .await
            .map_err(AppError::Database)?;
        }
    }
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_bundles_deleted ON bundles (deleted_at)")
        .execute(pool)
        .await
        .map_err(AppError::Database)?;
    let has_blob_unreferenced_at: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('blobs') WHERE name = 'unreferenced_at')",
    )
    .fetch_one(pool)
    .await
    .map_err(AppError::Database)?;
    if !has_blob_unreferenced_at {
        sqlx::query("ALTER TABLE blobs ADD COLUMN unreferenced_at TEXT")
            .execute(pool)
            .await
            .map_err(AppError::Database)?;
    }

    let has_content_size: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('bundles') WHERE name = 'content_size_bytes')",
    )
    .fetch_one(pool)
    .await
    .map_err(AppError::Database)?;
    if !has_content_size {
        sqlx::query(
            "ALTER TABLE bundles ADD COLUMN content_size_bytes INTEGER NOT NULL DEFAULT 0 CHECK (content_size_bytes >= 0)",
        )
        .execute(pool)
        .await
        .map_err(AppError::Database)?;
        backfill_bundle_content_sizes(pool).await?;
    }

    Ok(())
}

async fn ensure_trigram_fts(pool: &SqlitePool) -> Result<(), AppError> {
    let schema: Option<String> = sqlx::query_scalar(
        "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'log_segments_fts'",
    )
    .fetch_optional(pool)
    .await
    .map_err(AppError::Database)?;
    let Some(schema) = schema else {
        return Err(AppError::Database(sqlx::Error::RowNotFound));
    };
    if schema.contains("tokenize='trigram'")
        && schema.contains("content='log_segments'")
        && schema.contains("content_rowid='id'")
    {
        return Ok(());
    }

    let segment_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM log_segments")
        .fetch_one(pool)
        .await
        .map_err(AppError::Database)?;
    let started = std::time::Instant::now();
    tracing::info!(
        segment_count,
        "rebuilding full-text index with trigram tokenizer"
    );
    let mut tx = pool.begin().await.map_err(AppError::Database)?;
    sqlx::query("DROP TABLE log_segments_fts")
        .execute(&mut *tx)
        .await
        .map_err(AppError::Database)?;
    sqlx::query(
        r#"
        CREATE VIRTUAL TABLE log_segments_fts USING fts5(
            content,
            content='log_segments',
            content_rowid='id',
            tokenize='trigram'
        )
        "#,
    )
    .execute(&mut *tx)
    .await
    .map_err(AppError::Database)?;
    sqlx::query(
        r#"
        INSERT INTO log_segments_fts(log_segments_fts) VALUES('rebuild')
        "#,
    )
    .execute(&mut *tx)
    .await
    .map_err(AppError::Database)?;
    tx.commit().await.map_err(AppError::Database)?;
    tracing::info!(
        segment_count,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "trigram full-text index rebuild completed"
    );
    Ok(())
}

async fn backfill_bundle_content_sizes(pool: &SqlitePool) -> Result<(), AppError> {
    sqlx::query(
        r#"
        UPDATE bundles
        SET content_size_bytes = CASE
            WHEN status = 'READY' THEN (
                SELECT COALESCE(SUM(CASE
                    WHEN COALESCE(files.size_bytes, 0) > 0
                    THEN files.size_bytes
                    ELSE 0
                END), 0)
                FROM files
                WHERE files.bundle_id = bundles.id
                  AND files.is_dir = 0
                  AND COALESCE(json_extract(files.meta, '$.preview_kind'), '') <> 'archive'
            )
            ELSE 0
        END
        "#,
    )
    .execute(pool)
    .await
    .map_err(AppError::Database)?;
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

#[cfg(test)]
mod tests {
    use super::{backfill_bundle_content_sizes, checkpoint_wal};

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
    async fn backfill_counts_only_final_browsable_files() {
        let pool = super::init_pool("sqlite::memory:").expect("init pool");
        super::prepare_schema(&pool, true)
            .await
            .expect("prepare schema");
        sqlx::query("INSERT INTO issues (code, name) VALUES ('QUOTA', 'Quota')")
            .execute(&pool)
            .await
            .expect("insert issue");
        sqlx::query(
            "INSERT INTO bundles (id, issue_code, hash, name, status) VALUES ('bundle', 'QUOTA', 'hash', 'bundle', 'READY')",
        )
        .execute(&pool)
        .await
        .expect("insert bundle");
        for (name, path, is_dir, size, meta) in [
            (
                "direct.log",
                "/direct.log",
                false,
                Some(10_i64),
                r#"{"preview_kind":"text"}"#,
            ),
            (
                "logs.zip",
                "/logs.zip",
                false,
                Some(5_i64),
                r#"{"preview_kind":"archive"}"#,
            ),
            (
                "expanded",
                "/expanded",
                true,
                None,
                r#"{"preview_kind":"directory"}"#,
            ),
            (
                "nested.log",
                "/expanded/nested.log",
                false,
                Some(20_i64),
                r#"{"preview_kind":"text"}"#,
            ),
        ] {
            sqlx::query(
                "INSERT INTO files (bundle_id, name, path, is_dir, size_bytes, meta) VALUES ('bundle', ?, ?, ?, ?, ?)",
            )
            .bind(name)
            .bind(path)
            .bind(is_dir)
            .bind(size)
            .bind(meta)
            .execute(&pool)
            .await
            .expect("insert file");
        }

        backfill_bundle_content_sizes(&pool)
            .await
            .expect("backfill content sizes");

        let content_size: i64 =
            sqlx::query_scalar("SELECT content_size_bytes FROM bundles WHERE id = 'bundle'")
                .fetch_one(&pool)
                .await
                .expect("load content size");
        assert_eq!(content_size, 30);
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

    #[tokio::test]
    async fn legacy_fts_is_rebuilt_from_authoritative_segments() {
        let pool = super::init_pool("sqlite::memory:").expect("init pool");
        super::prepare_schema(&pool, true)
            .await
            .expect("prepare schema");
        sqlx::query("INSERT INTO issues (code, name) VALUES ('SEARCH', 'Search')")
            .execute(&pool)
            .await
            .expect("insert issue");
        sqlx::query(
            "INSERT INTO bundles (id, issue_code, hash, name, status) VALUES ('bundle', 'SEARCH', 'search-hash', 'Search', 'READY')",
        )
        .execute(&pool)
        .await
        .expect("insert bundle");
        let file_id: i64 = sqlx::query_scalar(
            "INSERT INTO files (bundle_id, name, path, is_dir) VALUES ('bundle', 'app.log', '/app.log', 0) RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .expect("insert file");
        sqlx::query("INSERT INTO log_segments (bundle_id, file_id, content) VALUES ('bundle', ?, 'prefix-abcdef-suffix')")
            .bind(file_id)
            .execute(&pool)
            .await
            .expect("insert segment");
        sqlx::query("DROP TABLE log_segments_fts")
            .execute(&pool)
            .await
            .expect("drop trigram fts");
        sqlx::query(
            "CREATE VIRTUAL TABLE log_segments_fts USING fts5(content, segment_id UNINDEXED, bundle_id UNINDEXED, file_id UNINDEXED, timeline UNINDEXED)",
        )
        .execute(&pool)
        .await
        .expect("create legacy fts");

        super::prepare_schema(&pool, false)
            .await
            .expect("migrate schema");

        let schema: String = sqlx::query_scalar(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'log_segments_fts'",
        )
        .fetch_one(&pool)
        .await
        .expect("load migrated fts schema");
        assert!(schema.contains("content='log_segments'"), "{schema}");
        assert!(schema.contains("content_rowid='id'"), "{schema}");

        let matches: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM log_segments_fts WHERE log_segments_fts MATCH 'cdef'",
        )
        .fetch_one(&pool)
        .await
        .expect("search rebuilt fts");
        assert_eq!(matches, 1);
    }
}
