# Architecture decisions

## ADR-001: Keep the pilot backend small

The service keeps application composition in `main.rs`/`http.rs` with explicit boundary modules. The domain/repository/service modules are present as boundaries, but the current pilot has limited business logic and does not introduce a dependency-injection framework.

## ADR-002: Operational schema first

The pilot uses one idempotent SQLx migration for operational tables. Splitting the migration into source, metadata, operational, and index phases remains a deployment task because the existing Neon source must be inspected and capacity-gated before any adoption/index operation.

## ADR-003: CSV fixture contract

The supplied contract artifact is CSV. The service validates the exact eight-column CSV schema with streaming reads, duplicate-key checks, row reconciliation, and SHA-256. XLSX/cal​amine support is not silently substituted or claimed complete without an XLSX contract fixture.

## ADR-004: Development JWT issuance

`POST /v1/auth/token` exists only as a local development signer using configured RS256 private keys. Production identity issuance is outside this dummy backend; protected routes still validate signature, issuer, audience, lifetime, subject, role, and scope claims.

## ADR-005: Deterministic encounter and summary provenance

Encounter IDs derive from the dataset and exact numeric suffix in the patient ID. Summary claims contain only source-row facts and store a deterministic SHA-256 of the cited source-record set.

## ADR-006: Migration layout remains capacity-gated

The checklist's ideal `0000_source_fixture.sql` / `0001_metadata.sql` /
`0002_operational.sql` / `0003_indexes.sql` split is not applied to the
existing Neon source. The current chain keeps the source table untouched and
uses idempotent operational SQL plus a queue-column migration and JWT
revocation migration. This avoids rebuilding or duplicating the million-row
table and avoids transactional `CREATE INDEX CONCURRENTLY` operations.

Local migration and constraint tests run against disposable PostgreSQL. Neon
source inspection remains read-only until capacity approval and a reviewed
adoption migration are available. The task checklist intentionally leaves the
source-fixture and capacity-gated index boxes unchecked.
