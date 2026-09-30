# CAS Dummy Backend

Rust/Axum backend for the Doctor 360 Patient Context API. This service is backend-only; it exposes REST endpoints and does not include a frontend.

## Pilot scope and unresolved hospital decisions

All checked-in fixtures and generated encounter IDs are synthetic development data. They are not live RS Bunda records, documents, diagnoses, prescriptions, or clinician identities. Before production use, the hospital must decide the SSO/identity integration, authoritative source contract, retention policy, ICD-10 dictionary/version, note availability, clinical evaluation process, and service targets.

See [`docs/api-contract.md`](docs/api-contract.md) for the route, role, status, JWT, and cursor contract.
See [`docs/failure-runbook.md`](docs/failure-runbook.md) for operational recovery steps.
See [`docs/route-coverage.md`](docs/route-coverage.md) for the Postman-to-route coverage map.

## Requirements

- Rust stable (edition 2024)
- PostgreSQL for database-backed tasks

## Configuration

Copy `.env.example` to `.env` and set the values. Never commit `.env`, database URLs, JWT private keys, or access tokens.

Required variables:

```env
# POSTGRES_URL takes precedence; DATABASE_URL is accepted as a direct local alias.
# Use one direct connection URL for the pilot; no separate pooled read URL is required.
POSTGRES_URL=postgres://user:password@localhost:5432/cas_dummy
JWT_ISSUER=cas-dummy-api
JWT_AUDIENCE=cas-dummy-client
JWT_PRIVATE_KEY=
JWT_PUBLIC_KEY=
JWT_KEY_ID=local-current
JWT_PREVIOUS_KEY_ID=
JWT_PREVIOUS_PUBLIC_KEY=
JWT_ACCESS_TOKEN_TTL_SECONDS=900
CURSOR_SIGNING_KEY=replace-with-a-long-random-secret
WORKER_CONCURRENCY=1
DATABASE_POOL_MAX=5
API_BIND_ADDR=127.0.0.1:8000
API_PUBLIC_BASE_URL=http://localhost:8000
ACTIVE_DATASET_ID=demo-1000000-v1
SUMMARY_LEASE_SECONDS=60
SUMMARY_MAX_ATTEMPTS=3
```

`CURSOR_SIGNING_KEY` must contain at least 32 random bytes. JWT signing keys must be supplied through secret configuration before enabling token issuance. The API will never log token or key values.

JWT access tokens use RS256 and carry a `kid`, `token_type=access`, issuer,
audience, subject, role, scope, issued-at, expiration, not-before, and `jti`.
To rotate a key, set the new private/public key and `JWT_KEY_ID`, retain the old
public key as `JWT_PREVIOUS_PUBLIC_KEY` with its former ID, then remove the old
pair after all old tokens expire. To revoke one compromised token immediately,
insert its `jti` and expiration into `revoked_jtis`; protected requests reject
active revocations. This is an operational action and must not log the token.

For a disposable local backend environment, generate an RS256 key pair outside
the repository and provide the PEM values only to the process environment. Do
not paste multiline PEM material into `.env.example` or commit it to `.env`:

```powershell
$keyDirectory = Join-Path $env:LOCALAPPDATA "cas-dummy"
New-Item -ItemType Directory -Force -Path $keyDirectory | Out-Null
$privateKey = Join-Path $keyDirectory "jwt-private.pem"
$publicKey = Join-Path $keyDirectory "jwt-public.pem"
ssh-keygen -t rsa -b 2048 -m PEM -N '""' -f $privateKey
ssh-keygen -e -m PEM -f "$privateKey.pub" | Set-Content -NoNewline $publicKey
$env:JWT_PRIVATE_KEY = [System.IO.File]::ReadAllText($privateKey)
$env:JWT_PUBLIC_KEY = [System.IO.File]::ReadAllText($publicKey)
```

## Run

From this directory:

```powershell
cargo run -- api
```

The API listens on the configured `API_BIND_ADDR` (by default,
`http://127.0.0.1:8000`).

Swagger UI is available at `http://127.0.0.1:8000/swagger`; the machine-readable
OpenAPI contract is at `http://127.0.0.1:8000/openapi.json`. Swagger UI is public,
while `/v1/*` resources remain protected by JWT middleware.

Startup connects to `POSTGRES_URL` and applies embedded SQLx migrations before serving requests. The public endpoint is `GET /healthz`; patient, encounter, source, context, queue, summary, feedback and import routes require `Authorization: Bearer <JWT>`. JWTs must be signed with the configured RS256 key pair and must contain the configured issuer and audience.

Place only allowlisted fixture files under `FIXTURE_DIR` (`fixtures` by default). The worker validates and stages an import before publishing it; client-supplied filesystem paths and URLs are rejected.

Check health:

```powershell
curl http://127.0.0.1:8000/healthz
```

The development token endpoint is `POST /v1/auth/token` with JSON `{ "subject": "demo-doctor-01", "role": "doctor", "scope": [] }`. It is disabled until `JWT_PRIVATE_KEY` is configured.

Other command modes are available for the checklist workflow:

```powershell
cargo run -- worker
cargo run -- check-fixture
cargo run -- seed-dev-users
```

`check-fixture` requires an explicit dataset and expected count. It validates an allowlisted CSV or small XLSX fixture (schema, dates, keys, duplicates, row count, and SHA-256) before checking the database count. XLSX validation is capped at 1,000 rows; the million-row source remains CSV/Neon-only:

```powershell
cargo run -- check-fixture --dataset demo-1000000-v1 --expected 1000000
```

`check-fixture` validates the existing schema and data without applying migrations, so it can be run with a read-only database role during adoption review.

`seed-dev-users` reads a local ignored JSON file from `DEV_TOKENS_CONFIG`. Each entry must contain `principal_id`, `role`, optional `department_scope`, and a random `token` of at least 32 characters. The command stores only SHA-256 token digests.

Run the worker in a second terminal after starting the API. It claims queued summary jobs with PostgreSQL row locks and writes deterministic source-cited summaries:

```powershell
cargo run -- worker
```

## Verify

```powershell
cargo fmt --check
cargo check
cargo test
```

Run the PostgreSQL-gated migration test separately against a disposable or approved database:

```powershell
$env:POSTGRES_URL = "postgres://user:password@localhost:5432/cas_dummy"
cargo test -- --ignored
```

It applies the embedded migrations twice and must not be pointed at a mutation-sensitive production dataset.

For a fully disposable local migration check, initialize a temporary PostgreSQL
cluster with local trust authentication, start it on an unused port, set
`POSTGRES_URL` to that local database, and run the same ignored test. The
recorded result is in [`docs/local-verification.md`](docs/local-verification.md).
Stop and remove the temporary cluster after the test; never use this procedure
against the Neon source database.

## Local backend runbook

1. Start a local PostgreSQL database and set `POSTGRES_URL` in `.env`.
2. Set `JWT_ISSUER`, `JWT_AUDIENCE`, `JWT_PRIVATE_KEY`, `JWT_PUBLIC_KEY`, `CURSOR_SIGNING_KEY`, and `FIXTURE_DIR`.
3. Start the API:

```powershell
cargo run -- api
```

4. In a second terminal, start the worker:

```powershell
cargo run -- worker
```

5. Confirm the service before authenticated calls:

```powershell
curl http://127.0.0.1:8000/healthz
```

6. Seed development identities only from an ignored local JSON file:

```powershell
$env:DEV_TOKENS_CONFIG = "dev-users.json"
cargo run -- seed-dev-users
```

Never point mutation tests at the existing Neon million-row database. Use a fresh local database for imports and failure-path tests; use Neon only for read-only adoption/benchmark checks after the capacity gate.

For Postman, import `docs/Doctor-360-Patient-Context-API.postman_collection.json`.
Use `docs/postman-local-100.environment.json` for local mutation and workflow
tests. Use `docs/postman-neon-readonly.environment.json` only with an approved
read-only API; its import, queue, and summary-create variables are deliberately
non-actionable placeholders. Keep role-specific development tokens in local
secret variables only. Never export either environment after adding tokens.

If Newman is installed, run the local collection after seeding the selected
role token and starting API/worker:

```powershell
npx --yes newman run docs/Doctor-360-Patient-Context-API.postman_collection.json `
  -e docs/postman-local-100.environment.json
```

The collection chains typed response IDs (`import_job_id`, `queue_item_id`,
and `summary_job_id`). Its Neon environment is read-only by policy and must
not be used with mutation requests.
