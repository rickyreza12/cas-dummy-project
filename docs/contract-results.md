# Contract execution results

## Local synthetic run

Date: 2026-09-30

Environment: disposable PostgreSQL 16, `demo-100-v1`, 100 checked-in CSV
rows, local RS256 development key pair. Secrets and connection values are not
recorded.

| Flow | Method/path | Observed |
|---|---|---:|
| Health | GET `/healthz` | 200 |
| Admin import | POST `/v1/import-jobs` | 202 + `Location` + `no-store` |
| Nurse context | GET `/v1/patients/SYN-BUNDA-P0001/context` | 200; 3 data gaps |
| Queue assignment | POST `/v1/queue-items` | 201 + `Location` |
| Doctor queue | GET `/v1/doctors/me/queue` | 200; assigned patient returned |
| Source read | GET `/v1/sources/SYN-BUNDA-V0001-01-SRC` | 200 |
| Summary creation | POST `/v1/encounters/enc-demo-100-v1-0001/summary-jobs` | 202 + `Location` |
| Summary poll/read | GET job, then GET summary | 200; 3 cited claims |
| Feedback | POST `/v1/summaries/{id}/feedback` | 201 + `Location` |

The run also verified JWT issuance, protected-route authentication, response
`Cache-Control: no-store`, and typed response IDs used by the Postman scripts.

## Automated contract checks

`cargo test --workspace` passed with 41 tests and 3 ignored PostgreSQL-gated
tests. The gated suite passed separately against disposable PostgreSQL:

- migrations applied twice;
- duplicate active queue and duplicate summary idempotency keys were rejected;
- 100-row and 1,000-row imports reached `ready` with exact row counts.

## Not claimed

The original 18-request collection was not run as one Newman session because
its requests require switching between admin, nurse, and doctor credentials.
The Neon environment remains read-only and no Neon mutation or production
acceptance claim is made. Neon capacity, branch/database baseline, and
million-row performance evidence remain separate release gates.
