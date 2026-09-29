# REST route coverage

Contract source: `Doctor-360-Patient-Context-API.postman_collection.json`.

| Contract area | Method/path | Role | Expected |
|---|---|---|---:|
| Health | GET `/healthz` | public | 200 |
| Token | POST `/v1/auth/token` | development config | 200/400/503 |
| Import | POST `/v1/import-jobs` | admin | 202 |
| Import status | GET `/v1/import-jobs/{id}` | admin | 200 |
| Search | GET `/v1/patients` | doctor/nurse/medical records | 200/422 |
| Header | GET `/v1/patients/{id}` | scoped clinical role | 200/404 |
| Timeline | GET `/v1/patients/{id}/encounters` | scoped clinical role | 200/404 |
| Context | GET `/v1/patients/{id}/context` | scoped clinical role | 200/404 |
| Encounter | GET `/v1/encounters/{id}` | scoped clinical role | 200/404 |
| Source | GET `/v1/sources/{id}` | scoped clinical role | 200/404 |
| Queue assignment | POST `/v1/queue-items` | nurse/admin/medical records | 201/409 |
| Queue item | GET `/v1/queue-items/{id}` | assigned doctor/manager role | 200/404 |
| Doctor queue | GET `/v1/doctors/me/queue` | doctor | 200 |
| Summary create | POST `/v1/encounters/{id}/summary-jobs` | scoped clinical role | 202/409/422 |
| Summary poll | GET `/v1/summary-jobs/{id}` | scoped clinical role | 200/404 |
| Summary read | GET `/v1/summaries/{id}` | scoped clinical role | 200/404 |
| Feedback create | POST `/v1/summaries/{id}/feedback` | assigned doctor | 201/422 |
| Feedback read | GET `/v1/feedback/{id}` | submitting doctor | 200/404 |

All `/v1` responses carry `X-Request-Id`; protected responses carry `Cache-Control: no-store`. Problem responses use `application/problem+json`, and 401 responses include `WWW-Authenticate: Bearer`. Runtime/Postman observations must be captured against local PostgreSQL before the corresponding acceptance boxes are checked.

## Postman request traceability

The original collection has 18 requests. Route-registration and shared
contract coverage are automated in `protected_route_surface_is_registered`,
`postman_collection_contains_expected_request_count`, and the focused HTTP
unit tests. Live request results remain an execution gate.

| Request | Method/path | Expected role | Automated reference |
|---|---|---|---|
| Start import · 100 rows | POST `/v1/import-jobs` | admin | `create_import_job` + import integration test |
| Start import · 1,000 rows | POST `/v1/import-jobs` | admin | `create_import_job` + import integration test |
| Start import · 1,000,000 rows | POST `/v1/import-jobs` | admin | `create_import_job` allowlist test |
| Read import job | GET `/v1/import-jobs/{id}` | admin | `protected_route_surface_is_registered` |
| Search patients | GET `/v1/patients` | doctor/nurse/medical_records | `parse_limit` and cursor tests |
| Get patient header | GET `/v1/patients/{id}` | scoped clinical role | route registration + local workflow |
| Get encounter timeline | GET `/v1/patients/{id}/encounters` | scoped clinical role | route registration + cursor tests |
| Get curated context | GET `/v1/patients/{id}/context` | scoped clinical role | local workflow + context response evidence |
| Get encounter | GET `/v1/encounters/{id}` | scoped clinical role | encounter ID/domain tests |
| Open original source | GET `/v1/sources/{id}` | scoped clinical role | local workflow + source route registration |
| Assign to doctor queue | POST `/v1/queue-items` | nurse/medical_records | local workflow + constraint test |
| Get my queue | GET `/v1/doctors/me/queue` | doctor | local workflow |
| Prepare summary | POST `/v1/encounters/{id}/summary-jobs` | nurse/doctor | local workflow + idempotency constraint test |
| Poll summary job | GET `/v1/summary-jobs/{id}` | scoped clinical role | route registration + local workflow |
| Read stored summary | GET `/v1/summaries/{id}` | scoped clinical role | route registration + local workflow |
| Submit doctor feedback | POST `/v1/summaries/{id}/feedback` | assigned doctor | local workflow |
| Patient not found · 404 | GET `/v1/patients/SYN-BUNDA-UNKNOWN` | authorized clinical role | problem-contract tests |
| Invalid search limit · 422 | GET `/v1/patients?query=P&limit=-1` | authorized clinical role | `invalid_pagination_limits_are_rejected_as_422` |
