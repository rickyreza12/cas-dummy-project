#![allow(dead_code)]

use anyhow::{Result, bail};
use csv::StringRecord;
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::File,
    path::{Path, PathBuf},
};

pub const HEADER: [&str; 8] = [
    "patient_id",
    "age_years",
    "sex",
    "visit_date",
    "specialist",
    "icd10_code",
    "prescription_recorded",
    "source_record_id",
];

#[derive(Debug, Clone)]
pub struct FixtureSpec {
    pub reference: &'static str,
    pub filename: &'static str,
    pub expected_rows: u64,
}

pub fn allowlisted(reference: &str, expected_rows: u64) -> Option<FixtureSpec> {
    match (reference, expected_rows) {
        ("demo-100-v1", 100) => Some(FixtureSpec {
            reference: "demo-100-v1",
            filename: "Doctor-360-Scale-100-Patients.csv",
            expected_rows: 100,
        }),
        ("demo-1000-v1", 1000) => Some(FixtureSpec {
            reference: "demo-1000-v1",
            filename: "Doctor-360-Scale-1000-Patients.csv",
            expected_rows: 1000,
        }),
        ("demo-1000000-v1", 1_000_000) => Some(FixtureSpec {
            reference: "demo-1000000-v1",
            filename: "Doctor-360-Scale-1000000-Patients.csv",
            expected_rows: 1_000_000,
        }),
        _ => None,
    }
}

pub fn resolve_fixture(
    reference: &str,
    expected_rows: u64,
    fixture_dir: &Path,
) -> Result<(FixtureSpec, PathBuf)> {
    let spec = allowlisted(reference, expected_rows).ok_or_else(|| {
        anyhow::anyhow!("fixture reference and expected row count are not allowlisted")
    })?;
    let root = fixture_dir.canonicalize()?;
    let path = root.join(spec.filename).canonicalize()?;
    if !path.starts_with(&root) {
        bail!("fixture path escapes allowlisted directory");
    }
    Ok((spec, path))
}

pub fn validate_csv(path: &Path, expected_rows: u64) -> Result<(u64, u64, String)> {
    let file = File::open(path)?;
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .from_reader(file);
    let headers = reader.headers()?.clone();
    let expected = StringRecord::from(HEADER.to_vec());
    if headers != expected {
        bail!("fixture header does not match the required eight-column schema");
    }
    let mut patients = HashSet::new();
    let mut sources = HashSet::new();
    let mut imported = 0_u64;
    let mut rejected = 0_u64;
    for (index, record) in reader.records().enumerate() {
        let record = record?;
        if validate_row(&record, &mut patients, &mut sources).is_ok() {
            imported += 1;
        } else {
            rejected += 1;
        }
        let _row_number = index + 2;
    }
    if imported + rejected != expected_rows {
        bail!("fixture row count does not match expected_rows");
    }
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    std::io::copy(&mut file, &mut hash)?;
    Ok((imported, rejected, format!("{:x}", hash.finalize())))
}

fn validate_row(
    row: &StringRecord,
    patients: &mut HashSet<String>,
    sources: &mut HashSet<String>,
) -> Result<()> {
    if row.len() != 8 {
        bail!("wrong column count");
    }
    let patient = row.get(0).unwrap_or_default();
    let age = row
        .get(1)
        .unwrap_or_default()
        .parse::<i32>()
        .map_err(|_| anyhow::anyhow!("invalid age"))?;
    if !(0..=120).contains(&age)
        || patient.is_empty()
        || row.get(7).unwrap_or_default().is_empty()
        || !patient
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
    {
        bail!("invalid required field");
    }
    if chrono::NaiveDate::parse_from_str(row.get(3).unwrap_or_default(), "%Y-%m-%d").is_err() {
        bail!("invalid date");
    }
    let icd = row.get(5).unwrap_or_default();
    let valid_icd = icd.len() >= 3
        && icd.as_bytes()[0].is_ascii_uppercase()
        && icd.as_bytes()[1..3].iter().all(u8::is_ascii_digit)
        && icd.strip_prefix(&icd[..3]).is_some_and(|suffix| {
            suffix.is_empty()
                || (suffix.starts_with('.')
                    && (1..=4).contains(&suffix.len().saturating_sub(1))
                    && suffix[1..]
                        .bytes()
                        .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit()))
        });
    if !valid_icd
        || !patients.insert(patient.to_owned())
        || !sources.insert(row.get(7).unwrap_or_default().to_owned())
    {
        bail!("invalid or duplicate key");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[test]
    fn allowlist_rejects_path_input() {
        assert!(allowlisted("../secret", 100).is_none());
    }
    #[test]
    fn row_validation_rejects_duplicate_keys() {
        let mut p = HashSet::new();
        let mut s = HashSet::new();
        let row = StringRecord::from(vec![
            "p",
            "20",
            "F",
            "2026-01-01",
            "spec",
            "A01",
            "None recorded",
            "s",
        ]);
        assert!(validate_row(&row, &mut p, &mut s).is_ok());
        assert!(validate_row(&row, &mut p, &mut s).is_err());
    }

    #[test]
    fn csv_validation_rejects_bad_header() {
        let path = std::env::temp_dir().join(format!("cas-fixture-{}.csv", uuid::Uuid::new_v4()));
        fs::write(&path, "wrong,header\np,20\n").unwrap();
        assert!(validate_csv(&path, 1).is_err());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn csv_validation_accepts_valid_row_and_checksum() {
        let path = std::env::temp_dir().join(format!("cas-fixture-{}.csv", uuid::Uuid::new_v4()));
        fs::write(&path, format!("{}\nSYN-BUNDA-P0001,20,F,2026-01-01,Cardiology,A01,None recorded,SYN-BUNDA-V0001-SRC\n", HEADER.join(","))).unwrap();
        let result = validate_csv(&path, 1).unwrap();
        assert_eq!(result.0, 1);
        assert_eq!(result.1, 0);
        assert_eq!(result.2.len(), 64);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn row_validation_rejects_invalid_id_age_date_and_icd10() {
        for (patient, age, date, icd) in [
            ("bad/id", "20", "2026-01-01", "A01"),
            ("SYN-BUNDA-P0001", "121", "2026-01-01", "A01"),
            ("SYN-BUNDA-P0001", "20", "not-a-date", "A01"),
            ("SYN-BUNDA-P0001", "20", "2026-01-01", "A0"),
            ("SYN-BUNDA-P0001", "20", "2026-01-01", "A01.bad"),
        ] {
            let mut patients = HashSet::new();
            let mut sources = HashSet::new();
            let row = StringRecord::from(vec![
                patient,
                age,
                "F",
                date,
                "Cardiology",
                icd,
                "None recorded",
                "SYN-BUNDA-V0001-SRC",
            ]);
            assert!(validate_row(&row, &mut patients, &mut sources).is_err());
        }
    }

    #[test]
    fn scale_fixtures_preserve_first_middle_last_ids_and_iso_dates() {
        for (filename, expected_rows) in [
            ("Doctor-360-Scale-100-Patients.csv", 100_usize),
            ("Doctor-360-Scale-1000-Patients.csv", 1_000_usize),
            ("Doctor-360-Scale-1000000-Patients.csv", 1_000_000_usize),
        ] {
            let path = Path::new("fixtures").join(filename);
            let mut reader = csv::Reader::from_path(path).unwrap();
            let rows: Vec<StringRecord> = reader.records().map(|record| record.unwrap()).collect();
            assert_eq!(rows.len(), expected_rows);
            for index in [0, expected_rows / 2, expected_rows - 1] {
                let row = &rows[index];
                assert!(row.get(0).unwrap().starts_with("SYN-BUNDA-P"));
                assert!(chrono::NaiveDate::parse_from_str(row.get(3).unwrap(), "%Y-%m-%d").is_ok());
                assert!((0..=120).contains(&row.get(1).unwrap().parse::<i32>().unwrap()));
            }
        }
    }

    #[test]
    fn first_row_context_fixture_preserves_age_and_explicit_data_gaps() {
        let context: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/first-row-context.json")).unwrap();
        assert_eq!(context["patient_id"], "SYN-BUNDA-P0001");
        assert_eq!(context["age_years"], 40);
        assert_eq!(context["encounters"][0]["visit_date"], "2024-05-29");
        assert!(context.get("birth_date").is_none());
        assert_eq!(context["data_gaps"].as_array().unwrap().len(), 3);
        assert!(
            context["data_gaps"]
                .as_array()
                .unwrap()
                .iter()
                .all(|gap| gap.as_str().is_some())
        );
    }
}
