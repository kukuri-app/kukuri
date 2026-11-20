use anyhow::{Result, anyhow, bail};
use axum::http::{HeaderMap, StatusCode, header::AUTHORIZATION};
use chrono::{DateTime, Duration, Utc};
use jsonwebtoken::{Algorithm, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::Row;
use sqlx::postgres::PgPool;

use crate::admission::{AdmissionMode, evaluate_admission};
use crate::config::{
    AUTH_CHALLENGE_TTL_SECONDS, AUTH_EVENT_MAX_SKEW_SECONDS, JWT_CRYPTO_PROVIDER_INIT, JwtConfig,
};
use crate::database::ensure_active_subscriber;
use crate::errors::{ApiError, ApiResult, auth_required_error};
use kukuri_cn_protocol::AUTH_ENVELOPE_KIND;
use kukuri_cn_protocol::models::{
    AuthChallengeResponse, AuthVerifyResponse, BearerIdentity, CommunityNodeSeedPeer,
};
use kukuri_cn_protocol::normalize::{
    first_tag_value, normalize_http_url, normalize_pubkey, parse_auth_envelope,
    verify_auth_envelope,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
struct AccessTokenClaims {
    sub: String,
    iss: String,
    iat: usize,
    exp: usize,
    #[serde(default)]
    endpoint_id: Option<String>,
}

pub async fn create_auth_challenge(pool: &PgPool, pubkey: &str) -> Result<AuthChallengeResponse> {
    let pubkey = normalize_pubkey(pubkey)?;
    let challenge = uuid::Uuid::new_v4().to_string();
    let expires_at = Utc::now() + Duration::seconds(AUTH_CHALLENGE_TTL_SECONDS);
    // `/v1/auth/challenge` is unauthenticated, so without opportunistic cleanup the
    // table would grow without bound under challenge spam. The index on `expires_at`
    // keeps this DELETE cheap.
    prune_expired_auth_challenges(pool).await?;
    sqlx::query(
        "INSERT INTO cn_auth.auth_challenges (challenge, pubkey, expires_at)
         VALUES ($1, $2, $3)",
    )
    .bind(&challenge)
    .bind(&pubkey)
    .bind(expires_at)
    .execute(pool)
    .await?;
    Ok(AuthChallengeResponse {
        challenge,
        expires_at: expires_at.timestamp(),
    })
}

pub(crate) async fn prune_expired_auth_challenges<'e, E>(executor: E) -> Result<()>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    sqlx::query(
        "DELETE FROM cn_auth.auth_challenges
         WHERE expires_at <= NOW()",
    )
    .execute(executor)
    .await?;
    Ok(())
}

pub async fn verify_auth_envelope_and_issue_token(
    pool: &PgPool,
    jwt_config: &JwtConfig,
    public_base_url: &str,
    auth_envelope_json: &Value,
    endpoint_id: Option<&str>,
    admission_mode: AdmissionMode,
    invite_code: Option<&str>,
) -> Result<AuthVerifyResponse> {
    let public_base_url = normalize_http_url(public_base_url)?;
    let envelope = parse_auth_envelope(auth_envelope_json)?;
    verify_auth_envelope(&envelope)?;
    if envelope.kind != AUTH_ENVELOPE_KIND {
        bail!("auth envelope kind mismatch");
    }
    let now = Utc::now();
    if (now.timestamp() - envelope.created_at).abs() > AUTH_EVENT_MAX_SKEW_SECONDS {
        bail!("auth envelope is stale");
    }
    let capability_url = first_tag_value(&envelope, "capability_url")
        .ok_or_else(|| anyhow!("missing capability_url tag"))?;
    if capability_url != public_base_url {
        bail!("capability_url tag mismatch");
    }
    let challenge =
        first_tag_value(&envelope, "challenge").ok_or_else(|| anyhow!("missing challenge tag"))?;
    let row = sqlx::query(
        "SELECT pubkey, expires_at, used_at
         FROM cn_auth.auth_challenges
         WHERE challenge = $1",
    )
    .bind(challenge)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else {
        bail!("challenge not found");
    };
    let stored_pubkey: String = row.try_get("pubkey")?;
    let expires_at: DateTime<Utc> = row.try_get("expires_at")?;
    let used_at: Option<DateTime<Utc>> = row.try_get("used_at")?;
    if used_at.is_some() || Utc::now() > expires_at {
        bail!("challenge expired or already used");
    }
    let normalized_pubkey = normalize_pubkey(envelope.pubkey.as_str())?;
    if normalized_pubkey != stored_pubkey {
        bail!("pubkey mismatch");
    }
    let bound_endpoint_id = endpoint_id
        .map(|value| CommunityNodeSeedPeer::new(value, None))
        .transpose()?
        .map(|seed_peer| seed_peer.endpoint_id);

    let mut tx = pool.begin().await?;
    // admission を challenge / invite 消費の前に評価する。拒否時は tx を rollback し、
    // challenge も invite も消費しない（無効な試行で消費させない）。invite redeem が成功した
    // 場合は subscriber 作成・challenge 消費と同一 tx でコミットされ原子性を保つ。
    evaluate_admission(
        &mut tx,
        normalized_pubkey.as_str(),
        admission_mode,
        invite_code,
    )
    .await?;
    sqlx::query(
        "UPDATE cn_auth.auth_challenges
         SET used_at = NOW()
         WHERE challenge = $1",
    )
    .bind(challenge)
    .execute(&mut *tx)
    .await?;
    ensure_active_subscriber(&mut *tx, normalized_pubkey.as_str()).await?;
    tx.commit().await?;

    let (access_token, expires_at) = issue_access_token(
        jwt_config,
        normalized_pubkey.as_str(),
        bound_endpoint_id.as_deref(),
    )?;
    Ok(AuthVerifyResponse {
        access_token,
        token_type: "Bearer".to_string(),
        expires_at,
        pubkey: normalized_pubkey,
    })
}

pub async fn require_bearer_identity(
    pool: &PgPool,
    jwt_config: &JwtConfig,
    headers: &HeaderMap,
) -> ApiResult<BearerIdentity> {
    let header = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| auth_required_error("missing bearer token"))?;
    let token = header
        .strip_prefix("Bearer ")
        .ok_or_else(|| auth_required_error("invalid bearer token"))?;
    let claims = verify_access_token(jwt_config, token)
        .map_err(|error| auth_required_error(format!("invalid bearer token: {error}")))?;
    let pubkey = normalize_pubkey(claims.sub.as_str())
        .map_err(|error| auth_required_error(error.to_string()))?;
    let endpoint_id = claims
        .endpoint_id
        .as_deref()
        .map(|value| CommunityNodeSeedPeer::new(value, None))
        .transpose()
        .map_err(|error| auth_required_error(format!("invalid bearer token endpoint: {error}")))?
        .map(|seed_peer| seed_peer.endpoint_id);
    let active = sqlx::query_scalar::<_, String>(
        "SELECT status FROM cn_user.subscriber_accounts WHERE subscriber_pubkey = $1",
    )
    .bind(&pubkey)
    .fetch_optional(pool)
    .await
    .map_err(|error| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "DB_ERROR",
            error.to_string(),
        )
    })?;
    match active.as_deref() {
        Some("active") => Ok(BearerIdentity {
            pubkey,
            endpoint_id,
        }),
        // banned subscriber は既存トークンでも即時失効する（#383）。
        Some("banned") => Err(auth_required_error("subscriber is banned")),
        Some(_) => Err(auth_required_error("subscriber is not active")),
        None => Err(auth_required_error("subscriber is not registered")),
    }
}

pub async fn require_bearer_pubkey(
    pool: &PgPool,
    jwt_config: &JwtConfig,
    headers: &HeaderMap,
) -> ApiResult<String> {
    Ok(require_bearer_identity(pool, jwt_config, headers)
        .await?
        .pubkey)
}

fn ensure_jwt_crypto_provider() {
    JWT_CRYPTO_PROVIDER_INIT.call_once(|| {
        let _ = jsonwebtoken::crypto::aws_lc::DEFAULT_PROVIDER.install_default();
    });
}

fn issue_access_token(
    jwt_config: &JwtConfig,
    pubkey: &str,
    endpoint_id: Option<&str>,
) -> Result<(String, i64)> {
    ensure_jwt_crypto_provider();
    let issued_at = Utc::now().timestamp();
    let expires_at = issued_at + jwt_config.ttl_seconds();
    let claims = AccessTokenClaims {
        sub: pubkey.to_string(),
        iss: jwt_config.issuer().to_string(),
        iat: issued_at as usize,
        exp: expires_at as usize,
        endpoint_id: endpoint_id.map(str::to_string),
    };
    let token = encode(&Header::default(), &claims, &jwt_config.encoding_key()?)?;
    Ok((token, expires_at))
}

fn verify_access_token(jwt_config: &JwtConfig, token: &str) -> Result<AccessTokenClaims> {
    ensure_jwt_crypto_provider();
    let mut validation = Validation::new(Algorithm::HS256);
    validation.set_issuer(&[jwt_config.issuer()]);
    let decoded = decode::<AccessTokenClaims>(token, &jwt_config.decoding_key()?, &validation)?;
    Ok(decoded.claims)
}
