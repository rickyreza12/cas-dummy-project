use axum::{
    Json,
    extract::{State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
};
use chrono::{Duration, Utc};
use jsonwebtoken::{
    Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, decode_header, encode,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Default)]
pub struct TokenVerifier;

pub fn identity_matches(
    claims: &Claims,
    stored_role: &str,
    active: bool,
    stored_scope: Option<&str>,
) -> bool {
    active && claims.role == stored_role && claims.scope.first().map(String::as_str) == stored_scope
}

use crate::{config::Config, error::problem, http::AppState};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub iss: String,
    pub aud: String,
    pub sub: String,
    pub role: String,
    pub scope: Vec<String>,
    pub token_type: String,
    pub iat: i64,
    pub exp: i64,
    #[serde(default)]
    pub nbf: Option<i64>,
    pub jti: String,
}

#[derive(Debug, Deserialize)]
pub struct TokenRequest {
    pub subject: String,
    pub role: String,
    #[serde(default)]
    pub scope: Vec<String>,
}

pub async fn issue_token(
    State(state): State<AppState>,
    payload: Result<Json<TokenRequest>, JsonRejection>,
) -> axum::response::Response {
    let Json(request) = match payload {
        Ok(payload) => payload,
        Err(_) => {
            return problem(
                StatusCode::BAD_REQUEST,
                "Invalid request",
                "request body must be valid JSON",
            );
        }
    };
    let config = &state.config;
    if request.subject.trim().is_empty() || request.role.trim().is_empty() {
        return problem(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Invalid request",
            "subject and role are required",
        );
    }
    if !matches!(
        request.role.as_str(),
        "doctor" | "nurse" | "admin" | "medical_records"
    ) {
        return problem(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Invalid request",
            "unsupported role",
        );
    }
    if config.jwt_private_key.trim().is_empty() {
        return problem(
            StatusCode::SERVICE_UNAVAILABLE,
            "Dependency unavailable",
            "JWT signing key is not configured",
        );
    }
    let now = Utc::now();
    let claims = Claims {
        iss: config.jwt_issuer.clone(),
        aud: config.jwt_audience.clone(),
        sub: request.subject,
        role: request.role,
        scope: request.scope,
        token_type: "access".into(),
        iat: now.timestamp(),
        exp: (now + Duration::seconds(config.jwt_access_token_ttl_seconds)).timestamp(),
        nbf: Some(now.timestamp()),
        jti: uuid::Uuid::new_v4().to_string(),
    };
    let key = EncodingKey::from_rsa_pem(config.jwt_private_key.as_bytes()).map_err(|_| {
        problem(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Internal error",
            "could not configure token signing",
        )
    });
    let key = match key {
        Ok(key) => key,
        Err(response) => return response,
    };
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(config.jwt_key_id.clone());
    let token = match encode(&header, &claims, &key) {
        Ok(token) => token,
        Err(_) => {
            return problem(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Internal error",
                "could not issue JWT",
            );
        }
    };
    (
        StatusCode::OK,
        Json(
            serde_json::json!({ "access_token": token, "token_type": "Bearer", "expires_in": config.jwt_access_token_ttl_seconds }),
        ),
    )
        .into_response()
}

pub fn validate(headers: &HeaderMap, config: &Config) -> Result<Claims, StatusCode> {
    let value = headers
        .get(header::AUTHORIZATION)
        .ok_or(StatusCode::UNAUTHORIZED)?
        .to_str()
        .map_err(|_| StatusCode::UNAUTHORIZED)?;
    if value.contains(',') {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let token = value
        .strip_prefix("Bearer ")
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if token.trim().is_empty() || token.chars().any(char::is_whitespace) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let parsed_header = decode_header(token).map_err(|_| StatusCode::UNAUTHORIZED)?;
    if parsed_header.alg != Algorithm::RS256 {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let key_pem = match parsed_header.kid.as_deref() {
        Some(kid) if kid == config.jwt_key_id => &config.jwt_public_key,
        Some(kid) if config.jwt_previous_key_id.as_deref() == Some(kid) => config
            .jwt_previous_public_key
            .as_deref()
            .ok_or(StatusCode::UNAUTHORIZED)?,
        _ => return Err(StatusCode::UNAUTHORIZED),
    };
    let key = DecodingKey::from_rsa_pem(key_pem.as_bytes())
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_issuer(std::slice::from_ref(&config.jwt_issuer));
    validation.set_audience(std::slice::from_ref(&config.jwt_audience));
    validation.validate_nbf = true;
    decode::<Claims>(token, &key, &validation)
        .map(|data| data.claims)
        .and_then(|claims| {
            if claims.sub.trim().is_empty()
                || !matches!(
                    claims.role.as_str(),
                    "doctor" | "nurse" | "admin" | "medical_records"
                )
                || claims.token_type != "access"
                || claims.jti.trim().is_empty()
            {
                return Err(jsonwebtoken::errors::Error::from(
                    jsonwebtoken::errors::ErrorKind::InvalidToken,
                ));
            }
            Ok(claims)
        })
        .map_err(|_| StatusCode::UNAUTHORIZED)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};

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
            cursor_signing_key: "cursor-key".into(),
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
    fn malformed_bearer_values_are_unauthorized() {
        for value in [
            "Basic abc",
            "Bearer ",
            "Bearer abc def",
            "Bearer a,Bearer b",
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(header::AUTHORIZATION, value.parse().unwrap());
            assert!(matches!(
                validate(&headers, &config()),
                Err(StatusCode::UNAUTHORIZED)
            ));
        }
    }

    #[test]
    fn unsupported_jwt_algorithm_is_unauthorized_before_key_lookup() {
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"HS256","typ":"JWT"}"#);
        let payload = URL_SAFE_NO_PAD.encode(r#"{"sub":"doctor-1"}"#);
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {header}.{payload}.signature")
                .parse()
                .unwrap(),
        );
        assert!(matches!(
            validate(&headers, &config()),
            Err(StatusCode::UNAUTHORIZED)
        ));
    }

    #[test]
    fn identity_policy_requires_active_matching_role_and_scope() {
        let mut claims = Claims {
            iss: "issuer".into(),
            aud: "audience".into(),
            sub: "nurse-1".into(),
            role: "nurse".into(),
            scope: vec!["Cardiology".into()],
            token_type: "access".into(),
            iat: 1,
            exp: 2,
            nbf: None,
            jti: "jti".into(),
        };
        assert!(identity_matches(&claims, "nurse", true, Some("Cardiology")));
        assert!(!identity_matches(
            &claims,
            "nurse",
            false,
            Some("Cardiology")
        ));
        assert!(!identity_matches(
            &claims,
            "doctor",
            true,
            Some("Cardiology")
        ));
        assert!(!identity_matches(&claims, "nurse", true, Some("Oncology")));
        claims.scope.clear();
        assert!(identity_matches(&claims, "nurse", true, None));
    }
}
