# Doctor 360 Patient Context API: Rust implementation plan

**Status:** Implementation-ready plan, 29 September 2026  
**Inputs:** `Doctor-360-Dummy-Data-Backend-PRD.md` and `Doctor-360-Patient-Context-API.postman_collection.json`  
**Starting point:** Neon PostgreSQL `neondb.public.synthetic_patients` contains 1,000,000 synthetic rows. A browser SQL Editor count confirmed this. No Rust service or live hospital integration exists yet.

## 1. Outcome and boundaries

Build one Rust HTTP service for the 14 application endpoints in the collection, plus four development/error examples (18 collection requests total). The service reads the active synthetic fixture, enforces access before revealing patient data, and produces a deterministic, cited history summary from the eight recorded columns. The summary is **not** a diagnosis, current medication list, allergy assessment, or LLM-generated clinical advice.

The API uses real HTTP status codes per RFC 9110, `application/problem+json` errors per RFC 9457, and `Cache-Control: no-store` for patient, source, queue, job, summary, and feedback responses. The Postman collection is the executable route checklist. Its `.example` problem-type host and example encounter IDs are illustrative, not deployable identifiers.

### Fixed decisions for this implementation

| Topic | Decision |
|---|---|
| Language/runtime | Rust stable, edition 2024, Tokio, Axum 0.8, Tower HTTP, SQLx with PostgreSQL, Serde, Chrono, UUID, CSV, SHA-256, tracing. Lock exact resolved versions in `Cargo.lock`; confirm APIs against installed crate docs when coding. |
| Deployment | One API/worker binary, PostgreSQL on Neon. Separate `api` and `worker` subcommands share code. Local development uses Docker PostgreSQL or a dedicated Neon test project. |
| Active dataset | Exactly one immutable published dataset per environment. `ACTIVE_DATASET_ID=demo-1000000-v1` for the current Neon environment. `demo-100-v1` and `demo-1000-v1` run in separate databases/projects; IDs overlap and must never be merged into a clinical view. |
| Existing million rows | Reuse `public.synthetic_patients` in place. Do not copy all million rows into another table or rebuild its primary key on the Neon Free project. Add small metadata/operational tables and a unique source index only after measuring remaining storage. |
| Record shape | One patient row and one dated visit per patient in the scale fixtures. No SOAP notes, allergies, labs, named doctor, or medication status. |
| Encounter ID | `enc-{dataset_id}-{suffix}`, where `suffix` is the **exact string** after `SYN-BUNDA-P` in the imported patient ID, including its original zero padding. For example, `SYN-BUNDA-P0001` maps to `enc-demo-100-v1-0001`, while million-row patient `SYN-BUNDA-P0000101` maps to `enc-demo-1000000-v1-0000101`. Reject IDs without the expected prefix and decimal suffix. Parse by stripping the active dataset prefix and restoring `SYN-BUNDA-P`; no million-row encounter backfill table is needed. Version this derivation as `v1`. |
| Source | `source_record_id` identifies the original CSV row; `GET /v1/sources/{id}` returns only the eight recorded fields plus provenance, never a fabricated source document. |
| Authentication | Development-only opaque bearer tokens, supplied as hashes/roles/scopes through configuration or seeded `dev_identities`. Never commit plaintext tokens. Production SSO is a later integration point. |
| AI | `SummaryGenerator` trait, with a deterministic template implementation for these structured rows. An LLM adapter is an explicit later change after approved notes and clinical evaluation exist. No vector DB, Snowflake query, or direct model-to-database access now. |
| Long-running work | Import and summary endpoints return `202` with `Location`; PostgreSQL job tables plus a polling worker support restarts. No in-memory-only queue. |

### Neon Free capacity gate

Before migrations or indexing, record `pg_total_relation_size('synthetic_patients')` and the project's remaining storage. The CSV is ~104 MB, but the table and index size is different. If a migration/index would exceed capacity, develop against local PostgreSQL and keep Neon as the already loaded read source. A successful `COPY 1000000` followed by a SQL Editor count of 1,000,000 is the import baseline; do not rerun it.

## 2. Repository layout and dependency direction

```text
doctor360-context-api/
  Cargo.toml
  Cargo.lock
  .env.example
  README.md
  migrations/
    0000_source_fixture.sql
    0001_metadata.sql
    0002_operational.sql
    0003_indexes.sql
  src/
    main.rs                   # api | worker | check-fixture | seed-dev-users
    config.rs                 # parsed, validated configuration
    app.rs                    # router and shared state
    auth.rs                   # bearer principal and access policy
    error.rs                  # typed errors -> RFC 9457
    domain/{ids,patient,context,summary,job}.rs
    repo/{patient,source,queue,job,summary,audit,import}.rs
    service/{patient,context,queue,summary,import}.rs
    http/{dto,patients,encounters,sources,queue,summaries,imports}.rs
    worker/{poll,import,summary}.rs
  tests/{contract,authorization,import,summary,scale}.rs
  docs/{api-contract,runbook,data-dictionary}.md
```

HTTP handlers deserialize/validate, call services, then serialize. Services enforce scope and business invariants. Repositories own SQL and return typed rows; they do not decide clinical access. Domain types have no Axum or SQLx dependencies. The worker calls the same service/repository rules as HTTP handlers. Prefer small explicit functions over generic frameworks or premature traits; use traits at external boundaries (`SummaryGenerator`, clock, optional identity provider) where substitution is necessary.

## 3. Data model and migrations

Keep existing `synthetic_patients(patient_id PK, age_years, sex, visit_date, specialist, icd10_code, prescription_recorded, source_record_id)` untouched initially. The migration checks all eight columns and types against this contract before applying changes. Its rows are the source of truth for the active dataset.

| Table | Essential fields and constraints |
|---|---|
| `dataset_registry` | `dataset_id PK`, `expected_rows`, `actual_rows`, `status` (`staging/active/failed`), `schema_version`, `origin`, optional `sha256`, timestamps. One active dataset in the environment. |
| Encounter IDs | Derived at read time from the exact patient ID suffix using derivation `v1`. No `encounter_keys` table or million-row backfill is created. `source_record_id` remains a recorded source field and is validated for uniqueness during import/adoption. |
| `dev_identities` | `principal_id PK`, `token_hash UNIQUE`, `role`, `department_scope`, `is_active`; tokens supplied externally. A doctor assignment is separate from an admin role. |
| `queue_items` | `queue_item_id PK`, `dataset_id`, `patient_id`, `encounter_id`, `doctor_id`, assigning nurse ID, status, timestamps, UNIQUE active `(dataset_id, encounter_id, doctor_id)`. |
| `import_jobs` | `id PK`, fixture allowlist key, expected/imported/rejected counts, SHA-256, status, error code, times. `import_rejects` has job ID, row number, safe reason, and no clinical payload in logs. |
| `summary_jobs` | `id PK`, dataset/encounter, requester, reason, idempotency key, canonical request hash, `queued/running/ready/failed`, lease and retry count, artifact ID, safe failure code, times; UNIQUE requester plus key. |
| `summaries` | `summary_id PK`, dataset/patient/encounter, generator and template versions, source set/hash, review state, generated time, version, JSON claims. |
| `feedback` | `feedback_id PK`, summary ID, doctor ID, rating enum, bounded reason, created time. |
| `audit_events` | ID, principal, action, dataset, target type and opaque target ID, outcome, request ID, time. Never store access tokens or note text. |

Use CHECK/FOREIGN KEY constraints where they protect persisted state. Enforce source uniqueness at import validation even if an index must wait for the capacity gate. Use SQLx parameter binding everywhere. The source row has no encounter ID or actual doctor ID; those belong only to synthetic operational metadata.

## 4. Import and adoption flow

`POST /v1/import-jobs` accepts only `fixture_reference` in the configured allowlist (`demo-100-v1`, `demo-1000-v1`, `demo-1000000-v1`) and exact `expected_rows`. It never accepts an arbitrary path/URL. The server maps a reference to a staged local CSV/XLSX path controlled by configuration; read `.xlsx` through a streaming converter or preconvert it to CSV. Calculate SHA-256 over the original staged bytes, validate exact header, dates and age, duplicate patient/source IDs, and stream bounded COPY batches into a staging table. Count/check before publishing. In a single-dataset environment, refuse a different fixture while an active dataset exists (`409`); replacement requires an explicit separate administrative operation outside this API. Do not `TRUNCATE` the current million-row table during an import request.

For the already loaded Neon table, provide `check-fixture --dataset demo-1000000-v1 --expected 1000000` to check count, unique IDs, sample source fields and optional staged-file hash. It records an adoption manifest (`origin=existing_neon_table`) without importing again. Mark a checksum absent if the source file is not available; never invent one. New imports in fresh environments record checksum and rejects. On crash, incomplete staging/job remains failed or resumable by job ID; it is never published. The worker acquires a job with `FOR UPDATE SKIP LOCKED`, a lease, and bounded retries.

## 5. Authorization and response rules

Resolve `Authorization: Bearer <token>` to a principal before a handler exposes data. `authorize_patient(principal, patient_id, dataset_id)` checks active dataset and patient scope; doctor access requires an active queue assignment, nurse access requires configured department scope, and admin import rights alone do not reveal clinical records. `authorize_encounter` and `authorize_source` first resolve their owner, then apply the same patient policy. For a patient outside the permitted scope, return `404` to avoid revealing existence; `403` applies to a forbidden operation on an otherwise known permitted resource. Missing/invalid token is `401` and includes `WWW-Authenticate: Bearer`.

The collection's search examples use `SYN-BUNDA-P0001`: run them with a nurse token scoped to that row's specialty, or seed an explicit doctor queue assignment first. Do not silently give all doctors global search. All SELECTs include the active dataset/scope predicates. Use `limit` 1..100, default 20, opaque signed cursor with dataset/query/sort binding; invalid cursor is `422`. Search uses indexed patient ID exact/prefix first. General substring/full-text search needs an explicit index and benchmark before enabling. Encounter timeline sorts `(visit_date DESC, encounter_id DESC)` and uses keyset pagination.

JSON success bodies are resources, without redundant `status: 200`. Error bodies include `type`, `title`, actual HTTP `status`, safe `detail`, `instance`, `request_id`, optional field `errors`, and `application/problem+json`. Do not echo query strings containing IDs into `instance`; use route path. Stable problem types live under a configurable API docs base URI, never the example host in production. Map malformed JSON to `400`, missing auth `401`, forbidden operation `403`, hidden/missing resource `404`, duplicate/state/idempotency mismatch `409`, valid JSON with invalid domain field `422`, unavailable DB/worker `503`. Set `Location` on `201` and `202`. Generate `X-Request-Id` and trace it without patient content.

## 6. Route contract and deterministic functions

| Route | Handler → service functions | Success and key invariant |
|---|---|---|
| `POST /v1/import-jobs` | `start_import` → `validate_fixture_reference`, `create_import_job` | `202`, Location; admin only, allowlisted fixture, exact expected count. |
| `GET /v1/import-jobs/{id}` | `get_import_job` → `find_import_job` | `200`; admin only, safe counts/status/reject summary. |
| `GET /v1/patients` | `search_patients` → `parse_page`, `authorize_search_scope`, `search_patient_rows` | `200`; filtered and paginated; no global scan response. |
| `GET /v1/patients/{id}` | `get_patient` → `authorize_patient`, `find_patient` | `200`; only recorded age/sex/header fields. |
| `GET /v1/patients/{id}/encounters` | `list_encounters` → `authorize_patient`, `list_patient_encounters` | `200`; dated timeline, one row for current scale fixture. |
| `GET /v1/patients/{id}/context` | `get_context` → `authorize_patient`, `build_patient_context` | `200`; requested encounter must belong to patient; bounded dated sources and explicit gaps. |
| `GET /v1/encounters/{id}` | `get_encounter` → `resolve_encounter`, `authorize_encounter` | `200`; recorded visit, diagnosis code and historical prescription. |
| `GET /v1/sources/{id}` | `get_source` → `resolve_source_owner`, `authorize_source`, `find_source` | `200`; original eight fields and provenance. |
| `POST /v1/queue-items` | `create_queue_item` → `authorize_nurse_assignment`, `validate_patient_encounter_pair`, `assign_queue` | `201`, Location; persisted doctor assignment, conflict for active duplicate. |
| `GET /v1/doctors/me/queue` | `get_my_queue` → `list_assigned_queue` | `200`; only principal's assignments, bounded cursor. |
| `POST /v1/encounters/{id}/summary-jobs` | `create_summary_job` → `authorize_summary_request`, `canonicalize_request`, `insert_or_get_job` | `202`, Location; required `Idempotency-Key`; same key+payload returns same job, different payload `409`. |
| `GET /v1/summary-jobs/{id}` | `get_summary_job` → `authorize_job_owner`, `find_summary_job` | `200`; state, safe failure code, summary URL if ready. |
| `GET /v1/summaries/{id}` | `get_summary` → `authorize_summary`, `find_summary` | `200`; stored version, claims, source links, review state. |
| `POST /v1/summaries/{id}/feedback` | `create_feedback` → `authorize_doctor_feedback`, `validate_feedback`, `insert_feedback` | `201`, Location; authenticated assigned doctor, rating allowlist and bounded reason. |

`GET /v1/patients/SYN-BUNDA-UNKNOWN` must yield `404` problem; `GET /v1/patients?query=P&limit=-1` must yield `422` problem after authentication. The Postman collection has no response fixtures or automated assertions yet; add them in a separately versioned collection update after the API contract is fixed.

## 7. Deterministic summary algorithm

`build_patient_context` fetches only authorized rows and emits `data_gaps = ["allergies_not_supplied", "labs_not_supplied", "notes_not_supplied"]`, plus other gaps only if justified. `build_summary_claims` emits limited dated claims: visit on date with specialist, recorded ICD-10 code, and historical prescription if the cell is not `None recorded`. It never labels a code as an active condition or prescription as current use. Each claim carries `category`, `text`, `as_of_date`, `source_record_ids` and `review_state=unreviewed`. `validate_claim_sources` checks each ID belongs to the same patient, dataset and selected context. Missing/foreign citation fails the job; never store an unsupported claim. Persist artifact, job ready state and audit event in one transaction. `SummaryGenerator` can later accept approved note context, but this release's implementation is deterministic and needs no LLM API key.

## 8. Delivery order, verification and exit gates

1. **Foundation:** crate, config, health check, migrations, adoption check, secret-safe logging.
2. **Read surface:** auth, search, header, timeline, encounter, source, context and RFC 9457 mapper.
3. **Workflow:** queue, summary job worker, stored summary, feedback, audit.
4. **Import path:** 100/1,000 fresh-project fixtures, million-row adoption in the existing Neon project; import/reject recovery.
5. **Contract and scale:** run all 18 collection requests with role-specific tokens; million-row indexed search and context benchmark; document actual measurements and remaining free storage.

Release gates: all 14 app routes match collection methods/paths/status/body shape; 404/422 samples are problem JSON; no unauthorized patient/source content; same idempotency key cannot create multiple jobs; 100% stored claims cite authorized source rows; worker can resume after restart; the Neon environment reports 1,000,000 records without reimport; tests use synthetic data only. Performance targets are recorded as baselines, not invented pass numbers.

## 9. Open hospital integration decisions

Production SSO identity and department mappings, actual HIS/Snowflake contract, source encounter IDs, clinician queue ownership, note availability, retention/audit policy, ICD dictionary version, clinical evaluation, and SLO targets require RS Bunda agreement. These do not block the synthetic Rust pilot. The exposed Neon credential in earlier screenshots should be rotated before putting its URL in any `.env` or deployment secret.
