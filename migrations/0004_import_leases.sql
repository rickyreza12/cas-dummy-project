ALTER TABLE import_jobs ADD COLUMN IF NOT EXISTS lease_owner text;
ALTER TABLE import_jobs ADD COLUMN IF NOT EXISTS lease_expires_at timestamptz;

CREATE INDEX IF NOT EXISTS import_jobs_claim_idx
    ON import_jobs (status, lease_expires_at, created_at);
