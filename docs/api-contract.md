# CAS Dummy REST Contract

All `/v1/*` routes require `Authorization: Bearer <JWT>` unless stated otherwise. Protected responses use `Cache-Control: no-store`; RFC 9457 failures use `application/problem+json`.

Pagination `limit` values must be integers from 1 through 100. Missing limits default to 20; negative, zero, nonnumeric, and oversized values return 422 problem JSON. Accepted summary jobs are polled through their `Location`; once ready, the poll response includes `summary_url`.

| Method | Path | Access | Success |
|---|---|---|---|
| GET | `/healthz` | Public | 200 static `ok` |
| POST | `/v1/auth/token` | Development configuration | 200 JWT |
| GET | `/v1/patients` | Doctor, Nurse, Medical Records | 200 paged patients |
| GET | `/v1/patients/{id}` | Scoped clinical roles | 200 patient or hidden 404 |
| GET | `/v1/patients/{id}/encounters` | Scoped clinical roles | 200 paged encounters |
| GET | `/v1/patients/{id}/context` | Scoped clinical roles | 200 context |
| GET | `/v1/encounters/{id}` | Scoped clinical roles | 200 encounter or hidden 404 |
| GET | `/v1/sources/{id}` | Scoped clinical roles | 200 source or hidden 404 |
| POST | `/v1/queue-items` | Nurse, Medical Records | 201 + `Location` |
| GET | `/v1/queue-items/{id}` | Authenticated | 200 queue item |
| GET | `/v1/doctors/me/queue` | Authenticated doctor | 200 paged queue |
| POST | `/v1/encounters/{id}/summary-jobs` | Scoped clinical roles | 202 + `Location`; requires `Idempotency-Key` |
| GET | `/v1/summary-jobs/{id}` | Scoped clinical roles | 200 job |
| GET | `/v1/summaries/{id}` | Scoped clinical roles | 200 summary |
| POST | `/v1/summaries/{id}/feedback` | Assigned Doctor | 201 + `Location` |
| GET | `/v1/feedback/{id}` | Scoped clinical roles | 200 feedback |
| POST | `/v1/import-jobs` | Admin | 202 + `Location` |
| GET | `/v1/import-jobs/{id}` | Admin | 200 import job |

Import jobs persist the validated fixture SHA-256 and schema version. A job is published only after imported and rejected rows reconcile with the requested count and all required rows validate.

Feedback ratings are exactly `Useful`, `Incomplete`, `Incorrect`, or `Unsafe`. A trimmed, non-empty reason is required for every rating and is limited to 2,000 characters. Only the submitting doctor can retrieve the feedback resource.

JWT validation checks the RS256 signature, issuer, audience, `exp`, `iat`, `sub`, role and scope claims. Protected requests also require a matching active `dev_identities` row; inactive, unknown, role-mismatched, or scope-mismatched identities return 401. Missing, malformed, expired or invalid tokens return 401; authenticated but disallowed roles return 403; hidden patient/source resources return 404.

## Role-action matrix

| Action | Doctor | Nurse | Medical Records | Admin |
|---|---:|---:|---:|---:|
| Patient search/context/source | Assigned only | Department scope | Clinical scope | Denied |
| Queue assignment | Denied | Allowed | Allowed | Allowed |
| Own doctor queue | Allowed | Denied | Denied | Denied |
| Summary create/read | Assigned only | Scoped preload | Scoped clinical | Denied |
| Feedback | Assigned doctor | Denied | Denied | Denied |
| Import jobs | Denied | Denied | Denied | Allowed |

Unauthenticated requests return 401 with `WWW-Authenticate: Bearer`. Authenticated role failures return 403. Hidden patient, encounter, and source records return the same 404 problem regardless of whether the resource is unknown or outside scope.

Patient cursors are HMAC-SHA256 signed with `CURSOR_SIGNING_KEY`, bound to the query, and expire after 15 minutes. Cursor failures return 422.
