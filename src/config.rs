use anyhow::{Result, bail};
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::time::Duration;

#[derive(Clone)]
pub struct Config {
    pub postgres_url: String,
    pub jwt_issuer: String,
    pub jwt_audience: String,
    pub jwt_private_key: String,
    pub jwt_public_key: String,
    pub jwt_key_id: String,
    pub jwt_previous_key_id: Option<String>,
    pub jwt_previous_public_key: Option<String>,
    pub jwt_access_token_ttl_seconds: i64,
    pub cursor_signing_key: String,
    pub fixture_dir: String,
    pub worker_concurrency: u32,
    pub pool_max_connections: u32,
    pub api_bind_addr: String,
    pub api_public_base_url: String,
    pub active_dataset_id: String,
    pub summary_lease_seconds: u64,
    pub summary_max_attempts: i32,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let postgres_url = std::env::var("POSTGRES_URL")
            .or_else(|_| std::env::var("DATABASE_URL"))
            .map_err(|_| {
                anyhow::anyhow!("missing required configuration: POSTGRES_URL or DATABASE_URL")
            })?;
        let postgres_url = normalize_postgres_url(postgres_url);
        let jwt_issuer = required("JWT_ISSUER")?;
        let jwt_audience = required("JWT_AUDIENCE")?;
        let jwt_private_key = pem_env("JWT_PRIVATE_KEY");
        let jwt_public_key = pem_env("JWT_PUBLIC_KEY");
        let jwt_key_id = std::env::var("JWT_KEY_ID").unwrap_or_else(|_| "local-current".into());
        if jwt_key_id.trim().is_empty() {
            bail!("JWT_KEY_ID must not be empty");
        }
        let jwt_previous_key_id = optional("JWT_PREVIOUS_KEY_ID");
        let jwt_previous_public_key = optional("JWT_PREVIOUS_PUBLIC_KEY");
        if jwt_previous_key_id.is_some() != jwt_previous_public_key.is_some() {
            bail!("JWT_PREVIOUS_KEY_ID and JWT_PREVIOUS_PUBLIC_KEY must be supplied together");
        }
        let cursor_signing_key = required("CURSOR_SIGNING_KEY")?;
        if cursor_signing_key.len() < 32 {
            bail!("CURSOR_SIGNING_KEY must contain at least 32 bytes");
        }
        let fixture_dir = std::env::var("FIXTURE_DIR").unwrap_or_else(|_| "fixtures".into());
        let worker_concurrency = bounded_u32("WORKER_CONCURRENCY", 1, 4)?;
        let pool_max_connections = std::env::var("DATABASE_POOL_MAX")
            .or_else(|_| std::env::var("POOL_MAX_CONNECTIONS"))
            .unwrap_or_else(|_| "5".into());
        let pool_max_connections = parse_positive_u32("DATABASE_POOL_MAX", &pool_max_connections)?;
        if !(1..=20).contains(&pool_max_connections) {
            bail!("DATABASE_POOL_MAX must be between 1 and 20");
        }
        let api_bind_addr =
            std::env::var("API_BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:8000".into());
        api_bind_addr
            .parse::<std::net::SocketAddr>()
            .map_err(|_| anyhow::anyhow!("API_BIND_ADDR must be a valid socket address"))?;
        let api_public_base_url =
            std::env::var("API_PUBLIC_BASE_URL").unwrap_or_else(|_| "http://localhost:8000".into());
        let public_uri = api_public_base_url
            .parse::<axum::http::Uri>()
            .map_err(|_| anyhow::anyhow!("API_PUBLIC_BASE_URL must be a valid URL"))?;
        if public_uri.scheme().is_none() || public_uri.authority().is_none() {
            bail!("API_PUBLIC_BASE_URL must include scheme and host");
        }
        let active_dataset_id =
            std::env::var("ACTIVE_DATASET_ID").unwrap_or_else(|_| "demo-1000000-v1".into());
        if !matches!(
            active_dataset_id.as_str(),
            "demo-100-v1" | "demo-1000-v1" | "demo-1000000-v1"
        ) {
            bail!("ACTIVE_DATASET_ID is not allowlisted");
        }
        let summary_lease_seconds = positive_u32("SUMMARY_LEASE_SECONDS", 60)? as u64;
        let summary_max_attempts = positive_u32("SUMMARY_MAX_ATTEMPTS", 3)? as i32;
        let ttl = std::env::var("JWT_ACCESS_TOKEN_TTL_SECONDS")
            .unwrap_or_else(|_| "900".into())
            .parse::<i64>()?;
        if ttl <= 0 {
            bail!("JWT_ACCESS_TOKEN_TTL_SECONDS must be positive");
        }
        Ok(Self {
            postgres_url,
            jwt_issuer,
            jwt_audience,
            jwt_private_key,
            jwt_public_key,
            jwt_key_id,
            jwt_previous_key_id,
            jwt_previous_public_key,
            jwt_access_token_ttl_seconds: ttl,
            cursor_signing_key,
            fixture_dir,
            worker_concurrency,
            pool_max_connections,
            api_bind_addr,
            api_public_base_url,
            active_dataset_id,
            summary_lease_seconds,
            summary_max_attempts,
        })
    }
}

fn positive_u32(name: &str, default: u32) -> Result<u32> {
    let value = std::env::var(name).unwrap_or_else(|_| default.to_string());
    parse_positive_u32(name, &value)
}

fn bounded_u32(name: &str, default: u32, maximum: u32) -> Result<u32> {
    let value = positive_u32(name, default)?;
    if value > maximum {
        bail!("{name} must be between 1 and {maximum}");
    }
    Ok(value)
}

fn parse_positive_u32(name: &str, value: &str) -> Result<u32> {
    let parsed = value
        .parse::<u32>()
        .map_err(|_| anyhow::anyhow!("{name} must be a positive integer"))?;
    if parsed == 0 {
        bail!("{name} must be a positive integer");
    }
    Ok(parsed)
}

pub async fn new_pool(config: &Config, max_connections: u32) -> Result<PgPool> {
    if max_connections == 0 {
        bail!("database pool size must be positive");
    }
    let pool = tokio::time::timeout(
        Duration::from_secs(10),
        PgPoolOptions::new()
            .max_connections(max_connections)
            .acquire_timeout(Duration::from_secs(10))
            .idle_timeout(Duration::from_secs(300))
            .max_lifetime(Duration::from_secs(1800))
            .connect(&config.postgres_url),
    )
    .await
    .map_err(|_| anyhow::anyhow!("database connection timed out"))??;
    sqlx::query("SELECT 1").execute(&pool).await?;
    Ok(pool)
}

fn required(name: &str) -> Result<String> {
    let value = std::env::var(name).unwrap_or_default();
    if value.trim().is_empty() {
        bail!("missing required configuration: {name}");
    }
    Ok(value)
}

fn optional(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

fn pem_env(name: &str) -> String {
    std::env::var(name)
        .unwrap_or_default()
        .replace("\\n", "\n")
}

/// Neon pooler endpoints infer the project from their hostname. Supplying a
/// separate `options=project=...` parameter can make the TLS/SNI project and
/// requested project disagree, so discard only that parameter for pooler URLs.
fn normalize_postgres_url(postgres_url: String) -> String {
    if !postgres_url.contains("-pooler.") {
        return postgres_url;
    }
    let Some((base, query)) = postgres_url.split_once('?') else {
        return postgres_url;
    };
    let query = query
        .split('&')
        .filter(|parameter| !parameter.to_ascii_lowercase().starts_with("options="))
        .collect::<Vec<_>>();
    if query.is_empty() {
        base.to_owned()
    } else {
        format!("{base}?{}", query.join("&"))
    }
}

#[cfg(test)]
mod tests {
    use super::{bounded_u32, normalize_postgres_url, parse_positive_u32, positive_u32};

    #[test]
    fn positive_pool_defaults_are_valid() {
        assert_eq!(positive_u32("TEST_UNUSED", 5).unwrap(), 5);
    }

    #[test]
    fn zero_pool_size_is_rejected() {
        assert!(parse_positive_u32("TEST_POOL_SIZE", "0").is_err());
    }

    #[test]
    fn malformed_pool_size_is_rejected() {
        assert!(parse_positive_u32("TEST_POOL_SIZE", "not-a-number").is_err());
    }

    #[test]
    fn worker_concurrency_is_bounded() {
        assert_eq!(bounded_u32("TEST_WORKERS", 1, 4).unwrap(), 1);
        assert!(parse_positive_u32("TEST_WORKERS", "0").is_err());
    }

    #[test]
    fn pooler_url_drops_conflicting_options_but_keeps_other_parameters() {
        let normalized = normalize_postgres_url(
            "postgres://user:password@project-pooler.example.test/db?sslmode=require&options=project%3Dproject&channel_binding=require".into(),
        );
        assert_eq!(
            normalized,
            "postgres://user:password@project-pooler.example.test/db?sslmode=require&channel_binding=require"
        );
    }

    #[test]
    fn direct_url_keeps_options_parameter() {
        let original = "postgres://user:password@project.example.test/db?options=project%3Dproject";
        assert_eq!(normalize_postgres_url(original.into()), original);
    }
}
