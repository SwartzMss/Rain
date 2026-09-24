CREATE TABLE IF NOT EXISTS upload_sessions (
    id TEXT PRIMARY KEY,
    issue_code TEXT NOT NULL REFERENCES issues(code) ON DELETE CASCADE,
    owner_user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    idempotency_key TEXT NOT NULL,
    file_name TEXT NOT NULL,
    file_size_bytes INTEGER NOT NULL CHECK (file_size_bytes >= 0),
    last_modified_ms INTEGER,
    chunk_size_bytes INTEGER NOT NULL CHECK (chunk_size_bytes > 0),
    committed_offset INTEGER NOT NULL DEFAULT 0 CHECK (committed_offset >= 0),
    next_chunk_index INTEGER NOT NULL DEFAULT 0 CHECK (next_chunk_index >= 0),
    status TEXT NOT NULL CHECK (status IN ('OPEN', 'FINALIZING', 'DELIVERED', 'CANCELLED', 'EXPIRED', 'FAILED')),
    input_path TEXT NOT NULL UNIQUE,
    bundle_id TEXT UNIQUE REFERENCES bundles(id) ON DELETE SET NULL,
    failure_code TEXT,
    failure_reason TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    expires_at TEXT NOT NULL,
    UNIQUE(owner_user_id, issue_code, idempotency_key)
);

CREATE TABLE IF NOT EXISTS upload_session_chunks (
    session_id TEXT NOT NULL REFERENCES upload_sessions(id) ON DELETE CASCADE,
    chunk_index INTEGER NOT NULL CHECK (chunk_index >= 0),
    offset_bytes INTEGER NOT NULL CHECK (offset_bytes >= 0),
    size_bytes INTEGER NOT NULL CHECK (size_bytes > 0),
    sha256 TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY(session_id, chunk_index)
);

CREATE INDEX IF NOT EXISTS idx_upload_sessions_owner_issue
    ON upload_sessions(owner_user_id, issue_code, status, updated_at DESC);

CREATE INDEX IF NOT EXISTS idx_upload_sessions_finalizing
    ON upload_sessions(status, updated_at);
