use axum::{
    Json,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Serialize;

#[derive(Serialize)]
pub struct ProblemDetails<'a> {
    #[serde(rename = "type")]
    pub problem_type: &'a str,
    pub title: &'a str,
    pub status: u16,
    pub detail: &'a str,
    pub instance: &'static str,
    pub request_id: String,
}

fn problem_type(status: StatusCode) -> &'static str {
    match status {
        StatusCode::BAD_REQUEST => "https://cas-dummy.local/problems/malformed-request",
        StatusCode::UNAUTHORIZED => "https://cas-dummy.local/problems/unauthenticated",
        StatusCode::FORBIDDEN => "https://cas-dummy.local/problems/forbidden",
        StatusCode::NOT_FOUND => "https://cas-dummy.local/problems/not-found",
        StatusCode::CONFLICT => "https://cas-dummy.local/problems/conflict",
        StatusCode::UNPROCESSABLE_ENTITY => "https://cas-dummy.local/problems/validation",
        StatusCode::SERVICE_UNAVAILABLE => {
            "https://cas-dummy.local/problems/dependency-unavailable"
        }
        _ => "https://cas-dummy.local/problems/problem",
    }
}

pub fn problem(status: StatusCode, title: &'static str, detail: &'static str) -> Response {
    let body = Json(ProblemDetails {
        problem_type: problem_type(status),
        title,
        status: status.as_u16(),
        detail,
        instance: "/",
        request_id: uuid::Uuid::new_v4().to_string(),
    });
    if status == StatusCode::UNAUTHORIZED {
        return (
            status,
            [
                (header::CONTENT_TYPE, "application/problem+json"),
                (header::CACHE_CONTROL, "no-store"),
                (header::WWW_AUTHENTICATE, "Bearer"),
            ],
            body,
        )
            .into_response();
    }
    (
        status,
        [
            (header::CONTENT_TYPE, "application/problem+json"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        body,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::problem;
    use axum::body::to_bytes;
    use axum::http::{StatusCode, header};

    #[tokio::test]
    async fn problem_response_uses_status_and_safe_headers() {
        let response = problem(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Invalid request",
            "The supplied value is invalid",
        );
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "application/problem+json"
        );
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], 422);
        assert_eq!(json["detail"], "The supplied value is invalid");
        assert_eq!(json["instance"], "/");
        assert!(json["request_id"].as_str().is_some_and(|id| !id.is_empty()));
    }

    #[tokio::test]
    async fn unauthorized_problem_adds_bearer_challenge() {
        let response = problem(
            StatusCode::UNAUTHORIZED,
            "Unauthenticated",
            "Sign in required",
        );
        assert_eq!(response.headers()[header::WWW_AUTHENTICATE], "Bearer");
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "application/problem+json"
        );
    }

    #[tokio::test]
    async fn every_documented_problem_status_has_safe_problem_headers() {
        for (status, title) in [
            (StatusCode::BAD_REQUEST, "Malformed request"),
            (StatusCode::UNAUTHORIZED, "Unauthenticated"),
            (StatusCode::FORBIDDEN, "Forbidden"),
            (StatusCode::NOT_FOUND, "Not found"),
            (StatusCode::CONFLICT, "Conflict"),
            (StatusCode::UNPROCESSABLE_ENTITY, "Invalid request"),
            (StatusCode::SERVICE_UNAVAILABLE, "Dependency unavailable"),
        ] {
            let response = problem(status, title, "Safe public detail");
            assert_eq!(response.status(), status);
            assert_eq!(
                response.headers()[header::CONTENT_TYPE],
                "application/problem+json"
            );
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
            if status == StatusCode::UNAUTHORIZED {
                assert_eq!(response.headers()[header::WWW_AUTHENTICATE], "Bearer");
            } else {
                assert!(!response.headers().contains_key(header::WWW_AUTHENTICATE));
            }
            let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(json["status"], status.as_u16());
            assert_eq!(json["detail"], "Safe public detail");
            assert_eq!(json["instance"], "/");
            assert!(
                json["detail"]
                    .as_str()
                    .is_some_and(|detail| !detail.contains("SELECT"))
            );
        }
    }
}
