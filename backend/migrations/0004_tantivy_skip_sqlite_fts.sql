-- Tantivy-owned Bundles do not need to duplicate every cleaned chunk into the
-- SQLite FTS5 shadow table. Legacy and SQLite-owned Bundles keep the original
-- trigger behavior, so Issue-wide SQLite search remains unchanged for them.
DROP TRIGGER IF EXISTS log_segments_fts_ai;
DROP TRIGGER IF EXISTS log_segments_fts_ad;
DROP TRIGGER IF EXISTS log_segments_fts_au;

CREATE TRIGGER log_segments_fts_ai AFTER INSERT ON log_segments
WHEN NOT EXISTS (
    SELECT 1 FROM bundle_search_indexes
    WHERE bundle_search_indexes.bundle_id = new.bundle_id AND backend = 'tantivy'
)
BEGIN
    INSERT INTO log_segments_fts(rowid, content) VALUES (new.id, new.content);
END;

CREATE TRIGGER log_segments_fts_ad AFTER DELETE ON log_segments
WHEN NOT EXISTS (
    SELECT 1 FROM bundle_search_indexes
    WHERE bundle_search_indexes.bundle_id = old.bundle_id AND backend = 'tantivy'
)
BEGIN
    INSERT INTO log_segments_fts(log_segments_fts, rowid, content)
    VALUES ('delete', old.id, old.content);
END;

CREATE TRIGGER log_segments_fts_au AFTER UPDATE OF content ON log_segments
WHEN NOT EXISTS (
    SELECT 1 FROM bundle_search_indexes
    WHERE bundle_search_indexes.bundle_id = new.bundle_id AND backend = 'tantivy'
)
BEGIN
    INSERT INTO log_segments_fts(log_segments_fts, rowid, content)
    VALUES ('delete', old.id, old.content);
    INSERT INTO log_segments_fts(rowid, content) VALUES (new.id, new.content);
END;
