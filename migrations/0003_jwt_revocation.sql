CREATE TABLE IF NOT EXISTS revoked_jtis (
    jti text PRIMARY KEY,
    expires_at timestamptz NOT NULL,
    revoked_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS revoked_jtis_expires_at_idx
    ON revoked_jtis (expires_at);
