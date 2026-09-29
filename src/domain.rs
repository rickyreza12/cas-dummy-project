#![allow(dead_code)]
//! Validated domain identifiers kept separate from transport strings.

use std::{fmt, str::FromStr};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainId(String);

impl DomainId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for DomainId {
    type Err = &'static str;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if !(1..=128).contains(&value.len())
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte))
        {
            return Err("identifier contains invalid characters or length");
        }
        Ok(Self(value.to_owned()))
    }
}

macro_rules! domain_id {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub struct $name(DomainId);
        impl $name {
            pub fn as_str(&self) -> &str {
                self.0.as_str()
            }
        }
        impl FromStr for $name {
            type Err = &'static str;
            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Ok(Self(value.parse()?))
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(self.as_str())
            }
        }
    };
}

domain_id!(DatasetId);
domain_id!(PatientId);
domain_id!(EncounterId);
domain_id!(SourceRecordId);
domain_id!(PrincipalId);
domain_id!(JobId);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncounterKey {
    pub dataset_id: String,
    pub patient_id: String,
}

pub fn encounter_patient_suffix(value: &str) -> Result<&str, &'static str> {
    let suffix = value
        .strip_prefix("enc-")
        .and_then(|rest| rest.rsplit_once('-').map(|(_, suffix)| suffix))
        .ok_or("malformed encounter identifier")?;
    if suffix.is_empty() || !suffix.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("encounter suffix must be numeric");
    }
    Ok(suffix)
}

pub fn derive_encounter_id(dataset_id: &str, patient_id: &str) -> Result<String, &'static str> {
    if !matches!(
        dataset_id,
        "demo-100-v1" | "demo-1000-v1" | "demo-1000000-v1"
    ) {
        return Err("encounter dataset is invalid");
    }
    let suffix = patient_id
        .strip_prefix("SYN-BUNDA-P")
        .filter(|suffix| !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit()))
        .ok_or("patient identifier cannot derive an encounter")?;
    Ok(format!("enc-{dataset_id}-{suffix}"))
}

pub fn encounter_dataset(value: &str) -> Result<&str, &'static str> {
    let rest = value
        .strip_prefix("enc-")
        .ok_or("malformed encounter identifier")?;
    let (dataset, suffix) = rest
        .rsplit_once('-')
        .ok_or("malformed encounter identifier")?;
    if !matches!(dataset, "demo-100-v1" | "demo-1000-v1" | "demo-1000000-v1")
        || suffix.is_empty()
        || !suffix.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err("encounter dataset or suffix is invalid");
    }
    Ok(dataset)
}

pub fn parse_encounter_id(value: &str) -> Result<EncounterKey, &'static str> {
    let dataset_id = encounter_dataset(value)?;
    let suffix = encounter_patient_suffix(value)?;
    Ok(EncounterKey {
        dataset_id: dataset_id.to_owned(),
        patient_id: format!("SYN-BUNDA-P{suffix}"),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordedPrescription {
    Historical(String),
    NoneRecorded,
}

impl RecordedPrescription {
    pub fn from_source(value: &str) -> Self {
        if value.trim().eq_ignore_ascii_case("none recorded") {
            Self::NoneRecorded
        } else {
            Self::Historical(value.to_owned())
        }
    }

    pub fn current_use(&self) -> &'static str {
        "unknown"
    }

    pub fn to_response(&self, source_record_id: &str) -> Option<serde_json::Value> {
        match self {
            Self::Historical(name) => Some(serde_json::json!({
                "name": name,
                "historical": true,
                "current_use": self.current_use(),
                "source_record_id": source_record_id,
            })),
            Self::NoneRecorded => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn domain_ids_round_trip() {
        let patient: PatientId = "SYN-BUNDA-P0001".parse().unwrap();
        assert_eq!(patient.as_str(), "SYN-BUNDA-P0001");
        let job: JobId = "job_01".parse().unwrap();
        assert_eq!(job.to_string(), "job_01");
    }
    #[test]
    fn domain_ids_reject_untrusted_values() {
        assert!("patient/../../secret".parse::<PatientId>().is_err());
        assert!("".parse::<DatasetId>().is_err());
        assert!("x".repeat(129).parse::<JobId>().is_err());
    }

    #[test]
    fn none_recorded_is_not_a_current_prescription() {
        assert_eq!(
            RecordedPrescription::from_source("None recorded"),
            RecordedPrescription::NoneRecorded
        );
        assert_eq!(
            RecordedPrescription::from_source("Paracetamol").current_use(),
            "unknown"
        );
        assert!(
            RecordedPrescription::from_source("None recorded")
                .to_response("SYN-BUNDA-V0001-SRC")
                .is_none()
        );
        let response = RecordedPrescription::from_source("Paracetamol")
            .to_response("SYN-BUNDA-V0001-SRC")
            .unwrap();
        assert_eq!(response["source_record_id"], "SYN-BUNDA-V0001-SRC");
        assert_eq!(response["current_use"], "unknown");
    }

    #[test]
    fn encounter_identifier_requires_numeric_suffix() {
        assert_eq!(
            encounter_patient_suffix("enc-demo-100-v1-0001").unwrap(),
            "0001"
        );
        assert_eq!(
            encounter_dataset("enc-demo-100-v1-0001").unwrap(),
            "demo-100-v1"
        );
        assert!(encounter_patient_suffix("enc-demo-100-v1-nope").is_err());
        assert!(encounter_dataset("enc-other-v1-0001").is_err());
        assert!(encounter_patient_suffix("wrong-0001").is_err());
    }

    #[test]
    fn encounter_derivation_preserves_fixture_suffixes_at_all_scales() {
        for (dataset, patient, expected) in [
            ("demo-100-v1", "SYN-BUNDA-P0001", "enc-demo-100-v1-0001"),
            ("demo-100-v1", "SYN-BUNDA-P0100", "enc-demo-100-v1-0100"),
            ("demo-1000-v1", "SYN-BUNDA-P0001", "enc-demo-1000-v1-0001"),
            ("demo-1000-v1", "SYN-BUNDA-P1000", "enc-demo-1000-v1-1000"),
            (
                "demo-1000000-v1",
                "SYN-BUNDA-P0000001",
                "enc-demo-1000000-v1-0000001",
            ),
            (
                "demo-1000000-v1",
                "SYN-BUNDA-P1000000",
                "enc-demo-1000000-v1-1000000",
            ),
        ] {
            assert_eq!(derive_encounter_id(dataset, patient).unwrap(), expected);
        }
        assert!(derive_encounter_id("other", "SYN-BUNDA-P0001").is_err());
        assert!(derive_encounter_id("demo-100-v1", "not-a-patient").is_err());
    }

    #[test]
    fn encounter_parser_round_trips_dataset_and_zero_padded_patient_id() {
        let encounter_id = derive_encounter_id("demo-1000000-v1", "SYN-BUNDA-P0000101").unwrap();
        assert_eq!(
            parse_encounter_id(&encounter_id).unwrap(),
            EncounterKey {
                dataset_id: "demo-1000000-v1".to_owned(),
                patient_id: "SYN-BUNDA-P0000101".to_owned(),
            }
        );
        assert!(parse_encounter_id("enc-demo-100-v1-not-a-number").is_err());
    }
}
