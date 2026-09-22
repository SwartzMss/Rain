CREATE TABLE IF NOT EXISTS file_deletion_jobs (
    id TEXT PRIMARY KEY,
    bundle_id TEXT NOT NULL REFERENCES bundles(id) ON DELETE CASCADE,
    root_file_id INTEGER NOT NULL,
    requested_by_user_id TEXT REFERENCES users(id) ON DELETE SET NULL,
    state TEXT NOT NULL DEFAULT 'QUEUED' CHECK (state IN ('QUEUED', 'RUNNING', 'RETRY_WAIT', 'SUCCEEDED', 'SUPERSEDED')),
    phase TEXT NOT NULL DEFAULT 'WALK' CHECK (phase IN ('WALK', 'OFFSETS', 'SEGMENTS', 'REMOVE_NODE', 'RECONCILE')),
    cursor_file_id INTEGER,
    lease_token TEXT,
    lease_until TEXT,
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    next_retry_at TEXT,
    last_error_code TEXT,
    reconcile_after_id INTEGER NOT NULL DEFAULT 0,
    reconcile_bytes INTEGER NOT NULL DEFAULT 0 CHECK (reconcile_bytes >= 0),
    deleted_files INTEGER NOT NULL DEFAULT 0 CHECK (deleted_files >= 0),
    deleted_offsets INTEGER NOT NULL DEFAULT 0 CHECK (deleted_offsets >= 0),
    deleted_segments INTEGER NOT NULL DEFAULT 0 CHECK (deleted_segments >= 0),
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    finished_at TEXT
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_file_deletion_jobs_active_bundle
    ON file_deletion_jobs(bundle_id)
    WHERE state IN ('QUEUED', 'RUNNING', 'RETRY_WAIT');

CREATE INDEX IF NOT EXISTS idx_file_deletion_jobs_due
    ON file_deletion_jobs(state, next_retry_at, updated_at);

CREATE INDEX IF NOT EXISTS idx_file_deletion_jobs_bundle_root
    ON file_deletion_jobs(bundle_id, root_file_id, created_at);

CREATE INDEX IF NOT EXISTS idx_files_bundle_parent_id
    ON files(bundle_id, parent_id, id);

CREATE VIEW IF NOT EXISTS visible_files AS
WITH RECURSIVE deleting_tree(id, bundle_id) AS (
    SELECT root_file_id, bundle_id
    FROM file_deletion_jobs
    WHERE state IN ('QUEUED', 'RUNNING', 'RETRY_WAIT')
    UNION
    SELECT files.id, files.bundle_id
    FROM files
    JOIN deleting_tree ON deleting_tree.bundle_id = files.bundle_id
        AND files.parent_id = deleting_tree.id
)
SELECT files.*
FROM files
WHERE NOT EXISTS (
    SELECT 1
    FROM deleting_tree
    WHERE deleting_tree.id = files.id
      AND deleting_tree.bundle_id = files.bundle_id
);
