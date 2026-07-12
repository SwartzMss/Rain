use std::{path::Path, str::FromStr};

use sqlx::{
    SqlitePool,
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

async fn reset_schema(pool: &SqlitePool) -> Result<(), AppError> {
    let statements = [
        "DROP TABLE IF EXISTS log_segments_fts",
        "DROP TABLE IF EXISTS log_events",
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
    ];

    for statement in statements {
        sqlx::query(statement)
            .execute(pool)
            .await
            .map_err(AppError::Database)?;
    }

    ensure_log_segment_column(pool, "line_end", "INTEGER").await?;
    ensure_log_segment_column(pool, "chunk_index", "INTEGER").await?;

    Ok(())
}

async fn ensure_log_segment_column(
    pool: &SqlitePool,
    column: &str,
    definition: &str,
) -> Result<(), AppError> {
    let exists: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM pragma_table_info('log_segments')
            WHERE name = ?
        )
        "#,
    )
    .bind(column)
    .fetch_one(pool)
    .await
    .map_err(AppError::Database)?;

    if !exists {
        let statement = format!("ALTER TABLE log_segments ADD COLUMN {column} {definition}");
        sqlx::query(&statement)
            .execute(pool)
            .await
            .map_err(AppError::Database)?;
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
