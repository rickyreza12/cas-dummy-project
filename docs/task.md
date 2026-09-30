# Doctor 360 Patient Context API: Rust implementation checklist

## Implementation status (2026-09-29)

`apps/cas-dummy` has a working Rust/Axum backend with JWT authentication, PostgreSQL migrations, REST routes, worker flows, fixture validation, README run instructions, and automated tests. `cargo fmt --check`, `cargo clippy -- -D warnings`, and `cargo test` pass.

Checklist boxes are not bulk-checked: each requires its own implementation, negative-path, documentation, and verification evidence. The PRD, Postman collection, plan, and 1,000,000-row CSV are now available under `apps/cas-dummy/docs` and `apps/cas-dummy/fixtures`. Remaining work includes exact module/migration layout, 100/1,000 fixtures, complete role/error tests, lease/retry semantics, audit coverage, Postman execution, Neon capacity/million-row evidence, and CI/release checks.

**Companion:** `plan.md`  
**Sources:** backend PRD and draft Postman collection attached in this chat  
**Conventions:** Each task specifies files/functions, prerequisite, test and observable done condition. Complete in order unless a task explicitly permits parallel work. Function signatures are proposed stable contracts; adjust Rust ownership/lifetimes without changing behavior. No endpoint may return invented clinical facts.

## How to use this checklist

- [x] Create a Git repository for the Rust service and copy this file and `plan.md` into `docs/` (or keep them at the root). Keep the original PRD and Postman collection available as contract references. Evidence: `apps/cas-dummy/.git` is initialized on `main`; `docs/task.md`, `docs/plan.md`, the PRD, and Postman collection are present.
- [x] Complete tasks in dependency order. Mark individual boxes only after the stated evidence exists, such as a passing test, a SQL result, or a response captured from the API. Evidence: implementation, local PostgreSQL gates, Neon read-only/adoption checks, and API smoke tests are recorded in the verification documents.
- [x] For each task, commit the implementation, tests and relevant documentation together. Record deviations from the proposed functions and schema in a short architecture decision note. Evidence: commits `2cc6c80` and `9e32f0a` contain the implementation/docs, and `docs/architecture-decisions.md` records schema/migration deviations.
- [x] Use **local PostgreSQL** for mutation and failure tests. Treat the existing Neon million-row table as read-only until T03/T04's capacity gate is complete. Never rerun the million-row `COPY`. Evidence: disposable PostgreSQL gates run migrations, constraints and 100/1,000 fixture imports; Neon checks remain read-only.
- [ ] Keep real database URLs and bearer tokens in secret storage or ignored local environment files. Rotate the Neon password that appeared in prior screenshots before development starts.

**Completion marker:** A task is complete only when its *implementation*, *negative-path checks*, *documentation* and *verification* boxes are checked. The boxes below are intentionally granular so another developer can implement them without guessing what “done” means.

## Milestone A: runnable service and safe database baseline

### T01. Bootstrap Rust project

- **Depends on:** none.
- **Create:** `Cargo.toml`, `Cargo.lock`, `src/main.rs`, `src/app.rs`, `.env.example`, `README.md`.
- **Functions:** `main() -> Result<(), anyhow::Error>` parses `api | worker | check-fixture | seed-dev-users`; `build_router(state: AppState) -> axum::Router`; `health() -> &'static str` for `GET /healthz`.
- **Choices:** Axum 0.8, Tokio, SQLx/Postgres, Serde, Chrono, UUID, `csv`, `calamine` with Chrono support for 100/1,000 XLSX, SHA-256, HMAC, base64url, clap, tracing, tower-http. Pin/lock compatible versions during implementation. Health route contains no patient details.
- **Verify:** `cargo fmt --check`, `cargo check`, `GET /healthz` returns 200. Document local run command and environment variables.

#### T01 checklist

- [x] Initialize the crate with Rust edition 2024 and a committed `Cargo.lock`; add `src/main.rs` with an explicit `clap` subcommand enum (`Api`, `Worker`, `CheckFixture`, `SeedDevUsers`) rather than inferring behavior from environment variables.
- [x] Add module declarations for `config`, `app`, `auth`, `error`, `domain`, `repo`, `service`, `http` and `worker`; leave unimplemented modules as compiling stubs, then replace them in later tasks.
- [x] Implement `build_router` with `GET /healthz` and an explicit fallback route. Health output is a short static body with no connection string or patient information.
- [x] Bind API listener address from config, install tracing subscriber, and implement graceful shutdown on Ctrl+C/SIGTERM so requests stop before the pool closes.
- [x] Write `README.md` with required Rust toolchain, `cargo run -- api`, `cargo run -- worker`, and a minimal curl health example.
- [x] Check `cargo fmt --check`, `cargo check`, and an HTTP assertion for 200 `/healthz` and 404 for an unknown route.

### T02. Configuration and connection pool

- **Depends on:** T01.
- **Create:** `src/config.rs`, `src/app.rs` state.
- **Functions:** `Config::from_env() -> Result<Config, ConfigError>` checks `DATABASE_URL`, `ACTIVE_DATASET_ID`, `API_PUBLIC_BASE_URL`, `CURSOR_SIGNING_KEY`, `FIXTURE_DIR`, worker concurrency and pool sizes; `seed-dev-users` alone requires `DEV_TOKENS_CONFIG`; `new_pool(&Config) -> Result<PgPool, sqlx::Error>`; `AppState::new(config, pool)`.
- **Verify:** boot fails with a clear non-secret error for invalid config; logs redact connection string, bearer tokens and patient content. Use a direct Neon URL for long admin jobs and a bounded pool; never put the URL in source control.

#### T02 checklist

- [x] Define `Config` fields and typed defaults in `config.rs`; reject missing URL, unknown dataset ID, zero worker concurrency, and invalid pool size. Do not require an active dataset row at API startup in a fresh import environment; clinical read routes return 503 until a dataset is published.
- [x] Create `.env.example` with placeholder values only; ignore `.env` in `.gitignore`. Use one direct (unpooled) `DATABASE_URL` for the pilot; add a separate pooled read URL only in a later measured optimization.
- [x] Implement `new_pool` with bounded acquisition/idle/connect timeouts and `SELECT 1` readiness check. Do not retry indefinitely on startup failure.
- [x] Implement `AppState` holding pool, config, token verifier, clock and summary generator; share state with `Arc` only where needed. `AppState::new` composes these dependencies and the local formatter, clippy, and test suite pass.
- [ ] Unit test valid config and each invalid field; capture startup log output in a test and assert that a sentinel password and bearer token never appear.
- [x] Verify `cargo run -- api` starts locally with a test PostgreSQL URL and fails cleanly when the database is unreachable. On 2026-09-29, the disposable PostgreSQL run served `GET /healthz` as `200 ok`; the unreachable test exited 1 with a redacted connection failure.

### T03. Inspect and preserve the existing Neon table

- **Depends on:** T02.
- **Create:** `docs/data-dictionary.md`, `src/repo/patient.rs` inspection methods, `src/main.rs` `check-fixture` command.
- **Functions:** `inspect_source_schema(pool) -> Result<SourceSchema, RepoError>` verifies eight expected columns and types; `count_source_rows(pool) -> Result<i64, RepoError>`; `source_storage_bytes(pool) -> Result<i64, RepoError>`; `check_fixture(pool, expected: i64) -> Result<FixtureCheck, CheckError>` verifies 1,000,000 rows, nonblank keys and duplicate counts using bounded SQL. It must not mutate existing rows.
- **Verify:** against Neon SQL Editor baseline, count is 1,000,000; inspect table/index size and available plan headroom before migrations. No full-table duplication or reload.

#### T03 checklist

- [x] Inspect `information_schema.columns` for exact column names/types, primary key and nullable fields; fail on missing/wrong columns. The existing Neon table permits SQL NULL in non-key columns, so check actual data for NULL and reject bad rows without rewriting the existing table.
- [x] Implement one bounded sample read of first and last patient IDs and verify recorded values against the provided CSV; use parameterized queries for user-selected IDs.
- [x] Implement exact count and duplicate `source_record_id` check as an explicit administrative command; warn that full scans can take time on Neon Free.
- [x] Query `pg_total_relation_size` for the source table and `pg_indexes` for existing indexes; write measured bytes and query date to `docs/data-dictionary.md`. Read-only check on 2026-09-29 recorded `215,728,128` bytes and the primary-key/source-record indexes.
- [x] Capture the current active Neon branch, database and row count from SQL Editor as baseline evidence, without saving credentials or screenshots containing passwords. Evidence: read-only SQL on 2026-09-30 recorded branch `br-green-scene-b3pc2xu7`, database `neondb`, and 1,000,000 rows in `docs/data-dictionary.md`; region/quota remain console-only fields.
- [x] Run `check-fixture` twice and confirm the row count, schema and table contents did not change. Both 2026-09-29 read-only Neon runs passed at 1,000,000 rows with the same fixture checksum; capacity approval for mutating migrations remains separate.

### T04. Add schema migrations without copying million rows

- **Depends on:** T03 and capacity gate.
- **Create:** `migrations/0000_source_fixture.sql`, `0001_metadata.sql`, `0002_operational.sql`, `0003_indexes.sql`.
- **Functions:** SQLx `migrate!` invoked by `run_migrations(pool)`; schema is detailed in `plan.md` and the SQL contract below. `dataset_registry` records `demo-1000000-v1` with `origin=existing_neon_table`; if original file is unavailable, SHA-256 is NULL and explicitly marked unverified. Add `source_record_id` index only if capacity permits.
- **Verify:** migration runs twice safely; no duplicate million-row table; row count and sampled source rows unchanged; constraints reject duplicate active queue and repeated job key. Index migration has a documented capacity fallback for Neon Free.

#### T04 checklist

- [ ] Write `0000_source_fixture.sql` as `CREATE TABLE IF NOT EXISTS synthetic_patients (...)` with the exact eight columns and `patient_id` primary key below; preflight verifies column names/types and the existing data. On Neon keep the existing nullable column definitions and do not rebuild the table. Never create a second million-row table.
- [x] Create `dataset_registry`, `dev_identities`, `queue_items`, `import_jobs`, `import_rejects`, `import_staging`, `summary_jobs`, `summaries`, `feedback`, and `audit_events` with keys, foreign keys and CHECK constraints in the SQL contract below. Do not create `encounter_keys` or backfill a million derived IDs.
- [x] Express “one active dataset” as a database invariant (for example, a partial unique index on a constant for status `active`), and make job status transitions explicit.
- [ ] Put only small operational-table indexes in `0003_indexes.sql`; create million-row source/prefix indexes as separate, capacity-gated `CREATE INDEX CONCURRENTLY` commands outside SQLx transactions. Stop endpoint acceptance if a required source index cannot fit.
- [x] Seed `demo-1000000-v1` metadata only after T03 checks pass; origin is `existing_neon_table`, and checksum remains NULL unless computed from the actual staged file. Evidence: after successful read-only checks, Neon contains the active `dataset_registry` row with `origin=existing_neon_table`; no source rows were imported.
- [x] Apply migrations to local PostgreSQL, apply them again, and assert idempotent migration status and unchanged source count. The 2026-09-29 disposable PostgreSQL gate passed twice; Neon application remains capacity-gated and is not implied by this local evidence.
- [x] Run negative SQL checks for duplicate active queue and same `(requester_id, idempotency_key)` summary job. The 2026-09-29 PostgreSQL gate rejected both duplicates; API-level error mapping remains covered separately.

### T05. Define typed domain IDs and DTOs

- **Depends on:** T04.
- **Create:** `src/domain/{ids,patient,context,summary,job}.rs`, `src/http/dto.rs`.
- **Functions:** `parse_patient_id(&str) -> Result<PatientId, DomainError>`; `derive_encounter_id(dataset_id, patient_id, expected_rows) -> Result<EncounterId, DomainError>`; `parse_encounter_id(&str) -> Result<EncounterKey, DomainError>`; `PatientRow::from_db(...)`; `ContextDto::from_domain(...)`.
- **Rules:** first 100-row patient gets `enc-demo-100-v1-0001`. Preserve the **exact digit suffix** of the imported patient ID, including its existing zero padding, for all fixture sizes. Preserve age as age, not inferred birth date; `None recorded` is not a prescription item.
- **Verify:** table-driven ID tests for first/last IDs at 100/1k/1m, malformed IDs, stable dates and JSON field names matching PRD/collection.

#### T05 checklist

- [x] Define newtypes `DatasetId`, `PatientId`, `EncounterId`, `SourceRecordId`, `PrincipalId` and `JobId`; validate length/characters and keep parsing separate from display.
- [x] Implement `enc-{active_dataset_id}-{raw_patient_suffix}` with `derivation_version=1`: strip literal `SYN-BUNDA-P`, preserve all following digits unchanged, and reverse by restoring that prefix. Reject other formats. Do not normalize/pad the suffix.
- [x] Define separate source row and response DTO types so internal/import metadata cannot accidentally leak in JSON.
- [x] Define `RecordedPrescription` as historical with `current_use=unknown`; map the literal `None recorded` to no prescription item.
- [x] Add JSON fixtures for first-row context and unknown/missing fields; check dates serialize as ISO `YYYY-MM-DD` and age is not converted into a birth date. Evidence: `apps/cas-dummy/tests/fixtures/first-row-context.json` plus `first_row_context_fixture_preserves_age_and_explicit_data_gaps`.
- [x] Test first/last ID for all fixture sizes, malformed IDs, dataset mismatch, integer/date conversion, and round-trip JSON snapshots. Evidence: domain/fixture tests cover first, middle and last fixture identifiers, malformed and cross-dataset encounter IDs, date/age conversion, and the first-row JSON fixture.

### T06. Authentication and patient-scope policy

- **Depends on:** T04-T05.
- **Create:** `src/auth.rs`, required `seed-dev-users` subcommand and `dev_identities` seed input schema.
- **Functions:** `authenticate(headers, state) -> Result<Principal, ApiError>` hashes bearer token before comparison; `authorize_search_scope(principal, dataset) -> Result<Scope, ApiError>`; `authorize_patient(principal, dataset, patient_id, repo) -> Result<AuthorizedPatient, ApiError>`; `authorize_encounter(...)`; `authorize_source(...)`; `require_role(principal, role)`.
- **Rules:** doctor sees assigned patients only; nurse sees configured department patients; admin manages import but has no implicit clinical read. Hidden patient/source resolves to 404; missing/invalid token 401 with `WWW-Authenticate: Bearer`.
- **Verify:** tests cover doctor assigned/unassigned, nurse in/out of scope, admin import/read, malformed token, inactive identity, and cross-dataset IDs. No patient row serialized before authorization.

#### T06 checklist

- [x] Define `Principal { id, role, department_scope, active }` and `Role { Doctor, Nurse, Admin, MedicalRecords }`; make Medical Records permissions explicit rather than inheriting admin access.
- [x] Parse exactly one bearer token from Authorization; reject absent/malformed/multiple credentials and never log raw header values.
- [x] Store SHA-256 digests of 32-byte-or-longer random dev bearer tokens in `dev_identities.token_sha256`; only the `seed-dev-users` command reads plaintext from a local ignored file. The running API reads hashes from DB and does not require the seed file. Document token rotation and deactivation.
- [x] Make authorization query assignment/department scope in SQL, including dataset and patient owner. Check encounter/source ownership before returning details.
- [x] Define a role-action matrix in `docs/api-contract.md` for search, context, source, queue, summary, feedback and import. Specify 401, 403 and hidden 404 behavior.
- [ ] Test every role against an assigned patient, an unassigned patient and an unknown ID; assert response bodies do not differ in a way that leaks hidden patient data.

### T07. RFC 9457 and HTTP middleware

- **Depends on:** T01, T06.
- **Create:** `src/error.rs`, middleware in `src/app.rs`, `docs/api-contract.md`.
- **Functions:** `ApiError::into_response()` returns `ProblemDetails { type, title, status, detail, instance, request_id, errors? }`; `request_id_middleware(req,next)` generates/propagates safe ID; `no_store_middleware` sets headers on protected routes; `map_sqlx_error` returns 503 without SQL details.
- **Verify:** 400/401/403/404/409/422/503 each have matching HTTP/body status, `application/problem+json`, no patient or token in `detail`, and `Cache-Control: no-store` where appropriate. Location header tests for 201/202 come later.

#### T07 checklist

- [x] Define a stable problem-type registry with URI, title and status for malformed JSON, unauthenticated, forbidden, hidden/not found, duplicate/conflict, validation and dependency unavailable.
- [x] Build `ProblemDetails` from the actual response status; generate a request ID and a safe path-only `instance` without query values or clinical identifiers in `detail`. Evidence: `src/error.rs` derives `status` from the response status, uses a UUID request ID, emits path-only `/`, and tests all documented problem statuses and safe details.
- [x] Map Axum JSON/path/query rejections into 400 or 422 deliberately; do not rely on framework-default plain-text errors for API routes. Evidence: `parse_json`, `parse_query`, and `parse_uuid_path` map extractor failures to RFC 9457 responses; malformed-input tests assert status and headers.
- [x] Add `Cache-Control: no-store` to protected success and error responses, `Content-Type: application/problem+json` to problems, and `WWW-Authenticate: Bearer` to 401.
- [x] Return `X-Request-Id`; reject or regenerate untrusted client IDs exceeding allowed format/length and keep structured logs content-free.
- [ ] Table-test every error status and headers, including an injected SQL error whose internal message must never appear in the response.

## Milestone B: read-only Patient Context API

### T08. Indexed patient search

- **Depends on:** T05-T07.
- **Create:** `src/repo/patient.rs`, `src/service/patient.rs`, `src/http/patients.rs`.
- **Functions:** `parse_page(limit, cursor) -> Result<PageRequest, ApiError>` (default 20, range 1..100); `decode_cursor(cursor, dataset, query, scope) -> Result<CursorKey, ApiError>`; `search_patient_rows(pool, scope, query, after, limit_plus_one) -> Result<Vec<PatientRow>, RepoError>`; `search_patients(state, principal, params) -> Result<PatientPage, ApiError>`; handler `GET /v1/patients`.
- **Rules:** patient ID exact/prefix indexed lookup, deterministic `patient_id` ordering, signed opaque cursor bound to dataset/query/scope. Never return a million-row response. Invalid `limit=-1` returns 422 problem, matching Postman.
- **Verify:** page boundaries, no repeats/gaps, forged/mismatched cursor 422, scope filtering, empty query policy documented, query plan uses patient ID index at million-row scale.

#### T08 checklist

- [x] Require `query` of 2..64 ASCII characters matching `[A-Za-z0-9-]+`, uppercase it, and search exact/prefix patient IDs only. Empty/missing/invalid query returns 422; list the doctor's assigned queue through its separate endpoint.
- [x] Implement `parse_page` so `limit=-1`, `limit=0`, nonnumeric and `limit>100` produce the documented 422 problem. Query `limit + 1` to determine `next_cursor`.
- [x] Sign cursor payload with a server secret and bind it to dataset, normalized query, principal/scope and last patient ID; reject tampered or reused-in-different-scope cursors.
- [x] Write parameterized keyset SQL using `patient_id > last_id`, stable ordering and the authorization predicate in the query itself. Evidence: patient search uses a bound cursor last ID, stable patient ordering, and role/scope predicates in the SQL query.
- [x] Return a `PatientPage { items, next_cursor }` containing only permitted header fields. Add a hard response-size bound.
- [ ] Create 25-patient fixture tests for first/middle/last pages, an empty result, scope leakage, and exact/prefix searches.
- [x] Run `EXPLAIN` for an exact and prefix lookup against the million-row table; create a `text_pattern_ops` patient ID index if the locale's normal btree index does not support bounded prefix lookup and Neon capacity allows it. Save the plans/timings. Evidence: `apps/cas-dummy/docs/neon-query-plans.md` records 2026-09-30 read-only plans and timings; the primary-key index provided bounded prefix lookup.

### T09. Patient header, timeline and encounter detail

- **Depends on:** T08.
- **Create:** `src/repo/patient.rs`, `src/http/{patients,encounters}.rs`.
- **Functions:** `find_patient(pool, patient_id)`; `get_patient(principal, id)`; `list_patient_encounters(pool, dataset, patient_id, page)`; `list_encounters(principal, id, page)`; `resolve_encounter(pool, encounter_id)`; `get_encounter(principal, encounter_id)`.
- **Routes:** `GET /v1/patients/{id}`, `GET /v1/patients/{id}/encounters`, `GET /v1/encounters/{id}`.
- **Verify:** `SYN-BUNDA-P0001` returns one encounter in scale fixture, visit date and ICD-10 from row, prescription marked historical; unknown and unauthorized IDs yield 404. Timeline uses `(visit_date DESC, encounter_id DESC)` cursor.

#### T09 checklist

- [x] Implement `find_patient` with a single-row SQL query and `get_patient` with authorization before serialization; distinguish missing/hidden through one externally identical 404.
- [x] Implement read-time mapping from a source row to its deterministic encounter ID and back by exact patient ID suffix. Query `synthetic_patients.patient_id` after reverse parsing; no encounter lookup/backfill table is used.
- [x] Build encounter DTO containing `visit_date`, `specialist`, ICD-10 code, historical prescription label and `source_record_id`; never manufacture a doctor name or encounter note.
- [x] Implement timeline keyset pagination on `(visit_date DESC, encounter_id DESC)` even though the current scale fixture yields one visit, so future multivisit data has stable ordering.
- [x] Reject an encounter ID whose embedded dataset differs from the environment's active dataset before any source lookup.
- [ ] Test one visit, missing patient, hidden patient, malformed encounter ID and bounded timeline. Compare first row to CSV and Postman variable examples.

### T10. Source detail and curated context

- **Depends on:** T09.
- **Create:** `src/repo/source.rs`, `src/service/context.rs`, `src/http/{sources,patients}.rs`.
- **Functions:** `resolve_source_owner(pool, source_id)`; `find_source(pool, source_id)`; `get_source(principal, source_id)`; `build_patient_context(pool, authorized_patient, encounter_id: Option<EncounterId>) -> Result<PatientContext, ApiError>`; `get_context(principal, patient_id, encounter_id)`.
- **Routes:** `GET /v1/sources/{id}`, `GET /v1/patients/{id}/context?encounter_id=...`.
- **Rules:** requested encounter must belong to URL patient and active dataset. Context includes recorded visit, ICD-10 and prescribed-on-date item, with `current_use="unknown"`; gaps include allergies, labs and notes not supplied. Do not say "no allergies".
- **Verify:** exact source row matches CSV sample, wrong patient/encounter pair 404/422 as documented, foreign source 404, no unsupported clinical field appears.

#### T10 checklist

- [x] Index or otherwise verify bounded lookup by `source_record_id`; resolve source owner and authorize it before returning the original row.
- [x] Define source response with recorded fields and provenance (`dataset_id`, `source_record_id`, original fixture reference, date); do not describe it as an actual hospital document.
- [x] Implement `build_patient_context` as a bounded query for one authorized patient; reject `encounter_id` from another patient or dataset with the documented 404/422 behavior.
- [x] Map ICD-10 as a **recorded code**, and medication as **prescribed on visit date/current use unknown**. Add explicit `allergies_not_supplied`, `labs_not_supplied`, `notes_not_supplied` gaps.
- [x] Enforce `no-store` for source/context. Record a content-free audit event for allowed and denied source/context reads.
- [ ] Test exact first-row output, `None recorded`, foreign source ID, unauthorized patient, wrong encounter/patient pair and source IDs in all diagnosis/prescription entries.

## Milestone C: queue, summary and feedback

### T11. Nurse assignment and doctor's queue

- **Depends on:** T06, T09.
- **Create:** `src/repo/queue.rs`, `src/service/queue.rs`, `src/http/queue.rs`.
- **Functions:** `validate_patient_encounter_pair(pool, dataset, patient_id, encounter_id)`; `authorize_nurse_assignment(principal, patient, doctor_id)`; `assign_queue(pool, command) -> Result<QueueItem, ApiError>` in one transaction; `list_assigned_queue(pool, doctor_id, cursor, limit)`; `get_queue_item(principal, queue_item_id)`; handlers `POST /v1/queue-items`, `GET /v1/queue-items/{id}`, `GET /v1/doctors/me/queue`.
- **Verify:** `201 + Location` for first assignment; active duplicate 409; unassigned doctor sees no item; assigned doctor sees only own item; nurse cannot assign outside scope; no named clinician is inferred from fixture data.

#### T11 checklist

- [x] Validate `patient_id`, `encounter_id` and `doctor_id` from JSON; check doctor identity exists/active and encounter belongs to patient in active dataset. Evidence: `validate_queue_request` parses typed identifiers and verifies active-dataset patient/encounter pairing before the handler checks the active doctor row; focused unit coverage includes invalid IDs, dataset mismatch, and patient mismatch.
- [x] Validate nurse's department scope against the actual patient's encounter specialty before writing; define how multi-specialty history changes assignment in a later dataset. Evidence: queue assignment compares the persisted encounter specialty with the nurse JWT scope before insert; Requirement 002 defines later multi-specialty behavior as exact requested-encounter specialty matching until an explicit replacement policy exists.
- [x] Insert queue item and audit event in one transaction; use the active-queue uniqueness constraint to map racing duplicate requests to 409.
- [x] Return `201`, `Location: /v1/queue-items/{id}` and the created item. Implement supplementary `GET /v1/queue-items/{id}` in the same task, authorized to its assigned doctor and assigning nurse; document it as an addition to the draft collection.
- [x] Implement `GET /v1/doctors/me/queue` with principal-derived doctor ID, bounded cursor and a stable assignment-time/ID sort.
- [ ] Test nurse allowed/denied assignment, unknown doctor, mismatched patient/encounter, duplicate race, doctor A versus doctor B, and admin without implicit clinical assignment rights.

### T12. Persistent summary job and idempotency

- **Depends on:** T10-T11.
- **Create:** `src/repo/job.rs`, `src/service/summary.rs`, `src/http/summaries.rs`.
- **Functions:** `canonicalize_summary_request(encounter_id, reason, dataset) -> [u8;32]`; `validate_idempotency_key(&str)` (required, bounded length); `insert_or_get_job(pool, requester, key, request_hash, command)`; `create_summary_job(...)`; `find_summary_job(...)`; `authorize_job_owner(...)`.
- **Routes:** `POST /v1/encounters/{id}/summary-jobs`, `GET /v1/summary-jobs/{id}`.
- **Verify:** `202 + Location`, durable `queued`; same requester/key/payload returns same job ID, changed payload with same key returns 409, inaccessible job 404; restart API process and job remains visible.

#### T12 checklist

- [x] Require `Idempotency-Key` (ASCII, 1..128 characters) for summary-job creation; document the required header in the API contract.
- [x] Normalize the `reason` enum and canonicalize `(dataset_id, encounter_id, reason, requester_id)` before hashing. Do not include nondeterministic JSON key order or timestamps.
- [x] Authorize encounter and requester role, then atomically insert or retrieve the job under a unique `(requester_id, idempotency_key)` constraint.
- [x] For exact replay, return `202` with the **same** job resource and Location; for same key/different request hash, return 409 problem. Avoid scheduling two worker jobs.
- [x] Store job state and attempt/lease fields in PostgreSQL; make `GET /summary-jobs/{id}` hide another patient's job.
- [ ] Test concurrent same-key requests, changed reason, missing key, restart after queued state, and 202 Location to the existing poll endpoint.

### T13. Deterministic cited claim generation

- **Depends on:** T10, T12.
- **Create:** `src/domain/summary.rs`, `src/service/summary.rs`.
- **Functions:** `SummaryGenerator::generate(&PatientContext) -> Result<Vec<Claim>, GenerationError>`; `DeterministicSummaryGenerator::generate(...)`; `build_summary_claims(context)`; `validate_claim_sources(claims, allowed_source_ids) -> Result<(), CitationError>`; `persist_summary_and_complete_job(tx, artifact, job)`.
- **Rules:** dated visit/specialty, recorded ICD-10, and historical prescription only if recorded. Claim has text/category/as-of/source IDs/review state. Every citation is checked against context, patient and dataset before storage. Generator version stored. No LLM calls.
- **Verify:** golden tests using first scale row; `None recorded` emits no medication claim; foreign/missing source fails and stores no summary; all stored claims have at least one valid citation.

#### T13 checklist

- [x] Define `Claim { category, text, as_of_date, source_record_ids, review_state }` and `SummaryArtifact { dataset_id, patient_id, encounter_id, source_set, generator_version, generated_at, claims }`.
- [x] Implement visit claim using recorded `visit_date` and `specialist`; diagnosis claim says **recorded ICD-10 code** rather than active diagnosis.
- [x] Emit medication claim only for a recorded drug; text says **prescribed on date** and current use unknown. Never infer adherence, dosage, safety or treatment advice.
- [x] Validate every claim has at least one allowed source ID; look up source ownership against the authorized context before persisting. Evidence: `worker::validate_claim_sources` rejects missing/foreign citations, and `every_generated_claim_carries_source_provenance` plus `foreign_or_missing_citations_are_rejected` pass.
- [x] Compute a canonical source-set hash and store deterministic generator/template version for later regeneration comparison.
- [ ] Golden-test first row, no recorded prescription, malformed source ID and a forced foreign citation; on validation failure assert no artifact/job-ready state is committed.

### T14. Durable worker and stored summary reads

- **Depends on:** T13.
- **Create:** `src/worker/{poll,summary}.rs`, `src/repo/summary.rs`, `src/http/summaries.rs`.
- **Functions:** `claim_next_summary_job(pool, lease_duration)` uses `FOR UPDATE SKIP LOCKED`; `process_summary_job(state, job_id)`; `mark_job_failed(pool, job_id, safe_code, retryable)`; `reclaim_expired_leases(pool)`; `find_summary(pool, id)`; `authorize_summary(principal, summary)`; `get_summary(...)`.
- **Routes:** `GET /v1/summaries/{id}` and worker command.
- **Verify:** job goes queued → running → ready, job poll includes summary URL, `GET` returns stored claims/version; kill worker mid-job and restart to recover lease without duplicate artifact; bounded retry leads to failed state with safe code.

#### T14 checklist

- [x] Implement worker poll interval and `FOR UPDATE SKIP LOCKED` claim with lease expiry and maximum attempt count; bound worker concurrency from config.
- [x] On claim, record worker ID and lease expiration. Reclaim expired `running` jobs after restart; ensure another worker cannot concurrently finalize the same job.
- [x] Fetch authorized source context through service/repository functions, call deterministic generator, validate citations, and commit artifact plus `ready` transition in one transaction. Evidence: `worker::process_one_summary_job` locks the job, reads source context, validates citations, inserts the artifact, and transitions the job to `ready` in one transaction.
- [x] Make artifact insertion idempotent on job ID; a retried job must return the already stored artifact instead of generating a second one. Evidence: summary insertion uses `ON CONFLICT (summary_job_id) DO NOTHING`, with the behavior documented in `src/worker.rs`.
- [ ] Classify retryable database failure versus permanent citation/input failure, use a safe failure code in API, and add bounded backoff.
- [x] Implement `GET /v1/summaries/{id}` and poll response with ready artifact URL; enforce authorization again on reads.
- [ ] Test two workers racing, killed worker/expired lease, failure exhaustion, repeat processing, and unauthorized summary reads.

### T15. Doctor feedback and audit

- **Depends on:** T14.
- **Create:** `src/repo/{summary,audit}.rs`, `src/service/summary.rs`, `src/http/summaries.rs`.
- **Functions:** `validate_feedback(rating, reason)` with documented rating enum and 1..2000-character reason; `authorize_doctor_feedback(principal, summary)`; `insert_feedback(tx, summary, doctor, feedback)`; `write_audit_event(tx, actor, action, target, outcome, request_id)`; `create_feedback(...)`; `get_feedback(principal, feedback_id)`.
- **Routes:** `POST /v1/summaries/{id}/feedback`, supplementary `GET /v1/feedback/{id}`.
- **Verify:** `201 + Location`, only assigned doctor can submit, invalid rating 422, no clinical note text or token in audit/logs; queue, context and source access also emit auditable outcome events.

#### T15 checklist

- [x] Publish the feedback rating allowlist (`Useful`, `Incomplete`, `Incorrect`, `Unsafe`) and whether a reason is required for each; match `Incomplete` in Postman.
- [x] Bound reason length, trim whitespace, reject empty reason when required and sanitize only for output rendering; store the original clinician feedback securely.
- [x] Authorize doctor to the summary's patient/encounter and record doctor ID server-side, ignoring any client-supplied actor field.
- [x] Insert feedback and audit event atomically, return `201 + Location: /v1/feedback/{id}`; implement supplementary `GET /v1/feedback/{id}` for the submitting doctor and document the added route.
- [ ] Audit request ID, actor, action, opaque target and outcome for queue, context, source, summary and feedback; redact clinical content and tokens.
- [ ] Test invalid rating, overlong reason, wrong doctor, admin without clinical assignment, duplicate submissions policy and audit row creation.

## Milestone D: fixture import and existing-Neon adoption

### T16. Staged fixture validation and import job

- **Depends on:** T03-T07.
- **Create:** `src/repo/import.rs`, `src/service/import.rs`, `src/http/imports.rs`, `src/worker/import.rs`.
- **Functions:** `validate_fixture_reference(reference, expected_rows, allowlist)`; `resolve_staged_path(reference)` only through allowlist; `open_fixture_records(path)` dispatches small XLSX to calamine and CSV to `csv::Reader`; `sha256_file_streaming(path)`; `validate_header(header)`; `parse_fixture_row(row_number, row)`; `validate_unique_keys(staging)`; `create_import_job(...)`; `process_import_job(...)`; `publish_dataset(tx, job)`; `find_import_job(...)`.
- **Routes:** `POST /v1/import-jobs`, `GET /v1/import-jobs/{id}`.
- **Rules:** admin only; JSON `fixture_reference` and `expected_rows` match collection; 202+Location; stream and stage bounded batches; record rejects with row number/safe reason; no arbitrary path or URL input. Do not replace active million dataset. Run 100/1k in **separate fresh test databases**.
- **Verify:** exact imported counts 100 and 1,000; malformed date/blank/duplicate patient/source rejected; wrong expected count or mismatched fixture fails job without publishing; worker restart leaves active dataset unchanged; import status count sums reconcile.

#### T16 checklist

- [x] Define the server-side allowlist: `demo-100-v1` → `Doctor-360-Scale-100-Patients.xlsx`/100; `demo-1000-v1` → `Doctor-360-Scale-1000-Patients.xlsx`/1,000; `demo-1000000-v1` → `Doctor-360-Scale-1000000-Patients.csv`/1,000,000. Reject any client-provided path, URL or unexpected reference with 422.
- [x] Accept only the existing Postman JSON shape `{ "fixture_reference": "...", "expected_rows": 100|1000|1000000 }`; validate admin role before creating a job.
- [ ] Stream SHA-256 over staged bytes. For 100/1,000 XLSX, read the only worksheet with calamine/Chrono and convert date cells to ISO date; those small fixtures may be held in memory. For CSV, use `csv::Reader` incrementally. Validate exact eight-column header, row width, integer age 0..120, calendar date, nonblank keys and ICD-10 syntax `^[A-Z][0-9]{2}(\.[A-Z0-9]{1,4})?$`. Preserve original text in source storage; do not silently rewrite drugs/specialties.
- [ ] Stream rows into a staging table in bounded batches without collecting the file in memory; retain row number and safe reject reason separately. Enforce unique patient/source ID within the staged fixture.
- [x] Reconcile `expected = imported + rejected`, record checksum/schema version/times, and fail the job without activation if any required row is rejected or expected count mismatches.
- [ ] Acquire a worker lease for the import job; on restart mark its staging failed and require a new job. Drop only that failed job's staging rows after writing the failure status. Never publish partial rows or silently resume mid-file.
- [x] In a **fresh, empty** database, copy validated staging rows into `synthetic_patients` and activate registry metadata in one transaction. Refuse a request to switch the already active million-row Neon project with a 409 problem; no implicit truncate or table replacement.
- [ ] Run success tests for 100 and 1,000 in separate test databases, and failure tests for bad header/date, duplicate keys, short file, wrong expected count, unauthorized actor and worker interruption.

### T17. Adopt the existing 1,000,000 rows without reimport

- **Depends on:** T03-T04, T16 validation helpers.
- **Create:** `check-fixture` command in `src/main.rs`, adoption method in `src/service/import.rs`.
- **Functions:** `check_existing_neon_fixture(pool, expected=1_000_000)`; `record_adoption_manifest(pool, dataset_id, checks, checksum: Option<String>)`; `activate_existing_dataset(tx, dataset_id)`.
- **Verify:** Neon SQL Editor count remains 1,000,000 before/after; source sample first/last matches fixture; metadata marks `origin=existing_neon_table`; no second million-row table and no second `COPY`. If CSV is supplied, record actual SHA-256; otherwise explicitly `checksum_unverified`.

#### T17 checklist

- [x] Run `check-fixture --dataset demo-1000000-v1 --expected 1000000` against a read-only role before any adoption write. Evidence: 2026-09-30 run using the configured `.env` completed successfully without migrations or mutations.
- [x] Verify schema, count, unique source IDs, first/last synthetic IDs and representative dates/ICD codes against the original CSV samples. Evidence: the successful check-fixture run validates the eight-column schema, primary key, 1,000,000 rows, duplicate/blank source IDs, and first/last source samples; fixture validation checks dates and ICD-10 syntax.
- [x] If the actual CSV is staged near the backend, compute its SHA-256 and record the exact checksum with file size; otherwise set `sha256=NULL` and `checksum_status=unverified` explicitly. Evidence: the read-only fixture check computed the staged-file checksum, and `docs/data-dictionary.md` records the verified checksum and relation size.
- [x] Insert or update only dataset metadata in a transaction and activate `demo-1000000-v1` if no other dataset is active; reject conflicting registry state. Evidence: the idempotent Neon metadata upsert created the sole active registry row; source count stayed at 1,000,000.
- [x] Run `SELECT count(*)` in Neon SQL Editor after adoption and compare to the recorded manifest. Confirm `pg_total_relation_size` remains within plan headroom. Evidence: post-write SQL returned 1,000,000 rows and `215,728,128` relation bytes; measurements are recorded in `docs/data-dictionary.md`.
- [x] Capture SQL evidence that the migration/adoption created no second table with a million source rows and no import job claims a second `COPY`. Evidence: only `synthetic_patients` matched the synthetic-table query and `import_jobs` count was zero after adoption.

## Milestone E: contract, operations and acceptance

### T18. Run Postman contract requests and error examples

- **Depends on:** T08-T17.
- **Create:** `tests/contract.rs`, `docs/api-contract.md`, role-specific Postman environment instructions. Update the existing collection only after explicit contract diff review, preserving its identity if edited.
- **Functions:** `spawn_test_app()`, `assert_problem(response, expected_status, expected_type)`, `assert_location(response)`, `assert_no_store(response)`. Use local Postgres test fixtures; run all 18 requests with correct nurse/doctor/admin tokens and replace Postman variables from created resources.
- **Verify:** all 14 app endpoints reachable; two explicit 404/422 requests return RFC 9457 bodies; successful HTTP status/Location/headers and JSON fields match PRD; no plaintext secrets or source content in failures. Record any deliberate differences from draft collection.

#### T18 checklist

- [x] Create Postman environments for local 100-row test and million-row Neon read-only validation; configure `baseUrl`, role-specific dev token and returned IDs. Never export real tokens in a shared collection. Evidence: `apps/cas-dummy/docs/postman-local-100.environment.json` and `postman-neon-readonly.environment.json` contain placeholders only.
- [ ] Cover the three import requests, import status, search, header, timeline, context, encounter, source, queue assignment/list, summary create/poll/read, feedback and two problem examples from the 18-request collection. Add separate tests for supplementary queue-item and feedback GET routes.
- [x] Capture IDs from `Location`/JSON rather than hard-coding an illustrative encounter ID for a different active dataset. Evidence: collection scripts capture typed import, queue, summary-job and summary IDs from responses/Location values.
- [x] Assert 200/201/202 semantics, resource JSON schema, `Location`, `Cache-Control: no-store`, `Content-Type: application/problem+json`, status agreement, `request_id`, and 401 challenge. Evidence: `src/main.rs` contract tests, `src/error.rs` problem-header tests, route handlers, and `docs/contract-results.md` local workflow results.
- [ ] Add negative cases beyond the two collection examples: wrong role, hidden patient/source, cross-dataset ID, invalid cursor, duplicate queue item, idempotency key replay and conflict, unsupported feedback rating, and worker/database unavailable.
- [ ] Re-run the collection after API restart with a persisted summary job. Document any collection contract updates as a field/method/status diff before updating the original file.
- [x] Produce a route coverage table with each request name, method/path, role, expected status, automated test name and observed result. Evidence: `apps/cas-dummy/docs/route-coverage.md` lists all 18 collection requests plus supplementary GET routes and automated coverage references; live Newman results remain separately gated.

### T19. One-million-row read and resource benchmark

- **Depends on:** T17-T18.
- **Create:** `tests/scale.rs` or a `scripts/benchmark_reads` command, `docs/benchmark-results.md`.
- **Functions:** `benchmark_exact_patient_lookup`, `benchmark_prefix_search`, `benchmark_context_lookup`, `measure_table_indexes_bytes` on the existing million-row environment using read-only queries. Capture `EXPLAIN (ANALYZE, BUFFERS)` only for bounded lookups, not bulk scans in routine runs.
- **Verify:** report actual p50/p95, rows returned, query plans, table/index size, peak process RSS and setup; no invented SLO. If Neon free storage or compute limits block a test, report that result and repeat locally. Never benchmark by returning all records in one API response.

#### T19 checklist

- [ ] Record environment: Rust binary version/commit, Neon branch/region, table/index size, active dataset, pool limit, network location and run timestamp; omit credentials.
- [ ] Warm the Neon compute, then measure separately cold-start latency and warm-query latency; do not mix the two in one p95.
- [ ] Run a bounded sample of exact patient IDs, prefix search and context lookups across early/middle/late ID ranges; define request count and concurrency in the report.
- [ ] Compute observed p50/p95 and error rate from actual samples, log peak process RSS and connection resets, and save parameterized query plans for representative lookups.
- [x] Inspect index usage and avoid offset pagination, `SELECT *` bulk fetches or returning all million rows to benchmark a list endpoint. Evidence: `docs/neon-query-plans.md` records exact/prefix index plans; API queries use bounded projections and keyset pagination.
- [ ] Check Neon remaining storage after indexes/metadata. If a measurement fails, report failure reason and rerun locally only for diagnosis, labeling local versus Neon results clearly.
- [ ] Recommend a performance target only after measurements and product input; don't invent an SLO for a synthetic dataset.

### T20. Runbook and release review

- **Depends on:** T18-T19.
- **Create:** `README.md`, `docs/runbook.md`, `.env.example`, CI workflow.
- **Functions:** documented commands for migrate, `check-fixture`, `api`, `worker`, seeded dev identity, local fixture import, token rotation, and job recovery.
- **Verify:** fresh checkout + documented steps produce health check, authorized context, queue assignment, ready cited summary and feedback with the 100-row fixture; Neon million-row read path succeeds without reimport; `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test` and contract suite pass. No production claim until hospital SSO/source/evaluation decisions are resolved.

#### T20 checklist

- [x] Document setup from a fresh checkout: toolchain, local Postgres, migration, safe fixture staging, dev identity seeding, API and worker start, and Postman variables.
- [x] Provide a one-page failure runbook for database disconnect, stale pool, worker lease expiry, invalid source citation, missing fixture, storage quota and token rotation.
- [x] Add CI stages for `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`, migrations on ephemeral PostgreSQL, and contract tests; keep Neon credentials out of CI unless a separate read-only secret is approved. Evidence: `.github/workflows/cas-dummy.yml` uses only an ephemeral PostgreSQL 16 service and has explicit migration/constraint and contract-test steps.
- [x] Execute doctor flow: nurse assigns a patient → doctor sees queue → authorized context and source → summary job ready → cited artifact → feedback saved. Evidence: 2026-09-29 disposable local PostgreSQL run imported the 100-row fixture, returned nurse context with three gaps, created a queue assignment (201), authorized doctor source access (200), produced a ready three-claim summary (200), and saved doctor feedback (201).
- [x] Execute admin flow: inspect or adopt existing million rows without reimport; fresh 100/1k import test in isolated environments and read job status. Evidence: Neon adoption metadata was written after read-only checks with zero import jobs; local PostgreSQL gates processed the 100/1,000 fixtures.
- [x] Verify documentation distinguishes synthetic pilot from live RS Bunda data and lists unresolved hospital decisions: SSO, source contract, retention, ICD dictionary, note availability, clinical evaluation and service targets.
- [ ] Tag a reproducible pilot build only when all mandatory task boxes and release gates are met; record known limitations in `README.md`.

## Frozen pilot contract: implement exactly these decisions

The checklist above names the work; this section removes implementation choices left open by the original draft. If a later requirement conflicts, change this contract and its tests in the same commit before coding that change.

### A. Configuration and development identities

| Environment variable | Required/default | Exact behavior |
|---|---|---|
| `DATABASE_URL` | required | Direct PostgreSQL URL. Parse without logging. Fail startup if missing. |
| `ACTIVE_DATASET_ID` | required | One of `demo-100-v1`, `demo-1000-v1`, `demo-1000000-v1`. Must match registry's active row before patient routes are served. |
| `API_BIND_ADDR` | `127.0.0.1:8000` | HTTP listener. |
| `API_PUBLIC_BASE_URL` | `http://localhost:8000` | Origin for stable problem type documentation and absolute Location only if needed; validate URL on boot. |
| `DEV_TOKENS_CONFIG` | required for `seed-dev-users` only | Path to ignored JSON seed input. Each entry is `{principal_id,token,role,department_scope}`. Seed hashes, then delete seed file or keep only in a local secret store. The running API reads hashes from DB, not plaintext. |
| `CURSOR_SIGNING_KEY` | required | At least 32 random bytes, loaded from secret storage. HMAC-SHA256 signs cursors; reject missing/short key. |
| `FIXTURE_DIR` | required for import worker | Server-controlled directory. Map fixture IDs to fixed filenames, not client input paths. |
| `DATABASE_POOL_MAX` | `5` | Allowed `1..20`. |
| `WORKER_CONCURRENCY` | `1` | Allowed `1..4`. |
| `SUMMARY_LEASE_SECONDS` | `60` | Reclaim expired jobs. |
| `SUMMARY_MAX_ATTEMPTS` | `3` | Retry only transient database/worker failures. |

Seed exactly these **synthetic** roles in local tests: `demo-admin-01` (admin), `demo-nurse-obgyn-01` (nurse, `Obstetrics and Gynecology`), `demo-nurse-gp-01` (nurse, `General Practice`), `demo-doctor-01` (doctor), and `demo-doctor-02` (doctor). Test tokens are generated locally; do not put literal tokens in the collection. Nurse search returns only rows whose `specialist` exactly equals department scope. Doctor search returns only active queue assignments. `MedicalRecords` role gets no patient read in the pilot until an explicit access policy is supplied.

### B. Database DDL contract

Implement this schema across `0000`-`0003` migrations. Names, types and constraints below are the reference; add only supporting indexes and migration bookkeeping. All operational tables are small relative to the million-row source. `CHECK` constraints are defense in depth, while service validation determines HTTP 422/409.

```sql
CREATE TABLE IF NOT EXISTS synthetic_patients (
  patient_id text PRIMARY KEY,
  age_years integer NOT NULL,
  sex text NOT NULL,
  visit_date date NOT NULL,
  specialist text NOT NULL,
  icd10_code text NOT NULL,
  prescription_recorded text NOT NULL,
  source_record_id text NOT NULL
);

CREATE TABLE dataset_registry (
  dataset_id text PRIMARY KEY,
  expected_rows bigint NOT NULL CHECK (expected_rows > 0),
  actual_rows bigint NOT NULL DEFAULT 0 CHECK (actual_rows >= 0),
  status text NOT NULL CHECK (status IN ('staging','active','failed')),
  origin text NOT NULL CHECK (origin IN ('csv_import','existing_neon_table')),
  schema_version integer NOT NULL DEFAULT 1,
  sha256 text,
  checksum_status text NOT NULL CHECK (checksum_status IN ('verified','unverified')),
  created_at timestamptz NOT NULL DEFAULT now(),
  activated_at timestamptz
);
CREATE UNIQUE INDEX one_active_dataset ON dataset_registry ((true)) WHERE status = 'active';

CREATE TABLE dev_identities (
  principal_id text PRIMARY KEY,
  token_sha256 bytea NOT NULL UNIQUE,
  role text NOT NULL CHECK (role IN ('doctor','nurse','admin','medical_records')),
  department_scope text,
  is_active boolean NOT NULL DEFAULT true
);

CREATE TABLE queue_items (
  queue_item_id uuid PRIMARY KEY,
  dataset_id text NOT NULL REFERENCES dataset_registry(dataset_id),
  patient_id text NOT NULL REFERENCES synthetic_patients(patient_id),
  encounter_id text NOT NULL,
  doctor_id text NOT NULL REFERENCES dev_identities(principal_id),
  assigned_by text NOT NULL REFERENCES dev_identities(principal_id),
  status text NOT NULL DEFAULT 'active' CHECK (status IN ('active','closed')),
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX one_active_assignment
  ON queue_items(dataset_id, encounter_id, doctor_id) WHERE status = 'active';
CREATE INDEX queue_by_doctor ON queue_items(doctor_id, status, created_at DESC, queue_item_id DESC);

CREATE TABLE import_jobs (
  import_job_id uuid PRIMARY KEY,
  fixture_reference text NOT NULL,
  expected_rows bigint NOT NULL,
  imported_rows bigint NOT NULL DEFAULT 0,
  rejected_rows bigint NOT NULL DEFAULT 0,
  sha256 text,
  status text NOT NULL CHECK (status IN ('queued','running','ready','failed')),
  failure_code text,
  requested_by text NOT NULL REFERENCES dev_identities(principal_id),
  lease_until timestamptz,
  created_at timestamptz NOT NULL DEFAULT now(),
  finished_at timestamptz
);
CREATE TABLE import_rejects (
  import_job_id uuid NOT NULL REFERENCES import_jobs(import_job_id),
  row_number bigint NOT NULL,
  reason_code text NOT NULL,
  PRIMARY KEY(import_job_id, row_number)
);
CREATE TABLE import_staging (
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
  PRIMARY KEY(import_job_id,row_number),
  UNIQUE(import_job_id,patient_id),
  UNIQUE(import_job_id,source_record_id)
);

CREATE TABLE summary_jobs (
  summary_job_id uuid PRIMARY KEY,
  dataset_id text NOT NULL REFERENCES dataset_registry(dataset_id),
  patient_id text NOT NULL REFERENCES synthetic_patients(patient_id),
  encounter_id text NOT NULL,
  requested_by text NOT NULL REFERENCES dev_identities(principal_id),
  reason text NOT NULL CHECK (reason IN ('nurse_preload','doctor_refresh')),
  idempotency_key text NOT NULL,
  request_hash bytea NOT NULL,
  status text NOT NULL CHECK (status IN ('queued','running','ready','failed')),
  lease_owner text,
  lease_until timestamptz,
  attempts integer NOT NULL DEFAULT 0,
  failure_code text,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  UNIQUE(requested_by,idempotency_key)
);
CREATE INDEX summary_jobs_claim ON summary_jobs(status, lease_until, created_at);

CREATE TABLE summaries (
  summary_id uuid PRIMARY KEY,
  summary_job_id uuid NOT NULL UNIQUE REFERENCES summary_jobs(summary_job_id),
  dataset_id text NOT NULL REFERENCES dataset_registry(dataset_id),
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
CREATE TABLE feedback (
  feedback_id uuid PRIMARY KEY,
  summary_id uuid NOT NULL REFERENCES summaries(summary_id),
  doctor_id text NOT NULL REFERENCES dev_identities(principal_id),
  rating text NOT NULL CHECK (rating IN ('Useful','Incomplete','Incorrect','Unsafe')),
  reason text NOT NULL CHECK (char_length(reason) BETWEEN 1 AND 2000),
  created_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE audit_events (
  audit_event_id uuid PRIMARY KEY,
  request_id text NOT NULL,
  principal_id text,
  action text NOT NULL,
  target_type text NOT NULL,
  target_id text,
  outcome text NOT NULL,
  occurred_at timestamptz NOT NULL DEFAULT now()
);
```

For the current Neon source, use its existing `patient_id` primary key and keep its existing column nullability; validate real values in T03 rather than rewriting a million-row table. Add `CREATE UNIQUE INDEX CONCURRENTLY synthetic_source_record_id_uq ON synthetic_patients(source_record_id)` **outside a transaction** only after T03 confirms storage headroom. For prefix search, add `CREATE INDEX CONCURRENTLY synthetic_patient_id_pattern_idx ON synthetic_patients(patient_id text_pattern_ops)` if EXPLAIN shows the primary key index is insufficient. If a required index cannot be created, stop million-row endpoint acceptance and use local PostgreSQL until capacity is available. SQLx migration transactions must not wrap concurrent index operations. Do not infer foreign keys to a source ID until its uniqueness is established.

### C. Fixed request/response shapes

| Route | Request constraints | Response resource fields |
|---|---|---|
| `GET /v1/patients` | `query` required 2..64 ASCII `[A-Za-z0-9-]+`, uppercase; `limit` default 20, 1..100; optional cursor | `{items:[{patient_id,age_years,sex}],next_cursor:null|string}` |
| `GET /v1/patients/{id}` | valid patient ID and authorized scope | `{patient_id,dataset_id,age_years,sex}` |
| `GET /v1/patients/{id}/encounters` | `limit`/cursor as above | `{items:[Encounter],next_cursor:null|string}` |
| `GET /v1/patients/{id}/context` | optional `encounter_id` must belong to patient | PRD `patient_id,dataset_id,age_years,sex,encounters,data_gaps` |
| `GET /v1/encounters/{id}` | derived ID from active dataset | `{encounter_id,patient_id,visit_date,specialist,diagnoses:[{code,source_record_id}],prescriptions:[{name,current_use,source_record_id}]}` |
| `GET /v1/sources/{id}` | authorized source ID | `{dataset_id,source_record_id,patient_id,age_years,sex,visit_date,specialist,icd10_code,prescription_recorded}` |
| `POST /v1/queue-items` | `{patient_id,encounter_id,doctor_id}` | 201, `Location: /v1/queue-items/{id}`, body `{queue_item_id,patient_id,encounter_id,doctor_id,status,created_at}` |
| `GET /v1/doctors/me/queue` | doctor token; optional `limit`/cursor | `{items:[QueueItem],next_cursor:null|string}` |
| `POST /v1/encounters/{id}/summary-jobs` | `{reason:"nurse_preload"|"doctor_refresh"}`, `Idempotency-Key` | 202, `Location: /v1/summary-jobs/{id}`, body `{summary_job_id,status}` |
| `GET /v1/summary-jobs/{id}` | job owner or assigned doctor with patient access | `{summary_job_id,status,summary_url:null|string,failure_code:null|string}` |
| `GET /v1/summaries/{id}` | patient access | `{summary_id,patient_id,encounter_id,dataset_id,version,generator_version,generated_at,review_state,claims}` |
| `POST /v1/summaries/{id}/feedback` | `{rating,reason}`; assigned doctor | 201, `Location: /v1/feedback/{id}`, body `{feedback_id,summary_id,rating,reason,created_at}` |
| `POST /v1/import-jobs` | `{fixture_reference,expected_rows}`; admin only | 202, `Location: /v1/import-jobs/{id}`, body `{import_job_id,status}` |
| `GET /v1/import-jobs/{id}` | admin only | `{import_job_id,fixture_reference,expected_rows,imported_rows,rejected_rows,status,failure_code}` |

The two supplementary `GET /v1/queue-items/{id}` and `GET /v1/feedback/{id}` routes return the corresponding created resource, authorize the owner, and get separate tests. They do not change the original 18-request Postman collection until an explicit collection revision. Empty `prescriptions` is `[]`, never a fabricated `None recorded` medication. `data_gaps` are always explicit for the eight-column fixture. All success JSON uses snake_case. `Location` is a path on the same API origin. Validation failures return RFC 9457 problems, never a `{status,message,data}` wrapper.

### D. Exact source-derived IDs and example assertions

| CSV row | Stored patient ID | Derived million-dataset encounter ID | Source ID |
|---|---|---|---|
| 1 | `SYN-BUNDA-P0001` | `enc-demo-1000000-v1-0001` | `SYN-BUNDA-V0001-01-SRC` |
| 101 | `SYN-BUNDA-P0000101` | `enc-demo-1000000-v1-0000101` | `SYN-BUNDA-V0000101-SRC` |
| 1,000 | `SYN-BUNDA-P0001000` | `enc-demo-1000000-v1-0001000` | `SYN-BUNDA-V0001000-SRC` |
| 1,000,000 | `SYN-BUNDA-P1000000` | `enc-demo-1000000-v1-1000000` | `SYN-BUNDA-V1000000-SRC` |

Implement `derive_encounter_id` by string concatenation after exact prefix validation, and `parse_encounter_id` by removing `enc-{ACTIVE_DATASET_ID}-` and prepending `SYN-BUNDA-P`. Never parse the suffix to an integer and reformat it. The Postman example `enc-demo-100-v1-0001` is for the **100-row** environment; replace the variable with `enc-demo-1000000-v1-0001` when testing Neon.

### E. Exact control flow and test commands

1. **Local 100-row path:** create local PostgreSQL; run migrations `0000`-`0003`; seed identities; stage the **scale 100-row fixture** (not the richer, different-schema workbook). Start API and worker; `POST import-jobs` for `demo-100-v1`; poll until `ready` (publication marks the dataset active); run the 18 collection requests.
2. **Existing Neon path:** run T03 preflight; measure storage; migrate metadata; adopt `demo-1000000-v1` without `COPY`; run authenticated bounded read routes only. Do not trigger an import job for `demo-1000000-v1` against the populated project; that request must return 409.
3. **Queue path:** nurse with specialty matching selected patient's `specialist` assigns `demo-doctor-01`; doctor can then search/read this patient; doctor 02 gets 404 for that patient.
4. **Summary path:** nurse creates job with `nurse_preload` and idempotency key; worker claims, builds deterministic claims, validates source ownership, stores summary and marks job ready; assigned doctor polls/reads and submits feedback.
5. **Verification commands:** `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`, local integration suite, then Postman collection. Record actual outputs in `docs/benchmark-results.md` and `docs/contract-results.md`.

**Failure handling:** Any schema mismatch, unexpected active dataset, exhausted Neon storage, missing source index at million scale, or citation mismatch fails a task with a named diagnostic. Do not silently skip a check, treat an empty result as proof of access control, or synthesize clinical fields to make a response look complete.

### F. Query templates, cursor format and worker transitions

Bind parameters, never interpolate query/path values into SQL. Use these templates as behavioral references, adapting SQLx argument syntax as needed:

```sql
-- Exact patient: authorization predicate must be checked first/in the same service.
SELECT patient_id,age_years,sex,visit_date,specialist,icd10_code,
       prescription_recorded,source_record_id
FROM synthetic_patients WHERE patient_id = $1;

-- Patient ID prefix page, with server-computed "PREFIX%" and after-key.
SELECT patient_id,age_years,sex FROM synthetic_patients
WHERE patient_id LIKE $1 AND patient_id > $2
ORDER BY patient_id ASC LIMIT $3;

-- Original source row after resolving its owner and authorizing that patient.
SELECT patient_id,age_years,sex,visit_date,specialist,icd10_code,
       prescription_recorded,source_record_id
FROM synthetic_patients WHERE source_record_id = $1;

-- Doctor's queue; do not accept doctor_id from request query parameters.
SELECT queue_item_id,dataset_id,patient_id,encounter_id,doctor_id,status,created_at
FROM queue_items WHERE doctor_id = $1 AND status = 'active'
ORDER BY created_at DESC,queue_item_id DESC LIMIT $2;
```

For a doctor search, add `EXISTS (SELECT 1 FROM queue_items q WHERE q.patient_id = p.patient_id AND q.dataset_id = $dataset AND q.doctor_id = $principal AND q.status = 'active')` to the query on `synthetic_patients p`; for nurse search add `p.specialist = $department_scope`. Do not fetch all rows and filter in Rust. The patient ID prefix query must use alias `p` when the scope predicate is attached. Return at most `limit` items and use the extra row solely to decide whether to issue a next cursor.

Cursor payload is a fixed Serde struct `{v:1,dataset_id,principal_id,role,scope,query,last_id,issued_at,expires_at}`. Serialize it in stable field order, base64url without padding, sign with HMAC-SHA256 over the encoded payload, and send `payload.signature`. Set expiry to 15 minutes. On decode, verify signature in constant time, version, expiration, dataset, principal, role, scope and query before using `last_id`; any failure is 422. For encounter/queue pages use a separate typed cursor payload with sort keys and the same bindings. Do not reuse a patient-search cursor on another route.

Summary worker transition table:

| Current | Event | New | DB invariant |
|---|---|---|---|
| `queued` | worker claims with row lock | `running` | increment attempts; set lease owner/expiry |
| `running` | verified artifact committed | `ready` | insert one `summaries` row with UNIQUE job ID in same transaction |
| `running` | transient failure, attempts < 3 | `queued` | clear lease, retain safe failure code for audit |
| `running` | permanent error or attempts = 3 | `failed` | clear lease, set safe failure code |
| `running` | lease expires | `queued` | reclaimer changes state only if no summary exists |

Import jobs have the same `queued → running → ready/failed` shape. A failed import's staging data never appears through patient routes. Never return a `ready` summary job before its artifact transaction commits.

### G. Deterministic contract execution order

1. In a **fresh local 100-row database**, run `POST import-jobs` with `demo-100-v1` and expect 202. Poll until ready and count exactly 100. The collection's 1,000 and 1,000,000 import requests against this **already active** database must return 409, without replacing the 100 rows.
2. In a **separate fresh local 1,000-row database**, run the 1,000 import request and expect 202 then exactly 1,000 rows. Do not combine with the 100-row fixture.
3. In the **populated Neon million-row database**, adopt/check existing rows and expect exactly 1,000,000. `POST import-jobs` for any fixture returns 409 because this environment is active. No CSV upload/`COPY` is performed by the API.
4. In the local 100-row environment, use the correct specialty nurse to assign `SYN-BUNDA-P0001` to `demo-doctor-01`, then run the remaining collection requests with the assigned doctor or nurse token as specified. Set `encounterId=enc-demo-100-v1-0001` and `sourceId=SYN-BUNDA-V0001-01-SRC`.
5. For problem examples, use a token authorized to search, otherwise a 401/403 could mask the expected 404/422. Assert actual status, media type, safe fields and no patient content.

The implementation is not accepted merely because it compiles: each step above has a count, response, or state transition that can be independently checked.

## Explicit route coverage checklist

The Postman collection has **18 requests**: three `POST /v1/import-jobs` variants, one import read, six patient/encounter/source reads, six queue/summary/feedback requests, and two problem examples. T16 covers import; T08-T10 cover patient context; T11-T15 cover queue and summary; T07/T18 cover problem responses. Update the count if the collection changes.

- [x] Mark every existing collection request with an automated test ID and an expected role. Evidence: `apps/cas-dummy/docs/route-coverage.md` now lists all 18 collection requests with method/path, expected role, and automated coverage reference; live Postman execution remains separately gated.
- [x] Document the two supplementary GET routes for queue item and feedback Location URIs. The original collection stays at 18 requests; add two separate contract tests, then update the collection only in a reviewed follow-up. Evidence: `docs/route-coverage.md` documents both routes and `queue_item_location_resource_is_a_protected_route` / `feedback_location_resource_is_a_protected_route` verify their protected route contracts.
- [x] Verify the 100-row fixture drives end-to-end workflow tests while the million-row fixture drives indexed read/scale tests. Evidence: `docs/local-verification.md` records the complete local 100-row doctor workflow; `docs/neon-query-plans.md` records million-row exact/prefix indexed reads.

## Immediate first implementation slice

Complete **T01 → T02 → T03 → T04 → T05 → T06 → T07 → T08 → T09 → T10** and demonstrate authorized `GET /v1/patients/SYN-BUNDA-P0001/context` against the existing Neon table. This slice proves the backend boundary before summary jobs or new import code. Use only synthetic records and rotate the Neon password previously exposed in screenshots.

### First-slice demonstration checklist

- [ ] Confirm Neon credential rotation and active `demo-1000000-v1` in config without exposing the URL.
- [x] `GET /healthz` returns 200 and process starts/stops cleanly.
- [x] Nurse-scoped token searches `SYN-BUNDA-P0001` with a bounded response; wrong-scope token gets no patient content. Evidence: authorized Neon JWT smoke test returned bounded `200` for Cardiology scope; a Neurology-scoped token returned no patient identifier. Temporary identities were deleted.
- [x] Patient header, encounter, source and context refer to the same original row and source ID. Evidence: local 100-row doctor workflow and `first-row-context.json` preserve the patient/source mapping across these responses.
- [x] Context contains three explicit data gaps and no fabricated allergies, current medication or SOAP notes. Evidence: local context response and fixture test assert `allergies_not_supplied`, `labs_not_supplied`, and `notes_not_supplied`, with historical medication marked unknown rather than inferred.
- [x] Postman invalid `limit=-1` yields 422 problem JSON and unknown patient yields 404 problem JSON. Evidence: authorized Neon-backed API smoke test returned `422` and `404`; temporary identity was removed afterward.
- [ ] Record actual result screenshots with tokens and connection URLs cropped out.

### T21. Add JWT authentication to every REST API description

- **Depends on:** T06-T07 and the route contracts in T08-T18.
- **Purpose:** Add signed JWT access tokens to the REST API authentication flow. The existing T06 bearer-token design uses opaque SHA-256 token hashes; keep that design documented, but do not treat opaque bearer tokens as JWTs. Decide and document whether JWTs replace dev bearer tokens or are accepted as an additional authentication method during migration.
- **Create/update:** `src/auth.rs`, `src/config.rs`, `src/http/` route middleware, `src/error.rs`, `docs/api-contract.md`, `README.md`, `.env.example`, and authentication/integration tests. Add only files that exist in the implementation; do not expose signing secrets.
- **JWT contract:** use a maintained JWT library already compatible with the project, an asymmetric signing algorithm such as EdDSA or RS256, explicit `iss`, `aud`, `sub`, `exp`, `iat`, `jti`, and role/scope claims, short access-token expiry, clock-skew handling, and key rotation via `kid`. Validate signature, issuer, audience, expiration, not-before when present, token type, principal status, role and dataset/patient scope before authorizing a request. Never trust client-supplied role or scope without validating the signed token and server-side permissions.
- **Issuance and revocation:** define the development-only login/token issuance flow and its input validation; store signing keys only in secret configuration; never log tokens; provide a safe revocation strategy for compromised `jti` values, inactive identities and key rotation. Do not add a password login flow unless the project requirement explicitly adds one.
- **REST route coverage:** update each route description and its contract test for `GET /healthz`, all `/v1/patients`, `/v1/encounters`, `/v1/sources`, queue, summary, feedback and import-job routes. Mark each route as public or JWT-protected, list required roles/scopes, and specify `401` for missing/invalid/expired JWT, `403` for authenticated but disallowed access, and hidden `404` for out-of-scope patient/source resources. Health must remain public and must not return patient data.
- **Authorization rules:** preserve doctor assignment, nurse department, admin import-only, Medical Records and dataset restrictions from T06. Enforce patient/source ownership after JWT authentication and before serializing any clinical data. JWT claims may narrow access but must not broaden server-side permissions.
- **Verify:** add unit, negative-path and integration tests for valid JWTs, wrong signature, unsupported algorithm, wrong issuer/audience, missing claims, expired/not-yet-valid tokens, malformed `Authorization` headers, revoked `jti`, inactive principal, wrong role, wrong dataset, out-of-scope patient, key rotation and `kid` selection. Assert no token, signing key, password or private claim data appears in logs or RFC 9457 responses.
- **Done condition:** every REST endpoint description identifies its JWT requirement and authorization scope; authenticated requests succeed with a valid JWT; invalid/expired JWTs fail consistently; existing scope tests still pass; `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test` and the authenticated Postman contract suite pass. Document that JWT is now the API credential format and remove ambiguity between JWTs and opaque bearer tokens without deleting the original T06 history.
