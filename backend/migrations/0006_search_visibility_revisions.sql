ALTER TABLE bundle_search_indexes
    ADD COLUMN visibility_revision INTEGER NOT NULL DEFAULT 0
        CHECK (visibility_revision >= 0);

ALTER TABLE bundle_search_indexes
    ADD COLUMN compacted_revision INTEGER NOT NULL DEFAULT 0
        CHECK (compacted_revision >= 0);

ALTER TABLE bundle_search_indexes
    ADD COLUMN pending_generation INTEGER
        CHECK (pending_generation IS NULL OR pending_generation > 0);

ALTER TABLE bundle_search_indexes
    ADD COLUMN pending_revision INTEGER
        CHECK (pending_revision IS NULL OR pending_revision >= 0);

ALTER TABLE bundle_search_indexes
    ADD COLUMN pending_state TEXT NOT NULL DEFAULT 'IDLE'
    CHECK (pending_state IN ('IDLE', 'BUILDING', 'FAILED', 'CLEANING'));

CREATE INDEX IF NOT EXISTS idx_bundle_search_rebuild_due
    ON bundle_search_indexes(state, visibility_revision, compacted_revision);

CREATE TABLE IF NOT EXISTS bundle_search_artifacts (
    bundle_id TEXT NOT NULL REFERENCES bundles(id) ON DELETE CASCADE,
    generation INTEGER NOT NULL CHECK (generation > 0),
    state TEXT NOT NULL CHECK (state IN ('ACTIVE', 'RETIRED')),
    active_readers INTEGER NOT NULL DEFAULT 0 CHECK (active_readers >= 0),
    retired_at TEXT,
    cleanup_claimed_at TEXT,
    PRIMARY KEY (bundle_id, generation)
);

INSERT OR IGNORE INTO bundle_search_artifacts(bundle_id, generation, state)
SELECT bundle_id, generation, 'ACTIVE'
FROM bundle_search_indexes
WHERE backend = 'tantivy' AND state = 'READY' AND generation > 0;
