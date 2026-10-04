CREATE TABLE temp_result_leases (
    id TEXT PRIMARY KEY,
    temp_result_id TEXT NOT NULL REFERENCES temp_results(id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_temp_result_leases_result_expiry
    ON temp_result_leases (temp_result_id, expires_at);

CREATE INDEX idx_temp_result_leases_expiry
    ON temp_result_leases (expires_at);
