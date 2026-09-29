# PRD: Doctor's 360 Dummy Data Backend and Patient Context API

**Status:** Draft for architecture review  
**Date:** 29 September 2026  
**Audience:** Backend, AI, frontend, product and hospital integration teams  
**Related:** Doctor's 360 OBGYN Pilot PRD; AI RS BUNDA Strategy Response; synthetic scale workbooks.

## 1. Goal

Build a backend service that imports **one selected synthetic dataset** of 100, 1,000 or 1,000,000 patient rows into an indexed operational store, exposes authorized patient context to the web app, and prepares a stored, source-referenced history summary. The three workbooks are separate scale fixtures, not three sources to union together: the smaller datasets are subsets of the larger one.

The service demonstrates the boundary in the strategy: **web app → Patient Context API → authorized record store → retrieval → summary job → stored summary → web app**. The AI service does not hold hospital database credentials or decide access rights.

## 2. What the data really contains

Each scale workbook has one worksheet, one row per fictional patient and one synthetic visit per row:

| Column | Meaning | Backend handling |
|---|---|---|
| `patient_id` | Invented stable identifier | Namespace by `dataset_id`; searchable unique key within dataset |
| `age_years` | Age in the fixture | Integer; do not infer birth date |
| `sex` | Recorded sex field | Preserve supplied value |
| `visit_date` | Date of the synthetic visit | Parse as date, retain source value |
| `specialist` | Specialty of that visit | Normalize for filtering, retain original |
| `icd10_code` | Recorded diagnosis code | Validate syntax and approved dictionary where available; do not infer active condition |
| `prescription_recorded` | Recorded prescription name or “None recorded” | Historical prescription only; current use unknown |
| `source_record_id` | Invented source identifier | Unique in dataset, used for claim verification |

**Not supplied in these three workbooks:** encounter ID, SOAP notes, allergies, labs, imaging, procedures, medication status, named doctors, queue assignments or user accounts. Create a deterministic internal `encounter_id` from `(dataset_id, source_record_id)` during import. Never claim that generated encounter IDs came from RS Bunda. A separate richer 100-patient workbook contains fictional notes and linked visits for optional AI experiments; it is not the same shape as the scale fixtures.

## 3. Scope

### In scope

- Dataset import job for each workbook size; one active `dataset_id` per environment or explicit dataset selection in test environments.
- Streaming parse, schema validation, bounded batches, row counts, duplicate detection, import manifest and reject report.
- Indexed patient, encounter and source tables in an operational database.
- Search, patient details, encounter timeline, source detail and Patient Context API.
- Synthetic role authentication for local development; backend authorization boundary designed for hospital SSO later.
- Queue, asynchronous summary job, stored artifact, feedback and activity audit using synthetic operational data.
- Deterministic, source-linked summary from the eight columns; optional LLM adapter only when approved notes are added.
- HTTP semantics and error responses defined below.

### Out of scope for the scale fixture

- Clinical diagnosis or treatment advice; declaring prescriptions currently used; allergy or critical-alert claims from absent data.
- OCR, vector indexing and patient-history chatbot for eight-column rows.
- A live Snowflake, HIS or RS Bunda connection until access method and contract are agreed.
- Loading all 1,000,000 rows into process memory or reading `.xlsx` on every request.

## 4. Actors and boundaries

| Actor | Operation | Boundary |
|---|---|---|
| Doctor | Search assigned/authorized patient, inspect source and summary, give feedback | Server validates patient scope on each request |
| Nurse | Search, assign queue, request preload | Server validates department and encounter scope |
| Admin | Import synthetic fixture, manage test access, inspect jobs | Admin role does not grant clinical access by itself |
| Medical Records (optional) | Investigate identity/source issues | Explicit review permission |
| Summary worker | Receives curated context and writes draft via backend contract | No direct patient-store credentials in LLM process |

## 5. Proposed components

1. **Import worker:** stream the workbook, validate and batch-insert into a staging area, then publish an immutable dataset version after checks pass.
2. **Operational database:** PostgreSQL is a reasonable pilot choice for patient, encounter, source, queue, job, summary, feedback and audit tables. Index `(dataset_id, patient_id)`, `(dataset_id, source_record_id)` and encounter date; profile the million-row import and search.
3. **Patient Context API:** checks identity and access, resolves the selected patient, retrieves only permitted dated records, and returns a bounded context DTO.
4. **Summary orchestration:** queued → running → ready/failed; idempotent job creation; source-link validation; versioned artifact store.
5. **AI adapter:** optional interface for approved free-text notes. The eight-column fixture uses a deterministic summary builder so missing clinical facts cannot be invented.

## 6. Import and scale behavior

- Import each workbook under a distinct immutable `dataset_id` such as `demo-100-v1`, `demo-1000-v1` or `demo-1000000-v1`. Never combine overlapping patient IDs across fixtures in one clinical view.
- Confirm headers and data types; reject blank/duplicate `patient_id` or `source_record_id` within a dataset. Preserve row number and reason in a reject report without exposing clinical content in logs.
- Record file checksum, expected/imported/rejected counts, start/end time, schema version and status. Publish dataset only after count and uniqueness checks complete.
- Use cursor pagination for patient search and encounter lists; apply server-side limits and indexes. Do not return a million-row response.
- Test 100 and 1,000 first, then benchmark one million for import duration, memory, indexed search, context latency and failure recovery. Targets for these backend benchmarks remain to be agreed; do not present synthetic measurements as hospital performance.

## 7. HTTP API contract (draft v1)

Base path: `/v1`. JSON success bodies describe the resource directly. HTTP status is in the response status line; repeating `status: 200` in every success body is not required. Use `Cache-Control: no-store` for patient context, source, summary and feedback responses. Correlation IDs in headers/logs must not contain patient identifiers.

| Endpoint | Success | Purpose |
|---|---|---|
| `POST /v1/import-jobs` | `202 Accepted` + `Location` | Admin starts import of an already staged fixture reference |
| `GET /v1/import-jobs/{id}` | `200 OK` | Status, counts and reject summary |
| `GET /v1/patients?query=&limit=&cursor=` | `200 OK` | Authorized, bounded search |
| `GET /v1/patients/{id}` | `200 OK` | Patient header in active dataset |
| `GET /v1/patients/{id}/encounters?limit=&cursor=` | `200 OK` | Dated encounter timeline |
| `GET /v1/patients/{id}/context?encounter_id=` | `200 OK` | Curated patient context and source IDs |
| `GET /v1/encounters/{id}` | `200 OK` | Encounter detail |
| `GET /v1/sources/{id}` | `200 OK` | Authorized original source row/detail |
| `POST /v1/queue-items` | `201 Created` + `Location` | Nurse assigns patient/encounter to doctor |
| `GET /v1/doctors/me/queue` | `200 OK` | Doctor's assigned queue |
| `POST /v1/encounters/{id}/summary-jobs` | `202 Accepted` + `Location` | Queue preparation or refresh, with `Idempotency-Key` |
| `GET /v1/summary-jobs/{id}` | `200 OK` | Job state and summary link when ready |
| `GET /v1/summaries/{id}` | `200 OK` | Stored, versioned claims with validated source links |
| `POST /v1/summaries/{id}/feedback` | `201 Created` + `Location` | Doctor feedback |

**Status choices:** `200` for retrieval, `201` for created queue/feedback resources, `202` for accepted asynchronous work, `204` only for a successful action with no body. `400` malformed request, `401` missing/invalid authentication, `403` authenticated but unauthorized, `404` not found within authorized scope, `409` state or identity conflict, `422` syntactically valid but semantically invalid input, `503` temporary dependency outage. Use actual HTTP status, not a success envelope with an embedded error code.

### Example: patient context (structured fixture)

```json
{
  "patient_id": "SYN-BUNDA-P0001",
  "dataset_id": "demo-100-v1",
  "age_years": 40,
  "sex": "Female",
  "encounters": [
    {
      "encounter_id": "enc-demo-100-v1-0001",
      "visit_date": "2024-05-29",
      "specialist": "General Practice",
      "diagnoses": [{"code": "Z30.9", "source_record_id": "SYN-BUNDA-V0001-01-SRC"}],
      "prescriptions": [{"name": "Paracetamol", "current_use": "unknown", "source_record_id": "SYN-BUNDA-V0001-01-SRC"}]
    }
  ],
  "data_gaps": ["allergies_not_supplied", "labs_not_supplied", "notes_not_supplied"]
}
```

The example uses the first row of the 100-patient scale fixture. The internal encounter ID is illustrative; the imported source values must remain unchanged.

### Problem Details (RFC 9457)

On failure set `Content-Type: application/problem+json`. Return a stable `type` URI (documented by this API), short `title`, actual HTTP `status`, safe `detail`, and request-specific `instance`; optional `request_id` and validation `errors` are extensions. Do not include a patient name, clinical note or access token in error detail.

```http
HTTP/1.1 409 Conflict
Content-Type: application/problem+json
Cache-Control: no-store
```

```json
{
  "type": "https://api.doctor360.example/problems/patient-identity-conflict",
  "title": "Patient identity requires review",
  "status": 409,
  "detail": "Summary preparation is blocked for an unresolved patient identity.",
  "instance": "/v1/encounters/enc-demo-100-v1-0001/summary-jobs",
  "request_id": "req-demo-001"
}
```

The `.example` host is a placeholder. The response status and the optional `status` member must agree.

## 8. Summary and source acceptance

- Each claim has `text`, `source_record_ids`, `as_of_date`, `category` and review state. The backend checks that every source ID belongs to this authorized patient/context.
- A claim without a valid source is withheld or the job fails; a citation string produced by an LLM alone is not proof.
- Historical prescriptions remain labeled *prescribed on date*, with current use unknown. Unavailable allergy/lab/note data is labeled unavailable, not negative.
- The generated artifact stores dataset version, patient/encounter ID, source set, algorithm/model and prompt version (if any), timestamps and feedback linkage.

## 9. API versus LLM tool calling

The **Patient Context API is required** for the web app and backend boundary. Ordinary backend code should call its authorized service/repository functions to assemble patient context. LLM tool calling is an optional orchestration pattern after an LLM is introduced: a model may request a narrowly defined function such as `get_patient_context(patient_id, encounter_id)`, but the backend validates the request, permission and result. The model never gets a raw SQL tool or direct Snowflake credentials. Tool calling cannot replace HTTP contracts, authorization or audit.

## 10. Snowflake and vector search decision

- **For these three files:** import to an indexed operational store. Snowflake is not required.
- **If RS Bunda supplies Snowflake:** treat it as an upstream governed source or approved view/API. Decide with its data team whether the app receives bounded extracts, scheduled sync, or a service-to-service query. Do not expose Snowflake directly to browsers or LLMs.
- **For eight structured columns:** PostgreSQL queries and indexes are enough; no vector database is needed.
- **If substantial free-text notes/documents arrive:** evaluate text search first, then hybrid retrieval/embeddings if tests show a benefit. Options include an existing Snowflake Cortex Search service or a separately governed vector index. Choose based on data location, permissions, evaluation, latency and cost, not because RAG appears on a strategy slide.

## 11. Acceptance and open decisions

1. Each fixture imports with exactly 100, 1,000 or 1,000,000 unique patient rows; source IDs remain unique per dataset.
2. Searching an overlapping ID returns records only from the active dataset; no cross-dataset mixing.
3. Unauthorized context/source/summary requests return no patient content and an appropriate HTTP error.
4. Summary jobs are idempotent for the same key; polling returns a stored artifact or a reviewable failure.
5. Every important summary claim has a backend-validated source link.
6. The API collection exercises success and RFC 9457 error examples against a local implementation.

**Decisions needed:** hospital source contract and access method; production identity/SSO; encounter ID source; whether notes are available; queue ownership; retention and audit policies; performance targets at one million rows; terminology dictionary version; model and clinical evaluation criteria.

## References

- RFC 9110, *HTTP Semantics*: https://www.rfc-editor.org/rfc/rfc9110.html
- RFC 9457, *Problem Details for HTTP APIs*: https://www.rfc-editor.org/rfc/rfc9457.html
- Snowflake SQL API: https://docs.snowflake.com/en/developer-guide/sql-api/index
- Snowflake Cortex Search: https://docs.snowflake.com/en/user-guide/snowflake-cortex/cortex-search/cortex-search-overview
