-- Keep filesystem replacement serialized after a rebuild has entered its
-- publication window; stale workers may only reclaim BUILDING claims.
ALTER TABLE bundle_search_indexes
    ADD COLUMN pending_phase TEXT NOT NULL DEFAULT 'BUILDING'
        CHECK (pending_phase IN ('BUILDING', 'PUBLISHING'));
