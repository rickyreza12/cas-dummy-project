use axum::{Json, response::Html};
use serde_json::{Value, json};

pub async fn openapi() -> Json<Value> {
    Json(json!({
        "openapi": "3.0.3",
        "info": {
            "title": "CAS Dummy Patient Context API",
            "version": env!("CARGO_PKG_VERSION"),
            "description": "Synthetic Doctor-360 patient-context backend. Protected routes require an RS256 bearer token."
        },
        "servers": [{"url": "/"}],
        "tags": [{"name": "system"}, {"name": "auth"}, {"name": "patients"}, {"name": "clinical"}, {"name": "queue"}, {"name": "summaries"}, {"name": "imports"}],
        "components": {
            "securitySchemes": {"bearerAuth": {"type": "http", "scheme": "bearer", "bearerFormat": "JWT"}},
            "schemas": {
                "TokenRequest": {"type": "object", "required": ["subject", "role"], "properties": {"subject": {"type": "string", "example": "demo-doctor-01"}, "role": {"type": "string", "enum": ["doctor", "nurse", "admin", "medical_records"], "example": "doctor"}, "scope": {"type": "array", "items": {"type": "string"}, "example": []}}},
                "TokenResponse": {"type": "object", "properties": {"access_token": {"type": "string"}, "token_type": {"type": "string", "example": "Bearer"}, "expires_in": {"type": "integer"}}}
            }
        },
        "paths": {
            "/healthz": {"get": {"tags": ["system"], "summary": "Health check", "responses": {"200": {"description": "Service is healthy"}}}},
            "/openapi.json": {"get": {"tags": ["system"], "summary": "OpenAPI contract", "responses": {"200": {"description": "OpenAPI document"}}}},
            "/swagger": {"get": {"tags": ["system"], "summary": "Swagger UI", "responses": {"200": {"description": "Swagger UI"}}}},
            "/v1/auth/token": {"post": {"tags": ["auth"], "summary": "Issue development JWT", "requestBody": {"required": true, "content": {"application/json": {"schema": {"$ref": "#/components/schemas/TokenRequest"}}}}, "responses": {"200": {"description": "JWT response", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/TokenResponse"}}}}, "400": {"description": "Malformed JSON"}, "422": {"description": "Invalid token request"}, "503": {"description": "Signing key unavailable"}}}},
            "/v1/patients": {"get": protected("patients", "Search patients")},
            "/v1/patients/{id}": {"get": protected("patients", "Read patient")},
            "/v1/patients/{id}/encounters": {"get": protected("clinical", "List patient encounters")},
            "/v1/patients/{id}/context": {"get": protected("clinical", "Read patient context")},
            "/v1/encounters/{id}": {"get": protected("clinical", "Read encounter")},
            "/v1/sources/{id}": {"get": protected("clinical", "Read source record")},
            "/v1/queue-items": {"post": protected("queue", "Assign queue item")},
            "/v1/queue-items/{id}": {"get": protected("queue", "Read queue item")},
            "/v1/doctors/me/queue": {"get": protected("queue", "List doctor queue")},
            "/v1/encounters/{id}/summary-jobs": {"post": protected("summaries", "Create summary job")},
            "/v1/summary-jobs/{id}": {"get": protected("summaries", "Poll summary job")},
            "/v1/summaries/{id}": {"get": protected("summaries", "Read summary")},
            "/v1/summaries/{id}/feedback": {"post": protected("summaries", "Submit summary feedback")},
            "/v1/feedback/{id}": {"get": protected("summaries", "Read feedback")},
            "/v1/import-jobs": {"post": protected("imports", "Create import job")},
            "/v1/import-jobs/{id}": {"get": protected("imports", "Poll import job")}
        }
    }))
}

fn protected(tag: &str, summary: &str) -> Value {
    json!({"tags": [tag], "summary": summary, "security": [{"bearerAuth": []}], "responses": {"200": {"description": "Successful response"}, "401": {"description": "Unauthorized"}, "404": {"description": "Not found"}, "503": {"description": "Dependency unavailable"}}})
}

pub async fn ui() -> Html<&'static str> {
    Html(
        r#"<!doctype html><html><head><title>CAS Dummy API Swagger UI</title><link rel="stylesheet" href="https://unpkg.com/swagger-ui-dist@5/swagger-ui.css"></head><body><div id="swagger-ui"></div><script src="https://unpkg.com/swagger-ui-dist@5/swagger-ui-bundle.js"></script><script>window.onload=()=>SwaggerUIBundle({url:'/openapi.json',dom_id:'#swagger-ui',deepLinking:true});</script></body></html>"#,
    )
}

#[cfg(test)]
mod tests {
    use super::openapi;

    #[tokio::test]
    async fn contract_lists_public_docs_and_protected_api_routes() {
        let document = openapi().await.0;
        assert_eq!(document["openapi"], "3.0.3");
        assert!(
            document["paths"]["/v1/patients"]["get"]["security"]
                .as_array()
                .is_some()
        );
        assert!(document["paths"]["/swagger"]["get"].is_object());
        assert!(document["components"]["securitySchemes"]["bearerAuth"].is_object());
    }
}
