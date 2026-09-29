# CAS Dummy failure runbook

| Symptom | Action |
|---|---|
| Database disconnect/startup failure | Check `POSTGRES_URL`, confirm PostgreSQL is reachable, then restart `cargo run -- api`. Pool startup has bounded timeouts and does not retry forever. |
| Stale pool or timeout | Inspect PostgreSQL connection saturation and `POOL_MAX_CONNECTIONS`; restart the API after the database recovers. |
| Worker lease expiry | Inspect `summary_jobs.status`, `attempt_count`, and `lease_expires_at`. The worker reclaims expired leases and fails jobs after the attempt limit. |
| Invalid source citation | Keep the job failed; inspect source IDs and fixture integrity. Never publish a summary with an unverified source. |
| Missing fixture | Confirm `FIXTURE_DIR` and an allowlisted filename; do not pass filesystem paths through the API. |
| Storage quota | Stop import workers, preserve the failed job record, check staging/table sizes, and do not retry against Neon without a capacity review. |
| Adoption schema mismatch | Stop before any adoption write, compare `information_schema.columns` and primary-key metadata with the eight-column fixture contract, and resolve the source contract explicitly; do not alter the existing Neon table in place. |
| Token rotation | Rotate any database credential exposed during local diagnostics before reuse. Generate new local tokens, rerun `seed-dev-users`, deactivate old identities, and never commit the seed file or bearer tokens. |

All recovery actions must preserve synthetic-only scope. Do not point mutation tests at the existing Neon million-row source.
