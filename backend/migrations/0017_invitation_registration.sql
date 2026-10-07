ALTER TABLE system_settings
    ADD COLUMN registration_requires_invite INTEGER NOT NULL DEFAULT 1
    CHECK (registration_requires_invite IN (0, 1));

CREATE TABLE invitations (
    id TEXT PRIMARY KEY,
    batch_id TEXT NOT NULL,
    code_hash TEXT NOT NULL UNIQUE,
    note TEXT NOT NULL DEFAULT '',
    created_by TEXT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    expires_at TEXT,
    revoked_at TEXT,
    revoked_by TEXT REFERENCES users(id) ON DELETE RESTRICT,
    used_at TEXT,
    used_by TEXT UNIQUE REFERENCES users(id) ON DELETE RESTRICT,
    CHECK ((revoked_at IS NULL) = (revoked_by IS NULL)),
    CHECK ((used_at IS NULL) = (used_by IS NULL)),
    CHECK (revoked_at IS NULL OR used_at IS NULL)
);

CREATE INDEX idx_invitations_created_at_id ON invitations(created_at DESC, id DESC);
CREATE INDEX idx_invitations_batch_id ON invitations(batch_id, created_at DESC, id DESC);
