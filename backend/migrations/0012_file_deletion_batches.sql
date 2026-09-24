CREATE TABLE IF NOT EXISTS file_deletion_batches (
    id TEXT PRIMARY KEY,
    requested_by_user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    state TEXT NOT NULL DEFAULT 'QUEUED' CHECK (state IN ('QUEUED', 'RUNNING', 'SUCCEEDED', 'PARTIAL', 'FAILED')),
    total_items INTEGER NOT NULL CHECK (total_items > 0),
    completed_items INTEGER NOT NULL DEFAULT 0 CHECK (completed_items >= 0),
    failed_items INTEGER NOT NULL DEFAULT 0 CHECK (failed_items >= 0),
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    finished_at TEXT
);

CREATE TABLE IF NOT EXISTS file_deletion_batch_items (
    id TEXT PRIMARY KEY,
    batch_id TEXT NOT NULL REFERENCES file_deletion_batches(id) ON DELETE CASCADE,
    bundle_id TEXT NOT NULL REFERENCES bundles(id) ON DELETE CASCADE,
    root_file_id INTEGER NOT NULL,
    state TEXT NOT NULL DEFAULT 'QUEUED' CHECK (state IN ('QUEUED', 'RUNNING', 'SUCCEEDED', 'FAILED')),
    job_id TEXT REFERENCES file_deletion_jobs(id) ON DELETE SET NULL,
    error_code TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    finished_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_file_deletion_batches_active
    ON file_deletion_batches(state, updated_at);

CREATE INDEX IF NOT EXISTS idx_file_deletion_batch_items_batch
    ON file_deletion_batch_items(batch_id, state, updated_at);

CREATE UNIQUE INDEX IF NOT EXISTS idx_file_deletion_batch_items_target
    ON file_deletion_batch_items(batch_id, bundle_id, root_file_id);

DROP VIEW IF EXISTS visible_files;

CREATE VIEW visible_files AS
WITH RECURSIVE deleting_tree(id, bundle_id) AS (
    SELECT root_file_id, bundle_id
    FROM file_deletion_jobs
    WHERE state IN ('QUEUED', 'RUNNING', 'RETRY_WAIT')
    UNION
    SELECT root_file_id, bundle_id
    FROM file_deletion_batch_items
    WHERE state IN ('QUEUED', 'RUNNING')
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
