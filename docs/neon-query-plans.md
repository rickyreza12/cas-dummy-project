# Neon read-only query plans

Captured 2026-09-30 from the configured read-only PostgreSQL connection. No
credentials, connection URL, or mutation command is recorded here.

## Exact patient ID

Query shape: `patient_id = 'SYN-BUNDA-P0001' LIMIT 1`.

- Plan: `Index Scan using synthetic_patients_pkey`
- Planning time: 0.289 ms
- Execution time: 0.109 ms
- Actual rows: 1

## Prefix patient ID

Query shape: `patient_id LIKE 'SYN-BUNDA-P00%' ORDER BY patient_id LIMIT 25`.

- Plan: bounded `Index Scan using synthetic_patients_pkey`
- Index condition: `patient_id >= 'SYN-BUNDA-P00' AND patient_id < 'SYN-BUNDA-P01'`
- Planning time: 4.351 ms
- Execution time: 1.322 ms
- Actual rows: 25

The normal primary-key index provided bounded prefix lookup for this locale and
query shape, so no `text_pattern_ops` index was added.

## Bounded API sample

Ten sequential authenticated `GET /v1/patients?query=...&limit=1` requests
were run across early, middle, and late synthetic patient IDs on 2026-09-30.
The temporary scoped identity was removed afterward.

- Requests: 10
- Statuses: 10 × `200`
- p50: 110.783 ms
- p95: 126.603 ms
- Minimum: 100.724 ms
- Maximum: 163.017 ms

These are one-session warm-sample observations, not a production SLO or a
complete concurrency benchmark.
