-- Count only non-deleted Bundles for each Issue without walking historical rows.
CREATE INDEX IF NOT EXISTS idx_bundles_active_issue
    ON bundles (issue_code)
    WHERE deleted_at IS NULL;
