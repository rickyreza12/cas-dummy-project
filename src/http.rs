use axum::{
    Json,
    extract::{
        Path, Query, State,
        rejection::{JsonRejection, QueryRejection},
    },
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use std::sync::Arc;

use crate::{auth, config::Config, error::problem};

#[derive(Clone)]
pub struct AppState {
    pub config: Config,
    pub pool: PgPool,
    pub token_verifier: auth::TokenVerifier,
    pub clock: Arc<dyn Clock>,
    pub summary_generator: Arc<dyn SummaryGenerator>,
}

pub trait Clock: Send + Sync {
    fn now(&self) -> chrono::DateTime<chrono::Utc>;
}

#[derive(Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> chrono::DateTime<chrono::Utc> {
        chrono::Utc::now()
    }
}

pub trait SummaryGenerator: Send + Sync {
    fn generator_version(&self) -> &'static str;
}

#[derive(Default)]
pub struct DeterministicSummaryGenerator;

impl SummaryGenerator for DeterministicSummaryGenerator {
    fn generator_version(&self) -> &'static str {
        "deterministic-v1"
    }
}

impl AppState {
    pub fn new(config: Config, pool: PgPool) -> Self {
        Self {
            config,
            pool,
            token_verifier: auth::TokenVerifier,
            clock: Arc::new(SystemClock),
            summary_generator: Arc::new(DeterministicSummaryGenerator),
        }
    }
}

#[derive(Deserialize)]
pub struct PageQuery {
    pub limit: Option<String>,
    pub cursor: Option<String>,
    pub query: Option<String>,
}

#[allow(clippy::result_large_err)]
pub(crate) fn parse_limit(query: &PageQuery) -> Result<u32, Response> {
    let limit = query
        .limit
        .as_deref()
        .map(str::parse::<u32>)
        .transpose()
        .map_err(|_| {
            problem(
                StatusCode::UNPROCESSABLE_ENTITY,
                "Invalid request",
                "limit must be an integer between 1 and 100",
            )
        })?
        .unwrap_or(20);
    if !(1..=100).contains(&limit) {
        return Err(problem(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Invalid request",
            "limit must be between 1 and 100",
        ));
    }
    Ok(limit)
}

#[allow(clippy::result_large_err)]
fn parse_json<T>(payload: Result<Json<T>, JsonRejection>) -> Result<T, Response> {
    payload.map(|Json(value)| value).map_err(|_| {
        problem(
            StatusCode::BAD_REQUEST,
            "Invalid request",
            "request body must be valid JSON",
        )
    })
}

#[allow(clippy::result_large_err)]
fn parse_uuid_path(value: String) -> Result<uuid::Uuid, Response> {
    value.parse().map_err(|_| {
        problem(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Invalid request",
            "resource identifier is not a valid UUID",
        )
    })
}

#[allow(clippy::result_large_err)]
fn parse_query<T>(payload: Result<Query<T>, QueryRejection>) -> Result<T, Response> {
    payload.map(|Query(value)| value).map_err(|_| {
        problem(
            StatusCode::BAD_REQUEST,
            "Invalid request",
            "query parameters are malformed",
        )
    })
}

#[derive(Deserialize)]
pub struct ContextQuery {
    pub encounter_id: Option<String>,
}

#[derive(Serialize)]
pub struct PatientList {
    pub items: Vec<Patient>,
    pub next_cursor: Option<String>,
}

#[derive(Serialize)]
pub struct Patient {
    pub patient_id: String,
    pub age_years: Option<i32>,
    pub sex: Option<String>,
}

#[derive(Deserialize)]
pub struct SummaryRequest {
    pub reason: String,
}

#[derive(Serialize, sqlx::FromRow)]
pub struct SummaryJob {
    pub summary_job_id: uuid::Uuid,
    pub status: String,
    pub failure_code: Option<String>,
}

#[derive(Deserialize)]
pub struct FeedbackRequest {
    pub rating: String,
    pub reason: String,
}

#[derive(Deserialize)]
pub struct ImportRequest {
    pub fixture_reference: String,
    pub expected_rows: i64,
}

#[derive(Serialize, sqlx::FromRow)]
pub struct ImportJob {
    pub import_job_id: uuid::Uuid,
    pub fixture_reference: String,
    pub expected_rows: i64,
    pub imported_rows: i64,
    pub rejected_rows: i64,
    pub status: String,
    pub failure_code: Option<String>,
}

#[derive(Clone, Deserialize)]
pub struct QueueRequest {
    pub patient_id: String,
    pub encounter_id: String,
    pub doctor_id: String,
}

fn validate_queue_request(
    request: &QueueRequest,
    active_dataset_id: &str,
) -> Result<crate::domain::EncounterKey, &'static str> {
    request
        .patient_id
        .parse::<crate::domain::PatientId>()
        .map_err(|_| "patient_id is invalid")?;
    request
        .doctor_id
        .parse::<crate::domain::PrincipalId>()
        .map_err(|_| "doctor_id is invalid")?;
    request
        .encounter_id
        .parse::<crate::domain::EncounterId>()
        .map_err(|_| "encounter_id is invalid")?;
    let encounter = crate::domain::parse_encounter_id(&request.encounter_id)
        .map_err(|_| "encounter_id is invalid")?;
    if encounter.dataset_id != active_dataset_id || encounter.patient_id != request.patient_id {
        return Err("encounter_id does not belong to patient_id");
    }
    Ok(encounter)
}

#[derive(Serialize, sqlx::FromRow)]
pub struct QueueItem {
    pub queue_item_id: uuid::Uuid,
    pub dataset_id: String,
    pub patient_id: String,
    pub encounter_id: String,
    pub doctor_id: String,
    pub assigned_by: Option<String>,
    pub status: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(sqlx::FromRow)]
struct PatientRow {
    patient_id: String,
    age_years: Option<i32>,
    sex: Option<String>,
}

fn sign_cursor(last_id: &str, query: &str, dataset: &str, scope: &str, config: &Config) -> String {
    let payload = serde_json::json!({"v":1,"query":query,"dataset":dataset,"scope":scope,"last_id":last_id,"exp":(chrono::Utc::now() + chrono::Duration::minutes(15)).timestamp()});
    let encoded =
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).expect("cursor payload serializes"));
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(config.cursor_signing_key.as_bytes())
        .expect("cursor key is non-empty");
    mac.update(encoded.as_bytes());
    format!(
        "{encoded}.{}",
        URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
    )
}

fn decode_cursor(
    cursor: &str,
    query: &str,
    dataset: &str,
    scope: &str,
    config: &Config,
) -> Result<String, ()> {
    let (payload, signature) = cursor.split_once('.').ok_or(())?;
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(config.cursor_signing_key.as_bytes())
        .map_err(|_| ())?;
    mac.update(payload.as_bytes());
    let expected = URL_SAFE_NO_PAD.decode(signature).map_err(|_| ())?;
    mac.verify_slice(&expected).map_err(|_| ())?;
    let value: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).map_err(|_| ())?)
        .map_err(|_| ())?;
    if value.get("v").and_then(Value::as_i64) != Some(1)
        || value.get("query").and_then(Value::as_str) != Some(query)
        || value.get("dataset").and_then(Value::as_str) != Some(dataset)
        || value.get("scope").and_then(Value::as_str) != Some(scope)
        || value.get("exp").and_then(Value::as_i64).unwrap_or_default()
            < chrono::Utc::now().timestamp()
    {
        return Err(());
    }
    value
        .get("last_id")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or(())
}

#[allow(clippy::result_large_err)]
fn authorized(headers: &HeaderMap, state: &AppState) -> Result<auth::Claims, Response> {
    let _ = (
        &state.token_verifier,
        state.clock.now(),
        state.summary_generator.generator_version(),
    );
    auth::validate(headers, &state.config)
        .map_err(|status| problem(status, "Authentication failed", "A valid JWT is required"))
}

async fn audit_event(
    state: &AppState,
    headers: &HeaderMap,
    principal_id: &str,
    action: &str,
    target_type: &str,
    target_id: &str,
    outcome: &str,
) {
    let request_id = headers
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("generated");
    let _ = sqlx::query("INSERT INTO audit_events (audit_event_id, request_id, principal_id, action, target_type, target_id, outcome) VALUES ($1,$2,$3,$4,$5,$6,$7)")
        .bind(uuid::Uuid::new_v4())
        .bind(request_id)
        .bind(principal_id)
        .bind(action)
        .bind(target_type)
        .bind(target_id)
        .bind(outcome)
        .execute(&state.pool)
        .await;
}

pub async fn list_patients(
    State(state): State<AppState>,
    headers: HeaderMap,
    query_payload: Result<Query<PageQuery>, QueryRejection>,
) -> Response {
    let claims = match authorized(&headers, &state) {
        Ok(claims) => claims,
        Err(response) => return response,
    };
    let query = match parse_query(query_payload) {
        Ok(query) => query,
        Err(response) => return response,
    };
    if claims.role == "admin" {
        audit_event(
            &state,
            &headers,
            &claims.sub,
            "patient.search",
            "query",
            "redacted",
            "denied",
        )
        .await;
        return problem(
            StatusCode::FORBIDDEN,
            "Forbidden",
            "Admin tokens do not grant clinical read access",
        );
    }
    let limit = match parse_limit(&query) {
        Ok(limit) => limit,
        Err(response) => return response,
    };
    let search = query.query.unwrap_or_default().to_ascii_uppercase();
    if !(2..=64).contains(&search.len())
        || !search
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return problem(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Invalid request",
            "query must contain only ASCII letters, digits, and hyphens",
        );
    }
    let dataset = state.config.active_dataset_id.as_str();
    let scope = format!(
        "{}:{}:{}",
        claims.role,
        claims.sub,
        claims.scope.first().cloned().unwrap_or_default()
    );
    let after = match query.cursor.as_deref() {
        Some(cursor) => match decode_cursor(cursor, &search, dataset, &scope, &state.config) {
            Ok(value) => value,
            Err(_) => {
                return problem(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "Invalid request",
                    "cursor is invalid or expired",
                );
            }
        },
        None => String::new(),
    };
    let pattern = format!("{search}%");
    let department = claims.scope.first().cloned().unwrap_or_default();
    let rows = sqlx::query_as::<_, PatientRow>("SELECT p.patient_id, p.age_years, p.sex FROM synthetic_patients p WHERE ($1 = '' OR p.patient_id LIKE $2) AND p.patient_id > $3 AND ($5 = 'medical_records' OR ($5 = 'nurse' AND p.specialist = $6) OR ($5 = 'doctor' AND EXISTS (SELECT 1 FROM queue_items q WHERE q.patient_id = p.patient_id AND q.doctor_id = $7 AND q.status = 'active'))) ORDER BY p.patient_id ASC LIMIT $4")
        .bind(&search)
        .bind(pattern)
        .bind(after)
        .bind(i64::from(limit) + 1)
        .bind(&claims.role)
        .bind(&department)
        .bind(&claims.sub)
        .fetch_all(&state.pool)
        .await;
    match rows {
        Ok(mut rows) => {
            let has_more = rows.len() > limit as usize;
            if has_more {
                rows.truncate(limit as usize);
            }
            let next_cursor = rows
                .last()
                .filter(|_| has_more)
                .map(|row| sign_cursor(&row.patient_id, &search, dataset, &scope, &state.config));
            Json(PatientList {
                items: rows
                    .into_iter()
                    .map(|row| Patient {
                        patient_id: row.patient_id,
                        age_years: row.age_years,
                        sex: row.sex,
                    })
                    .collect(),
                next_cursor,
            })
            .into_response()
        }
        Err(_) => problem(
            StatusCode::SERVICE_UNAVAILABLE,
            "Dependency unavailable",
            "patient data is temporarily unavailable",
        ),
    }
}

pub async fn patient_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let claims = match authorized(&headers, &state) {
        Ok(claims) => claims,
        Err(response) => return response,
    };
    if claims.role == "admin" {
        audit_event(
            &state,
            &headers,
            &claims.sub,
            "patient.read",
            "patient",
            &id,
            "denied",
        )
        .await;
        return problem(
            StatusCode::FORBIDDEN,
            "Forbidden",
            "Admin tokens do not grant clinical read access",
        );
    }
    #[derive(sqlx::FromRow, Serialize)]
    struct Detail {
        patient_id: String,
        age_years: Option<i32>,
        sex: Option<String>,
    }
    let department = claims.scope.first().cloned().unwrap_or_default();
    match sqlx::query_as::<_, Detail>(
        "SELECT p.patient_id, p.age_years, p.sex FROM synthetic_patients p WHERE p.patient_id = $1 AND ($2 = 'medical_records' OR ($2 = 'nurse' AND p.specialist = $3) OR ($2 = 'doctor' AND EXISTS (SELECT 1 FROM queue_items q WHERE q.patient_id = p.patient_id AND q.doctor_id = $4 AND q.status = 'active')))",
    )
        .bind(&id)
        .bind(&claims.role)
        .bind(&department)
        .bind(&claims.sub)
    .fetch_optional(&state.pool)
    .await
    {
        Ok(Some(detail)) => {
            audit_event(&state, &headers, &claims.sub, "patient.read", "patient", &detail.patient_id, "allowed").await;
            Json(detail).into_response()
        }
        Ok(None) => {
            audit_event(&state, &headers, &claims.sub, "patient.read", "patient", &id, "hidden_or_missing").await;
            problem(StatusCode::NOT_FOUND, "Not found", "Patient was not found")
        },
        Err(_) => problem(
            StatusCode::SERVICE_UNAVAILABLE,
            "Dependency unavailable",
            "patient data is temporarily unavailable",
        ),
    }
}

pub async fn patient_encounters(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    query_payload: Result<Query<PageQuery>, QueryRejection>,
) -> Response {
    let claims = match authorized(&headers, &state) {
        Ok(claims) => claims,
        Err(response) => return response,
    };
    let query = match parse_query(query_payload) {
        Ok(query) => query,
        Err(response) => return response,
    };
    if claims.role == "admin" {
        audit_event(
            &state,
            &headers,
            &claims.sub,
            "context.read",
            "patient",
            &id,
            "denied",
        )
        .await;
        return problem(
            StatusCode::FORBIDDEN,
            "Forbidden",
            "Admin tokens do not grant clinical read access",
        );
    }
    let limit = match parse_limit(&query) {
        Ok(limit) => limit,
        Err(response) => return response,
    };
    let department = claims.scope.first().cloned().unwrap_or_default();
    let scope = format!("{}:{}:{}", claims.role, claims.sub, department);
    let cursor = match query.cursor.as_deref() {
        Some(value) => match decode_cursor(
            value,
            &id,
            &state.config.active_dataset_id,
            &scope,
            &state.config,
        ) {
            Ok(value) => {
                let (date, encounter) = value.split_once('|').unwrap_or(("", ""));
                let Ok(date) = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d") else {
                    return problem(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        "Invalid request",
                        "cursor is invalid",
                    );
                };
                Some((date, encounter.to_owned()))
            }
            Err(_) => {
                return problem(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "Invalid request",
                    "cursor is invalid",
                );
            }
        },
        None => None,
    };
    let rows = if let Some((date, encounter)) = cursor {
        sqlx::query("SELECT p.visit_date, p.specialist, p.icd10_code, p.source_record_id, ('enc-' || $6 || '-' || substring(p.patient_id from 13)) AS encounter_id FROM synthetic_patients p WHERE p.patient_id=$1 AND (p.visit_date < $2 OR (p.visit_date = $2 AND ('enc-' || $6 || '-' || substring(p.patient_id from 13)) < $3)) AND ($4='medical_records' OR ($4='nurse' AND p.specialist=$5) OR ($4='doctor' AND EXISTS (SELECT 1 FROM queue_items q WHERE q.patient_id=p.patient_id AND q.doctor_id=$7 AND q.status='active'))) ORDER BY p.visit_date DESC NULLS LAST, encounter_id DESC LIMIT $8")
            .bind(&id).bind(date).bind(encounter).bind(&claims.role).bind(department).bind(&state.config.active_dataset_id).bind(&claims.sub).bind(i64::from(limit) + 1)
            .fetch_all(&state.pool).await
    } else {
        sqlx::query("SELECT p.visit_date, p.specialist, p.icd10_code, p.source_record_id, ('enc-' || $6 || '-' || substring(p.patient_id from 13)) AS encounter_id FROM synthetic_patients p WHERE p.patient_id=$1 AND ($3='medical_records' OR ($3='nurse' AND p.specialist=$4) OR ($3='doctor' AND EXISTS (SELECT 1 FROM queue_items q WHERE q.patient_id=p.patient_id AND q.doctor_id=$5 AND q.status='active'))) ORDER BY p.visit_date DESC NULLS LAST, encounter_id DESC LIMIT $2")
            .bind(&id).bind(i64::from(limit) + 1).bind(&claims.role).bind(department).bind(&claims.sub).bind(&state.config.active_dataset_id)
            .fetch_all(&state.pool).await
    };
    match rows {
        Ok(mut rows) => {
            let next_cursor = if rows.len() > limit as usize {
                let row = rows.pop().expect("length checked");
                let date = row.get::<Option<chrono::NaiveDate>, _>("visit_date");
                date.map(|date| {
                    sign_cursor(
                        &format!("{}|{}", date, row.get::<String, _>("encounter_id")),
                        &id,
                        &state.config.active_dataset_id,
                        &scope,
                        &state.config,
                    )
                })
            } else {
                None
            };
            Json(serde_json::json!({"items": rows.iter().map(|row| serde_json::json!({"encounter_id": row.get::<String,_>("encounter_id"), "visit_date": row.get::<Option<chrono::NaiveDate>,_>("visit_date"), "specialist": row.get::<Option<String>,_>("specialist"), "diagnoses": [{"code": row.get::<Option<String>,_>("icd10_code"), "source_record_id": row.get::<String,_>("source_record_id")}]})).collect::<Vec<_>>(), "next_cursor": next_cursor})).into_response()
        }
        Err(_) => problem(
            StatusCode::SERVICE_UNAVAILABLE,
            "Dependency unavailable",
            "patient data is temporarily unavailable",
        ),
    }
}

#[derive(sqlx::FromRow, Serialize)]
pub struct SourceDetail {
    pub patient_id: String,
    pub age_years: Option<i32>,
    pub sex: Option<String>,
    pub visit_date: Option<chrono::NaiveDate>,
    pub specialist: Option<String>,
    pub icd10_code: Option<String>,
    pub prescription_recorded: Option<String>,
    pub source_record_id: String,
}

pub async fn source_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let claims = match authorized(&headers, &state) {
        Ok(claims) => claims,
        Err(response) => return response,
    };
    let department = claims.scope.first().cloned().unwrap_or_default();
    match sqlx::query_as::<_, SourceDetail>("SELECT p.patient_id, p.age_years, p.sex, p.visit_date, p.specialist, p.icd10_code, p.prescription_recorded, p.source_record_id FROM synthetic_patients p WHERE p.source_record_id = $1 AND ($2='medical_records' OR ($2='nurse' AND p.specialist=$3) OR ($2='doctor' AND EXISTS (SELECT 1 FROM queue_items q WHERE q.patient_id=p.patient_id AND q.doctor_id=$4 AND q.status='active')))")
        .bind(&id).bind(&claims.role).bind(department).bind(&claims.sub).fetch_optional(&state.pool).await {
        Ok(Some(row)) => {
            audit_event(&state, &headers, &claims.sub, "source.read", "source", &id, "allowed").await;
            Json(serde_json::json!({
                "patient_id": row.patient_id,
                "age_years": row.age_years,
                "sex": row.sex,
                "visit_date": row.visit_date,
                "specialist": row.specialist,
                "icd10_code": row.icd10_code,
                "prescription_recorded": row.prescription_recorded,
                "source_record_id": row.source_record_id,
                "provenance": {
                    "dataset_id": state.config.active_dataset_id,
                    "fixture_reference": "Doctor-360-Scale-<configured>-Patients.csv",
                    "recorded_date": row.visit_date
                }
            })).into_response()
        }
        Ok(None) => {
            audit_event(&state, &headers, &claims.sub, "source.read", "source", &id, "hidden_or_missing").await;
            problem(StatusCode::NOT_FOUND, "Not found", "Source was not found")
        },
        Err(_) => problem(StatusCode::SERVICE_UNAVAILABLE, "Dependency unavailable", "patient data is temporarily unavailable"),
    }
}

pub async fn encounter_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let claims = match authorized(&headers, &state) {
        Ok(claims) => claims,
        Err(response) => return response,
    };
    if claims.role == "admin" {
        return problem(
            StatusCode::FORBIDDEN,
            "Forbidden",
            "Admin tokens do not grant clinical read access",
        );
    }
    let Ok(encounter) = crate::domain::parse_encounter_id(&id) else {
        return problem(
            StatusCode::NOT_FOUND,
            "Not found",
            "Encounter was not found",
        );
    };
    if encounter.dataset_id != state.config.active_dataset_id {
        return problem(
            StatusCode::NOT_FOUND,
            "Not found",
            "Encounter was not found",
        );
    }
    let patient_id = encounter.patient_id;
    let department = claims.scope.first().cloned().unwrap_or_default();
    match sqlx::query_as::<_, SourceDetail>("SELECT p.patient_id, p.age_years, p.sex, p.visit_date, p.specialist, p.icd10_code, p.prescription_recorded, p.source_record_id FROM synthetic_patients p WHERE p.patient_id = $1 AND ($2='medical_records' OR ($2='nurse' AND p.specialist=$3) OR ($2='doctor' AND EXISTS (SELECT 1 FROM queue_items q WHERE q.patient_id=p.patient_id AND q.doctor_id=$4 AND q.status='active')))")
        .bind(patient_id).bind(&claims.role).bind(department).bind(&claims.sub).fetch_optional(&state.pool).await {
        Ok(Some(row)) => {
            let source_record_id = row.source_record_id.clone();
            let prescriptions = row
                .prescription_recorded
                .as_deref()
                .map(crate::domain::RecordedPrescription::from_source)
                .and_then(|prescription| prescription.to_response(&source_record_id))
                .into_iter()
                .collect::<Vec<_>>();
            Json(serde_json::json!({"encounter_id": id, "patient_id": row.patient_id, "visit_date": row.visit_date, "specialist": row.specialist, "diagnoses": [{"code": row.icd10_code, "source_record_id": row.source_record_id}], "prescriptions": prescriptions})).into_response()
        },
        Ok(None) => {
            audit_event(&state, &headers, &claims.sub, "encounter.read", "encounter", &id, "hidden_or_missing").await;
            problem(StatusCode::NOT_FOUND, "Not found", "Encounter was not found")
        },
        Err(_) => problem(StatusCode::SERVICE_UNAVAILABLE, "Dependency unavailable", "patient data is temporarily unavailable"),
    }
}

pub async fn patient_context(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    query_payload: Result<Query<ContextQuery>, QueryRejection>,
) -> Response {
    let claims = match authorized(&headers, &state) {
        Ok(claims) => claims,
        Err(response) => return response,
    };
    let query = match parse_query(query_payload) {
        Ok(query) => query,
        Err(response) => return response,
    };
    if claims.role == "admin" {
        return problem(
            StatusCode::FORBIDDEN,
            "Forbidden",
            "Admin tokens do not grant clinical read access",
        );
    }
    if let Some(encounter_id) = query.encounter_id.as_deref() {
        let encounter = match crate::domain::parse_encounter_id(encounter_id) {
            Ok(encounter) => encounter,
            Err(_) => {
                return problem(
                    StatusCode::NOT_FOUND,
                    "Not found",
                    "Encounter was not found",
                );
            }
        };
        if encounter.dataset_id != state.config.active_dataset_id || encounter.patient_id != id {
            return problem(
                StatusCode::NOT_FOUND,
                "Not found",
                "Encounter was not found",
            );
        }
    }
    let department = claims.scope.first().cloned().unwrap_or_default();
    match sqlx::query_as::<_, SourceDetail>("SELECT p.patient_id, p.age_years, p.sex, p.visit_date, p.specialist, p.icd10_code, p.prescription_recorded, p.source_record_id FROM synthetic_patients p WHERE p.patient_id = $1 AND ($2='medical_records' OR ($2='nurse' AND p.specialist=$3) OR ($2='doctor' AND EXISTS (SELECT 1 FROM queue_items q WHERE q.patient_id=p.patient_id AND q.doctor_id=$4 AND q.status='active')))")
        .bind(&id).bind(&claims.role).bind(department).bind(&claims.sub).fetch_all(&state.pool).await {
        Ok(rows) if rows.is_empty() => {
            audit_event(&state, &headers, &claims.sub, "context.read", "patient", &id, "hidden_or_missing").await;
            problem(StatusCode::NOT_FOUND, "Not found", "Patient was not found")
        },
        Ok(rows) => {
            let first = &rows[0];
            audit_event(&state, &headers, &claims.sub, "context.read", "patient", &id, "allowed").await;
            Json(serde_json::json!({"patient_id": first.patient_id, "dataset_id": state.config.active_dataset_id, "age_years": first.age_years, "sex": first.sex, "encounters": rows.iter().map(|row| serde_json::json!({"visit_date": row.visit_date, "specialist": row.specialist})).collect::<Vec<_>>(), "data_gaps": ["allergies not recorded", "current medication not recorded", "SOAP notes not recorded"]})).into_response()
        }
        Err(_) => problem(StatusCode::SERVICE_UNAVAILABLE, "Dependency unavailable", "patient data is temporarily unavailable"),
    }
}

pub async fn create_queue_item(
    State(state): State<AppState>,
    headers: HeaderMap,
    payload: Result<Json<QueueRequest>, JsonRejection>,
) -> Response {
    let claims = match authorized(&headers, &state) {
        Ok(claims) => claims,
        Err(response) => return response,
    };
    let request = match parse_json(payload) {
        Ok(request) => request,
        Err(response) => return response,
    };
    if !matches!(claims.role.as_str(), "nurse" | "medical_records") {
        return problem(
            StatusCode::FORBIDDEN,
            "Forbidden",
            "This role cannot assign queue items",
        );
    }
    if let Err(detail) = validate_queue_request(&request, &state.config.active_dataset_id) {
        return problem(StatusCode::UNPROCESSABLE_ENTITY, "Invalid request", detail);
    }
    let doctor = sqlx::query("SELECT role, active FROM dev_identities WHERE principal_id=$1")
        .bind(&request.doctor_id)
        .fetch_optional(&state.pool)
        .await;
    match doctor {
        Ok(Some(row))
            if row.get::<String, _>("role") == "doctor" && row.get::<bool, _>("active") => {}
        Ok(_) => {
            return problem(
                StatusCode::UNPROCESSABLE_ENTITY,
                "Invalid request",
                "doctor_id is not an active doctor",
            );
        }
        Err(_) => {
            return problem(
                StatusCode::SERVICE_UNAVAILABLE,
                "Dependency unavailable",
                "identity storage is temporarily unavailable",
            );
        }
    }
    let patient = sqlx::query("SELECT specialist FROM synthetic_patients WHERE patient_id=$1")
        .bind(&request.patient_id)
        .fetch_optional(&state.pool)
        .await;
    match patient {
        Ok(Some(row)) => {
            if claims.role == "nurse"
                && row.get::<Option<String>, _>("specialist").as_deref()
                    != claims.scope.first().map(String::as_str)
            {
                return problem(
                    StatusCode::FORBIDDEN,
                    "Forbidden",
                    "Patient is outside the nurse department scope",
                );
            }
        }
        Ok(None) => return problem(StatusCode::NOT_FOUND, "Not found", "Patient was not found"),
        Err(_) => {
            return problem(
                StatusCode::SERVICE_UNAVAILABLE,
                "Dependency unavailable",
                "patient data is temporarily unavailable",
            );
        }
    }
    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(_) => {
            return problem(
                StatusCode::SERVICE_UNAVAILABLE,
                "Dependency unavailable",
                "queue storage is temporarily unavailable",
            );
        }
    };
    let result = sqlx::query_as::<_, QueueItem>("INSERT INTO queue_items (queue_item_id, dataset_id, patient_id, encounter_id, doctor_id, assigned_by, status) VALUES ($1, $2, $3, $4, $5, $6, 'active') RETURNING queue_item_id, dataset_id, patient_id, encounter_id, doctor_id, assigned_by, status, created_at")
        .bind(uuid::Uuid::new_v4()).bind(&state.config.active_dataset_id).bind(request.patient_id).bind(request.encounter_id).bind(request.doctor_id).bind(&claims.sub).fetch_one(&mut *tx).await;
    match result {
        Ok(item) => {
            let request_id = headers
                .get("x-request-id")
                .and_then(|value| value.to_str().ok())
                .unwrap_or("generated");
            if sqlx::query("INSERT INTO audit_events (audit_event_id, request_id, principal_id, action, target_type, target_id, outcome) VALUES ($1,$2,$3,$4,$5,$6,$7)")
                .bind(uuid::Uuid::new_v4()).bind(request_id).bind(&claims.sub).bind("queue.assign").bind("patient").bind(&item.patient_id).bind("allowed").execute(&mut *tx).await.is_err()
            {
                return problem(StatusCode::SERVICE_UNAVAILABLE, "Dependency unavailable", "audit storage is temporarily unavailable");
            }
            if tx.commit().await.is_err() {
                return problem(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Dependency unavailable",
                    "queue storage is temporarily unavailable",
                );
            }
            (
                StatusCode::CREATED,
                [
                    (
                        header::LOCATION,
                        format!("/v1/queue-items/{}", item.queue_item_id),
                    ),
                    (header::CACHE_CONTROL, "no-store".to_owned()),
                ],
                Json(item),
            )
                .into_response()
        }
        Err(error) if error.to_string().contains("duplicate") => problem(
            StatusCode::CONFLICT,
            "Conflict",
            "This queue assignment already exists",
        ),
        Err(_) => problem(
            StatusCode::SERVICE_UNAVAILABLE,
            "Dependency unavailable",
            "queue storage is temporarily unavailable",
        ),
    }
}

pub async fn get_queue_item(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(raw_id): Path<String>,
) -> Response {
    let claims = match authorized(&headers, &state) {
        Ok(claims) => claims,
        Err(response) => return response,
    };
    let id = match parse_uuid_path(raw_id) {
        Ok(id) => id,
        Err(response) => return response,
    };
    match sqlx::query_as::<_, QueueItem>("SELECT queue_item_id, dataset_id, patient_id, encounter_id, doctor_id, assigned_by, status, created_at FROM queue_items WHERE queue_item_id=$1 AND (doctor_id=$2 OR assigned_by=$2)").bind(id).bind(&claims.sub).fetch_optional(&state.pool).await { Ok(Some(item)) => Json(item).into_response(), Ok(None) => problem(StatusCode::NOT_FOUND, "Not found", "Queue item was not found"), Err(_) => problem(StatusCode::SERVICE_UNAVAILABLE, "Dependency unavailable", "queue storage is temporarily unavailable") }
}

pub async fn doctor_queue(
    State(state): State<AppState>,
    headers: HeaderMap,
    query_payload: Result<Query<PageQuery>, QueryRejection>,
) -> Response {
    let claims = match authorized(&headers, &state) {
        Ok(claims) => claims,
        Err(response) => return response,
    };
    let query = match parse_query(query_payload) {
        Ok(query) => query,
        Err(response) => return response,
    };
    let limit = match parse_limit(&query) {
        Ok(limit) => limit,
        Err(response) => return response,
    };
    let scope = format!("{}:{}", claims.role, claims.sub);
    let cursor = match query.cursor.as_deref() {
        Some(value) => match decode_cursor(
            value,
            "queue",
            &state.config.active_dataset_id,
            &scope,
            &state.config,
        ) {
            Ok(value) => {
                let Some((created_at, queue_id)) = value.split_once('|') else {
                    return problem(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        "Invalid request",
                        "cursor is invalid",
                    );
                };
                let Ok(created_at) = chrono::DateTime::parse_from_rfc3339(created_at) else {
                    return problem(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        "Invalid request",
                        "cursor is invalid",
                    );
                };
                let Ok(queue_id) = queue_id.parse::<uuid::Uuid>() else {
                    return problem(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        "Invalid request",
                        "cursor is invalid",
                    );
                };
                Some((created_at.with_timezone(&chrono::Utc), queue_id))
            }
            Err(_) => {
                return problem(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "Invalid request",
                    "cursor is invalid",
                );
            }
        },
        None => None,
    };
    let result = if let Some((created_at, queue_id)) = cursor {
        sqlx::query_as::<_, QueueItem>("SELECT queue_item_id, dataset_id, patient_id, encounter_id, doctor_id, assigned_by, status, created_at FROM queue_items WHERE doctor_id = $1 AND status = 'active' AND (created_at < $2 OR (created_at = $2 AND queue_item_id < $3)) ORDER BY created_at DESC, queue_item_id DESC LIMIT $4")
            .bind(&claims.sub).bind(created_at).bind(queue_id).bind(i64::from(limit) + 1).fetch_all(&state.pool).await
    } else {
        sqlx::query_as::<_, QueueItem>("SELECT queue_item_id, dataset_id, patient_id, encounter_id, doctor_id, assigned_by, status, created_at FROM queue_items WHERE doctor_id = $1 AND status = 'active' ORDER BY created_at DESC, queue_item_id DESC LIMIT $2")
            .bind(&claims.sub).bind(i64::from(limit) + 1).fetch_all(&state.pool).await
    };
    match result {
        Ok(mut items) => {
            let next_cursor = if items.len() > limit as usize {
                let item = items.pop().expect("length checked");
                Some(sign_cursor(
                    &format!("{}|{}", item.created_at.to_rfc3339(), item.queue_item_id),
                    "queue",
                    &state.config.active_dataset_id,
                    &scope,
                    &state.config,
                ))
            } else {
                None
            };
            Json(serde_json::json!({"items": items, "next_cursor": next_cursor})).into_response()
        }
        Err(_) => problem(
            StatusCode::SERVICE_UNAVAILABLE,
            "Dependency unavailable",
            "queue storage is temporarily unavailable",
        ),
    }
}

pub async fn create_summary_job(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(encounter_id): Path<String>,
    request_headers: HeaderMap,
    payload: Result<Json<SummaryRequest>, JsonRejection>,
) -> Response {
    let claims = match authorized(&headers, &state) {
        Ok(claims) => claims,
        Err(response) => return response,
    };
    let request = match parse_json(payload) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let reason = request.reason.trim().to_ascii_lowercase();
    if !matches!(reason.as_str(), "nurse_preload" | "doctor_refresh") {
        return problem(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Invalid request",
            "reason must be nurse_preload or doctor_refresh",
        );
    }
    let Some(idempotency_key) = request_headers
        .get("Idempotency-Key")
        .and_then(|v| v.to_str().ok())
    else {
        return problem(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Invalid request",
            "Idempotency-Key is required",
        );
    };
    if idempotency_key.is_empty()
        || idempotency_key.len() > 128
        || !idempotency_key
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && byte != b'"')
    {
        return problem(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Invalid request",
            "Idempotency-Key has an invalid length",
        );
    }
    let Ok(encounter) = crate::domain::parse_encounter_id(&encounter_id) else {
        return problem(
            StatusCode::NOT_FOUND,
            "Not found",
            "Encounter was not found",
        );
    };
    if encounter.dataset_id != state.config.active_dataset_id {
        return problem(
            StatusCode::NOT_FOUND,
            "Not found",
            "Encounter was not found",
        );
    }
    if (reason == "nurse_preload" && claims.role != "nurse")
        || (reason == "doctor_refresh" && claims.role != "doctor")
    {
        return problem(
            StatusCode::FORBIDDEN,
            "Forbidden",
            "This role cannot request that summary job",
        );
    }
    let patient_id = encounter.patient_id;
    let department = claims.scope.first().cloned().unwrap_or_default();
    let authorized_patient = sqlx::query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 FROM synthetic_patients p WHERE p.patient_id=$1 AND ($2='nurse' AND p.specialist=$3 OR $2='doctor' AND EXISTS (SELECT 1 FROM queue_items q WHERE q.patient_id=p.patient_id AND q.doctor_id=$4 AND q.status='active')))" )
        .bind(&patient_id).bind(&claims.role).bind(department).bind(&claims.sub).fetch_one(&state.pool).await;
    match authorized_patient {
        Ok(true) => {}
        Ok(false) => {
            return problem(
                StatusCode::NOT_FOUND,
                "Not found",
                "Encounter was not found",
            );
        }
        Err(_) => {
            return problem(
                StatusCode::SERVICE_UNAVAILABLE,
                "Dependency unavailable",
                "patient data is temporarily unavailable",
            );
        }
    }
    let mut hash = Sha256::new();
    hash.update(format!(
        "{}:{}:{}:{}",
        state.config.active_dataset_id, encounter_id, reason, claims.sub
    ));
    let request_hash = hash.finalize().to_vec();
    if let Ok(Some(existing)) = sqlx::query_as::<_, SummaryJob>(
        "SELECT summary_job_id, status, failure_code FROM summary_jobs WHERE requested_by = $1 AND idempotency_key = $2",
    )
    .bind(&claims.sub)
    .bind(idempotency_key)
    .fetch_optional(&state.pool)
    .await
    {
        let stored_hash: Vec<u8> = sqlx::query_scalar(
            "SELECT request_hash FROM summary_jobs WHERE summary_job_id = $1",
        )
        .bind(existing.summary_job_id)
        .fetch_one(&state.pool)
        .await
        .unwrap_or_default();
        if stored_hash != request_hash {
            return problem(
                StatusCode::CONFLICT,
                "Conflict",
                "Idempotency-Key was reused with a different request",
            );
        }
        return (
            StatusCode::ACCEPTED,
            [
                (
                    header::LOCATION,
                    format!("/v1/summary-jobs/{}", existing.summary_job_id),
                ),
                (header::CACHE_CONTROL, "no-store".to_owned()),
            ],
            Json(existing),
        )
            .into_response();
    }
    let result = sqlx::query_as::<_, SummaryJob>("INSERT INTO summary_jobs (summary_job_id, dataset_id, patient_id, encounter_id, requested_by, reason, idempotency_key, request_hash, status) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'queued') RETURNING summary_job_id, status, failure_code").bind(uuid::Uuid::new_v4()).bind(&state.config.active_dataset_id).bind(patient_id).bind(encounter_id).bind(&claims.sub).bind(reason).bind(idempotency_key).bind(&request_hash).fetch_one(&state.pool).await;
    match result {
        Ok(job) => (
            StatusCode::ACCEPTED,
            [
                (
                    header::LOCATION,
                    format!("/v1/summary-jobs/{}", job.summary_job_id),
                ),
                (header::CACHE_CONTROL, "no-store".to_owned()),
            ],
            Json(job),
        )
            .into_response(),
        Err(error) if error.to_string().contains("duplicate") => {
            match sqlx::query_as::<_, SummaryJob>("SELECT summary_job_id, status, failure_code FROM summary_jobs WHERE requested_by = $1 AND idempotency_key = $2")
                .bind(&claims.sub)
                .bind(idempotency_key)
                .fetch_optional(&state.pool)
                .await
            {
                Ok(Some(existing)) => {
                    let stored_hash: Vec<u8> = sqlx::query_scalar("SELECT request_hash FROM summary_jobs WHERE summary_job_id = $1")
                        .bind(existing.summary_job_id)
                        .fetch_one(&state.pool)
                        .await
                        .unwrap_or_default();
                    if stored_hash == request_hash {
                        return (StatusCode::ACCEPTED, [(header::LOCATION, format!("/v1/summary-jobs/{}", existing.summary_job_id)), (header::CACHE_CONTROL, "no-store".to_owned())], Json(existing)).into_response();
                    }
                    problem(StatusCode::CONFLICT, "Conflict", "Idempotency-Key was reused with a different request")
                }
                _ => problem(StatusCode::CONFLICT, "Conflict", "Idempotency-Key was already used"),
            }
        },
        Err(_) => problem(
            StatusCode::SERVICE_UNAVAILABLE,
            "Dependency unavailable",
            "summary storage is temporarily unavailable",
        ),
    }
}

pub async fn get_summary_job(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(raw_id): Path<String>,
) -> Response {
    let claims = match authorized(&headers, &state) {
        Ok(claims) => claims,
        Err(response) => return response,
    };
    let id = match parse_uuid_path(raw_id) {
        Ok(id) => id,
        Err(response) => return response,
    };
    match sqlx::query(
        "SELECT j.summary_job_id, j.status, j.failure_code, s.summary_id FROM summary_jobs j LEFT JOIN summaries s ON s.summary_job_id=j.summary_job_id WHERE j.summary_job_id = $1 AND EXISTS (SELECT 1 FROM synthetic_patients p WHERE p.patient_id=j.patient_id AND (($2='nurse' AND p.specialist=$3) OR ($2='doctor' AND EXISTS (SELECT 1 FROM queue_items q WHERE q.patient_id=p.patient_id AND q.doctor_id=$4 AND q.status='active'))))",
    )
    .bind(id)
    .bind(&claims.role)
    .bind(claims.scope.first().cloned().unwrap_or_default())
    .bind(&claims.sub)
    .fetch_optional(&state.pool)
    .await
    {
        Ok(Some(row)) => {
            let summary_id = row.get::<Option<uuid::Uuid>, _>("summary_id");
            Json(serde_json::json!({
                "summary_job_id": row.get::<uuid::Uuid, _>("summary_job_id"),
                "status": row.get::<String, _>("status"),
                "failure_code": row.get::<Option<String>, _>("failure_code"),
                "summary_url": summary_id.map(|id| format!("/v1/summaries/{id}"))
            })).into_response()
        }
        Ok(None) => problem(
            StatusCode::NOT_FOUND,
            "Not found",
            "Summary job was not found",
        ),
        Err(_) => problem(
            StatusCode::SERVICE_UNAVAILABLE,
            "Dependency unavailable",
            "summary storage is temporarily unavailable",
        ),
    }
}

pub async fn get_summary(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(raw_id): Path<String>,
) -> Response {
    let claims = match authorized(&headers, &state) {
        Ok(claims) => claims,
        Err(response) => return response,
    };
    let id = match parse_uuid_path(raw_id) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let row = sqlx::query("SELECT s.summary_id, s.patient_id, s.encounter_id, s.dataset_id, s.version, s.generator_version, s.generated_at, s.review_state, s.claims FROM summaries s JOIN synthetic_patients p ON p.patient_id=s.patient_id WHERE s.summary_id = $1 AND (($2='nurse' AND p.specialist=$3) OR ($2='doctor' AND EXISTS (SELECT 1 FROM queue_items q WHERE q.patient_id=p.patient_id AND q.doctor_id=$4 AND q.status='active')))").bind(id).bind(&claims.role).bind(claims.scope.first().cloned().unwrap_or_default()).bind(&claims.sub).fetch_optional(&state.pool).await;
    match row { Ok(Some(row)) => Json(serde_json::json!({"summary_id": row.get::<uuid::Uuid,_>("summary_id"), "patient_id": row.get::<String,_>("patient_id"), "encounter_id": row.get::<String,_>("encounter_id"), "dataset_id": row.get::<String,_>("dataset_id"), "version": row.get::<i32,_>("version"), "generator_version": row.get::<String,_>("generator_version"), "generated_at": row.get::<chrono::DateTime<chrono::Utc>,_>("generated_at"), "review_state": row.get::<String,_>("review_state"), "claims": row.get::<serde_json::Value,_>("claims")})).into_response(), Ok(None) => problem(StatusCode::NOT_FOUND, "Not found", "Summary was not found"), Err(_) => problem(StatusCode::SERVICE_UNAVAILABLE, "Dependency unavailable", "summary storage is temporarily unavailable") }
}

pub async fn add_feedback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(raw_summary_id): Path<String>,
    payload: Result<Json<FeedbackRequest>, JsonRejection>,
) -> Response {
    let claims = match authorized(&headers, &state) {
        Ok(claims) => claims,
        Err(response) => return response,
    };
    let summary_id = match parse_uuid_path(raw_summary_id) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let request = match parse_json(payload) {
        Ok(request) => request,
        Err(response) => return response,
    };
    if claims.role != "doctor" {
        return problem(
            StatusCode::FORBIDDEN,
            "Forbidden",
            "Only doctors may submit feedback",
        );
    }
    let reason = request.reason.trim().to_owned();
    if !matches!(
        request.rating.as_str(),
        "Useful" | "Incomplete" | "Incorrect" | "Unsafe"
    ) || reason.is_empty()
        || reason.chars().count() > 2000
    {
        return problem(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Invalid request",
            "rating or reason is invalid",
        );
    }
    let authorized_summary = sqlx::query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 FROM summaries s JOIN synthetic_patients p ON p.patient_id=s.patient_id WHERE s.summary_id=$1 AND EXISTS (SELECT 1 FROM queue_items q WHERE q.patient_id=s.patient_id AND q.doctor_id=$2 AND q.status='active'))")
        .bind(summary_id).bind(&claims.sub).fetch_one(&state.pool).await;
    match authorized_summary {
        Ok(true) => {}
        Ok(false) => return problem(StatusCode::NOT_FOUND, "Not found", "Summary was not found"),
        Err(_) => {
            return problem(
                StatusCode::SERVICE_UNAVAILABLE,
                "Dependency unavailable",
                "summary storage is temporarily unavailable",
            );
        }
    }
    let id = uuid::Uuid::new_v4();
    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(_) => {
            return problem(
                StatusCode::SERVICE_UNAVAILABLE,
                "Dependency unavailable",
                "feedback storage is temporarily unavailable",
            );
        }
    };
    let result = sqlx::query("INSERT INTO feedback (feedback_id, summary_id, doctor_id, rating, reason) VALUES ($1, $2, $3, $4, $5)").bind(id).bind(summary_id).bind(&claims.sub).bind(&request.rating).bind(&reason).execute(&mut *tx).await;
    match result {
        Ok(_) => {
            let request_id = headers
                .get("x-request-id")
                .and_then(|value| value.to_str().ok())
                .unwrap_or("generated");
            if sqlx::query("INSERT INTO audit_events (audit_event_id, request_id, principal_id, action, target_type, target_id, outcome) VALUES ($1,$2,$3,$4,$5,$6,$7)")
                .bind(uuid::Uuid::new_v4()).bind(request_id).bind(&claims.sub).bind("feedback.create").bind("summary").bind(summary_id.to_string()).bind("allowed").execute(&mut *tx).await.is_err()
            {
                return problem(StatusCode::SERVICE_UNAVAILABLE, "Dependency unavailable", "audit storage is temporarily unavailable");
            }
            if tx.commit().await.is_err() {
                return problem(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Dependency unavailable",
                    "feedback storage is temporarily unavailable",
                );
            }
            (StatusCode::CREATED, [(header::LOCATION, format!("/v1/feedback/{id}")), (header::CACHE_CONTROL, "no-store".to_owned())], Json(serde_json::json!({"feedback_id": id, "summary_id": summary_id, "rating": request.rating, "reason": reason}))).into_response()
        }
        Err(_) => problem(
            StatusCode::SERVICE_UNAVAILABLE,
            "Dependency unavailable",
            "feedback storage is temporarily unavailable",
        ),
    }
}

pub async fn get_feedback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(raw_id): Path<String>,
) -> Response {
    let claims = match authorized(&headers, &state) {
        Ok(claims) => claims,
        Err(response) => return response,
    };
    let id = match parse_uuid_path(raw_id) {
        Ok(id) => id,
        Err(response) => return response,
    };
    match sqlx::query("SELECT feedback_id, summary_id, rating, reason, created_at FROM feedback WHERE feedback_id=$1 AND doctor_id=$2").bind(id).bind(&claims.sub).fetch_optional(&state.pool).await { Ok(Some(row)) => Json(serde_json::json!({"feedback_id": row.get::<uuid::Uuid,_>("feedback_id"), "summary_id": row.get::<uuid::Uuid,_>("summary_id"), "rating": row.get::<String,_>("rating"), "reason": row.get::<String,_>("reason"), "created_at": row.get::<chrono::DateTime<chrono::Utc>,_>("created_at")})).into_response(), Ok(None) => problem(StatusCode::NOT_FOUND, "Not found", "Feedback was not found"), Err(_) => problem(StatusCode::SERVICE_UNAVAILABLE, "Dependency unavailable", "feedback storage is temporarily unavailable") }
}

pub async fn create_import_job(
    State(state): State<AppState>,
    headers: HeaderMap,
    payload: Result<Json<ImportRequest>, JsonRejection>,
) -> Response {
    let claims = match authorized(&headers, &state) {
        Ok(claims) => claims,
        Err(response) => return response,
    };
    let request = match parse_json(payload) {
        Ok(request) => request,
        Err(response) => return response,
    };
    if claims.role != "admin" {
        return problem(
            StatusCode::FORBIDDEN,
            "Forbidden",
            "Only admins may create import jobs",
        );
    }
    let allowed = matches!(
        (request.fixture_reference.as_str(), request.expected_rows),
        ("demo-100-v1", 100) | ("demo-1000-v1", 1000) | ("demo-1000000-v1", 1_000_000)
    );
    if !allowed {
        return problem(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Invalid request",
            "fixture_reference and expected_rows do not match the server allowlist",
        );
    }
    if request.fixture_reference.trim().is_empty() || request.expected_rows <= 0 {
        return problem(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Invalid request",
            "fixture_reference and positive expected_rows are required",
        );
    }
    let active = sqlx::query_scalar::<_, String>(
        "SELECT dataset_id FROM dataset_registry WHERE status='active' LIMIT 1",
    )
    .fetch_optional(&state.pool)
    .await;
    match active {
        Ok(Some(dataset)) if dataset != request.fixture_reference => {
            return problem(
                StatusCode::CONFLICT,
                "Conflict",
                "A different dataset is already active",
            );
        }
        Ok(_) => {}
        Err(_) => {
            return problem(
                StatusCode::SERVICE_UNAVAILABLE,
                "Dependency unavailable",
                "dataset registry is temporarily unavailable",
            );
        }
    }
    let result = sqlx::query_as::<_, ImportJob>("INSERT INTO import_jobs (import_job_id, fixture_reference, expected_rows, status, requested_by) VALUES ($1, $2, $3, 'queued', $4) RETURNING import_job_id, fixture_reference, expected_rows, imported_rows, rejected_rows, status, failure_code")
        .bind(uuid::Uuid::new_v4()).bind(request.fixture_reference).bind(request.expected_rows).bind(claims.sub).fetch_one(&state.pool).await;
    match result {
        Ok(job) => (
            StatusCode::ACCEPTED,
            [
                (
                    header::LOCATION,
                    format!("/v1/import-jobs/{}", job.import_job_id),
                ),
                (header::CACHE_CONTROL, "no-store".to_owned()),
            ],
            Json(job),
        )
            .into_response(),
        Err(_) => problem(
            StatusCode::SERVICE_UNAVAILABLE,
            "Dependency unavailable",
            "import storage is temporarily unavailable",
        ),
    }
}

pub async fn get_import_job(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(raw_id): Path<String>,
) -> Response {
    let claims = match authorized(&headers, &state) {
        Ok(claims) => claims,
        Err(response) => return response,
    };
    let id = match parse_uuid_path(raw_id) {
        Ok(id) => id,
        Err(response) => return response,
    };
    if claims.role != "admin" {
        return problem(
            StatusCode::FORBIDDEN,
            "Forbidden",
            "Only admins may read import jobs",
        );
    }
    match sqlx::query_as::<_, ImportJob>("SELECT import_job_id, fixture_reference, expected_rows, imported_rows, rejected_rows, status, failure_code FROM import_jobs WHERE import_job_id = $1").bind(id).fetch_optional(&state.pool).await { Ok(Some(job)) => Json(job).into_response(), Ok(None) => problem(StatusCode::NOT_FOUND, "Not found", "Import job was not found"), Err(_) => problem(StatusCode::SERVICE_UNAVAILABLE, "Dependency unavailable", "import storage is temporarily unavailable") }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        Config {
            postgres_url: "postgres://test".into(),
            jwt_issuer: "issuer".into(),
            jwt_audience: "audience".into(),
            jwt_private_key: String::new(),
            jwt_public_key: String::new(),
            jwt_key_id: "test-current".into(),
            jwt_previous_key_id: None,
            jwt_previous_public_key: None,
            jwt_access_token_ttl_seconds: 900,
            cursor_signing_key: "test-cursor-signing-key".into(),
            fixture_dir: "fixtures".into(),
            worker_concurrency: 1,
            pool_max_connections: 5,
            api_bind_addr: "127.0.0.1:3000".into(),
            api_public_base_url: "http://localhost:3000".into(),
            summary_lease_seconds: 60,
            summary_max_attempts: 3,
            active_dataset_id: "demo-1000000-v1".into(),
        }
    }

    #[test]
    fn queue_request_validates_identifiers_dataset_and_patient_pair() {
        let valid = QueueRequest {
            patient_id: "SYN-BUNDA-P0001".into(),
            encounter_id: "enc-demo-100-v1-0001".into(),
            doctor_id: "doctor-1".into(),
        };
        assert!(validate_queue_request(&valid, "demo-100-v1").is_ok());

        for invalid in [
            QueueRequest {
                patient_id: "../patient".into(),
                ..valid.clone()
            },
            QueueRequest {
                doctor_id: "doctor/../../secret".into(),
                ..valid.clone()
            },
            QueueRequest {
                encounter_id: "enc-demo-1000-v1-0001".into(),
                ..valid.clone()
            },
            QueueRequest {
                patient_id: "SYN-BUNDA-P0002".into(),
                ..valid.clone()
            },
        ] {
            assert!(validate_queue_request(&invalid, "demo-100-v1").is_err());
        }
    }

    #[test]
    fn cursor_round_trips_and_binds_query() {
        let config = config();
        let cursor = sign_cursor(
            "SYN-BUNDA-P0001",
            "SYN-BUNDA-P",
            "dataset",
            "scope",
            &config,
        );
        assert_eq!(
            decode_cursor(&cursor, "SYN-BUNDA-P", "dataset", "scope", &config).unwrap(),
            "SYN-BUNDA-P0001"
        );
        assert!(decode_cursor(&cursor, "SYN-BUNDA-P", "other-dataset", "scope", &config).is_err());
        assert!(decode_cursor(&cursor, "SYN-BUNDA-P", "dataset", "other-scope", &config).is_err());
        assert!(decode_cursor(&cursor, "other", "dataset", "scope", &config).is_err());
    }

    #[test]
    fn cursor_tampering_is_rejected() {
        let config = config();
        let cursor = sign_cursor("patient-1", "patient", "dataset", "scope", &config);
        let mut parts = cursor.split('.');
        let mut payload = parts.next().unwrap().to_owned();
        payload.push('x');
        let signature = parts.next().unwrap();
        assert!(
            decode_cursor(
                &format!("{payload}.{signature}"),
                "patient",
                "dataset",
                "scope",
                &config
            )
            .is_err()
        );
    }

    #[test]
    fn malformed_uuid_path_is_a_problem_response() {
        let response = parse_uuid_path("not-a-uuid".to_owned()).unwrap_err();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/problem+json"
        );
    }
}
