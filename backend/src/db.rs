use sqlx::{PgPool, postgres::PgPoolOptions};

use crate::error::AppError;

pub fn init_pool(database_url: &str, schema: &str) -> Result<PgPool, AppError> {
    let schema_ident = format_schema_identifier(schema)?;
    let create_schema_sql = format!("CREATE SCHEMA IF NOT EXISTS {}", schema_ident);
    let set_search_path_sql = format!("SET search_path TO {}, public", schema_ident);

    PgPoolOptions::new()
        .max_connections(5)
        .after_connect({
            let create_schema_sql = create_schema_sql.clone();
            let set_search_path_sql = set_search_path_sql.clone();
            move |conn, _meta| {
                let create_schema_sql = create_schema_sql.clone();
                let set_search_path_sql = set_search_path_sql.clone();
                Box::pin(async move {
                    sqlx::query(&create_schema_sql).execute(&mut *conn).await?;
                    sqlx::query(&set_search_path_sql)
                        .execute(&mut *conn)
                        .await?;
                    Ok(())
                })
            }
        })
        .connect_lazy(database_url)
        .map_err(AppError::Database)
}

pub async fn prepare_schema(pool: &PgPool, reset: bool, schema: &str) -> Result<(), AppError> {
    if reset {
        reset_schema(pool, schema).await?;
    }
    create_schema(pool, schema).await
}

async fn reset_schema(pool: &PgPool, schema: &str) -> Result<(), AppError> {
    let schema_ident = format_schema_identifier(schema)?;
    let type_name = format!("{}.upload_status", schema_ident);
    let drop_statements = [
        format!("DROP TABLE IF EXISTS {}.log_segments", schema_ident),
        format!("DROP TABLE IF EXISTS {}.files", schema_ident),
        format!("DROP TABLE IF EXISTS {}.bundles", schema_ident),
        format!("DROP TABLE IF EXISTS {}.issues", schema_ident),
    ];

    let mut conn = pool.acquire().await.map_err(AppError::Database)?;

    for statement in drop_statements {
        sqlx::query(&statement)
            .execute(&mut *conn)
            .await
            .map_err(AppError::Database)?;
    }

    sqlx::query(&format!("DROP TYPE IF EXISTS {}", type_name))
        .execute(&mut *conn)
        .await
        .map_err(AppError::Database)?;

    Ok(())
}

async fn create_schema(pool: &PgPool, schema: &str) -> Result<(), AppError> {
    let schema_ident = format_schema_identifier(schema)?;
    let type_name = format!("{}.upload_status", schema_ident);
    let mut conn = pool.acquire().await.map_err(AppError::Database)?;
    sqlx::query(&format!("CREATE SCHEMA IF NOT EXISTS {}", schema_ident))
        .execute(&mut *conn)
        .await
        .map_err(AppError::Database)?;
    sqlx::query(&format!("SET search_path TO {}, public", schema_ident))
        .execute(&mut *conn)
        .await
        .map_err(AppError::Database)?;

    let statements = [
        r#"CREATE EXTENSION IF NOT EXISTS "pgcrypto""#,
        r#"CREATE EXTENSION IF NOT EXISTS "pg_trgm""#,
        &format!(
            "
        DO $$
        BEGIN
            IF NOT EXISTS (
                SELECT 1 FROM pg_type t
                JOIN pg_namespace n ON n.oid = t.typnamespace
                WHERE t.typname = 'upload_status' AND n.nspname = '{}'
            ) THEN
                CREATE TYPE {} AS ENUM ('READY', 'PROCESSING', 'FAILED', 'PENDING');
            END IF;
        END
        $$;
        ",
            schema, type_name
        ),
        &format!(
            "
        CREATE TABLE IF NOT EXISTS {}.issues (
            code TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            description TEXT,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        )
        ",
            schema_ident
        ),
        &format!(
            "
        CREATE TABLE IF NOT EXISTS {}.bundles (
            id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
            issue_code TEXT NOT NULL REFERENCES {}.issues(code) ON DELETE CASCADE,
            hash TEXT NOT NULL UNIQUE,
            name TEXT NOT NULL,
            status {} NOT NULL DEFAULT 'PENDING',
            size_bytes BIGINT,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        )
        ",
            schema_ident, schema_ident, type_name
        ),
        &format!(
            "
        CREATE TABLE IF NOT EXISTS {}.files (
            id BIGSERIAL PRIMARY KEY,
            bundle_id UUID NOT NULL REFERENCES {}.bundles(id) ON DELETE CASCADE,
            parent_id BIGINT REFERENCES {}.files(id) ON DELETE CASCADE,
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
        ",
            schema_ident, schema_ident, schema_ident
        ),
        &format!(
            "
        CREATE TABLE IF NOT EXISTS {}.log_segments (
            id BIGSERIAL PRIMARY KEY,
            bundle_id UUID NOT NULL REFERENCES {}.bundles(id) ON DELETE CASCADE,
            file_id BIGINT REFERENCES {}.files(id) ON DELETE SET NULL,
            timeline TEXT,
            content TEXT NOT NULL,
            line_offset BIGINT,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            tsv tsvector GENERATED ALWAYS AS (
                to_tsvector('simple', coalesce(content, ''))
            ) STORED
        )
        ",
            schema_ident, schema_ident, schema_ident
        ),
        &format!(
            "CREATE INDEX IF NOT EXISTS idx_bundles_issue ON {}.bundles (issue_code, created_at DESC)",
            schema_ident
        ),
        &format!(
            "CREATE INDEX IF NOT EXISTS idx_files_parent ON {}.files (parent_id)",
            schema_ident
        ),
        &format!(
            "CREATE INDEX IF NOT EXISTS idx_files_bundle ON {}.files (bundle_id)",
            schema_ident
        ),
        &format!(
            "CREATE INDEX IF NOT EXISTS idx_logs_bundle_timeline ON {}.log_segments (bundle_id, timeline)",
            schema_ident
        ),
        &format!(
            "CREATE INDEX IF NOT EXISTS idx_logs_tsv ON {}.log_segments USING GIN (tsv)",
            schema_ident
        ),
        &format!(
            "CREATE INDEX IF NOT EXISTS idx_files_path_trgm ON {}.files USING GIN (path gin_trgm_ops)",
            schema_ident
        ),
    ];

    for statement in statements {
        sqlx::query(statement)
            .execute(&mut *conn)
            .await
            .map_err(AppError::Database)?;
    }

    Ok(())
}

fn format_schema_identifier(schema: &str) -> Result<String, AppError> {
    if schema.is_empty()
        || !schema
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        return Err(AppError::Config(format!(
            "invalid DATABASE_SCHEMA: {schema}"
        )));
    }
    Ok(format!("\"{}\"", schema))
}
