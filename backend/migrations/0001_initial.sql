-- Rain's complete SQLite schema baseline.
--
-- This migration is intentionally safe to execute after a successful legacy
-- schema compatibility check. It creates objects that are absent, but does
-- not alter or repair existing application data.

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
);

CREATE TABLE IF NOT EXISTS system_settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    allow_registration INTEGER NOT NULL CHECK (allow_registration IN (0, 1)),
    updated_by_user_id TEXT REFERENCES users(id) ON DELETE SET NULL,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    login_ip_limit_per_minute INTEGER NOT NULL DEFAULT 20 CHECK (login_ip_limit_per_minute BETWEEN 1 AND 1000),
    login_username_failure_limit_per_5_minutes INTEGER NOT NULL DEFAULT 10 CHECK (login_username_failure_limit_per_5_minutes BETWEEN 1 AND 100),
    issue_inactive_days INTEGER NOT NULL DEFAULT 0 CHECK (issue_inactive_days = 0 OR issue_inactive_days BETWEEN 7 AND 30)
);

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
);

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
);

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
);

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
);

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
);

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
);

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
);

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
);

CREATE TABLE IF NOT EXISTS log_line_offsets (
    file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    line_number INTEGER NOT NULL,
    byte_offset INTEGER NOT NULL,
    PRIMARY KEY (file_id, line_number)
);

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
);

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
);

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
);

CREATE TABLE IF NOT EXISTS ai_provider_settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    base_url TEXT NOT NULL,
    encrypted_api_key TEXT NOT NULL,
    model TEXT NOT NULL,
    request_timeout_seconds INTEGER NOT NULL CHECK (request_timeout_seconds BETWEEN 1 AND 300),
    updated_by_user_id TEXT REFERENCES users(id) ON DELETE SET NULL,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

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
);

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
);

CREATE TABLE IF NOT EXISTS rain_ready_probe (
    id TEXT PRIMARY KEY,
    value INTEGER NOT NULL
);

CREATE VIRTUAL TABLE IF NOT EXISTS log_segments_fts USING fts5(
    content,
    content='log_segments',
    content_rowid='id',
    tokenize='trigram'
);

CREATE TRIGGER IF NOT EXISTS log_segments_fts_ai AFTER INSERT ON log_segments BEGIN
    INSERT INTO log_segments_fts(rowid, content) VALUES (new.id, new.content);
END;

CREATE TRIGGER IF NOT EXISTS log_segments_fts_ad AFTER DELETE ON log_segments BEGIN
    INSERT INTO log_segments_fts(log_segments_fts, rowid, content)
    VALUES ('delete', old.id, old.content);
END;

CREATE TRIGGER IF NOT EXISTS log_segments_fts_au AFTER UPDATE OF content ON log_segments BEGIN
    INSERT INTO log_segments_fts(log_segments_fts, rowid, content)
    VALUES ('delete', old.id, old.content);
    INSERT INTO log_segments_fts(rowid, content) VALUES (new.id, new.content);
END;

CREATE UNIQUE INDEX IF NOT EXISTS idx_skill_runs_one_active_per_user
    ON skill_runs(user_id) WHERE status IN ('QUEUED', 'RUNNING');
CREATE INDEX IF NOT EXISTS idx_skill_runs_terminal_cleanup
    ON skill_runs(status, completed_at);
CREATE INDEX IF NOT EXISTS idx_bundles_issue
    ON bundles (issue_code, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_issues_activity
    ON issues (status, last_activity_at);
CREATE INDEX IF NOT EXISTS idx_files_parent
    ON files (parent_id);
CREATE INDEX IF NOT EXISTS idx_files_bundle
    ON files (bundle_id);
CREATE INDEX IF NOT EXISTS idx_files_path
    ON files (path);
CREATE INDEX IF NOT EXISTS idx_logs_bundle_timeline
    ON log_segments (bundle_id, timeline);
CREATE INDEX IF NOT EXISTS idx_logs_file_chunk
    ON log_segments (file_id, chunk_index);
CREATE INDEX IF NOT EXISTS idx_logs_file_event_time
    ON log_segments (file_id, event_time_start_ms, event_time_end_ms);
CREATE INDEX IF NOT EXISTS idx_logs_event_time_indexed
    ON log_segments (event_time_indexed, id);
CREATE INDEX IF NOT EXISTS idx_line_offsets_file_line
    ON log_line_offsets (file_id, line_number);
CREATE INDEX IF NOT EXISTS idx_temp_results_expiry
    ON temp_results (expires_at);
CREATE INDEX IF NOT EXISTS idx_user_sessions_user
    ON user_sessions (user_id);
CREATE INDEX IF NOT EXISTS idx_user_sessions_expiry
    ON user_sessions (expires_at);
CREATE INDEX IF NOT EXISTS idx_users_role_status
    ON users (role, status, created_at, id);
CREATE UNIQUE INDEX IF NOT EXISTS idx_users_single_admin
    ON users (role) WHERE role = 'ADMIN';
CREATE INDEX IF NOT EXISTS idx_admin_audit_created
    ON admin_audit_logs (created_at DESC, id DESC);
CREATE INDEX IF NOT EXISTS idx_admin_audit_target
    ON admin_audit_logs (target_user_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_saved_searches_user
    ON saved_searches (user_id, is_pinned DESC, sort_order, updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_files_blob
    ON files (blob_id);
CREATE INDEX IF NOT EXISTS idx_bundles_deleted
    ON bundles (deleted_at);
