ALTER TABLE temp_results ADD COLUMN issue_code TEXT;

CREATE TABLE issue_workspace_sessions (
    id TEXT PRIMARY KEY,
    issue_code TEXT NOT NULL REFERENCES issues(code) ON DELETE CASCADE,
    subject_key TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'ACTIVE' CHECK (state IN ('ACTIVE', 'ENDED')),
    created_at TEXT NOT NULL,
    last_activity_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    ended_at TEXT
);

CREATE INDEX idx_issue_workspace_sessions_subject_expiry
    ON issue_workspace_sessions (subject_key, expires_at);

CREATE TABLE temp_result_workspace_refs (
    session_id TEXT NOT NULL REFERENCES issue_workspace_sessions(id) ON DELETE CASCADE,
    result_id TEXT NOT NULL REFERENCES temp_results(id) ON DELETE CASCADE,
    created_at TEXT NOT NULL,
    PRIMARY KEY (session_id, result_id)
);

CREATE INDEX idx_temp_result_workspace_refs_result
    ON temp_result_workspace_refs (result_id, session_id);
