-- Support the per-user storage aggregate used by the Admin Users page.
-- The partial predicate matches the aggregate exactly, so deleted and
-- non-active bundles do not occupy this index.
CREATE INDEX IF NOT EXISTS idx_bundles_uploader_active
    ON bundles (uploader_user_id)
    WHERE deleted_at IS NULL
      AND status IN ('READY', 'PROCESSING');
