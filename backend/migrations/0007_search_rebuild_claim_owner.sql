-- A rebuild owner token prevents a stale worker from publishing after another
-- worker has reclaimed the same pending generation.
ALTER TABLE bundle_search_indexes
    ADD COLUMN pending_owner TEXT;
