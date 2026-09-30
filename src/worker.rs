use anyhow::Result;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use std::collections::HashSet;
use std::path::Path;
use uuid::Uuid;

fn build_claims(
    visit_date: Option<chrono::NaiveDate>,
    specialist: Option<&str>,
    icd10: Option<&str>,
    prescription: Option<&str>,
    source_record_id: &str,
) -> serde_json::Value {
    let mut claims = Vec::new();
    if let Some(date) = visit_date {
        claims.push(serde_json::json!({
            "category": "visit",
            "text": format!("Recorded visit on {date} with {}", specialist.unwrap_or("an unspecified specialist")),
            "as_of_date": date,
            "source_record_ids": [source_record_id],
            "review_state": "unreviewed"
        }));
    }
    if let Some(code) = icd10 {
        claims.push(serde_json::json!({
            "category": "diagnosis",
            "text": format!("Recorded ICD-10 code: {code}"),
            "as_of_date": visit_date,
            "source_record_ids": [source_record_id],
            "review_state": "unreviewed"
        }));
    }
    if let Some(drug) = prescription.filter(|value| {
        let normalized = value.trim();
        !normalized.is_empty() && !normalized.eq_ignore_ascii_case("none recorded")
    }) {
        claims.push(serde_json::json!({
            "category": "medication",
            "text": format!("Prescribed on visit date: {drug}; current use unknown"),
            "as_of_date": visit_date,
            "source_record_ids": [source_record_id],
            "review_state": "unreviewed"
        }));
    }
    serde_json::Value::Array(claims)
}

fn validate_claim_sources(
    claims: &serde_json::Value,
    allowed_source_ids: &HashSet<&str>,
) -> Result<(), &'static str> {
    let Some(claims) = claims.as_array() else {
        return Err("claims must be an array");
    };
    for claim in claims {
        let Some(source_ids) = claim["source_record_ids"].as_array() else {
            return Err("claim is missing source_record_ids");
        };
        if source_ids.is_empty()
            || source_ids.iter().any(|source_id| {
                source_id.as_str().is_none_or(|source_id| {
                    !allowed_source_ids.contains(source_id) || !is_valid_source_id(source_id)
                })
            })
        {
            return Err("claim cites an unauthorized source");
        }
    }
    Ok(())
}

fn is_valid_source_id(source_id: &str) -> bool {
    let Some(body) = source_id
        .strip_prefix("SYN-BUNDA-V")
        .and_then(|value| value.strip_suffix("-SRC"))
    else {
        return false;
    };
    !body.is_empty()
        && body
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'-')
}

pub async fn run(
    pool: PgPool,
    fixture_dir: String,
    concurrency: u32,
    lease_seconds: u64,
    max_attempts: i32,
) -> Result<()> {
    loop {
        let mut workers = tokio::task::JoinSet::new();
        for _ in 0..concurrency {
            let worker_pool = pool.clone();
            workers.spawn(async move {
                process_one_summary_job(&worker_pool, lease_seconds, max_attempts).await
            });
        }
        while let Some(result) = workers.join_next().await {
            result??;
        }
        process_one_import_job(&pool, Path::new(&fixture_dir)).await?;
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
}

pub async fn process_one_import_job(pool: &PgPool, fixture_dir: &Path) -> Result<bool> {
    let mut tx = pool.begin().await?;
    let worker_id = format!("import-worker-{}", Uuid::new_v4());
    let job = sqlx::query("SELECT import_job_id, fixture_reference, expected_rows FROM import_jobs WHERE (status = 'queued' OR (status = 'running' AND lease_expires_at < now())) ORDER BY created_at FOR UPDATE SKIP LOCKED LIMIT 1").fetch_optional(&mut *tx).await?;
    let Some(job) = job else {
        tx.commit().await?;
        return Ok(false);
    };
    let job_id: Uuid = job.get("import_job_id");
    let reference: String = job.get("fixture_reference");
    let expected: i64 = job.get("expected_rows");
    sqlx::query("UPDATE import_jobs SET status='running', lease_owner=$2, lease_expires_at=now() + interval '5 minutes' WHERE import_job_id=$1")
        .bind(job_id).bind(&worker_id).execute(&mut *tx).await?;
    let Some(spec) = crate::fixture::allowlisted(&reference, expected as u64) else {
        sqlx::query("UPDATE import_jobs SET status='failed', failure_code='invalid_fixture_reference' WHERE import_job_id=$1").bind(job_id).execute(&mut *tx).await?;
        tx.commit().await?;
        return Ok(true);
    };
    let path = match crate::fixture::resolve_fixture(&reference, expected as u64, fixture_dir) {
        Ok((_, path)) => path,
        Err(_) => {
            sqlx::query("UPDATE import_jobs SET status='failed', failure_code='fixture_not_found' WHERE import_job_id=$1").bind(job_id).execute(&mut *tx).await?;
            tx.commit().await?;
            return Ok(true);
        }
    };
    let (imported, rejected, checksum) = match crate::fixture::validate_file(&path, expected as u64)
    {
        Ok(result) => result,
        Err(_) => {
            sqlx::query("UPDATE import_jobs SET status='failed', failure_code='fixture_validation_failed' WHERE import_job_id=$1").bind(job_id).execute(&mut *tx).await?;
            tx.commit().await?;
            return Ok(true);
        }
    };
    if imported != expected as u64 || rejected != 0 {
        sqlx::query("UPDATE import_jobs SET status='failed', imported_rows=$2, rejected_rows=$3, failure_code='row_reconciliation_failed' WHERE import_job_id=$1").bind(job_id).bind(imported as i64).bind(rejected as i64).execute(&mut *tx).await?;
        tx.commit().await?;
        return Ok(true);
    }
    let active_dataset = sqlx::query_scalar::<_, String>(
        "SELECT dataset_id FROM dataset_registry WHERE status='active' LIMIT 1 FOR UPDATE",
    )
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(active_dataset) = active_dataset
        && active_dataset != spec.reference
    {
        sqlx::query("UPDATE import_jobs SET status='failed', imported_rows=$2, rejected_rows=$3, failure_code='active_dataset_conflict' WHERE import_job_id=$1")
                .bind(job_id).bind(imported as i64).bind(rejected as i64).execute(&mut *tx).await?;
        tx.commit().await?;
        return Ok(true);
    }
    let mut reader = csv::Reader::from_path(&path)?;
    sqlx::query("UPDATE import_jobs SET status='running', imported_rows=$2, rejected_rows=0, checksum_sha256=$3, schema_version='doctor-360-v1' WHERE import_job_id=$1").bind(job_id).bind(imported as i64).bind(&checksum).execute(&mut *tx).await?;
    for (index, record) in reader.records().enumerate() {
        let row = record?;
        sqlx::query("INSERT INTO import_staging (import_job_id,row_number,patient_id,age_years,sex,visit_date,specialist,icd10_code,prescription_recorded,source_record_id) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)")
            .bind(job_id).bind((index + 2) as i64).bind(row.get(0).unwrap_or_default()).bind(row.get(1).unwrap_or_default().parse::<i32>()?).bind(row.get(2).unwrap_or_default()).bind(chrono::NaiveDate::parse_from_str(row.get(3).unwrap_or_default(), "%Y-%m-%d")?).bind(row.get(4).unwrap_or_default()).bind(row.get(5).unwrap_or_default()).bind(row.get(6).unwrap_or_default()).bind(row.get(7).unwrap_or_default()).execute(&mut *tx).await?;
    }
    sqlx::query("INSERT INTO synthetic_patients (patient_id,age_years,sex,visit_date,specialist,icd10_code,prescription_recorded,source_record_id) SELECT patient_id,age_years,sex,visit_date,specialist,icd10_code,prescription_recorded,source_record_id FROM import_staging WHERE import_job_id=$1").bind(job_id).execute(&mut *tx).await?;
    sqlx::query("UPDATE import_jobs SET status='ready', lease_owner=NULL, lease_expires_at=NULL, finished_at=now() WHERE import_job_id=$1")
        .bind(job_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO dataset_registry (dataset_id,status,origin) VALUES ($1,'active','fixture_import') ON CONFLICT (dataset_id) DO NOTHING").bind(spec.reference).execute(&mut *tx).await?;
    let _ = checksum;
    tx.commit().await?;
    Ok(true)
}

pub async fn process_one_summary_job(
    pool: &PgPool,
    lease_seconds: u64,
    max_attempts: i32,
) -> Result<bool> {
    let mut tx = pool.begin().await?;
    let worker_id = format!("worker-{}", Uuid::new_v4());
    let job = sqlx::query("SELECT summary_job_id, dataset_id, patient_id, encounter_id FROM summary_jobs WHERE (status = 'queued' OR (status = 'running' AND lease_expires_at < now())) AND attempt_count < $1 ORDER BY created_at FOR UPDATE SKIP LOCKED LIMIT 1")
        .bind(max_attempts)
        .fetch_optional(&mut *tx).await?;
    let Some(job) = job else {
        tx.commit().await?;
        return Ok(false);
    };
    let job_id: Uuid = job.get("summary_job_id");
    let dataset_id: String = job.get("dataset_id");
    let patient_id: String = job.get("patient_id");
    let encounter_id: String = job.get("encounter_id");
    sqlx::query("UPDATE summary_jobs SET status = 'running', attempt_count = attempt_count + 1, lease_owner = $2, lease_expires_at = now() + ($3 * interval '1 second') WHERE summary_job_id = $1")
        .bind(job_id)
        .bind(&worker_id)
        .bind(lease_seconds as i64)
        .execute(&mut *tx)
        .await?;
    let source = sqlx::query("SELECT visit_date, specialist, icd10_code, prescription_recorded, source_record_id FROM synthetic_patients WHERE patient_id = $1 ORDER BY visit_date DESC NULLS LAST LIMIT 1").bind(&patient_id).fetch_optional(&mut *tx).await?;
    let Some(source) = source else {
        sqlx::query("UPDATE summary_jobs SET status = 'failed', failure_code = 'patient_not_found', lease_owner = NULL, lease_expires_at = NULL WHERE summary_job_id = $1").bind(job_id).execute(&mut *tx).await?;
        tx.commit().await?;
        return Ok(true);
    };
    let visit_date = source.try_get::<Option<chrono::NaiveDate>, _>("visit_date")?;
    let specialist = source.try_get::<Option<String>, _>("specialist")?;
    let icd10 = source.try_get::<Option<String>, _>("icd10_code")?;
    let prescription = source.try_get::<Option<String>, _>("prescription_recorded")?;
    let source_record_id: String = source.try_get("source_record_id")?;
    let claims = build_claims(
        visit_date,
        specialist.as_deref(),
        icd10.as_deref(),
        prescription.as_deref(),
        &source_record_id,
    );
    let allowed_source_ids = HashSet::from([source_record_id.as_str()]);
    if validate_claim_sources(&claims, &allowed_source_ids).is_err() {
        sqlx::query("UPDATE summary_jobs SET status = 'failed', failure_code = 'citation_invalid', lease_owner = NULL, lease_expires_at = NULL WHERE summary_job_id = $1")
            .bind(job_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        return Ok(true);
    }
    let mut source_hash = Sha256::new();
    source_hash.update(source_record_id.as_bytes());
    let source_set_sha256 = format!("{:x}", source_hash.finalize());
    let summary_id = Uuid::new_v4();
    // A worker may be retried after committing the artifact but before it can
    // acknowledge the job. The job's unique key makes the artifact write
    // idempotent, so a retry never creates a second summary.
    sqlx::query("INSERT INTO summaries (summary_id, summary_job_id, dataset_id, patient_id, encounter_id, generator_version, source_set, source_set_sha256, claims) VALUES ($1, $2, $3, $4, $5, 'deterministic-v1', $6, $7, $8) ON CONFLICT (summary_job_id) DO NOTHING")
        .bind(summary_id).bind(job_id).bind(dataset_id).bind(&patient_id).bind(&encounter_id)
        .bind(serde_json::json!([&source_record_id])).bind(source_set_sha256).bind(claims).execute(&mut *tx).await?;
    sqlx::query("UPDATE summary_jobs SET status = 'ready', lease_owner = NULL, lease_expires_at = NULL WHERE summary_job_id = $1")
        .bind(job_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::{build_claims, validate_claim_sources};
    use std::collections::HashSet;

    #[test]
    fn none_recorded_does_not_create_medication_claim() {
        let claims = build_claims(
            Some(chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
            Some("Cardiology"),
            Some("A01"),
            Some("None recorded"),
            "SYN-BUNDA-V0001-SRC",
        );
        assert_eq!(claims.as_array().unwrap().len(), 2);
        assert!(
            claims
                .as_array()
                .unwrap()
                .iter()
                .all(|claim| claim["category"] != "medication")
        );
    }

    #[test]
    fn every_generated_claim_carries_source_provenance() {
        let claims = build_claims(
            Some(chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
            None,
            Some("A01"),
            Some("Paracetamol"),
            "SYN-BUNDA-V0001-SRC",
        );
        for claim in claims.as_array().unwrap() {
            assert_eq!(claim["source_record_ids"][0], "SYN-BUNDA-V0001-SRC");
        }
        assert!(validate_claim_sources(&claims, &HashSet::from(["SYN-BUNDA-V0001-SRC"])).is_ok());
    }

    #[test]
    fn foreign_or_missing_citations_are_rejected() {
        let foreign = serde_json::json!([{
            "category": "diagnosis",
            "source_record_ids": ["SRC-foreign"]
        }]);
        let missing = serde_json::json!([{
            "category": "diagnosis",
            "source_record_ids": []
        }]);
        let allowed = HashSet::from(["SYN-BUNDA-V0001-SRC"]);
        assert!(validate_claim_sources(&foreign, &allowed).is_err());
        assert!(validate_claim_sources(&missing, &allowed).is_err());

        let malformed = serde_json::json!([{
            "category": "diagnosis",
            "source_record_ids": ["not-a-source-id"]
        }]);
        let malformed_allowed = HashSet::from(["not-a-source-id"]);
        assert!(validate_claim_sources(&malformed, &malformed_allowed).is_err());
    }
}
