CREATE TABLE IF NOT EXISTS dataset_registry (
    dataset_id text PRIMARY KEY,
    status text NOT NULL CHECK (status IN ('staged','active','retired')),
    origin text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX IF NOT EXISTS dataset_registry_one_active_idx
    ON dataset_registry ((status))
    WHERE status = 'active';

CREATE TABLE IF NOT EXISTS synthetic_patients (
    patient_id text PRIMARY KEY,
    age_years integer,
    sex text,
    visit_date date,
    specialist text,
    icd10_code text,
    prescription_recorded text,
    source_record_id text NOT NULL
);

CREATE INDEX IF NOT EXISTS synthetic_patients_source_record_id_idx
    ON synthetic_patients (source_record_id);

CREATE TABLE IF NOT EXISTS dev_identities (
    principal_id text PRIMARY KEY,
    role text NOT NULL,
    department_scope text,
    token_sha256 bytea,
    active boolean NOT NULL DEFAULT true
);

CREATE TABLE IF NOT EXISTS audit_events (
    audit_event_id uuid PRIMARY KEY,
    request_id text NOT NULL,
    principal_id text,
    action text NOT NULL,
    target_type text NOT NULL,
    target_id text,
    outcome text NOT NULL,
    occurred_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS queue_items (
    queue_item_id uuid PRIMARY KEY,
    dataset_id text NOT NULL,
    patient_id text NOT NULL REFERENCES synthetic_patients(patient_id),
    encounter_id text NOT NULL,
    doctor_id text NOT NULL REFERENCES dev_identities(principal_id),
    status text NOT NULL CHECK (status IN ('active','completed','cancelled')),
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (patient_id, encounter_id, doctor_id, status)
);

CREATE INDEX IF NOT EXISTS queue_items_doctor_status_idx
    ON queue_items (doctor_id, status, created_at DESC);

CREATE TABLE IF NOT EXISTS summary_jobs (
    summary_job_id uuid PRIMARY KEY,
    dataset_id text NOT NULL,
    patient_id text NOT NULL REFERENCES synthetic_patients(patient_id),
    encounter_id text NOT NULL,
    requested_by text NOT NULL REFERENCES dev_identities(principal_id),
    reason text NOT NULL CHECK (reason IN ('nurse_preload','doctor_refresh')),
    idempotency_key text NOT NULL,
    request_hash bytea NOT NULL,
    status text NOT NULL CHECK (status IN ('queued','running','ready','failed')),
    failure_code text,
    attempt_count integer NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    lease_owner text,
    lease_expires_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (requested_by, idempotency_key)
);

ALTER TABLE summary_jobs ADD COLUMN IF NOT EXISTS attempt_count integer NOT NULL DEFAULT 0;
ALTER TABLE summary_jobs ADD COLUMN IF NOT EXISTS lease_owner text;
ALTER TABLE summary_jobs ADD COLUMN IF NOT EXISTS lease_expires_at timestamptz;

CREATE TABLE IF NOT EXISTS summaries (
    summary_id uuid PRIMARY KEY,
    summary_job_id uuid NOT NULL UNIQUE REFERENCES summary_jobs(summary_job_id),
    dataset_id text NOT NULL,
    patient_id text NOT NULL REFERENCES synthetic_patients(patient_id),
    encounter_id text NOT NULL,
    generator_version text NOT NULL,
    source_set jsonb NOT NULL,
    source_set_sha256 text NOT NULL,
    claims jsonb NOT NULL,
    review_state text NOT NULL DEFAULT 'unreviewed',
    version integer NOT NULL DEFAULT 1,
    generated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS feedback (
    feedback_id uuid PRIMARY KEY,
    summary_id uuid NOT NULL REFERENCES summaries(summary_id),
    doctor_id text NOT NULL REFERENCES dev_identities(principal_id),
    rating text NOT NULL CHECK (rating IN ('Useful','Incomplete','Incorrect','Unsafe')),
    reason text NOT NULL CHECK (char_length(reason) BETWEEN 1 AND 2000),
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS import_jobs (
    import_job_id uuid PRIMARY KEY,
    fixture_reference text NOT NULL,
    expected_rows bigint NOT NULL CHECK (expected_rows > 0),
    imported_rows bigint NOT NULL DEFAULT 0,
    rejected_rows bigint NOT NULL DEFAULT 0,
    status text NOT NULL CHECK (status IN ('queued','running','ready','failed')),
    failure_code text,
    checksum_sha256 text,
    schema_version text NOT NULL DEFAULT 'doctor-360-v1',
    requested_by text NOT NULL REFERENCES dev_identities(principal_id),
    created_at timestamptz NOT NULL DEFAULT now(),
    finished_at timestamptz
);

ALTER TABLE import_jobs ADD COLUMN IF NOT EXISTS checksum_sha256 text;
ALTER TABLE import_jobs ADD COLUMN IF NOT EXISTS schema_version text NOT NULL DEFAULT 'doctor-360-v1';

CREATE TABLE IF NOT EXISTS import_staging (
    import_job_id uuid NOT NULL REFERENCES import_jobs(import_job_id),
    row_number bigint NOT NULL,
    patient_id text NOT NULL,
    age_years integer NOT NULL,
    sex text NOT NULL,
    visit_date date NOT NULL,
    specialist text NOT NULL,
    icd10_code text NOT NULL,
    prescription_recorded text NOT NULL,
    source_record_id text NOT NULL,
    PRIMARY KEY (import_job_id, row_number),
    UNIQUE (import_job_id, patient_id),
    UNIQUE (import_job_id, source_record_id)
);

CREATE TABLE IF NOT EXISTS import_rejects (
    import_job_id uuid NOT NULL REFERENCES import_jobs(import_job_id),
    row_number bigint NOT NULL,
    reason_code text NOT NULL,
    PRIMARY KEY (import_job_id, row_number)
);
