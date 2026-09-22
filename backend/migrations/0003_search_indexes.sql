-- Search publication metadata. Existing bundles remain on the SQLite FTS
-- backend until an explicit publication flow selects another backend.
CREATE TABLE IF NOT EXISTS bundle_search_indexes (
    bundle_id TEXT PRIMARY KEY REFERENCES bundles(id) ON DELETE CASCADE,
    backend TEXT NOT NULL DEFAULT 'sqlite_fts'
        CHECK (backend IN ('sqlite_fts', 'tantivy')),
    schema_version INTEGER NOT NULL DEFAULT 1 CHECK (schema_version > 0),
    tokenizer_version INTEGER NOT NULL DEFAULT 1 CHECK (tokenizer_version > 0),
    generation INTEGER NOT NULL DEFAULT 0 CHECK (generation >= 0),
    artifact_key TEXT,
    state TEXT NOT NULL DEFAULT 'LEGACY'
        CHECK (state IN ('LEGACY', 'BUILDING', 'READY', 'FAILED', 'NEEDS_REBUILD')),
    built_at TEXT,
    last_error_code TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_bundle_search_indexes_state
    ON bundle_search_indexes(state, updated_at);

INSERT OR IGNORE INTO bundle_search_indexes (
    bundle_id, backend, schema_version, tokenizer_version, generation, state
)
SELECT id, 'sqlite_fts', 1, 1, 0, 'LEGACY'
FROM bundles;

