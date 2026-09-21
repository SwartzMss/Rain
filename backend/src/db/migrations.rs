use sqlx::{Row, SqlitePool, migrate::Migrator};

use crate::error::AppError;

pub(crate) static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

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

pub async fn prepare(pool: &SqlitePool, reset: bool) -> Result<(), AppError> {
    if reset {
        reset_schema(pool).await?;
    }

    match classify(pool).await? {
        DatabaseState::Empty => {
            tracing::info!(migration_state = "empty", "running database migrations");
        }
        DatabaseState::Legacy => {
            tracing::info!(migration_state = "legacy", "validating database baseline");
            validate_baseline(pool).await?;
            tracing::info!(
                migration_state = "legacy",
                "legacy database baseline validated"
            );
        }
        DatabaseState::Managed => {
            tracing::info!(migration_state = "managed", "checking database migrations");
        }
    }

    MIGRATOR
        .run(pool)
        .await
        .map_err(|error| AppError::Config(format!("database migration failed: {error}")))?;

    let latest_migration = MIGRATOR.iter().map(|migration| migration.version).max();
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

async fn validate_baseline(pool: &SqlitePool) -> Result<(), AppError> {
    for (table, columns) in REQUIRED_TABLES {
        let object = find_object(pool, table).await?;
        if object.as_ref().is_none_or(|object| object.kind != "table") {
            return Err(schema_error(table, "required table is missing"));
        }
        validate_columns(pool, table, columns).await?;
    }

    for (table, from, target, target_column, on_delete) in FOREIGN_KEYS {
        let matched: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM pragma_foreign_key_list(?)
                WHERE \"table\" = ? AND \"from\" = ? AND \"to\" = ? AND upper(on_delete) = ?
            )",
        )
        .bind(table)
        .bind(target)
        .bind(from)
        .bind(target_column)
        .bind(on_delete)
        .fetch_one(pool)
        .await
        .map_err(AppError::Database)?;
        if !matched {
            return Err(schema_error(
                table,
                format!(
                    "missing foreign key {from} -> {target}.{target_column} ON DELETE {on_delete}"
                ),
            ));
        }
    }

    for index in REQUIRED_INDEXES {
        validate_index(pool, index).await?;
    }

    validate_sql_object(
        pool,
        "log_segments_fts",
        "table",
        &[
            "USING FTS5",
            "CONTENT='LOG_SEGMENTS'",
            "CONTENT_ROWID='ID'",
            "TOKENIZE='TRIGRAM'",
        ],
    )
    .await?;
    validate_sql_object(
        pool,
        "log_segments_fts_ai",
        "trigger",
        &[
            "AFTER INSERT ON LOG_SEGMENTS",
            "INSERT INTO LOG_SEGMENTS_FTS",
        ],
    )
    .await?;
    validate_sql_object(
        pool,
        "log_segments_fts_ad",
        "trigger",
        &[
            "AFTER DELETE ON LOG_SEGMENTS",
            "VALUES('DELETE',OLD.ID,OLD.CONTENT)",
        ],
    )
    .await?;
    validate_sql_object(
        pool,
        "log_segments_fts_au",
        "trigger",
        &[
            "AFTER UPDATE OF CONTENT ON LOG_SEGMENTS",
            "VALUES('DELETE',OLD.ID,OLD.CONTENT)",
        ],
    )
    .await?;

    validate_table_constraints(pool).await
}

async fn validate_columns(
    pool: &SqlitePool,
    table: &str,
    requirements: &[ColumnRequirement],
) -> Result<(), AppError> {
    let rows = sqlx::query("SELECT name, type, \"notnull\", dflt_value FROM pragma_table_info(?)")
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

    let columns: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_index_info(?) ORDER BY seqno")
            .bind(requirement.name)
            .fetch_all(pool)
            .await
            .map_err(AppError::Database)?;
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

async fn validate_table_constraints(pool: &SqlitePool) -> Result<(), AppError> {
    const REQUIRED_SQL_FRAGMENTS: &[(&str, &[&str])] = &[
        (
            "users",
            &["CHECK(ROLE!='ADMIN'ORSTATUS='ACTIVE')", "UNIQUE"],
        ),
        (
            "saved_searches",
            &[
                "UNIQUE(USER_ID,NAME)",
                "SCOPE_TYPE='GLOBAL'",
                "SCOPE_TYPE='ISSUE'",
            ],
        ),
        ("issues", &["DELETION_REASONISNULL", "DELETION_ATTEMPTS>=0"]),
        (
            "bundles",
            &["CONTENT_SIZE_BYTES>=0", "HASH TEXT NOT NULL UNIQUE"],
        ),
        (
            "files",
            &["CONSTRAINTFILES_BUNDLE_PATHUNIQUE(BUNDLE_ID,PATH)"],
        ),
        ("log_line_offsets", &["PRIMARY KEY(FILE_ID,LINE_NUMBER)"]),
        ("skill_run_steps", &["UNIQUE(RUN_ID,SEQUENCE)"]),
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

async fn validate_sql_object(
    pool: &SqlitePool,
    name: &str,
    expected_kind: &str,
    fragments: &[&str],
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
    let sql = object.sql.map(|sql| compact_sql(&sql)).unwrap_or_default();
    for fragment in fragments {
        if !sql.contains(&compact_sql(fragment)) {
            return Err(schema_error(
                name,
                format!("definition is missing required fragment {fragment}"),
            ));
        }
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
    value
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>()
        .to_uppercase()
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

    use super::{MIGRATOR, prepare};
    use crate::db::init_pool;

    async fn pool() -> sqlx::SqlitePool {
        init_pool("sqlite::memory:").expect("init sqlite pool")
    }

    async fn make_legacy(pool: &sqlx::SqlitePool) {
        MIGRATOR.run(pool).await.expect("create legacy schema");
        sqlx::query("DROP TABLE _sqlx_migrations")
            .execute(pool)
            .await
            .expect("remove migration metadata");
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
