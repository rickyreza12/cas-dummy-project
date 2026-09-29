# CAS Dummy data dictionary

## Supplied fixture

The checked-in contract fixture is `fixtures/Doctor-360-Scale-1000000-Patients.csv`. The 100-row and 1,000-row fixtures are deterministic prefixes of the supplied million-row file. The import validator requires this exact column order:

| Column | Type | Meaning |
|---|---|---|
| `patient_id` | text | Stable synthetic patient identifier; primary key on import |
| `age_years` | integer | Recorded age, range 0–120 |
| `sex` | text | Source-provided sex value |
| `visit_date` | ISO date | Source-provided visit date |
| `specialist` | text | Source-provided specialist label |
| `icd10_code` | text | Source-provided ICD-10 code |
| `prescription_recorded` | text | Historical source value; `None recorded` means no prescription item |
| `source_record_id` | text | Stable source citation identifier; unique within an imported fixture |

Validation is streaming and rejects malformed rows, invalid dates/ages/codes, blank keys, and duplicate patient/source identifiers. It records a SHA-256 checksum without mutating the source file.

## Database

`migrations/0001_operational.sql` creates the operational registry, identity, queue, import, summary, feedback, audit, staging, and synthetic-patient tables. The API does not rewrite or duplicate an existing Neon source table. Neon relation/index sizes and active-branch measurements remain deployment evidence and must be captured from the target environment before adoption migrations.

## Neon read-only inspection (2026-09-29)

`check-fixture --dataset demo-1000000-v1 --expected 1000000` completed twice against the configured Neon source without applying migrations or mutations. Both runs reported:

- row count: `1,000,000`
- source table size: `215,728,128` bytes
- indexes: `synthetic_patients_pkey`, `synthetic_patients_source_record_id_idx`
- fixture checksum: `0f5b68a931031e34af4721b61428b6a135f4c2e1778c9713a1463b2e0719df3e`

The legacy source schema permits SQL `NULL` for `source_record_id`; the verifier separately rejects any null or blank source value in actual data.

## Neon capacity snapshot (2026-09-30)

Read-only SQL against `neondb.public` reported a database size of `214 MB` and
the same source relation size of `215,728,128` bytes (`206 MB`). This is a
database-size observation, not a remaining-quota measurement; Neon console
storage headroom is still required before any source-table mutation or index
addition.

The same read-only session reported active database `neondb`, branch
`br-green-scene-b3pc2xu7`, and project `cold-rice-86584489`. Neon did not
expose a region through the SQL session setting (`NULL`); capture region and
remaining quota from the Neon console before adoption.

## Adoption metadata write (2026-09-30)

After the read-only checks passed, the only Neon write was an idempotent
`dataset_registry` upsert for `demo-1000000-v1` with `status=active` and
`origin=existing_neon_table`. Verification reported 1,000,000 source rows,
only the existing `synthetic_patients` table, and zero `import_jobs`. No source
rows were copied, rebuilt, or reimported.
