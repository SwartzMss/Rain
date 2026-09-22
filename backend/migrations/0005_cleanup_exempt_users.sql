ALTER TABLE system_settings ADD COLUMN cleanup_exempt_usernames_json TEXT NOT NULL DEFAULT '[]';
ALTER TABLE system_settings ADD COLUMN cleanup_exempt_users_initialized INTEGER NOT NULL DEFAULT 0 CHECK (cleanup_exempt_users_initialized IN (0, 1));
