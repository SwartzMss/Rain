use std::borrow::Cow;

use sqlx::{Row, SqlitePool, migrate::Migrator, sqlite::SqlitePoolOptions};

use crate::error::AppError;

pub(crate) static MIGRATOR: Migrator = sqlx::migrate!("./migrations");
const LEGACY_EVENT_TIME_BACKFILL_BATCH_SIZE: i64 = 500;
const LEGACY_BASELINE_VERSION: i64 = 1;

macro_rules! col {
    ($name:literal, $type_name:literal, $not_null:literal, $default:expr) => {
        ColumnRequirement {
            name: $name,
            type_name: $type_name,
            not_null: $not_null,
            default: $default,
        }
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DatabaseState {
    Empty,
    Legacy,
    Managed,
}

#[derive(Debug, Clone, Copy)]
struct ColumnRequirement {
    name: &'static str,
    type_name: &'static str,
    not_null: bool,
    default: Option<&'static str>,
}

#[derive(Debug, Clone, Copy)]
struct IndexRequirement {
    table: &'static str,
    name: &'static str,
    columns: &'static [&'static str],
    unique: bool,
    partial_sql: Option<&'static str>,
}

const USERS_COLUMNS: &[ColumnRequirement] = &[
    col!("id", "TEXT", false, None),
    col!("username", "TEXT", true, None),
    col!("username_normalized", "TEXT", true, None),
    col!("password_hash", "TEXT", true, None),
    col!("status", "TEXT", true, Some("'ACTIVE'")),
    col!("role", "TEXT", true, Some("'USER'")),
    col!("created_at", "TEXT", true, Some("CURRENT_TIMESTAMP")),
    col!("updated_at", "TEXT", true, Some("CURRENT_TIMESTAMP")),
    col!("last_login_at", "TEXT", false, None),
    col!("password_changed_at", "TEXT", false, None),
];

const SYSTEM_SETTINGS_COLUMNS: &[ColumnRequirement] = &[
    col!("id", "INTEGER", false, None),
    col!("allow_registration", "INTEGER", true, None),
    col!("updated_by_user_id", "TEXT", false, None),
    col!("updated_at", "TEXT", true, Some("CURRENT_TIMESTAMP")),
    col!("login_ip_limit_per_minute", "INTEGER", true, Some("20")),
    col!(
        "login_username_failure_limit_per_5_minutes",
        "INTEGER",
        true,
        Some("10")
    ),
    col!("issue_inactive_days", "INTEGER", true, Some("0")),
];

const ADMIN_AUDIT_LOGS_COLUMNS: &[ColumnRequirement] = &[
    col!("id", "TEXT", false, None),
    col!("actor_type", "TEXT", true, None),
    col!("actor_user_id", "TEXT", false, None),
    col!("target_user_id", "TEXT", false, None),
    col!("action", "TEXT", true, None),
    col!("old_value", "TEXT", false, None),
    col!("new_value", "TEXT", false, None),
    col!("client_ip", "TEXT", false, None),
    col!("user_agent", "TEXT", false, None),
    col!("created_at", "TEXT", true, Some("CURRENT_TIMESTAMP")),
];

const USER_SESSIONS_COLUMNS: &[ColumnRequirement] = &[
    col!("id", "TEXT", false, None),
    col!("user_id", "TEXT", true, None),
    col!("token_hash", "TEXT", true, None),
    col!("created_at", "TEXT", true, Some("CURRENT_TIMESTAMP")),
    col!("last_seen_at", "TEXT", true, Some("CURRENT_TIMESTAMP")),
    col!("expires_at", "TEXT", true, None),
    col!("revoked_at", "TEXT", false, None),
    col!("user_agent", "TEXT", false, None),
    col!("client_ip", "TEXT", false, None),
];

const SAVED_SEARCHES_COLUMNS: &[ColumnRequirement] = &[
    col!("id", "TEXT", false, None),
    col!("user_id", "TEXT", true, None),
    col!("name", "TEXT", true, None),
    col!("search_type", "TEXT", true, None),
    col!("query_text", "TEXT", true, None),
    col!("scope_type", "TEXT", true, Some("'GLOBAL'")),
    col!("scope_key", "TEXT", false, None),
    col!("options_json", "TEXT", true, Some("'{}'")),
    col!("is_pinned", "INTEGER", true, Some("0")),
    col!("sort_order", "INTEGER", true, Some("0")),
    col!("created_at", "TEXT", true, Some("CURRENT_TIMESTAMP")),
    col!("updated_at", "TEXT", true, Some("CURRENT_TIMESTAMP")),
    col!("last_used_at", "TEXT", false, None),
];

const ISSUES_COLUMNS: &[ColumnRequirement] = &[
    col!("code", "TEXT", false, None),
    col!("name", "TEXT", true, None),
    col!("description", "TEXT", false, None),
    col!("owner_user_id", "TEXT", false, None),
    col!("status", "TEXT", true, Some("'ACTIVE'")),
    col!("created_at", "TEXT", true, Some("CURRENT_TIMESTAMP")),
    col!("last_activity_at", "TEXT", true, Some("CURRENT_TIMESTAMP")),
    col!("deletion_reason", "TEXT", false, None),
    col!("inactive_claim_days", "INTEGER", false, None),
    col!("deletion_lease_token", "TEXT", false, None),
    col!("deletion_lease_until", "TEXT", false, None),
    col!("deletion_retry_at", "TEXT", false, None),
    col!("deletion_attempts", "INTEGER", true, Some("0")),
];

const BUNDLES_COLUMNS: &[ColumnRequirement] = &[
    col!("id", "TEXT", false, None),
    col!("issue_code", "TEXT", true, None),
    col!("hash", "TEXT", true, None),
    col!("name", "TEXT", true, None),
    col!("status", "TEXT", true, Some("'PENDING'")),
    col!("process_stage", "TEXT", true, Some("'PENDING'")),
    col!("failure_stage", "TEXT", false, None),
    col!("failure_code", "TEXT", false, None),
    col!("failure_reason", "TEXT", false, None),
    col!("retryable", "INTEGER", false, None),
    col!("deleted_at", "TEXT", false, None),
    col!("uploader_user_id", "TEXT", false, None),
    col!("size_bytes", "INTEGER", false, None),
    col!("content_size_bytes", "INTEGER", true, Some("0")),
    col!("created_at", "TEXT", true, Some("CURRENT_TIMESTAMP")),
];

const BLOBS_COLUMNS: &[ColumnRequirement] = &[
    col!("id", "INTEGER", false, None),
    col!("content_hash", "TEXT", true, None),
    col!("size_bytes", "INTEGER", true, None),
    col!("storage_backend", "TEXT", true, None),
    col!("storage_key", "TEXT", true, None),
    col!("state", "TEXT", true, None),
    col!("last_attempt_at", "TEXT", false, None),
    col!("unreferenced_at", "TEXT", false, None),
    col!("verified_at", "TEXT", false, None),
];

const FILES_COLUMNS: &[ColumnRequirement] = &[
    col!("id", "INTEGER", false, None),
    col!("bundle_id", "TEXT", true, None),
    col!("parent_id", "INTEGER", false, None),
    col!("blob_id", "INTEGER", false, None),
    col!("name", "TEXT", true, None),
    col!("path", "TEXT", true, None),
    col!("is_dir", "INTEGER", true, None),
    col!("size_bytes", "INTEGER", false, None),
    col!("line_count", "INTEGER", false, None),
    col!("mime_type", "TEXT", false, None),
    col!("status", "TEXT", false, None),
    col!("meta", "TEXT", false, None),
    col!("created_at", "TEXT", true, Some("CURRENT_TIMESTAMP")),
];

const LOG_SEGMENTS_COLUMNS: &[ColumnRequirement] = &[
    col!("id", "INTEGER", false, None),
    col!("bundle_id", "TEXT", true, None),
    col!("file_id", "INTEGER", false, None),
    col!("timeline", "TEXT", false, None),
    col!("content", "TEXT", true, None),
    col!("line_offset", "INTEGER", false, None),
    col!("line_end", "INTEGER", false, None),
    col!("chunk_index", "INTEGER", false, None),
    col!("event_time_start_ms", "INTEGER", false, None),
    col!("event_time_end_ms", "INTEGER", false, None),
    col!("event_time_indexed", "INTEGER", true, Some("0")),
    col!("created_at", "TEXT", true, Some("CURRENT_TIMESTAMP")),
];

const LOG_LINE_OFFSETS_COLUMNS: &[ColumnRequirement] = &[
    col!("file_id", "INTEGER", true, None),
    col!("line_number", "INTEGER", true, None),
    col!("byte_offset", "INTEGER", true, None),
];

const TEMP_RESULTS_COLUMNS: &[ColumnRequirement] = &[
    col!("id", "TEXT", false, None),
    col!("status", "TEXT", true, Some("'ACTIVE'")),
    col!("name", "TEXT", true, None),
    col!("expression", "TEXT", true, None),
    col!("source_label", "TEXT", true, None),
    col!("storage_path", "TEXT", true, None),
    col!("line_count", "INTEGER", true, None),
    col!("size_bytes", "INTEGER", true, None),
    col!("created_at", "TEXT", true, None),
    col!("expires_at", "TEXT", true, None),
];

const USER_SKILLS_COLUMNS: &[ColumnRequirement] = &[
    col!("id", "TEXT", false, None),
    col!("owner_user_id", "TEXT", true, None),
    col!("name", "TEXT", true, None),
    col!("description", "TEXT", false, None),
    col!("skill_markdown", "TEXT", true, None),
    col!("content_hash", "TEXT", true, None),
    col!("version", "INTEGER", true, Some("1")),
    col!("enabled", "INTEGER", true, Some("1")),
    col!("created_at", "TEXT", true, Some("CURRENT_TIMESTAMP")),
    col!("updated_at", "TEXT", true, Some("CURRENT_TIMESTAMP")),
];

const SKILL_REVIEWS_COLUMNS: &[ColumnRequirement] = &[
    col!("skill_id", "TEXT", false, None),
    col!("skill_version", "INTEGER", true, None),
    col!("skill_content_hash", "TEXT", true, None),
    col!("reviewer_model", "TEXT", true, None),
    col!("rubric_version", "TEXT", true, None),
    col!("overall_score", "INTEGER", true, None),
    col!("grade", "TEXT", true, None),
    col!("dimension_scores_json", "TEXT", true, None),
    col!("findings_json", "TEXT", true, None),
    col!("evaluated_at", "TEXT", true, Some("CURRENT_TIMESTAMP")),
];

const AI_PROVIDER_SETTINGS_COLUMNS: &[ColumnRequirement] = &[
    col!("id", "INTEGER", false, None),
    col!("base_url", "TEXT", true, None),
    col!("encrypted_api_key", "TEXT", true, None),
    col!("model", "TEXT", true, None),
    col!("request_timeout_seconds", "INTEGER", true, None),
    col!("updated_by_user_id", "TEXT", false, None),
    col!("updated_at", "TEXT", true, Some("CURRENT_TIMESTAMP")),
];

const SKILL_RUNS_COLUMNS: &[ColumnRequirement] = &[
    col!("id", "TEXT", false, None),
    col!("user_id", "TEXT", true, None),
    col!("issue_code", "TEXT", true, None),
    col!("skill_id", "TEXT", true, None),
    col!("skill_version", "INTEGER", true, None),
    col!("skill_name", "TEXT", true, None),
    col!("skill_snapshot_markdown", "TEXT", true, None),
    col!("status", "TEXT", true, None),
    col!("iteration_count", "INTEGER", true, Some("0")),
    col!("tool_call_count", "INTEGER", true, Some("0")),
    col!("cancel_requested", "INTEGER", true, Some("0")),
    col!("result_json", "TEXT", false, None),
    col!("error_code", "TEXT", false, None),
    col!("error_message", "TEXT", false, None),
    col!("started_at", "TEXT", false, None),
    col!("completed_at", "TEXT", false, None),
    col!("analysis_start_time", "TEXT", false, None),
    col!("analysis_end_time", "TEXT", false, None),
    col!("analysis_start_ms", "INTEGER", false, None),
    col!("analysis_end_ms", "INTEGER", false, None),
    col!("created_at", "TEXT", true, Some("CURRENT_TIMESTAMP")),
];

const SKILL_RUN_STEPS_COLUMNS: &[ColumnRequirement] = &[
    col!("id", "TEXT", false, None),
    col!("run_id", "TEXT", true, None),
    col!("sequence", "INTEGER", true, None),
    col!("iteration", "INTEGER", true, None),
    col!("tool_name", "TEXT", false, None),
    col!("arguments_summary", "TEXT", false, None),
    col!("hit_count", "INTEGER", false, None),
    col!("evidence_json", "TEXT", false, None),
    col!("elapsed_ms", "INTEGER", true, Some("0")),
    col!("status", "TEXT", true, None),
    col!("created_at", "TEXT", true, Some("CURRENT_TIMESTAMP")),
];

const RAIN_READY_PROBE_COLUMNS: &[ColumnRequirement] = &[
    col!("id", "TEXT", false, None),
    col!("value", "INTEGER", true, None),
];

const REQUIRED_TABLES: &[(&str, &[ColumnRequirement])] = &[
    ("users", USERS_COLUMNS),
    ("system_settings", SYSTEM_SETTINGS_COLUMNS),
    ("admin_audit_logs", ADMIN_AUDIT_LOGS_COLUMNS),
    ("user_sessions", USER_SESSIONS_COLUMNS),
    ("saved_searches", SAVED_SEARCHES_COLUMNS),
    ("issues", ISSUES_COLUMNS),
    ("bundles", BUNDLES_COLUMNS),
    ("blobs", BLOBS_COLUMNS),
    ("files", FILES_COLUMNS),
    ("log_segments", LOG_SEGMENTS_COLUMNS),
    ("log_line_offsets", LOG_LINE_OFFSETS_COLUMNS),
    ("temp_results", TEMP_RESULTS_COLUMNS),
    ("user_skills", USER_SKILLS_COLUMNS),
    ("skill_reviews", SKILL_REVIEWS_COLUMNS),
    ("ai_provider_settings", AI_PROVIDER_SETTINGS_COLUMNS),
    ("skill_runs", SKILL_RUNS_COLUMNS),
    ("skill_run_steps", SKILL_RUN_STEPS_COLUMNS),
    ("rain_ready_probe", RAIN_READY_PROBE_COLUMNS),
];

const IDX_USER: &[&str] = &["user_id"];
const IDX_STATUS_COMPLETED: &[&str] = &["status", "completed_at"];
const IDX_ISSUE_CREATED: &[&str] = &["issue_code", "created_at"];
const IDX_ISSUE_ACTIVITY: &[&str] = &["status", "last_activity_at"];
const IDX_PARENT: &[&str] = &["parent_id"];
const IDX_BUNDLE: &[&str] = &["bundle_id"];
const IDX_PATH: &[&str] = &["path"];
const IDX_BUNDLE_TIMELINE: &[&str] = &["bundle_id", "timeline"];
const IDX_FILE_CHUNK: &[&str] = &["file_id", "chunk_index"];
const IDX_FILE_EVENT: &[&str] = &["file_id", "event_time_start_ms", "event_time_end_ms"];
const IDX_EVENT_INDEXED: &[&str] = &["event_time_indexed", "id"];
const IDX_FILE_LINE: &[&str] = &["file_id", "line_number"];
const IDX_EXPIRES: &[&str] = &["expires_at"];
const IDX_ROLE_STATUS: &[&str] = &["role", "status", "created_at", "id"];
const IDX_AUDIT_CREATED: &[&str] = &["created_at", "id"];
const IDX_AUDIT_TARGET: &[&str] = &["target_user_id", "created_at"];
const IDX_SAVED_USER: &[&str] = &["user_id", "is_pinned", "sort_order", "updated_at"];
const IDX_BLOB: &[&str] = &["blob_id"];
const IDX_DELETED: &[&str] = &["deleted_at"];

const INDEX_DESCENDING_COLUMNS: &[(&str, &[bool])] = &[
    ("idx_bundles_issue", &[false, true]),
    ("idx_admin_audit_created", &[true, true]),
    ("idx_admin_audit_target", &[false, true]),
    ("idx_saved_searches_user", &[false, true, false, true]),
];

const REQUIRED_INDEXES: &[IndexRequirement] = &[
    IndexRequirement {
        table: "skill_runs",
        name: "idx_skill_runs_one_active_per_user",
        columns: IDX_USER,
        unique: true,
        partial_sql: Some("WHERE status IN ('QUEUED', 'RUNNING')"),
    },
    IndexRequirement {
        table: "skill_runs",
        name: "idx_skill_runs_terminal_cleanup",
        columns: IDX_STATUS_COMPLETED,
        unique: false,
        partial_sql: None,
    },
    IndexRequirement {
        table: "bundles",
        name: "idx_bundles_issue",
        columns: IDX_ISSUE_CREATED,
        unique: false,
        partial_sql: None,
    },
    IndexRequirement {
        table: "issues",
        name: "idx_issues_activity",
        columns: IDX_ISSUE_ACTIVITY,
        unique: false,
        partial_sql: None,
    },
    IndexRequirement {
        table: "files",
        name: "idx_files_parent",
        columns: IDX_PARENT,
        unique: false,
        partial_sql: None,
    },
    IndexRequirement {
        table: "files",
        name: "idx_files_bundle",
        columns: IDX_BUNDLE,
        unique: false,
        partial_sql: None,
    },
    IndexRequirement {
        table: "files",
        name: "idx_files_path",
        columns: IDX_PATH,
        unique: false,
        partial_sql: None,
    },
    IndexRequirement {
        table: "log_segments",
        name: "idx_logs_bundle_timeline",
        columns: IDX_BUNDLE_TIMELINE,
        unique: false,
        partial_sql: None,
    },
    IndexRequirement {
        table: "log_segments",
        name: "idx_logs_file_chunk",
        columns: IDX_FILE_CHUNK,
        unique: false,
        partial_sql: None,
    },
    IndexRequirement {
        table: "log_segments",
        name: "idx_logs_file_event_time",
        columns: IDX_FILE_EVENT,
        unique: false,
        partial_sql: None,
    },
    IndexRequirement {
        table: "log_segments",
        name: "idx_logs_event_time_indexed",
        columns: IDX_EVENT_INDEXED,
        unique: false,
        partial_sql: None,
    },
    IndexRequirement {
        table: "log_line_offsets",
        name: "idx_line_offsets_file_line",
        columns: IDX_FILE_LINE,
        unique: false,
        partial_sql: None,
    },
    IndexRequirement {
        table: "temp_results",
        name: "idx_temp_results_expiry",
        columns: IDX_EXPIRES,
        unique: false,
        partial_sql: None,
    },
    IndexRequirement {
        table: "user_sessions",
        name: "idx_user_sessions_user",
        columns: IDX_USER,
        unique: false,
        partial_sql: None,
    },
    IndexRequirement {
        table: "user_sessions",
        name: "idx_user_sessions_expiry",
        columns: IDX_EXPIRES,
        unique: false,
        partial_sql: None,
    },
    IndexRequirement {
        table: "users",
        name: "idx_users_role_status",
        columns: IDX_ROLE_STATUS,
        unique: false,
        partial_sql: None,
    },
    IndexRequirement {
        table: "users",
        name: "idx_users_single_admin",
        columns: &["role"],
        unique: true,
        partial_sql: Some("WHERE role = 'ADMIN'"),
    },
    IndexRequirement {
        table: "admin_audit_logs",
        name: "idx_admin_audit_created",
        columns: IDX_AUDIT_CREATED,
        unique: false,
        partial_sql: None,
    },
    IndexRequirement {
        table: "admin_audit_logs",
        name: "idx_admin_audit_target",
        columns: IDX_AUDIT_TARGET,
        unique: false,
        partial_sql: None,
    },
    IndexRequirement {
        table: "saved_searches",
        name: "idx_saved_searches_user",
        columns: IDX_SAVED_USER,
        unique: false,
        partial_sql: None,
    },
    IndexRequirement {
        table: "files",
        name: "idx_files_blob",
        columns: IDX_BLOB,
        unique: false,
        partial_sql: None,
    },
    IndexRequirement {
        table: "bundles",
        name: "idx_bundles_deleted",
        columns: IDX_DELETED,
        unique: false,
        partial_sql: None,
    },
];

const FOREIGN_KEYS: &[(&str, &str, &str, &str, &str)] = &[
    (
        "system_settings",
        "updated_by_user_id",
        "users",
        "id",
        "SET NULL",
    ),
    (
        "admin_audit_logs",
        "actor_user_id",
        "users",
        "id",
        "SET NULL",
    ),
    (
        "admin_audit_logs",
        "target_user_id",
        "users",
        "id",
        "SET NULL",
    ),
    ("user_sessions", "user_id", "users", "id", "CASCADE"),
    ("saved_searches", "user_id", "users", "id", "CASCADE"),
    ("issues", "owner_user_id", "users", "id", "SET NULL"),
    ("bundles", "issue_code", "issues", "code", "CASCADE"),
    ("bundles", "uploader_user_id", "users", "id", "SET NULL"),
    ("files", "bundle_id", "bundles", "id", "CASCADE"),
    ("files", "parent_id", "files", "id", "CASCADE"),
    ("files", "blob_id", "blobs", "id", "NO ACTION"),
    ("log_segments", "bundle_id", "bundles", "id", "CASCADE"),
    ("log_segments", "file_id", "files", "id", "CASCADE"),
    ("log_line_offsets", "file_id", "files", "id", "CASCADE"),
    ("user_skills", "owner_user_id", "users", "id", "CASCADE"),
    ("skill_reviews", "skill_id", "user_skills", "id", "CASCADE"),
    (
        "ai_provider_settings",
        "updated_by_user_id",
        "users",
        "id",
        "SET NULL",
    ),
    ("skill_runs", "user_id", "users", "id", "CASCADE"),
    ("skill_runs", "issue_code", "issues", "code", "CASCADE"),
    ("skill_run_steps", "run_id", "skill_runs", "id", "CASCADE"),
];

const PRIMARY_KEYS: &[(&str, &[&str])] = &[
    ("users", &["id"]),
    ("system_settings", &["id"]),
    ("admin_audit_logs", &["id"]),
    ("user_sessions", &["id"]),
    ("saved_searches", &["id"]),
    ("issues", &["code"]),
    ("bundles", &["id"]),
    ("blobs", &["id"]),
    ("files", &["id"]),
    ("log_segments", &["id"]),
    ("log_line_offsets", &["file_id", "line_number"]),
    ("temp_results", &["id"]),
    ("user_skills", &["id"]),
    ("skill_reviews", &["skill_id"]),
    ("ai_provider_settings", &["id"]),
    ("skill_runs", &["id"]),
    ("skill_run_steps", &["id"]),
    ("rain_ready_probe", &["id"]),
];

const UNIQUE_CONSTRAINTS: &[(&str, &[&str])] = &[
    ("users", &["username_normalized"]),
    ("user_sessions", &["token_hash"]),
    ("saved_searches", &["user_id", "name"]),
    ("bundles", &["hash"]),
    ("blobs", &["content_hash"]),
    ("blobs", &["storage_key"]),
    ("files", &["bundle_id", "path"]),
    ("user_skills", &["owner_user_id", "name"]),
    ("skill_run_steps", &["run_id", "sequence"]),
];

pub async fn prepare(pool: &SqlitePool, reset: bool) -> Result<(), AppError> {
    prepare_with_migrator(pool, reset, &MIGRATOR).await
}

async fn prepare_with_migrator(
    pool: &SqlitePool,
    reset: bool,
    migrator: &Migrator,
) -> Result<(), AppError> {
    if reset {
        reset_schema(pool).await?;
        tracing::info!(
            migration_state = "reset",
            "database schema reset; running database migrations"
        );
    } else {
        match classify(pool).await? {
            DatabaseState::Empty => {
                tracing::info!(migration_state = "empty", "running database migrations");
            }
            DatabaseState::Legacy => {
                tracing::info!(migration_state = "legacy", "validating database baseline");
                validate_baseline(pool, migrator).await?;
                backfill_legacy_event_times(pool).await?;
                tracing::info!(
                    migration_state = "legacy",
                    "legacy database baseline validated"
                );
            }
            DatabaseState::Managed => {
                tracing::info!(migration_state = "managed", "checking database migrations");
            }
        }
    }

    migrator
        .run(pool)
        .await
        .map_err(|error| AppError::Config(format!("database migration failed: {error}")))?;

    let latest_migration = migrator.iter().map(|migration| migration.version).max();
    tracing::info!(?latest_migration, "database migrations ready");
    Ok(())
}

async fn classify(pool: &SqlitePool) -> Result<DatabaseState, AppError> {
    let has_metadata: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='_sqlx_migrations')",
    )
    .fetch_one(pool)
    .await
    .map_err(AppError::Database)?;
    if has_metadata {
        return Ok(DatabaseState::Managed);
    }

    let has_user_objects: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name NOT LIKE 'sqlite_%')",
    )
    .fetch_one(pool)
    .await
    .map_err(AppError::Database)?;
    Ok(if has_user_objects {
        DatabaseState::Legacy
    } else {
        DatabaseState::Empty
    })
}

async fn backfill_legacy_event_times(pool: &SqlitePool) -> Result<(), AppError> {
    let mut last_id = 0_i64;
    let mut rows = 0_i64;
    let mut batches = 0_i64;

    loop {
        let mut tx = pool.begin().await.map_err(AppError::Database)?;
        let segments: Vec<(i64, String)> = sqlx::query_as(
            "SELECT id, content
             FROM log_segments
             WHERE id > ? AND event_time_indexed = 0
             ORDER BY id
             LIMIT ?",
        )
        .bind(last_id)
        .bind(LEGACY_EVENT_TIME_BACKFILL_BATCH_SIZE)
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
            .expect("non-empty event-time batch has a last id");
        for (id, content) in segments {
            let (start_ms, end_ms) = crate::ingest::event_time_range(&content);
            sqlx::query(
                "UPDATE log_segments
                 SET event_time_start_ms = COALESCE(event_time_start_ms, ?),
                     event_time_end_ms = COALESCE(event_time_end_ms, ?),
                     event_time_indexed = 1
                 WHERE id = ? AND event_time_indexed = 0",
            )
            .bind(start_ms)
            .bind(end_ms)
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(AppError::Database)?;
            rows += 1;
        }
        tx.commit().await.map_err(AppError::Database)?;
        batches += 1;
        last_id = batch_last_id;
    }

    if rows > 0 {
        tracing::info!(rows, batches, "legacy event-time backfill completed");
    } else {
        tracing::debug!("legacy event-time backfill found no pending rows");
    }
    Ok(())
}

async fn validate_baseline(pool: &SqlitePool, migrator: &Migrator) -> Result<(), AppError> {
    for (table, columns) in REQUIRED_TABLES {
        let object = find_object(pool, table).await?;
        if object.as_ref().is_none_or(|object| object.kind != "table") {
            return Err(schema_error(table, "required table is missing"));
        }
        validate_columns(pool, table, columns).await?;
    }

    let baseline_pool = legacy_baseline_pool(migrator).await?;

    validate_foreign_keys(pool).await?;

    for index in REQUIRED_INDEXES {
        validate_index(pool, index).await?;
    }

    validate_primary_keys(pool).await?;
    validate_unique_constraints(pool).await?;

    validate_sql_object_exact(
        pool,
        "log_segments_fts",
        "table",
        "CREATE VIRTUAL TABLE log_segments_fts USING fts5(
            content,
            content='log_segments',
            content_rowid='id',
            tokenize='trigram'
        )",
    )
    .await?;
    validate_sql_object_exact(
        pool,
        "log_segments_fts_ai",
        "trigger",
        "CREATE TRIGGER log_segments_fts_ai AFTER INSERT ON log_segments BEGIN
            INSERT INTO log_segments_fts(rowid, content) VALUES (new.id, new.content);
        END",
    )
    .await?;
    validate_sql_object_exact(
        pool,
        "log_segments_fts_ad",
        "trigger",
        "CREATE TRIGGER log_segments_fts_ad AFTER DELETE ON log_segments BEGIN
            INSERT INTO log_segments_fts(log_segments_fts, rowid, content)
            VALUES ('delete', old.id, old.content);
        END",
    )
    .await?;
    validate_sql_object_exact(
        pool,
        "log_segments_fts_au",
        "trigger",
        "CREATE TRIGGER log_segments_fts_au AFTER UPDATE OF content ON log_segments BEGIN
            INSERT INTO log_segments_fts(log_segments_fts, rowid, content)
            VALUES ('delete', old.id, old.content);
            INSERT INTO log_segments_fts(rowid, content) VALUES (new.id, new.content);
        END",
    )
    .await?;

    validate_owned_triggers(pool).await?;
    validate_check_constraints(pool, &baseline_pool).await?;
    validate_table_constraints(pool).await
}

async fn legacy_baseline_pool(migrator: &Migrator) -> Result<SqlitePool, AppError> {
    if !migrator
        .iter()
        .any(|migration| migration.version == LEGACY_BASELINE_VERSION)
    {
        return Err(AppError::Config(format!(
            "database baseline generation failed: migration version {LEGACY_BASELINE_VERSION} is missing"
        )));
    }
    let baseline_pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .map_err(AppError::Database)?;
    let baseline_migrator = Migrator {
        migrations: Cow::Owned(
            migrator
                .iter()
                .filter(|migration| migration.version <= LEGACY_BASELINE_VERSION)
                .cloned()
                .collect(),
        ),
        ignore_missing: false,
        locking: true,
    };
    baseline_migrator
        .run(&baseline_pool)
        .await
        .map_err(|error| {
            AppError::Config(format!("database baseline generation failed: {error}"))
        })?;
    Ok(baseline_pool)
}

async fn validate_columns(
    pool: &SqlitePool,
    table: &str,
    requirements: &[ColumnRequirement],
) -> Result<(), AppError> {
    let rows = sqlx::query(
        "SELECT name, type, \"notnull\", dflt_value, hidden FROM pragma_table_xinfo(?)",
    )
    .bind(table)
    .fetch_all(pool)
    .await
    .map_err(AppError::Database)?;

    for requirement in requirements {
        let Some(row) = rows.iter().find(|row| {
            row.try_get::<String, _>("name")
                .is_ok_and(|name| name == requirement.name)
        }) else {
            return Err(schema_error(
                format!("{table}.{name}", name = requirement.name),
                "required column is missing",
            ));
        };
        let actual_type: String = row.try_get("type").map_err(AppError::Database)?;
        if actual_type.to_uppercase() != requirement.type_name {
            return Err(schema_error(
                format!("{table}.{}", requirement.name),
                format!("type is {actual_type}, expected {}", requirement.type_name),
            ));
        }
        let actual_not_null: i64 = row.try_get("notnull").map_err(AppError::Database)?;
        if (actual_not_null != 0) != requirement.not_null {
            return Err(schema_error(
                format!("{table}.{}", requirement.name),
                format!(
                    "NOT NULL is {actual_not_null}, expected {}",
                    requirement.not_null
                ),
            ));
        }
        let actual_default: Option<String> =
            row.try_get("dflt_value").map_err(AppError::Database)?;
        let actual_default_normalized = actual_default.as_deref().map(compact_sql);
        let expected_default_normalized = requirement.default.map(compact_sql);
        if actual_default_normalized != expected_default_normalized {
            let expected_default = requirement.default.unwrap_or("none");
            let actual_default = actual_default.as_deref().unwrap_or("none");
            return Err(schema_error(
                format!("{table}.{}", requirement.name),
                format!("default is {actual_default}, expected {expected_default}"),
            ));
        }
        let hidden: i64 = row.try_get("hidden").map_err(AppError::Database)?;
        if hidden != 0 {
            return Err(schema_error(
                format!("{table}.{}", requirement.name),
                "generated or hidden columns are not compatible with the baseline",
            ));
        }
    }

    let actual_columns = rows
        .iter()
        .map(|row| row.try_get::<String, _>("name"))
        .collect::<Result<Vec<_>, _>>()
        .map_err(AppError::Database)?;
    let expected_columns = requirements
        .iter()
        .map(|requirement| requirement.name.to_owned())
        .collect::<Vec<_>>();
    let mut actual_columns = actual_columns;
    let mut expected_columns = expected_columns;
    actual_columns.sort();
    expected_columns.sort();
    if actual_columns != expected_columns {
        return Err(schema_error(
            table,
            format!("columns are {actual_columns:?}, expected {expected_columns:?}"),
        ));
    }
    Ok(())
}

async fn validate_index(pool: &SqlitePool, requirement: &IndexRequirement) -> Result<(), AppError> {
    let row = sqlx::query("SELECT \"unique\", partial FROM pragma_index_list(?) WHERE name = ?")
        .bind(requirement.table)
        .bind(requirement.name)
        .fetch_optional(pool)
        .await
        .map_err(AppError::Database)?;
    let Some(row) = row else {
        return Err(schema_error(
            requirement.name,
            format!("required index on {} is missing", requirement.table),
        ));
    };
    let unique: i64 = row.try_get("unique").map_err(AppError::Database)?;
    let partial: i64 = row.try_get("partial").map_err(AppError::Database)?;
    if (unique != 0) != requirement.unique {
        return Err(schema_error(
            requirement.name,
            format!("UNIQUE is {unique}, expected {}", requirement.unique),
        ));
    }
    if (partial != 0) != requirement.partial_sql.is_some() {
        return Err(schema_error(
            requirement.name,
            format!(
                "partial is {partial}, expected {}",
                requirement.partial_sql.is_some()
            ),
        ));
    }

    let index_info: Vec<(String, i64, String)> = sqlx::query_as(
        "SELECT name, \"desc\", coll
         FROM pragma_index_xinfo(?)
         WHERE key = 1
         ORDER BY seqno",
    )
    .bind(requirement.name)
    .fetch_all(pool)
    .await
    .map_err(AppError::Database)?;
    let columns = index_info
        .iter()
        .map(|(name, _, _)| name.clone())
        .collect::<Vec<_>>();
    let expected = requirement
        .columns
        .iter()
        .map(|column| (*column).to_owned())
        .collect::<Vec<_>>();
    if columns != expected {
        return Err(schema_error(
            requirement.name,
            format!("columns are {columns:?}, expected {expected:?}"),
        ));
    }

    let actual_descending = index_info
        .iter()
        .map(|(_, descending, _)| *descending != 0)
        .collect::<Vec<_>>();
    let expected_descending = INDEX_DESCENDING_COLUMNS
        .iter()
        .find(|(name, _)| *name == requirement.name)
        .map(|(_, directions)| directions.to_vec())
        .unwrap_or_else(|| vec![false; expected.len()]);
    if actual_descending != expected_descending {
        return Err(schema_error(
            requirement.name,
            format!("sort directions are {actual_descending:?}, expected {expected_descending:?}"),
        ));
    }

    let actual_collations = index_info
        .iter()
        .map(|(_, _, collation)| collation.as_str())
        .collect::<Vec<_>>();
    if actual_collations
        .iter()
        .any(|collation| *collation != "BINARY")
    {
        return Err(schema_error(
            requirement.name,
            format!("collations are {actual_collations:?}, expected BINARY"),
        ));
    }

    if let Some(expected_sql) = requirement.partial_sql {
        let object = find_object(pool, requirement.name).await?;
        let actual_sql = object
            .and_then(|object| object.sql)
            .map(|sql| compact_sql(&sql))
            .unwrap_or_default();
        if !actual_sql.contains(&compact_sql(expected_sql)) {
            return Err(schema_error(
                requirement.name,
                format!("partial predicate is missing or differs from {expected_sql}"),
            ));
        }
    }
    Ok(())
}

async fn validate_primary_keys(pool: &SqlitePool) -> Result<(), AppError> {
    for (table, expected) in PRIMARY_KEYS {
        let actual: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_table_info(?) WHERE pk > 0 ORDER BY pk")
                .bind(table)
                .fetch_all(pool)
                .await
                .map_err(AppError::Database)?;
        let expected = expected
            .iter()
            .map(|column| (*column).to_owned())
            .collect::<Vec<_>>();
        if actual != expected {
            return Err(schema_error(
                table,
                format!("primary key is {actual:?}, expected {expected:?}"),
            ));
        }
    }
    Ok(())
}

async fn validate_unique_constraints(pool: &SqlitePool) -> Result<(), AppError> {
    for (table, _) in REQUIRED_TABLES {
        let indexes: Vec<(String, i64, String)> = sqlx::query_as(
            "SELECT name, \"unique\", origin
             FROM pragma_index_list(?)",
        )
        .bind(table)
        .fetch_all(pool)
        .await
        .map_err(AppError::Database)?;
        let mut actual_table_unique = Vec::new();
        for (index_name, _, origin) in indexes {
            match origin.as_str() {
                "pk" => {}
                "u" => {
                    let actual: Vec<String> =
                        sqlx::query_scalar("SELECT name FROM pragma_index_info(?) ORDER BY seqno")
                            .bind(&index_name)
                            .fetch_all(pool)
                            .await
                            .map_err(AppError::Database)?;
                    actual_table_unique.push(actual);
                }
                "c" if REQUIRED_INDEXES
                    .iter()
                    .any(|index| index.table == *table && index.name == index_name) => {}
                _ => {
                    return Err(schema_error(
                        index_name,
                        format!("unknown index attached to Rain table {table}"),
                    ));
                }
            }
        }

        let mut expected_table_unique = UNIQUE_CONSTRAINTS
            .iter()
            .filter(|(expected_table, _)| *expected_table == *table)
            .map(|(_, columns)| {
                columns
                    .iter()
                    .map(|column| (*column).to_owned())
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        actual_table_unique.sort();
        expected_table_unique.sort();
        if actual_table_unique != expected_table_unique {
            return Err(schema_error(
                table,
                format!(
                    "UNIQUE constraints are {actual_table_unique:?}, expected {expected_table_unique:?}"
                ),
            ));
        }
    }

    for (table, expected) in UNIQUE_CONSTRAINTS {
        let index_names: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_index_list(?) WHERE \"unique\" = 1")
                .bind(table)
                .fetch_all(pool)
                .await
                .map_err(AppError::Database)?;
        let expected = expected
            .iter()
            .map(|column| (*column).to_owned())
            .collect::<Vec<_>>();
        let mut found = false;
        for index_name in index_names {
            let actual: Vec<String> =
                sqlx::query_scalar("SELECT name FROM pragma_index_info(?) ORDER BY seqno")
                    .bind(index_name)
                    .fetch_all(pool)
                    .await
                    .map_err(AppError::Database)?;
            if actual == expected {
                found = true;
                break;
            }
        }
        if !found {
            return Err(schema_error(
                table,
                format!("missing UNIQUE constraint on {expected:?}"),
            ));
        }
    }
    Ok(())
}

async fn validate_foreign_keys(pool: &SqlitePool) -> Result<(), AppError> {
    for (table, _) in REQUIRED_TABLES {
        let rows = sqlx::query(
            "SELECT \"table\", \"from\", \"to\", on_update, on_delete, \"match\"
             FROM pragma_foreign_key_list(?)",
        )
        .bind(table)
        .fetch_all(pool)
        .await
        .map_err(AppError::Database)?;

        let mut actual = Vec::with_capacity(rows.len());
        for row in rows {
            actual.push((
                row.try_get::<String, _>("table")
                    .map_err(AppError::Database)?,
                row.try_get::<String, _>("from")
                    .map_err(AppError::Database)?,
                row.try_get::<String, _>("to").map_err(AppError::Database)?,
                row.try_get::<String, _>("on_update")
                    .map_err(AppError::Database)?,
                row.try_get::<String, _>("on_delete")
                    .map_err(AppError::Database)?,
                row.try_get::<String, _>("match")
                    .map_err(AppError::Database)?,
            ));
        }

        let mut expected = FOREIGN_KEYS
            .iter()
            .filter(|(expected_table, _, _, _, _)| *expected_table == *table)
            .map(|(_, from, target, target_column, on_delete)| {
                (
                    (*target).to_owned(),
                    (*from).to_owned(),
                    (*target_column).to_owned(),
                    "NO ACTION".to_owned(),
                    (*on_delete).to_owned(),
                    "NONE".to_owned(),
                )
            })
            .collect::<Vec<_>>();
        actual.sort();
        expected.sort();
        if actual != expected {
            return Err(schema_error(
                table,
                format!("foreign keys are {actual:?}, expected {expected:?}"),
            ));
        }
    }
    Ok(())
}

async fn validate_owned_triggers(pool: &SqlitePool) -> Result<(), AppError> {
    let mut rain_tables = REQUIRED_TABLES
        .iter()
        .map(|(table, _)| *table)
        .collect::<Vec<_>>();
    rain_tables.push("log_segments_fts");

    for table in rain_tables {
        let trigger_names: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type = 'trigger' AND tbl_name = ?",
        )
        .bind(table)
        .fetch_all(pool)
        .await
        .map_err(AppError::Database)?;
        for trigger_name in trigger_names {
            if !matches!(
                trigger_name.as_str(),
                "log_segments_fts_ai" | "log_segments_fts_ad" | "log_segments_fts_au"
            ) {
                return Err(schema_error(
                    trigger_name,
                    format!("unknown trigger attached to Rain table {table}"),
                ));
            }
        }
    }
    Ok(())
}

async fn validate_check_constraints(
    pool: &SqlitePool,
    baseline_pool: &SqlitePool,
) -> Result<(), AppError> {
    for (table, _) in REQUIRED_TABLES {
        let actual = find_object(pool, table)
            .await?
            .and_then(|object| object.sql)
            .map(|sql| extract_check_constraints(&sql))
            .unwrap_or_default();
        let expected = find_object(baseline_pool, table)
            .await?
            .and_then(|object| object.sql)
            .map(|sql| extract_check_constraints(&sql))
            .unwrap_or_default();
        if actual != expected {
            return Err(schema_error(
                table,
                format!("CHECK constraints are {actual:?}, expected {expected:?}"),
            ));
        }
    }
    Ok(())
}

async fn validate_table_constraints(pool: &SqlitePool) -> Result<(), AppError> {
    const REQUIRED_SQL_FRAGMENTS: &[(&str, &[&str])] = &[
        (
            "users",
            &[
                "USERNAME_NORMALIZED TEXT NOT NULL UNIQUE",
                "STATUS TEXT NOT NULL DEFAULT 'ACTIVE' CHECK(STATUS IN('ACTIVE','DISABLED'))",
                "ROLE TEXT NOT NULL DEFAULT 'USER' CHECK(ROLE IN('USER','ADMIN'))",
                "CHECK(ROLE!='ADMIN'ORSTATUS='ACTIVE')",
            ],
        ),
        (
            "system_settings",
            &[
                "CHECK(ID=1)",
                "ALLOW_REGISTRATION INTEGER NOT NULL CHECK(ALLOW_REGISTRATION IN(0,1))",
                "LOGIN_IP_LIMIT_PER_MINUTE INTEGER NOT NULL DEFAULT 20 CHECK(LOGIN_IP_LIMIT_PER_MINUTE BETWEEN 1 AND 1000)",
                "LOGIN_USERNAME_FAILURE_LIMIT_PER_5_MINUTES INTEGER NOT NULL DEFAULT 10 CHECK(LOGIN_USERNAME_FAILURE_LIMIT_PER_5_MINUTES BETWEEN 1 AND 100)",
                "ISSUE_INACTIVE_DAYS INTEGER NOT NULL DEFAULT 0 CHECK(ISSUE_INACTIVE_DAYS=0ORISSUE_INACTIVE_DAYS BETWEEN 7 AND 30)",
            ],
        ),
        (
            "admin_audit_logs",
            &["ACTOR_TYPE TEXT NOT NULL CHECK(ACTOR_TYPE IN('USER','SYSTEM'))"],
        ),
        ("user_sessions", &["TOKEN_HASH TEXT NOT NULL UNIQUE"]),
        (
            "saved_searches",
            &[
                "NAME TEXT COLLATE NOCASE NOT NULL",
                "SEARCH_TYPE TEXT NOT NULL CHECK(SEARCH_TYPE IN('FILENAME','DETAIL'))",
                "UNIQUE(USER_ID,NAME)",
                "SCOPE_TYPE TEXT NOT NULL DEFAULT 'GLOBAL' CHECK(SCOPE_TYPE IN('GLOBAL','ISSUE'))",
                "(SCOPE_TYPE='GLOBAL'AND SCOPE_KEY IS NULL)OR(SCOPE_TYPE='ISSUE'AND SCOPE_KEY IS NOT NULL)",
            ],
        ),
        (
            "issues",
            &[
                "STATUS TEXT NOT NULL DEFAULT 'ACTIVE'",
                "DELETION_REASON TEXT CHECK(DELETION_REASON IS NULL OR DELETION_REASON IN('MANUAL','INACTIVE'))",
                "INACTIVE_CLAIM_DAYS INTEGER CHECK(INACTIVE_CLAIM_DAYS BETWEEN 1 AND 30)",
                "DELETION_REASONISNULL",
                "DELETION_ATTEMPTS INTEGER NOT NULL DEFAULT 0 CHECK(DELETION_ATTEMPTS>=0)",
            ],
        ),
        (
            "bundles",
            &[
                "HASH TEXT NOT NULL UNIQUE",
                "CONTENT_SIZE_BYTES INTEGER NOT NULL DEFAULT 0 CHECK(CONTENT_SIZE_BYTES>=0)",
            ],
        ),
        (
            "blobs",
            &[
                "ID INTEGER PRIMARY KEY AUTOINCREMENT",
                "CONTENT_HASH TEXT NOT NULL UNIQUE",
                "SIZE_BYTES INTEGER NOT NULL CHECK(SIZE_BYTES>=0)",
                "STORAGE_KEY TEXT NOT NULL UNIQUE",
            ],
        ),
        (
            "files",
            &[
                "ID INTEGER PRIMARY KEY AUTOINCREMENT",
                "CONSTRAINTFILES_BUNDLE_PATHUNIQUE(BUNDLE_ID,PATH)",
            ],
        ),
        ("log_segments", &["ID INTEGER PRIMARY KEY AUTOINCREMENT"]),
        ("log_line_offsets", &["PRIMARY KEY(FILE_ID,LINE_NUMBER)"]),
        (
            "temp_results",
            &[
                "STATUS TEXT NOT NULL DEFAULT 'ACTIVE' CHECK(STATUS IN('STAGING','ACTIVE','DELETING'))",
            ],
        ),
        (
            "user_skills",
            &[
                "NAME TEXT COLLATE NOCASE NOT NULL",
                "VERSION INTEGER NOT NULL DEFAULT 1 CHECK(VERSION>0)",
                "ENABLED INTEGER NOT NULL DEFAULT 1 CHECK(ENABLED IN(0,1))",
                "UNIQUE(OWNER_USER_ID,NAME)",
            ],
        ),
        (
            "skill_reviews",
            &[
                "SKILL_VERSION INTEGER NOT NULL CHECK(SKILL_VERSION>0)",
                "OVERALL_SCORE INTEGER NOT NULL CHECK(OVERALL_SCORE BETWEEN 0 AND 100)",
            ],
        ),
        (
            "ai_provider_settings",
            &[
                "CHECK(ID=1)",
                "REQUEST_TIMEOUT_SECONDS INTEGER NOT NULL CHECK(REQUEST_TIMEOUT_SECONDS BETWEEN 1 AND 300)",
            ],
        ),
        (
            "skill_runs",
            &[
                "SKILL_VERSION INTEGER NOT NULL CHECK(SKILL_VERSION>0)",
                "STATUS TEXT NOT NULL CHECK(STATUS IN('QUEUED','RUNNING','SUCCEEDED','FAILED','CANCELLED'))",
                "ITERATION_COUNT INTEGER NOT NULL DEFAULT 0 CHECK(ITERATION_COUNT>=0)",
                "TOOL_CALL_COUNT INTEGER NOT NULL DEFAULT 0 CHECK(TOOL_CALL_COUNT>=0)",
                "CANCEL_REQUESTED INTEGER NOT NULL DEFAULT 0 CHECK(CANCEL_REQUESTED IN(0,1))",
            ],
        ),
        (
            "skill_run_steps",
            &[
                "SEQUENCE INTEGER NOT NULL CHECK(SEQUENCE>=0)",
                "ITERATION INTEGER NOT NULL CHECK(ITERATION>=0)",
                "ELAPSED_MS INTEGER NOT NULL DEFAULT 0 CHECK(ELAPSED_MS>=0)",
                "UNIQUE(RUN_ID,SEQUENCE)",
            ],
        ),
    ];
    for (table, fragments) in REQUIRED_SQL_FRAGMENTS {
        let object = find_object(pool, table).await?;
        let sql = object
            .and_then(|object| object.sql)
            .map(|sql| compact_sql(&sql))
            .unwrap_or_default();
        for fragment in *fragments {
            if !sql.contains(&compact_sql(fragment)) {
                return Err(schema_error(
                    *table,
                    format!("table definition is missing required constraint {fragment}"),
                ));
            }
        }
    }
    Ok(())
}

async fn validate_sql_object_exact(
    pool: &SqlitePool,
    name: &str,
    expected_kind: &str,
    expected_sql: &str,
) -> Result<(), AppError> {
    let object = find_object(pool, name).await?;
    let Some(object) = object else {
        return Err(schema_error(
            name,
            format!("required {expected_kind} is missing"),
        ));
    };
    if object.kind != expected_kind {
        return Err(schema_error(
            name,
            format!("object type is {}, expected {expected_kind}", object.kind),
        ));
    }
    let actual_sql = object.sql.map(|sql| compact_sql(&sql)).unwrap_or_default();
    let expected_sql = compact_sql(expected_sql);
    if actual_sql != expected_sql {
        return Err(schema_error(
            name,
            "definition differs from the migration baseline",
        ));
    }
    Ok(())
}

#[derive(Debug)]
struct SqliteObject {
    kind: String,
    sql: Option<String>,
}

async fn find_object(pool: &SqlitePool, name: &str) -> Result<Option<SqliteObject>, AppError> {
    let row = sqlx::query("SELECT type, sql FROM sqlite_master WHERE name = ?")
        .bind(name)
        .fetch_optional(pool)
        .await
        .map_err(AppError::Database)?;
    row.map(|row| {
        Ok(SqliteObject {
            kind: row.try_get("type").map_err(AppError::Database)?,
            sql: row.try_get("sql").map_err(AppError::Database)?,
        })
    })
    .transpose()
}

fn compact_sql(value: &str) -> String {
    let mut compact = String::with_capacity(value.len());
    let mut quote = None;
    let mut characters = value.chars().peekable();
    while let Some(character) = characters.next() {
        if let Some(delimiter) = quote {
            compact.push(character);
            if character == delimiter {
                if characters.peek().copied() == Some(delimiter) {
                    compact.push(characters.next().expect("quoted SQL delimiter was peeked"));
                } else {
                    quote = None;
                }
            }
            continue;
        }

        let delimiter = match character {
            '\'' | '"' | '`' => Some(character),
            '[' => Some(']'),
            _ => None,
        };
        if let Some(delimiter) = delimiter {
            compact.push(character);
            quote = Some(delimiter);
        } else if !character.is_whitespace() {
            compact.extend(character.to_uppercase());
        }
    }
    compact
}

fn extract_check_constraints(sql: &str) -> Vec<String> {
    let compact = compact_sql(sql);
    let characters = compact.chars().collect::<Vec<_>>();
    let mut checks = Vec::new();
    let mut index = 0;
    let mut quote = None;
    while index < characters.len() {
        if let Some(delimiter) = quote {
            if characters[index] == delimiter {
                if characters.get(index + 1) == Some(&delimiter) {
                    index += 2;
                    continue;
                }
                quote = None;
            }
            index += 1;
            continue;
        }
        let delimiter = match characters[index] {
            '\'' | '"' | '`' => Some(characters[index]),
            '[' => Some(']'),
            _ => None,
        };
        if let Some(delimiter) = delimiter {
            quote = Some(delimiter);
            index += 1;
            continue;
        }
        if characters[index..].starts_with(&['C', 'H', 'E', 'C', 'K'])
            && (index == 0 || !is_sql_identifier_character(characters[index - 1]))
            && characters.get(index + 5) == Some(&'(')
        {
            let open = index + 5;
            if let Some(close) = matching_parenthesis(&characters, open) {
                checks.push(characters[open + 1..close].iter().collect());
                index = close + 1;
                continue;
            }
        }
        index += 1;
    }
    checks.sort();
    checks
}

fn matching_parenthesis(characters: &[char], open: usize) -> Option<usize> {
    let mut depth = 0;
    let mut quote = None;
    let mut index = open;
    while index < characters.len() {
        if let Some(delimiter) = quote {
            if characters[index] == delimiter {
                if characters.get(index + 1) == Some(&delimiter) {
                    index += 2;
                    continue;
                }
                quote = None;
            }
            index += 1;
            continue;
        }
        let delimiter = match characters[index] {
            '\'' | '"' | '`' => Some(characters[index]),
            '[' => Some(']'),
            _ => None,
        };
        if let Some(delimiter) = delimiter {
            quote = Some(delimiter);
        } else if characters[index] == '(' {
            depth += 1;
        } else if characters[index] == ')' {
            depth -= 1;
            if depth == 0 {
                return Some(index);
            }
        }
        index += 1;
    }
    None
}

fn is_sql_identifier_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '_'
}

fn schema_error(object: impl std::fmt::Display, reason: impl std::fmt::Display) -> AppError {
    AppError::Config(format!(
        "database migration baseline validation failed for {object}: {reason}"
    ))
}

async fn reset_schema(pool: &SqlitePool) -> Result<(), AppError> {
    let statements = [
        "DROP TRIGGER IF EXISTS log_segments_fts_ai",
        "DROP TRIGGER IF EXISTS log_segments_fts_ad",
        "DROP TRIGGER IF EXISTS log_segments_fts_au",
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
        "DROP TABLE IF EXISTS _sqlx_migrations",
    ];
    for statement in statements {
        sqlx::query(statement)
            .execute(pool)
            .await
            .map_err(AppError::Database)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use sqlx::migrate::{Migration, MigrationType, Migrator};

    use super::{MIGRATOR, prepare, prepare_with_migrator};
    use crate::db::init_pool;

    async fn pool() -> sqlx::SqlitePool {
        init_pool("sqlite::memory:").expect("init sqlite pool")
    }

    async fn make_legacy(pool: &sqlx::SqlitePool) {
        let fixture = include_str!("../../tests/fixtures/legacy_pre_145.sql");
        make_legacy_from_sql(pool, fixture).await;
    }

    async fn make_legacy_from_sql(pool: &sqlx::SqlitePool, fixture: &str) {
        for statement in fixture.split("-- RAIN_LEGACY_STATEMENT").skip(1) {
            let statement = statement.trim();
            if !statement.is_empty() {
                sqlx::query(statement)
                    .execute(pool)
                    .await
                    .expect("create legacy schema fixture");
            }
        }
    }

    #[tokio::test]
    async fn empty_database_runs_baseline_and_records_metadata() {
        let pool = pool().await;

        prepare(&pool, false).await.expect("run baseline migration");

        let applied: (i64, i64) =
            sqlx::query_as("SELECT version, success FROM _sqlx_migrations WHERE version = 1")
                .fetch_one(&pool)
                .await
                .expect("inspect migration metadata");
        assert_eq!(applied, (1, 1));
        let table_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='log_segments')",
        )
        .fetch_one(&pool)
        .await
        .expect("inspect baseline table");
        assert!(table_exists);
    }

    #[tokio::test]
    async fn legacy_database_is_adopted_without_losing_rows() {
        let pool = pool().await;
        make_legacy(&pool).await;
        sqlx::query("INSERT INTO issues(code, name) VALUES ('LEGACY', 'Legacy issue')")
            .execute(&pool)
            .await
            .expect("insert legacy row");

        prepare(&pool, false).await.expect("adopt legacy schema");

        let name: String = sqlx::query_scalar("SELECT name FROM issues WHERE code='LEGACY'")
            .fetch_one(&pool)
            .await
            .expect("read preserved row");
        assert_eq!(name, "Legacy issue");
        let applied: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM _sqlx_migrations WHERE version = 1 AND success = 1",
        )
        .fetch_one(&pool)
        .await
        .expect("inspect adopted metadata");
        assert_eq!(applied, 1);

        prepare(&pool, false).await.expect("restart managed schema");
        let migration_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations WHERE version = 1")
                .fetch_one(&pool)
                .await
                .expect("inspect idempotent metadata");
        assert_eq!(migration_count, 1);
    }

    #[tokio::test]
    async fn legacy_adoption_resumes_event_time_backfill_before_recording_metadata() {
        let pool = pool().await;
        make_legacy(&pool).await;
        sqlx::query("INSERT INTO issues(code, name) VALUES ('EVENTS', 'Events')")
            .execute(&pool)
            .await
            .expect("insert issue");
        sqlx::query(
            "INSERT INTO bundles(id, issue_code, hash, name) VALUES ('EVENT-BUNDLE', 'EVENTS', 'EVENT-HASH', 'Events')",
        )
        .execute(&pool)
        .await
        .expect("insert bundle");
        let file_id: i64 = sqlx::query_scalar(
            "INSERT INTO files(bundle_id, name, path, is_dir) VALUES ('EVENT-BUNDLE', 'events.log', '/events.log', 0) RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .expect("insert file");
        let segment_id: i64 = sqlx::query_scalar(
            "INSERT INTO log_segments(bundle_id, file_id, content) VALUES ('EVENT-BUNDLE', ?, '2026-08-14T09:32:15 first\nnoise\n2026-08-14T09:33:15 second') RETURNING id",
        )
        .bind(file_id)
        .fetch_one(&pool)
        .await
        .expect("insert pending event-time segment");

        prepare(&pool, false).await.expect("adopt legacy schema");

        let indexed: (i64, Option<i64>, Option<i64>) = sqlx::query_as(
            "SELECT event_time_indexed, event_time_start_ms, event_time_end_ms FROM log_segments WHERE id = ?",
        )
        .bind(segment_id)
        .fetch_one(&pool)
        .await
        .expect("read backfilled event-time segment");
        assert_eq!(indexed.0, 1);
        assert_eq!(
            indexed.1,
            crate::ingest::parse_event_time_ms("2026-08-14T09:32:15 first")
        );
        assert_eq!(
            indexed.2,
            crate::ingest::parse_event_time_ms("2026-08-14T09:33:15 second")
        );
        let metadata_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM _sqlx_migrations WHERE version = 1 AND success = 1",
        )
        .fetch_one(&pool)
        .await
        .expect("inspect adopted metadata");
        assert_eq!(metadata_count, 1);
    }

    #[tokio::test]
    async fn legacy_adoption_validates_v1_before_running_future_migrations() {
        let pool = pool().await;
        make_legacy(&pool).await;

        let mut migrations = MIGRATOR.migrations.to_vec();
        migrations.push(Migration::new(
            2,
            Cow::Borrowed("test future legacy migration"),
            MigrationType::Simple,
            Cow::Borrowed("ALTER TABLE issues ADD COLUMN migration_v2_marker TEXT;"),
        ));
        let migrator = Migrator {
            migrations: Cow::Owned(migrations),
            ignore_missing: false,
            locking: true,
        };

        prepare_with_migrator(&pool, false, &migrator)
            .await
            .expect("adopt legacy v1 schema and run future migration");

        let marker_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM pragma_table_info('issues')
                WHERE name = 'migration_v2_marker'
            )",
        )
        .fetch_one(&pool)
        .await
        .expect("inspect future migration column");
        assert!(marker_exists);
        let versions: Vec<i64> = sqlx::query_scalar(
            "SELECT version FROM _sqlx_migrations WHERE success = 1 ORDER BY version",
        )
        .fetch_all(&pool)
        .await
        .expect("inspect legacy future migration metadata");
        assert_eq!(versions, vec![1, 2]);
    }

    #[tokio::test]
    async fn legacy_database_with_historical_optional_column_order_is_adopted() {
        let pool = pool().await;
        let fixture = include_str!("../../tests/fixtures/legacy_pre_optional_columns.sql");
        make_legacy_from_sql(&pool, fixture).await;

        for statement in [
            "ALTER TABLE skill_runs ADD COLUMN analysis_start_time TEXT",
            "ALTER TABLE skill_runs ADD COLUMN analysis_end_time TEXT",
            "ALTER TABLE skill_runs ADD COLUMN analysis_start_ms INTEGER",
            "ALTER TABLE skill_runs ADD COLUMN analysis_end_ms INTEGER",
            "ALTER TABLE log_segments ADD COLUMN event_time_start_ms INTEGER",
            "ALTER TABLE log_segments ADD COLUMN event_time_end_ms INTEGER",
            "ALTER TABLE log_segments ADD COLUMN event_time_indexed INTEGER NOT NULL DEFAULT 0",
            "CREATE INDEX idx_logs_file_event_time ON log_segments (file_id, event_time_start_ms, event_time_end_ms)",
            "CREATE INDEX idx_logs_event_time_indexed ON log_segments (event_time_indexed, id)",
        ] {
            sqlx::query(statement)
                .execute(&pool)
                .await
                .expect("apply historical optional-column upgrade");
        }

        prepare(&pool, false)
            .await
            .expect("adopt legacy schema upgraded by historical ensure statements");

        let skill_columns: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_table_info('skill_runs') ORDER BY cid")
                .fetch_all(&pool)
                .await
                .expect("inspect historical skill_runs columns");
        assert_eq!(
            skill_columns.last().map(String::as_str),
            Some("analysis_end_ms")
        );
        let log_columns: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_table_info('log_segments') ORDER BY cid")
                .fetch_all(&pool)
                .await
                .expect("inspect historical log_segments columns");
        assert_eq!(
            log_columns.last().map(String::as_str),
            Some("event_time_indexed")
        );
        let metadata_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM _sqlx_migrations WHERE version = 1 AND success = 1",
        )
        .fetch_one(&pool)
        .await
        .expect("inspect historical adoption metadata");
        assert_eq!(metadata_count, 1);
    }

    #[tokio::test]
    async fn incompatible_legacy_schema_fails_before_adoption() {
        let pool = pool().await;
        make_legacy(&pool).await;
        sqlx::query("ALTER TABLE skill_runs DROP COLUMN analysis_start_time")
            .execute(&pool)
            .await
            .expect("remove required legacy column");

        let error = prepare(&pool, false)
            .await
            .expect_err("incompatible legacy schema must fail");
        let message = error.to_string();
        assert!(
            message.contains("skill_runs.analysis_start_time"),
            "{message}"
        );
        let metadata_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='_sqlx_migrations')",
        )
        .fetch_one(&pool)
        .await
        .expect("inspect failed adoption state");
        assert!(!metadata_exists);
    }

    #[tokio::test]
    async fn incompatible_legacy_index_fails_before_adoption() {
        let pool = pool().await;
        make_legacy(&pool).await;
        sqlx::query("DROP INDEX idx_logs_event_time_indexed")
            .execute(&pool)
            .await
            .expect("remove required legacy index");

        let error = prepare(&pool, false)
            .await
            .expect_err("incompatible legacy index must fail");
        assert!(error.to_string().contains("idx_logs_event_time_indexed"));
        let metadata_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='_sqlx_migrations')",
        )
        .fetch_one(&pool)
        .await
        .expect("inspect failed index adoption state");
        assert!(!metadata_exists);
    }

    #[tokio::test]
    async fn incompatible_legacy_fts_trigger_fails_before_adoption() {
        let pool = pool().await;
        make_legacy(&pool).await;
        sqlx::query("DROP TRIGGER log_segments_fts_au")
            .execute(&pool)
            .await
            .expect("remove required legacy FTS trigger");

        let error = prepare(&pool, false)
            .await
            .expect_err("incompatible legacy FTS trigger must fail");
        assert!(error.to_string().contains("log_segments_fts_au"));
        let metadata_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='_sqlx_migrations')",
        )
        .fetch_one(&pool)
        .await
        .expect("inspect failed FTS adoption state");
        assert!(!metadata_exists);
    }

    #[tokio::test]
    async fn incompatible_legacy_fts_trigger_with_extra_logic_fails_before_adoption() {
        let pool = pool().await;
        make_legacy(&pool).await;
        sqlx::query("DROP TRIGGER log_segments_fts_ai")
            .execute(&pool)
            .await
            .expect("remove original legacy FTS trigger");
        sqlx::query(
            "CREATE TRIGGER log_segments_fts_ai AFTER INSERT ON log_segments BEGIN
                INSERT INTO log_segments_fts(rowid, content) VALUES (new.id, new.content);
                UPDATE issues SET name = name WHERE 0;
            END",
        )
        .execute(&pool)
        .await
        .expect("create altered legacy FTS trigger");

        let error = prepare(&pool, false)
            .await
            .expect_err("altered legacy FTS trigger must fail");
        assert!(error.to_string().contains("log_segments_fts_ai"));
        let metadata_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='_sqlx_migrations')",
        )
        .fetch_one(&pool)
        .await
        .expect("inspect failed FTS adoption state");
        assert!(!metadata_exists);
    }

    #[tokio::test]
    async fn incompatible_legacy_index_sort_order_fails_before_adoption() {
        let pool = pool().await;
        make_legacy(&pool).await;
        sqlx::query("DROP INDEX idx_bundles_issue")
            .execute(&pool)
            .await
            .expect("remove original legacy index");
        sqlx::query("CREATE INDEX idx_bundles_issue ON bundles (issue_code ASC, created_at ASC)")
            .execute(&pool)
            .await
            .expect("create altered legacy index");

        let error = prepare(&pool, false)
            .await
            .expect_err("altered legacy index sort order must fail");
        assert!(error.to_string().contains("idx_bundles_issue"));
        let metadata_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='_sqlx_migrations')",
        )
        .fetch_one(&pool)
        .await
        .expect("inspect failed index adoption state");
        assert!(!metadata_exists);
    }

    #[tokio::test]
    async fn incompatible_legacy_index_collation_fails_before_adoption() {
        let pool = pool().await;
        make_legacy(&pool).await;
        sqlx::query("DROP INDEX idx_bundles_issue")
            .execute(&pool)
            .await
            .expect("remove original legacy index");
        sqlx::query(
            "CREATE INDEX idx_bundles_issue ON bundles (issue_code ASC, created_at COLLATE NOCASE ASC)",
        )
        .execute(&pool)
        .await
        .expect("create altered legacy index");

        let error = prepare(&pool, false)
            .await
            .expect_err("altered legacy index collation must fail");
        assert!(error.to_string().contains("idx_bundles_issue"));
        let metadata_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='_sqlx_migrations')",
        )
        .fetch_one(&pool)
        .await
        .expect("inspect failed index adoption state");
        assert!(!metadata_exists);
    }

    #[tokio::test]
    async fn incompatible_legacy_autoincrement_definition_fails_before_adoption() {
        let pool = pool().await;
        let fixture = include_str!("../../tests/fixtures/legacy_pre_145.sql").replacen(
            "id INTEGER PRIMARY KEY AUTOINCREMENT",
            "id INTEGER PRIMARY KEY",
            1,
        );
        make_legacy_from_sql(&pool, &fixture).await;

        let error = prepare(&pool, false)
            .await
            .expect_err("missing AUTOINCREMENT must fail");
        assert!(error.to_string().contains("blobs"));
        let metadata_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='_sqlx_migrations')",
        )
        .fetch_one(&pool)
        .await
        .expect("inspect failed AUTOINCREMENT adoption state");
        assert!(!metadata_exists);
    }

    #[tokio::test]
    async fn incompatible_legacy_extra_column_fails_before_adoption() {
        let pool = pool().await;
        let fixture = include_str!("../../tests/fixtures/legacy_pre_145.sql").replacen(
            "code TEXT PRIMARY KEY,\n            name TEXT NOT NULL,",
            "code TEXT PRIMARY KEY,\n            name TEXT NOT NULL,\n            extra TEXT,",
            1,
        );
        make_legacy_from_sql(&pool, &fixture).await;

        let error = prepare(&pool, false)
            .await
            .expect_err("extra Rain column must fail");
        assert!(error.to_string().contains("issues"));
    }

    #[tokio::test]
    async fn incompatible_legacy_extra_check_fails_before_adoption() {
        let pool = pool().await;
        let fixture = include_str!("../../tests/fixtures/legacy_pre_145.sql").replacen(
            "status TEXT NOT NULL DEFAULT 'ACTIVE',\n            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,",
            "status TEXT NOT NULL DEFAULT 'ACTIVE' CHECK (name != ''),\n            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,",
            1,
        );
        make_legacy_from_sql(&pool, &fixture).await;

        let error = prepare(&pool, false)
            .await
            .expect_err("extra Rain CHECK must fail");
        assert!(error.to_string().contains("issues"));
    }

    #[tokio::test]
    async fn incompatible_legacy_extra_trigger_fails_before_adoption() {
        let pool = pool().await;
        make_legacy(&pool).await;
        sqlx::query(
            "CREATE TRIGGER custom_block_issue BEFORE INSERT ON issues BEGIN
                SELECT RAISE(ABORT, 'blocked');
            END",
        )
        .execute(&pool)
        .await
        .expect("create extra Rain trigger");

        let error = prepare(&pool, false)
            .await
            .expect_err("extra Rain trigger must fail");
        assert!(error.to_string().contains("custom_block_issue"));
    }

    #[tokio::test]
    async fn incompatible_legacy_extra_unique_index_fails_before_adoption() {
        let pool = pool().await;
        make_legacy(&pool).await;
        sqlx::query("CREATE UNIQUE INDEX custom_issue_name_unique ON issues(name)")
            .execute(&pool)
            .await
            .expect("create extra Rain unique index");

        let error = prepare(&pool, false)
            .await
            .expect_err("extra Rain UNIQUE index must fail");
        assert!(error.to_string().contains("custom_issue_name_unique"));
    }

    #[tokio::test]
    async fn reset_recreates_schema_through_the_migration_chain() {
        let pool = pool().await;
        prepare(&pool, false).await.expect("initial migration");
        sqlx::query("INSERT INTO issues(code, name) VALUES ('RESET', 'Reset me')")
            .execute(&pool)
            .await
            .expect("insert row before reset");

        prepare(&pool, true).await.expect("reset and migrate");

        let issue_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM issues")
            .fetch_one(&pool)
            .await
            .expect("inspect reset schema");
        assert_eq!(issue_count, 0);
        let migration_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM _sqlx_migrations WHERE version = 1 AND success = 1",
        )
        .fetch_one(&pool)
        .await
        .expect("inspect reset metadata");
        assert_eq!(migration_count, 1);
    }

    #[tokio::test]
    async fn reset_rebuilds_rain_schema_without_reclassifying_extra_objects() {
        let pool = pool().await;
        prepare(&pool, false).await.expect("initial migration");
        sqlx::query("CREATE TABLE my_debug_table (id INTEGER PRIMARY KEY)")
            .execute(&pool)
            .await
            .expect("create unrelated table");

        prepare(&pool, true)
            .await
            .expect("reset should bypass legacy classification");

        let debug_table_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='my_debug_table')",
        )
        .fetch_one(&pool)
        .await
        .expect("inspect unrelated table");
        assert!(debug_table_exists);
        let migration_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM _sqlx_migrations WHERE version = 1 AND success = 1",
        )
        .fetch_one(&pool)
        .await
        .expect("inspect reset metadata");
        assert_eq!(migration_count, 1);
    }

    #[tokio::test]
    async fn dirty_metadata_fails_fast() {
        let pool = pool().await;
        prepare(&pool, false).await.expect("initial migration");
        sqlx::query("UPDATE _sqlx_migrations SET success = 0 WHERE version = 1")
            .execute(&pool)
            .await
            .expect("mark migration dirty");

        let error = prepare(&pool, false)
            .await
            .expect_err("dirty migration must fail");
        assert!(error.to_string().contains("migration"));
    }

    #[tokio::test]
    async fn future_migration_runs_once_after_the_baseline() {
        let pool = pool().await;
        prepare(&pool, false).await.expect("initial migration");

        let mut migrations = MIGRATOR.migrations.to_vec();
        migrations.push(Migration::new(
            2,
            Cow::Borrowed("test marker"),
            MigrationType::Simple,
            Cow::Borrowed(
                "CREATE TABLE migration_test_marker (id INTEGER PRIMARY KEY, value TEXT NOT NULL);",
            ),
        ));
        let migrator = Migrator {
            migrations: Cow::Owned(migrations),
            ignore_missing: false,
            locking: true,
        };

        migrator.run(&pool).await.expect("run future migration");
        migrator.run(&pool).await.expect("restart future migration");
        let marker_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM _sqlx_migrations WHERE version = 2 AND success = 1",
        )
        .fetch_one(&pool)
        .await
        .expect("inspect future migration metadata");
        assert_eq!(marker_count, 1);
    }

    #[tokio::test]
    async fn checksum_mismatch_fails_fast() {
        let pool = pool().await;
        prepare(&pool, false).await.expect("initial migration");
        sqlx::query("UPDATE _sqlx_migrations SET checksum = zeroblob(48) WHERE version = 1")
            .execute(&pool)
            .await
            .expect("corrupt migration checksum");

        let error = prepare(&pool, false)
            .await
            .expect_err("checksum mismatch must fail");
        assert!(error.to_string().contains("migration"));
    }
}
