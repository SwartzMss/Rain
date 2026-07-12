use std::{path::Path, str::FromStr};

use sqlx::{
    FromRow, SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};

use crate::error::AppError;

pub fn init_pool(database_url: &str) -> Result<SqlitePool, AppError> {
    ensure_sqlite_parent(database_url)?;

    let options = SqliteConnectOptions::from_str(database_url)
        .map_err(AppError::Database)?
        .create_if_missing(true)
        .foreign_keys(true);

    Ok(SqlitePoolOptions::new()
        .max_connections(5)
        .connect_lazy_with(options))
}

pub async fn prepare_schema(pool: &SqlitePool, reset: bool) -> Result<(), AppError> {
    if reset {
        reset_schema(pool).await?;
    }
    create_schema(pool).await
}

pub async fn cleanup_expired_bundles(
    pool: &SqlitePool,
    data_root: &Path,
    retention_days: u64,
) -> Result<u64, AppError> {
    let cutoff = format!("-{retention_days} days");
    let bundles = sqlx::query_as::<_, ExpiredBundle>(
        r#"
        SELECT id, hash
        FROM bundles
        WHERE datetime(created_at) < datetime('now', ?)
        "#,
    )
    .bind(cutoff)
    .fetch_all(pool)
    .await
    .map_err(AppError::Database)?;

    if bundles.is_empty() {
        return Ok(0);
    }

    let mut tx = pool.begin().await.map_err(AppError::Database)?;
    for bundle in &bundles {
        sqlx::query(
            "DELETE FROM log_line_offsets WHERE file_id IN (SELECT id FROM files WHERE bundle_id = ?)",
        )
        .bind(&bundle.id)
        .execute(&mut *tx)
        .await
        .map_err(AppError::Database)?;

        sqlx::query("DELETE FROM log_events WHERE bundle_id = ?")
            .bind(&bundle.id)
            .execute(&mut *tx)
            .await
            .map_err(AppError::Database)?;

        sqlx::query("DELETE FROM log_segments_fts WHERE bundle_id = ?")
            .bind(&bundle.id)
            .execute(&mut *tx)
            .await
            .map_err(AppError::Database)?;

        sqlx::query("DELETE FROM log_segments WHERE bundle_id = ?")
            .bind(&bundle.id)
            .execute(&mut *tx)
            .await
            .map_err(AppError::Database)?;

        sqlx::query("DELETE FROM files WHERE bundle_id = ?")
            .bind(&bundle.id)
            .execute(&mut *tx)
            .await
            .map_err(AppError::Database)?;

        sqlx::query("DELETE FROM bundles WHERE id = ?")
            .bind(&bundle.id)
            .execute(&mut *tx)
            .await
            .map_err(AppError::Database)?;
    }

    sqlx::query(
        r#"
        DELETE FROM issues
        WHERE NOT EXISTS (
            SELECT 1 FROM bundles WHERE bundles.issue_code = issues.code
        )
        "#,
    )
    .execute(&mut *tx)
    .await
    .map_err(AppError::Database)?;

    tx.commit().await.map_err(AppError::Database)?;

    for bundle in &bundles {
        let bundle_dir = data_root.join(&bundle.hash);
        if tokio::fs::metadata(&bundle_dir).await.is_ok() {
            let _ = tokio::fs::remove_dir_all(&bundle_dir).await;
        }
    }

    Ok(bundles.len() as u64)
}

#[derive(FromRow)]
struct ExpiredBundle {
    id: String,
    hash: String,
}

async fn reset_schema(pool: &SqlitePool) -> Result<(), AppError> {
    let statements = [
        "DROP TABLE IF EXISTS log_segments_fts",
        "DROP TABLE IF EXISTS log_events",
        "DROP TABLE IF EXISTS log_line_offsets",
        "DROP TABLE IF EXISTS log_segments",
        "DROP TABLE IF EXISTS files",
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
            size_bytes INTEGER,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        )
        "#,
        r#"
        CREATE TABLE IF NOT EXISTS files (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            bundle_id TEXT NOT NULL REFERENCES bundles(id) ON DELETE CASCADE,
            parent_id INTEGER REFERENCES files(id) ON DELETE CASCADE,
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
        r#"
        CREATE TABLE IF NOT EXISTS log_events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            bundle_id TEXT NOT NULL REFERENCES bundles(id) ON DELETE CASCADE,
            file_id INTEGER REFERENCES files(id) ON DELETE CASCADE,
            segment_id INTEGER REFERENCES log_segments(id) ON DELETE CASCADE,
            line_number INTEGER,
            timestamp TEXT,
            level TEXT,
            component TEXT,
            message TEXT NOT NULL,
            raw TEXT NOT NULL,
            parser_name TEXT NOT NULL,
            parser_confidence REAL NOT NULL,
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
        CREATE VIRTUAL TABLE IF NOT EXISTS log_segments_fts USING fts5(
            content,
            segment_id UNINDEXED,
            bundle_id UNINDEXED,
            file_id UNINDEXED,
            timeline UNINDEXED
        )
        "#,
        r#"
        INSERT INTO log_segments_fts (content, segment_id, bundle_id, file_id, timeline)
        SELECT ls.content, ls.id, ls.bundle_id, ls.file_id, ls.timeline
        FROM log_segments ls
        WHERE NOT EXISTS (
            SELECT 1 FROM log_segments_fts fts WHERE fts.segment_id = ls.id
        )
        "#,
        "CREATE INDEX IF NOT EXISTS idx_bundles_issue ON bundles (issue_code, created_at DESC)",
        "CREATE INDEX IF NOT EXISTS idx_files_parent ON files (parent_id)",
        "CREATE INDEX IF NOT EXISTS idx_files_bundle ON files (bundle_id)",
        "CREATE INDEX IF NOT EXISTS idx_files_path ON files (path)",
        "CREATE INDEX IF NOT EXISTS idx_logs_bundle_timeline ON log_segments (bundle_id, timeline)",
        "CREATE INDEX IF NOT EXISTS idx_logs_file_chunk ON log_segments (file_id, chunk_index)",
        "CREATE INDEX IF NOT EXISTS idx_events_bundle_level ON log_events (bundle_id, level)",
        "CREATE INDEX IF NOT EXISTS idx_events_file_line ON log_events (file_id, line_number)",
        "CREATE INDEX IF NOT EXISTS idx_line_offsets_file_line ON log_line_offsets (file_id, line_number)",
    ];

    for statement in statements {
        sqlx::query(statement)
            .execute(pool)
            .await
            .map_err(AppError::Database)?;
    }

    ensure_log_segment_column(pool, "line_end", "INTEGER").await?;
    ensure_log_segment_column(pool, "chunk_index", "INTEGER").await?;
    ensure_table_column(pool, "files", "line_count", "INTEGER").await?;

    Ok(())
}

async fn ensure_table_column(
    pool: &SqlitePool,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<(), AppError> {
    let rows = sqlx::query(&format!("PRAGMA table_info({table})"))
        .fetch_all(pool)
        .await
        .map_err(AppError::Database)?;
    let exists = rows.iter().any(|row| {
        use sqlx::Row;
        row.try_get::<String, _>("name")
            .map(|name| name == column)
            .unwrap_or(false)
    });

    if !exists {
        let statement = format!("ALTER TABLE {table} ADD COLUMN {column} {definition}");
        sqlx::query(&statement)
            .execute(pool)
            .await
            .map_err(AppError::Database)?;
    }

    Ok(())
}

async fn ensure_log_segment_column(
    pool: &SqlitePool,
    column: &str,
    definition: &str,
) -> Result<(), AppError> {
    ensure_table_column(pool, "log_segments", column, definition).await
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
