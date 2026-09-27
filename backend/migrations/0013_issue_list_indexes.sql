-- Keep the cursor-paginated Issue list ordered without scanning and sorting
-- every active row. The status key makes the preferred plan deterministic
-- even before SQLite has collected table statistics.
CREATE INDEX IF NOT EXISTS idx_issues_active_created_code
    ON issues (status, created_at DESC, code DESC)
    WHERE status = 'ACTIVE';

CREATE INDEX IF NOT EXISTS idx_issues_active_owner_created_code
    ON issues (owner_user_id, status, created_at DESC, code DESC)
    WHERE status = 'ACTIVE';
