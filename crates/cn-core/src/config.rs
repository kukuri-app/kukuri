use std::sync::Once;

use anyhow::{Context, Result, anyhow, bail};
use jsonwebtoken::{DecodingKey, EncodingKey};
use serde::{Deserialize, Serialize};

pub const AUTH_CHALLENGE_TTL_SECONDS: i64 = 300;
pub const AUTH_EVENT_MAX_SKEW_SECONDS: i64 = 600;
pub const DEFAULT_TOKEN_TTL_SECONDS: i64 = 86_400;
pub const MIN_JWT_SECRET_BYTES: usize = 32;
pub const BOOTSTRAP_PEER_REGISTRATION_TTL_SECONDS: i64 = 90;
pub const TOPIC_RENDEZVOUS_TTL_SECONDS: u64 = 45;
pub const COMMUNITY_NODE_RENDEZVOUS_REDIS_URL_ENV: &str = "COMMUNITY_NODE_RENDEZVOUS_REDIS_URL";
pub const COMMUNITY_NODE_RENDEZVOUS_KEY_PREFIX_ENV: &str = "COMMUNITY_NODE_RENDEZVOUS_KEY_PREFIX";
pub const COMMUNITY_NODE_AUTH_SERVICE_NAME: &str = "community_node_auth";
pub const COMMUNITY_NODE_ADMISSION_SERVICE_NAME: &str = "community_node_admission";
pub const USER_API_BEARER_CHALLENGE: &str = r#"Bearer realm="cn-user-api""#;
pub const COMMUNITY_NODE_DATABASE_INIT_MODE_ENV: &str = "COMMUNITY_NODE_DATABASE_INIT_MODE";
pub(crate) const DATABASE_PREPARE_HINT: &str =
    "run `cn-cli --database-url <url> prepare` before starting cn-user-api";
pub(crate) static JWT_CRYPTO_PROVIDER_INIT: Once = Once::new();

#[derive(Clone, Debug)]
pub struct JwtConfig {
    issuer: String,
    secret: String,
    ttl_seconds: i64,
}

impl JwtConfig {
    pub fn new(issuer: impl Into<String>, secret: impl Into<String>, ttl_seconds: i64) -> Self {
        Self {
            issuer: issuer.into(),
            secret: secret.into(),
            ttl_seconds: ttl_seconds.max(60),
        }
    }

    pub fn from_env() -> Result<Self> {
        let issuer = std::env::var("COMMUNITY_NODE_JWT_ISSUER")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "kukuri-cn".to_string());
        let secret = std::env::var("COMMUNITY_NODE_JWT_SECRET")
            .context("COMMUNITY_NODE_JWT_SECRET is required")?;
        validate_jwt_secret(secret.as_str())?;
        let ttl_seconds = std::env::var("COMMUNITY_NODE_JWT_TTL_SECONDS")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(|value| value.parse::<i64>())
            .transpose()
            .context("failed to parse COMMUNITY_NODE_JWT_TTL_SECONDS")?
            .unwrap_or(DEFAULT_TOKEN_TTL_SECONDS);
        Ok(Self::new(issuer, secret, ttl_seconds))
    }

    pub(crate) fn issuer(&self) -> &str {
        &self.issuer
    }

    pub(crate) fn ttl_seconds(&self) -> i64 {
        self.ttl_seconds
    }

    pub(crate) fn encoding_key(&self) -> jsonwebtoken::errors::Result<EncodingKey> {
        EncodingKey::from_secret(self.secret.as_bytes())
    }

    pub(crate) fn decoding_key(&self) -> jsonwebtoken::errors::Result<DecodingKey> {
        DecodingKey::from_secret(self.secret.as_bytes())
    }
}

/// Reject weak or placeholder JWT secrets at startup so a community node never
/// runs with a brute-forceable or sample signing key.
pub(crate) fn validate_jwt_secret(secret: &str) -> Result<()> {
    if secret.len() < MIN_JWT_SECRET_BYTES {
        bail!(
            "COMMUNITY_NODE_JWT_SECRET must be at least {MIN_JWT_SECRET_BYTES} bytes; got {}",
            secret.len()
        );
    }
    if kukuri_core::is_placeholder_secret(secret) {
        bail!(
            "COMMUNITY_NODE_JWT_SECRET still contains a placeholder value; set a strong unique secret"
        );
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DatabaseInitMode {
    RequireReady,
    Prepare,
}

impl DatabaseInitMode {
    pub fn from_env() -> Result<Self> {
        match std::env::var(COMMUNITY_NODE_DATABASE_INIT_MODE_ENV) {
            Ok(value) => Self::parse(value.as_str()),
            Err(std::env::VarError::NotPresent) => Ok(Self::RequireReady),
            Err(error) => Err(anyhow!("{COMMUNITY_NODE_DATABASE_INIT_MODE_ENV}: {error}")),
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        match value.trim() {
            "" | "require_ready" => Ok(Self::RequireReady),
            "prepare" => Ok(Self::Prepare),
            other => bail!(
                "unsupported {COMMUNITY_NODE_DATABASE_INIT_MODE_ENV} `{other}`: expected `require_ready` or `prepare`"
            ),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    Off,
    Required,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthRolloutConfig {
    pub mode: AuthMode,
    pub enforce_at: Option<i64>,
    pub grace_seconds: i64,
    pub ws_auth_timeout_seconds: i64,
}

impl Default for AuthRolloutConfig {
    fn default() -> Self {
        Self {
            mode: AuthMode::Off,
            enforce_at: None,
            grace_seconds: 900,
            ws_auth_timeout_seconds: 10,
        }
    }
}

impl AuthRolloutConfig {
    pub fn requires_auth(&self, now: i64) -> bool {
        match self.mode {
            AuthMode::Off => false,
            AuthMode::Required => self.enforce_at.map(|ts| now >= ts).unwrap_or(true),
        }
    }

    pub fn disconnect_deadline_for_connection(&self, connected_at: i64) -> Option<i64> {
        if self.mode != AuthMode::Required {
            return None;
        }
        let enforce_at = self.enforce_at?;
        if connected_at >= enforce_at {
            return None;
        }
        enforce_at.checked_add(self.grace_seconds.max(0))
    }
}
