# Local verification evidence

## PostgreSQL migration gate

On 2026-09-29, a disposable PostgreSQL 16 cluster was initialized locally with
trust authentication on port `55432`. The ignored migration test was run with
`POSTGRES_URL` pointed only at that temporary cluster:

```text
cargo test --workspace -- --ignored --nocapture
```

The ignored database suite applied the embedded SQLx migrations twice, verified
operational uniqueness constraints, and exercised both small allowlisted
fixture imports:

```text
test tests::migrations_are_idempotent_against_postgres ... ok
test tests::operational_unique_constraints_reject_duplicate_active_work ... ok
test tests::import_worker_processes_each_small_allowlisted_fixture ... ok
test result: ok. 3 passed; 0 failed
```

The temporary server was stopped after the test. No application or Neon
database was modified by this verification.

## Unreachable-database startup check

With a disposable test configuration and `POSTGRES_URL` pointed at
`127.0.0.1:59999`, `cas-dummy api` exited with status 1 after the bounded
connection attempt and reported only `pool timed out while waiting for an open
connection`. No credentials, bearer tokens, or patient data were logged.

## Local API runtime check

On 2026-09-29, the service started against the disposable PostgreSQL 16
cluster on `127.0.0.1:55432`, with a disposable test-only configuration and
`API_BIND_ADDR=127.0.0.1:18080`. The following HTTP probes passed before both
the API and database were stopped:

- `GET /healthz` returned `200` with body `ok`.
- An unknown route returned `404`, `application/problem+json`,
  `Cache-Control: no-store`, and `X-Request-Id`.

## Operational constraint gate

On 2026-09-29, the disposable PostgreSQL cluster ran the ignored
database-backed tests. Migrations applied idempotently and the database
rejected both a duplicate active queue item and a repeated
`(requester_id, idempotency_key)` summary-job request. The test used random
temporary IDs and removed its rows before completion.

## Authenticated local import flow

On 2026-09-29, a separate disposable runtime configuration used an ephemeral
RSA key pair and a local PostgreSQL 16 cluster. The checked-in `.env` was not
changed and no token or connection value was recorded. After seeding only
synthetic local identities, the following flow passed:

- development JWT issuance for the local admin identity;
- authenticated `POST /v1/import-jobs` for `demo-100-v1`, returning `202`, a
  `Location` header, and `Cache-Control: no-store`;
- worker processing to `ready` with `100` imported and `0` rejected rows;
- `100` rows persisted in `synthetic_patients`.

The API, worker, and local PostgreSQL server were stopped after the check.
The generated key material was kept only in a local temporary directory and
was not added to source control or `.env`.

## JWT key-ID runtime check

After adding JWT key-ID selection and server-side revocation storage, a
separate disposable runtime check issued a new local RS256 access token with
the configured `kid`. The token authenticated `GET /v1/patients` successfully
with `200` and `Cache-Control: no-store`. The token, PEM values, and database
URL were not recorded. The migration suite also applied the `revoked_jtis`
table migration idempotently.

## Empty-registry import regression check

With an empty `dataset_registry` in the disposable local database, an
authenticated admin request to create the first `demo-100-v1` import job
returned `202`, `queued`, and a `Location` header. This verifies that an empty
registry is treated as the valid first-import state rather than a dependency
failure.

## End-to-end doctor workflow

On 2026-09-29, a disposable local PostgreSQL 16 run completed the 100-row
synthetic workflow without recording tokens or connection values:

- an admin queued and the worker imported `demo-100-v1`;
- the scoped nurse read the first patient context (`200`) with three explicit
  data gaps and assigned the patient to the doctor (`201`); the doctor queue
  then returned that assignment (`200`);
- the assigned doctor read the source row (`200`);
- the nurse created a summary job (`202`), the worker marked it ready, and the
  doctor read a three-claim cited summary (`200`);
- the assigned doctor submitted feedback (`201`).

The API, worker, and database were stopped after the run. This is local
synthetic-fixture evidence only; it does not prove Neon adoption or hospital
production readiness.

## Remaining evidence gates

This local result does not prove the existing Neon source-table schema, row
count, index sizes, credential rotation, or million-row performance claims.
Those require a separately authorized read-only Neon connection and must remain
unchecked until captured without exposing credentials.

## Authorized Neon JWT smoke test (2026-09-30)

After explicit write approval, a temporary `nurse` identity scoped to
`Cardiology` was inserted into `dev_identities`, used to issue an RS256 JWT,
and used for bounded `SYN-BUNDA-P0001` search against the adopted active
dataset. The request returned `200` with `Cache-Control: no-store`. The
temporary identity was deleted immediately afterward; no patient rows were
changed.

A second temporary nurse scoped to `Neurology` searched the same Cardiology
patient and received `200` with no patient identifier in the response. That
identity was also deleted immediately afterward.

The same approved Neon-backed API session returned `422` for
`limit=-1` and `404` for an unknown patient context request. The temporary
identity used for these checks was deleted after the requests.
