use anyhow::Result;
use axum::{Router, middleware, routing::get};
use clap::{Parser, Subcommand};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use sqlx::Row;
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tower_http::trace::TraceLayer;

mod app;
mod auth;
mod config;
mod domain;
mod error;
mod fixture;
mod http;
mod repo;
mod service;
mod swagger;
mod worker;

use config::Config;
use http::AppState;

#[derive(Parser)]
#[command(name = "cas-dummy")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Api,
    Worker,
    CheckFixture {
        #[arg(long)]
        dataset: String,
        #[arg(long, default_value_t = 1_000_000)]
        expected: i64,
    },
    SeedDevUsers,
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();
    let config = Config::from_env()?;

    match cli.command {
        Command::Api => run_api(config).await,
        Command::Worker => {
            let pool = config::new_pool(&config, config.pool_max_connections).await?;
            tracing::info!(
                concurrency = config.worker_concurrency,
                "worker configuration loaded"
            );
            sqlx::migrate!().run(&pool).await?;
            worker::run(
                pool,
                config.fixture_dir,
                config.worker_concurrency,
                config.summary_lease_seconds,
                config.summary_max_attempts,
            )
            .await
        }
        Command::CheckFixture { dataset, expected } => {
            let (spec, path) = fixture::resolve_fixture(
                &dataset,
                expected as u64,
                std::path::Path::new(&config.fixture_dir),
            )?;
            let (imported, rejected, checksum) = fixture::validate_file(&path, expected as u64)?;
            if imported != expected as u64 || rejected != 0 {
                anyhow::bail!(
                    "fixture {} has {imported} valid and {rejected} rejected rows",
                    spec.reference
                );
            }
            tracing::info!(%dataset, %checksum, "fixture validation passed");
            let pool = config::new_pool(&config, config.pool_max_connections).await?;
            let columns = sqlx::query(
                "SELECT column_name, data_type, is_nullable FROM information_schema.columns WHERE table_schema='public' AND table_name='synthetic_patients' ORDER BY ordinal_position",
            )
            .fetch_all(&pool)
            .await?;
            let expected_columns = vec![
                ("patient_id".to_owned(), "text".to_owned(), "NO".to_owned()),
                (
                    "age_years".to_owned(),
                    "integer".to_owned(),
                    "YES".to_owned(),
                ),
                ("sex".to_owned(), "text".to_owned(), "YES".to_owned()),
                ("visit_date".to_owned(), "date".to_owned(), "YES".to_owned()),
                ("specialist".to_owned(), "text".to_owned(), "YES".to_owned()),
                ("icd10_code".to_owned(), "text".to_owned(), "YES".to_owned()),
                (
                    "prescription_recorded".to_owned(),
                    "text".to_owned(),
                    "YES".to_owned(),
                ),
                (
                    "source_record_id".to_owned(),
                    "text".to_owned(),
                    "YES".to_owned(),
                ),
            ];
            let actual_columns: Vec<_> = columns
                .iter()
                .map(|row| {
                    (
                        row.get::<String, _>("column_name"),
                        row.get::<String, _>("data_type"),
                        row.get::<String, _>("is_nullable"),
                    )
                })
                .collect();
            let columns_match = actual_columns.len() == expected_columns.len()
                && actual_columns
                    .iter()
                    .zip(expected_columns.iter())
                    .all(|(actual, expected)| {
                        actual.0 == expected.0
                            && actual.1 == expected.1
                            && (actual.2 == expected.2
                                || (actual.0 == "source_record_id"
                                    && matches!(actual.2.as_str(), "YES" | "NO")))
                    });
            if !columns_match {
                anyhow::bail!(
                    "synthetic_patients schema does not match the fixture contract: expected {expected_columns:?}, found {actual_columns:?}"
                );
            }
            let primary_key_columns: Vec<String> = sqlx::query_scalar(
                "SELECT kcu.column_name FROM information_schema.table_constraints tc JOIN information_schema.key_column_usage kcu ON kcu.constraint_name=tc.constraint_name AND kcu.table_schema=tc.table_schema WHERE tc.table_schema='public' AND tc.table_name='synthetic_patients' AND tc.constraint_type='PRIMARY KEY' ORDER BY kcu.ordinal_position",
            )
            .fetch_all(&pool)
            .await?;
            if primary_key_columns != ["patient_id".to_owned()] {
                anyhow::bail!("synthetic_patients primary key does not match the fixture contract");
            }
            let source_storage_bytes: i64 =
                sqlx::query_scalar("SELECT pg_total_relation_size('public.synthetic_patients')")
                    .fetch_one(&pool)
                    .await?;
            let source_indexes: Vec<String> = sqlx::query_scalar(
                "SELECT indexname FROM pg_indexes WHERE schemaname='public' AND tablename='synthetic_patients' ORDER BY indexname",
            )
            .fetch_all(&pool)
            .await?;
            let count: i64 = sqlx::query_scalar("SELECT count(*) FROM synthetic_patients")
                .fetch_one(&pool)
                .await?;
            if count != expected {
                anyhow::bail!("fixture {dataset} expected {expected} rows, found {count}");
            }
            let duplicate_source_ids: i64 = sqlx::query_scalar(
                "SELECT count(*) - count(DISTINCT source_record_id) FROM synthetic_patients",
            )
            .fetch_one(&pool)
            .await?;
            if duplicate_source_ids != 0 {
                anyhow::bail!(
                    "fixture {dataset} contains {duplicate_source_ids} duplicate source_record_id values"
                );
            }
            let invalid_source_ids: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM synthetic_patients WHERE source_record_id IS NULL OR btrim(source_record_id) = ''",
            )
            .fetch_one(&pool)
            .await?;
            if invalid_source_ids != 0 {
                anyhow::bail!(
                    "fixture {dataset} contains {invalid_source_ids} null or blank source_record_id values"
                );
            }
            let mut reader = csv::Reader::from_path(&path)?;
            let mut first_sample: Option<csv::StringRecord> = None;
            let mut last_sample: Option<csv::StringRecord> = None;
            for record in reader.records() {
                let record = record?;
                if first_sample.is_none() {
                    first_sample = Some(record.clone());
                }
                last_sample = Some(record);
            }
            let (Some(first), Some(last)) = (first_sample, last_sample) else {
                anyhow::bail!("fixture {dataset} contains no data rows");
            };
            for sample in [&first, &last] {
                let patient_id = sample.get(0).unwrap_or_default();
                let expected_source = sample.get(7).unwrap_or_default();
                let actual_source: Option<String> = sqlx::query_scalar(
                    "SELECT source_record_id FROM synthetic_patients WHERE patient_id=$1",
                )
                .bind(patient_id)
                .fetch_optional(&pool)
                .await?;
                if actual_source.as_deref() != Some(expected_source) {
                    anyhow::bail!("fixture {dataset} sample mismatch for patient {patient_id}");
                }
            }
            tracing::info!(
                %dataset,
                %count,
                source_storage_bytes,
                ?source_indexes,
                "fixture check passed"
            );
            Ok(())
        }
        Command::SeedDevUsers => seed_dev_users(config).await,
    }
}

#[derive(Deserialize)]
struct DevUser {
    principal_id: String,
    role: String,
    department_scope: Option<String>,
    token: String,
}

async fn seed_dev_users(config: Config) -> Result<()> {
    let path = std::env::var("DEV_TOKENS_CONFIG")
        .map_err(|_| anyhow::anyhow!("DEV_TOKENS_CONFIG is required for seed-dev-users"))?;
    let users: Vec<DevUser> = serde_json::from_str(&std::fs::read_to_string(path)?)?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&config.postgres_url)
        .await?;
    sqlx::migrate!().run(&pool).await?;
    for user in users {
        if user.token.len() < 32 {
            anyhow::bail!(
                "dev token for {} must be at least 32 characters",
                user.principal_id
            );
        }
        let digest = Sha256::digest(user.token.as_bytes()).to_vec();
        sqlx::query("INSERT INTO dev_identities (principal_id, role, department_scope, token_sha256, active) VALUES ($1,$2,$3,$4,true) ON CONFLICT (principal_id) DO UPDATE SET role=EXCLUDED.role, department_scope=EXCLUDED.department_scope, token_sha256=EXCLUDED.token_sha256, active=true")
            .bind(user.principal_id).bind(user.role).bind(user.department_scope).bind(digest).execute(&pool).await?;
    }
    Ok(())
}

async fn run_api(config: Config) -> Result<()> {
    let pool = config::new_pool(&config, config.pool_max_connections).await?;
    sqlx::migrate!().run(&pool).await?;
    let listener = TcpListener::bind(&config.api_bind_addr).await?;
    let public_base_url = config.api_public_base_url.clone();
    let app = build_router(AppState::new(config, pool));
    let address: SocketAddr = listener.local_addr()?;
    tracing::info!(%address, %public_base_url, "API listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/openapi.json", get(swagger::openapi))
        .route("/swagger", get(swagger::ui))
        .route("/v1/auth/token", axum::routing::post(auth::issue_token))
        .route("/v1/patients", get(http::list_patients))
        .route("/v1/patients/{id}", get(http::patient_detail))
        .route(
            "/v1/patients/{id}/encounters",
            get(http::patient_encounters),
        )
        .route("/v1/patients/{id}/context", get(http::patient_context))
        .route("/v1/encounters/{id}", get(http::encounter_detail))
        .route("/v1/sources/{id}", get(http::source_detail))
        .route(
            "/v1/queue-items",
            axum::routing::post(http::create_queue_item),
        )
        .route("/v1/queue-items/{id}", get(http::get_queue_item))
        .route("/v1/doctors/me/queue", get(http::doctor_queue))
        .route(
            "/v1/encounters/{id}/summary-jobs",
            axum::routing::post(http::create_summary_job),
        )
        .route("/v1/summary-jobs/{id}", get(http::get_summary_job))
        .route("/v1/summaries/{id}", get(http::get_summary))
        .route(
            "/v1/summaries/{id}/feedback",
            axum::routing::post(http::add_feedback),
        )
        .route("/v1/feedback/{id}", get(http::get_feedback))
        .route(
            "/v1/import-jobs",
            axum::routing::post(http::create_import_job),
        )
        .route("/v1/import-jobs/{id}", get(http::get_import_job))
        .fallback(|| async {
            error::problem(
                axum::http::StatusCode::NOT_FOUND,
                "Not found",
                "The requested resource was not found",
            )
        })
        .layer(TraceLayer::new_for_http())
        .layer(middleware::from_fn_with_state(
            state.clone(),
            active_identity_middleware,
        ))
        .layer(middleware::from_fn(request_id_middleware))
        .with_state(state)
}

async fn active_identity_middleware(
    axum::extract::State(state): axum::extract::State<AppState>,
    request: axum::http::Request<axum::body::Body>,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let path = request.uri().path();
    if !path.starts_with("/v1/") || path == "/v1/auth/token" {
        return next.run(request).await;
    }

    let claims = match auth::validate(request.headers(), &state.config) {
        Ok(claims) => claims,
        Err(status) => {
            return error::problem(status, "Authentication failed", "A valid JWT is required");
        }
    };
    let identity = sqlx::query(
        "SELECT role, active, department_scope FROM dev_identities WHERE principal_id = $1 AND NOT EXISTS (SELECT 1 FROM revoked_jtis WHERE jti = $2 AND expires_at > now())",
    )
    .bind(&claims.sub)
    .bind(&claims.jti)
    .fetch_optional(&state.pool)
    .await;
    match identity {
        Ok(Some(identity)) if identity.get::<bool, _>("active") => {
            let stored_role: String = identity.get("role");
            let stored_scope: Option<String> = identity.get("department_scope");
            if auth::identity_matches(&claims, &stored_role, true, stored_scope.as_deref()) {
                next.run(request).await
            } else {
                error::problem(
                    axum::http::StatusCode::UNAUTHORIZED,
                    "Authentication failed",
                    "The token identity is not active",
                )
            }
        }
        Ok(_) => error::problem(
            axum::http::StatusCode::UNAUTHORIZED,
            "Authentication failed",
            "The token identity is not active",
        ),
        Err(_) => error::problem(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "Dependency unavailable",
            "identity storage is temporarily unavailable",
        ),
    }
}

async fn request_id_middleware(
    request: axum::http::Request<axum::body::Body>,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let is_api_route = request.uri().path().starts_with("/v1/");
    let request_id = request
        .headers()
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .filter(|value| {
            (1..=64).contains(&value.len())
                && value
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        })
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        "x-request-id",
        axum::http::HeaderValue::from_str(&request_id).expect("validated request id"),
    );
    if is_api_route {
        response.headers_mut().insert(
            axum::http::header::CACHE_CONTROL,
            axum::http::HeaderValue::from_static("no-store"),
        );
    }
    response
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    fn database_test_lock() -> &'static tokio::sync::Mutex<()> {
        static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
    }

    fn state() -> AppState {
        AppState::new(
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
                cursor_signing_key: "cursor-test-key".into(),
                fixture_dir: "fixtures".into(),
                worker_concurrency: 1,
                pool_max_connections: 5,
                api_bind_addr: "127.0.0.1:3000".into(),
                api_public_base_url: "http://localhost:3000".into(),
                summary_lease_seconds: 60,
                summary_max_attempts: 3,
                active_dataset_id: "demo-1000000-v1".into(),
            },
            sqlx::postgres::PgPoolOptions::new()
                .connect_lazy("postgres://test")
                .unwrap(),
        )
    }

    #[tokio::test]
    async fn health_is_public() {
        let response = build_router(state())
            .oneshot(Request::get("/healthz").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().contains_key("x-request-id"));
    }

    #[tokio::test]
    async fn protected_patient_route_requires_bearer_jwt() {
        let response = build_router(state())
            .oneshot(Request::get("/v1/patients").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            response
                .headers()
                .get(axum::http::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("application/problem+json")
        );
        assert_eq!(
            response
                .headers()
                .get(axum::http::header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok()),
            Some("no-store")
        );
        assert_eq!(
            response
                .headers()
                .get(axum::http::header::WWW_AUTHENTICATE)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer")
        );
        assert!(response.headers().contains_key("x-request-id"));
    }

    #[tokio::test]
    async fn protected_route_surface_is_registered() {
        let routes = [
            ("GET", "/v1/patients?query=SYN-BUNDA-P"),
            ("GET", "/v1/patients/SYN-BUNDA-P0001"),
            ("GET", "/v1/patients/SYN-BUNDA-P0001/encounters"),
            ("GET", "/v1/patients/SYN-BUNDA-P0001/context"),
            ("GET", "/v1/encounters/enc-demo-100-v1-0001"),
            ("GET", "/v1/sources/SYN-BUNDA-V0001-SRC"),
            ("POST", "/v1/queue-items"),
            (
                "GET",
                "/v1/queue-items/00000000-0000-0000-0000-000000000000",
            ),
            ("GET", "/v1/doctors/me/queue"),
            ("POST", "/v1/encounters/enc-demo-100-v1-0001/summary-jobs"),
            (
                "GET",
                "/v1/summary-jobs/00000000-0000-0000-0000-000000000000",
            ),
            ("GET", "/v1/summaries/00000000-0000-0000-0000-000000000000"),
            (
                "POST",
                "/v1/summaries/00000000-0000-0000-0000-000000000000/feedback",
            ),
            ("GET", "/v1/feedback/00000000-0000-0000-0000-000000000000"),
            ("POST", "/v1/import-jobs"),
            (
                "GET",
                "/v1/import-jobs/00000000-0000-0000-0000-000000000000",
            ),
        ];
        for (method, path) in routes {
            let response = build_router(state())
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(path)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert!(
                !matches!(
                    response.status(),
                    StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED
                ),
                "route was not registered: {method} {path}"
            );
        }
    }

    #[tokio::test]
    async fn queue_item_location_resource_is_a_protected_route() {
        let response = build_router(state())
            .oneshot(
                Request::get("/v1/queue-items/00000000-0000-0000-0000-000000000000")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            response.headers().get(axum::http::header::WWW_AUTHENTICATE),
            Some(&axum::http::HeaderValue::from_static("Bearer"))
        );
    }

    #[tokio::test]
    async fn feedback_location_resource_is_a_protected_route() {
        let response = build_router(state())
            .oneshot(
                Request::get("/v1/feedback/00000000-0000-0000-0000-000000000000")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            response.headers().get(axum::http::header::WWW_AUTHENTICATE),
            Some(&axum::http::HeaderValue::from_static("Bearer"))
        );
    }

    #[tokio::test]
    async fn unknown_route_returns_problem_and_safe_request_id() {
        let response = build_router(state())
            .oneshot(
                Request::get("/unknown?patient=SYN-BUNDA-P0001")
                    .header("x-request-id", "test-request-1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            response.headers().get("x-request-id").unwrap(),
            "test-request-1"
        );
        assert_eq!(
            response
                .headers()
                .get(axum::http::header::CONTENT_TYPE)
                .unwrap(),
            "application/problem+json"
        );
        assert_eq!(
            response
                .headers()
                .get(axum::http::header::CACHE_CONTROL)
                .unwrap(),
            "no-store"
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let problem: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            problem["type"],
            "https://cas-dummy.local/problems/not-found"
        );
    }

    #[tokio::test]
    async fn token_configuration_error_uses_problem_contract() {
        let response = build_router(state())
            .oneshot(
                Request::post("/v1/auth/token")
                    .header(axum::http::header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"subject":"doctor-1","role":"doctor"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            response
                .headers()
                .get(axum::http::header::CONTENT_TYPE)
                .unwrap(),
            "application/problem+json"
        );
        assert_eq!(
            response
                .headers()
                .get(axum::http::header::CACHE_CONTROL)
                .unwrap(),
            "no-store"
        );
    }

    #[tokio::test]
    async fn malformed_token_json_uses_problem_contract() {
        let response = build_router(state())
            .oneshot(
                Request::post("/v1/auth/token")
                    .header(axum::http::header::CONTENT_TYPE, "application/json")
                    .body(Body::from("not-json"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response
                .headers()
                .get(axum::http::header::CONTENT_TYPE)
                .unwrap(),
            "application/problem+json"
        );
    }

    #[test]
    fn invalid_pagination_limits_are_rejected_as_422() {
        for value in ["-1", "0", "not-a-number", "101"] {
            let query = http::PageQuery {
                limit: Some(value.to_owned()),
                cursor: None,
                query: None,
            };
            let response = http::parse_limit(&query).expect_err("invalid limit accepted");
            assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        }
    }

    #[tokio::test]
    #[ignore = "requires PostgreSQL; run in CI or with POSTGRES_URL"]
    async fn migrations_are_idempotent_against_postgres() {
        let _guard = database_test_lock().lock().await;
        let url = std::env::var("POSTGRES_URL").expect("POSTGRES_URL must be set");
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        let table_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM information_schema.tables WHERE table_name = 'summary_jobs'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(table_count, 1);
        let revocation_table_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM information_schema.tables WHERE table_name = 'revoked_jtis'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(revocation_table_count, 1);
    }

    #[tokio::test]
    #[ignore = "requires PostgreSQL; run in CI or with POSTGRES_URL"]
    async fn operational_unique_constraints_reject_duplicate_active_work() {
        let _guard = database_test_lock().lock().await;
        let url = std::env::var("POSTGRES_URL").expect("POSTGRES_URL must be set");
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();

        let suffix = uuid::Uuid::new_v4().simple().to_string();
        let patient_id = format!("test-patient-{suffix}");
        let doctor_id = format!("test-doctor-{suffix}");
        let encounter_id = format!("enc-test-{suffix}");
        let queue_id = uuid::Uuid::new_v4();
        let summary_id = uuid::Uuid::new_v4();

        sqlx::query(
            "INSERT INTO synthetic_patients (patient_id, source_record_id) VALUES ($1, $2)",
        )
        .bind(&patient_id)
        .bind(format!("test-source-{suffix}"))
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO dev_identities (principal_id, role, active) VALUES ($1, 'doctor', true)",
        )
        .bind(&doctor_id)
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query("INSERT INTO queue_items (queue_item_id, dataset_id, patient_id, encounter_id, doctor_id, status) VALUES ($1, 'test', $2, $3, $4, 'active')")
            .bind(queue_id)
            .bind(&patient_id)
            .bind(&encounter_id)
            .bind(&doctor_id)
            .execute(&pool)
            .await
            .unwrap();
        assert!(sqlx::query("INSERT INTO queue_items (queue_item_id, dataset_id, patient_id, encounter_id, doctor_id, status) VALUES ($1, 'test', $2, $3, $4, 'active')")
            .bind(uuid::Uuid::new_v4())
            .bind(&patient_id)
            .bind(&encounter_id)
            .bind(&doctor_id)
            .execute(&pool)
            .await
            .is_err());

        sqlx::query("INSERT INTO summary_jobs (summary_job_id, dataset_id, patient_id, encounter_id, requested_by, reason, idempotency_key, request_hash, status) VALUES ($1, 'test', $2, $3, $4, 'doctor_refresh', 'same-key', $5, 'queued')")
            .bind(summary_id)
            .bind(&patient_id)
            .bind(&encounter_id)
            .bind(&doctor_id)
            .bind(vec![0_u8; 32])
            .execute(&pool)
            .await
            .unwrap();
        assert!(sqlx::query("INSERT INTO summary_jobs (summary_job_id, dataset_id, patient_id, encounter_id, requested_by, reason, idempotency_key, request_hash, status) VALUES ($1, 'test', $2, $3, $4, 'doctor_refresh', 'same-key', $5, 'queued')")
            .bind(uuid::Uuid::new_v4())
            .bind(&patient_id)
            .bind(&encounter_id)
            .bind(&doctor_id)
            .bind(vec![0_u8; 32])
            .execute(&pool)
            .await
            .is_err());

        sqlx::query("DELETE FROM summary_jobs WHERE summary_job_id = $1")
            .bind(summary_id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM queue_items WHERE queue_item_id = $1")
            .bind(queue_id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM dev_identities WHERE principal_id = $1")
            .bind(&doctor_id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM synthetic_patients WHERE patient_id = $1")
            .bind(&patient_id)
            .execute(&pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore = "requires an isolated PostgreSQL database; run in CI or with POSTGRES_URL"]
    async fn import_worker_processes_each_small_allowlisted_fixture() {
        let _guard = database_test_lock().lock().await;
        let url = std::env::var("POSTGRES_URL").expect("POSTGRES_URL must be set");
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();

        for (fixture_reference, expected_rows) in
            [("demo-100-v1", 100_i64), ("demo-1000-v1", 1_000_i64)]
        {
            sqlx::query(
                "TRUNCATE TABLE audit_events, feedback, summaries, summary_jobs, import_rejects, import_staging, import_jobs, queue_items, dataset_registry, dev_identities, synthetic_patients CASCADE",
            )
            .execute(&pool)
            .await
            .unwrap();

            let principal_id = "integration-admin";
            let job_id = uuid::Uuid::new_v4();
            sqlx::query(
                "INSERT INTO dev_identities (principal_id, role, active) VALUES ($1, 'admin', true)",
            )
            .bind(principal_id)
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO import_jobs (import_job_id, fixture_reference, expected_rows, status, requested_by) VALUES ($1, $2, $3, 'queued', $4)",
            )
            .bind(job_id)
            .bind(fixture_reference)
            .bind(expected_rows)
            .bind(principal_id)
            .execute(&pool)
            .await
            .unwrap();

            assert!(
                worker::process_one_import_job(&pool, std::path::Path::new("fixtures"))
                    .await
                    .unwrap()
            );

            let job: (String, i64, i64) = sqlx::query_as(
                "SELECT status, imported_rows, rejected_rows FROM import_jobs WHERE import_job_id = $1",
            )
            .bind(job_id)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(job, ("ready".to_owned(), expected_rows, 0));
            let source_count: i64 = sqlx::query_scalar("SELECT count(*) FROM synthetic_patients")
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(source_count, expected_rows);
            let active_dataset: String = sqlx::query_scalar(
                "SELECT dataset_id FROM dataset_registry WHERE status = 'active'",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(active_dataset, fixture_reference);
        }
    }

    #[tokio::test]
    #[ignore = "requires an isolated PostgreSQL database; run in CI or with POSTGRES_URL"]
    async fn summary_workers_recover_expired_lease_without_duplicate_artifact() {
        let _guard = database_test_lock().lock().await;
        let url = std::env::var("POSTGRES_URL").expect("POSTGRES_URL must be set");
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(4)
            .connect(&url)
            .await
            .unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        sqlx::query(
            "TRUNCATE TABLE audit_events, feedback, summaries, summary_jobs, import_rejects, import_staging, import_jobs, queue_items, dataset_registry, dev_identities, synthetic_patients CASCADE",
        )
        .execute(&pool)
        .await
        .unwrap();

        let patient_id = "SYN-BUNDA-P0001";
        let encounter_id = "enc-demo-100-v1-0001";
        let doctor_id = "summary-worker-doctor";
        let job_id = uuid::Uuid::new_v4();
        sqlx::query("INSERT INTO synthetic_patients (patient_id, age_years, sex, visit_date, specialist, icd10_code, prescription_recorded, source_record_id) VALUES ($1, 40, 'F', '2026-01-01', 'Cardiology', 'A01', 'None recorded', 'SYN-BUNDA-V0001-SRC')")
            .bind(patient_id).execute(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO dev_identities (principal_id, role, active) VALUES ($1, 'doctor', true)",
        )
        .bind(doctor_id)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO summary_jobs (summary_job_id, dataset_id, patient_id, encounter_id, requested_by, reason, idempotency_key, request_hash, status, attempt_count, lease_owner, lease_expires_at) VALUES ($1, 'demo-100-v1', $2, $3, $4, 'doctor_refresh', 'recovery-key', $5, 'running', 1, 'dead-worker', now() - interval '1 minute')")
            .bind(job_id).bind(patient_id).bind(encounter_id).bind(doctor_id).bind(vec![0_u8; 32]).execute(&pool).await.unwrap();

        let (first, second) = tokio::join!(
            worker::process_one_summary_job(&pool, 60, 3),
            worker::process_one_summary_job(&pool, 60, 3)
        );
        assert_eq!(
            usize::from(first.unwrap()) + usize::from(second.unwrap()),
            1
        );

        let job: (String, i32, Option<String>) = sqlx::query_as(
            "SELECT status, attempt_count, lease_owner FROM summary_jobs WHERE summary_job_id=$1",
        )
        .bind(job_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(job, ("ready".to_owned(), 2, None));
        let artifact_count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM summaries WHERE summary_job_id=$1")
                .bind(job_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(artifact_count, 1);
        assert!(!worker::process_one_summary_job(&pool, 60, 3).await.unwrap());

        let exhausted_job = uuid::Uuid::new_v4();
        sqlx::query("INSERT INTO summary_jobs (summary_job_id, dataset_id, patient_id, encounter_id, requested_by, reason, idempotency_key, request_hash, status, attempt_count) VALUES ($1, 'demo-100-v1', $2, $3, $4, 'doctor_refresh', 'exhausted-key', $5, 'queued', 3)")
            .bind(exhausted_job).bind(patient_id).bind(encounter_id).bind(doctor_id).bind(vec![1_u8; 32]).execute(&pool).await.unwrap();
        assert!(!worker::process_one_summary_job(&pool, 60, 3).await.unwrap());
    }

    #[test]
    fn postman_collection_contains_expected_request_count() {
        let collection: serde_json::Value = serde_json::from_str(include_str!(
            "../docs/Doctor-360-Patient-Context-API.postman_collection.json"
        ))
        .unwrap();
        let groups = collection["item"].as_array().unwrap();
        let request_count: usize = groups
            .iter()
            .map(|group| group["item"].as_array().map_or(0, Vec::len))
            .sum();
        assert_eq!(request_count, 18);
    }
}
