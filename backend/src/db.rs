use sqlx::{PgPool, postgres::PgPoolOptions};

use crate::error::AppError;

pub fn init_pool(database_url: &str) -> Result<PgPool, AppError> {
    PgPoolOptions::new()
        .max_connections(5)
        .connect_lazy(database_url)
        .map_err(AppError::Database)
}

pub async fn prepare_schema(pool: &PgPool, reset: bool) -> Result<(), AppError> {
    if reset {
        reset_schema(pool).await?;
    }
    create_schema(pool).await
}

async fn reset_schema(pool: &PgPool) -> Result<(), AppError> {
    let drop_statements = [
        "DROP TABLE IF EXISTS log_segments",
        "DROP TABLE IF EXISTS files",
        "DROP TABLE IF EXISTS bundles",
        "DROP TABLE IF EXISTS issues",
    ];

    for statement in drop_statements {
        sqlx::query(statement)
            .execute(pool)
            .await
            .map_err(AppError::Database)?;
    }

    sqlx::query("DROP TYPE IF EXISTS upload_status")
        .execute(pool)
        .await
        .map_err(AppError::Database)?;

    Ok(())
}

async fn create_schema(pool: &PgPool) -> Result<(), AppError> {
    let statements = [
        r#"CREATE EXTENSION IF NOT EXISTS "pgcrypto""#,
        r#"
        DO $$
        BEGIN
            IF NOT EXISTS (SELECT 1 FROM pg_type WHERE typname = 'upload_status') THEN
                CREATE TYPE upload_status AS ENUM ('READY', 'PROCESSING', 'FAILED', 'PENDING');
            END IF;
        END
        $$;
        "#,
        r#"
        CREATE TABLE IF NOT EXISTS issues (
            code TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            description TEXT,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        )
        "#,
        r#"
        CREATE TABLE IF NOT EXISTS bundles (
            id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
            issue_code TEXT NOT NULL REFERENCES issues(code) ON DELETE CASCADE,
            hash TEXT NOT NULL UNIQUE,
            name TEXT NOT NULL,
            status upload_status NOT NULL DEFAULT 'PENDING',
            size_bytes BIGINT,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        )
        "#,
        r#"
        CREATE TABLE IF NOT EXISTS files (
            id BIGSERIAL PRIMARY KEY,
            bundle_id UUID NOT NULL REFERENCES bundles(id) ON DELETE CASCADE,
            parent_id BIGINT REFERENCES files(id) ON DELETE CASCADE,
            name TEXT NOT NULL,
            path TEXT NOT NULL,
            is_dir BOOLEAN NOT NULL,
            size_bytes BIGINT,
            mime_type TEXT,
            status TEXT,
            meta JSONB,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            CONSTRAINT files_bundle_path UNIQUE (bundle_id, path)
        )
        "#,
        r#"
        CREATE TABLE IF NOT EXISTS log_segments (
            id BIGSERIAL PRIMARY KEY,
            bundle_id UUID NOT NULL REFERENCES bundles(id) ON DELETE CASCADE,
            file_id BIGINT REFERENCES files(id) ON DELETE SET NULL,
            timeline TEXT,
            content TEXT NOT NULL,
            offset BIGINT,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            tsv tsvector GENERATED ALWAYS AS (
                to_tsvector('simple', coalesce(content, ''))
            ) STORED
        )
        "#,
        "CREATE INDEX IF NOT EXISTS idx_bundles_issue ON bundles (issue_code, created_at DESC)",
        "CREATE INDEX IF NOT EXISTS idx_files_parent ON files (parent_id)",
        "CREATE INDEX IF NOT EXISTS idx_files_bundle ON files (bundle_id)",
        "CREATE INDEX IF NOT EXISTS idx_logs_bundle_timeline ON log_segments (bundle_id, timeline)",
        "CREATE INDEX IF NOT EXISTS idx_logs_tsv ON log_segments USING GIN (tsv)",
    ];

    for statement in statements {
        sqlx::query(statement)
            .execute(pool)
            .await
            .map_err(AppError::Database)?;
    }

    Ok(())
}
