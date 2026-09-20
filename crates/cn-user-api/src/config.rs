//! user-api の起動設定(env からの読込)。

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result};
use kukuri_cn_core::{
    COMMUNITY_NODE_RENDEZVOUS_KEY_PREFIX_ENV, COMMUNITY_NODE_RENDEZVOUS_REDIS_URL_ENV, JwtConfig,
    parse_bool_env, parse_csv_env, parse_u32_env, parse_u64_env,
};
use kukuri_cn_protocol::{normalize_http_url, normalize_http_url_list};

pub const RELATION_DISTANCE_OPTOUT_MIN_PROXIMITY_ENV: &str =
    "COMMUNITY_NODE_RELATION_DISTANCE_OPTOUT_MIN_PROXIMITY";

#[derive(Clone)]
pub struct UserApiConfig {
    pub bind_addr: SocketAddr,
    pub database_url: String,
    pub rendezvous_redis_url: String,
    pub rendezvous_key_prefix: String,
    pub base_url: String,
    pub public_base_url: String,
    pub connectivity_urls: Vec<String>,
    pub jwt_config: JwtConfig,
    /// 公開 manifest を生成する operator-config.yaml のパス。
    /// 未設定なら manifest endpoint は 404 を返す(client は別 node / 直接 P2P へ fallback)。
    pub operator_config_path: Option<PathBuf>,
    /// private channel の indexing request で渡される channel secret を at-rest 暗号化する鍵 material。
    /// 未設定なら private channel の indexing request は受け付けない(#413 / ADR 0025 §6.3)。
    pub channel_secret_key: Option<String>,
    /// 通報・権利案件の連絡先、本人確認、証拠参照を at-rest 暗号化する用途分離鍵。
    pub legal_data_key: Option<String>,
    /// ユーザー向け index query(search / discovery / recommendation)を公開するか(#404)。
    /// 既定 false。capability は Available だが、readiness activation 完了までは
    /// `/v1/index/*` を公開しない。
    /// 有効化すると ArcadeDB(`COMMUNITY_NODE_ARCADEDB_*`)へ接続する。
    pub index_query_enabled: bool,
    /// trust / relation read surface を公開するか(#415)。
    /// 既定 false。capability は Available だが、readiness activation 完了までは
    /// `/v1/trust/*` / `/v1/relation/*` は 404。有効化すると relation graph の
    /// ArcadeDB(`COMMUNITY_NODE_ARCADEDB_*`)へ接続し、trust パラメータを
    /// `COMMUNITY_NODE_TRUST_*` env から読む。
    pub trust_read_enabled: bool,
    /// distance opt-out が「遠い」と判定する node-local proximity 境界。
    /// index / trust surface のどちらかを有効にする場合は明示設定が必須。
    pub relation_distance_optout_min_proximity: Option<f64>,
    /// image/config/provider一式を識別する非秘匿のdeploy revision。
    pub deployment_revision: String,
    /// readiness activationを有効とみなす最大経過秒数。
    pub readiness_activation_max_age_secs: u64,
    /// この配備がリスク判定に載せる `issuer_node_id`(署名鍵の公開鍵 hex、または明示指定)。
    /// 公開ノード情報の `node_id` と一致しなければ起動を拒否する(#706)。
    /// 署名鍵も明示指定も無い構成では `None`(検査を省略)。
    pub expected_issuer_node_id: Option<String>,
}

impl std::fmt::Debug for UserApiConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // channel_secret_key(at-rest 暗号鍵)を Debug 出力に含めない。
        f.debug_struct("UserApiConfig")
            .field("bind_addr", &self.bind_addr)
            .field("database_url", &self.database_url)
            .field("rendezvous_redis_url", &self.rendezvous_redis_url)
            .field("rendezvous_key_prefix", &self.rendezvous_key_prefix)
            .field("base_url", &self.base_url)
            .field("public_base_url", &self.public_base_url)
            .field("connectivity_urls", &self.connectivity_urls)
            .field("jwt_config", &self.jwt_config)
            .field("operator_config_path", &self.operator_config_path)
            .field(
                "channel_secret_key",
                &self.channel_secret_key.as_ref().map(|_| "<redacted>"),
            )
            .field(
                "legal_data_key",
                &self.legal_data_key.as_ref().map(|_| "<redacted>"),
            )
            .field("index_query_enabled", &self.index_query_enabled)
            .field("trust_read_enabled", &self.trust_read_enabled)
            .field(
                "relation_distance_optout_min_proximity",
                &self.relation_distance_optout_min_proximity,
            )
            .field("deployment_revision", &self.deployment_revision)
            .field(
                "readiness_activation_max_age_secs",
                &self.readiness_activation_max_age_secs,
            )
            .field("expected_issuer_node_id", &self.expected_issuer_node_id)
            .finish()
    }
}

impl UserApiConfig {
    pub fn from_env() -> Result<Self> {
        let bind_addr = std::env::var("COMMUNITY_NODE_BIND_ADDR")
            .unwrap_or_else(|_| "127.0.0.1:8080".to_string())
            .parse::<SocketAddr>()
            .context("failed to parse COMMUNITY_NODE_BIND_ADDR")?;
        let database_url = std::env::var("COMMUNITY_NODE_DATABASE_URL")
            .context("COMMUNITY_NODE_DATABASE_URL is required")?;
        let rendezvous_redis_url = std::env::var(COMMUNITY_NODE_RENDEZVOUS_REDIS_URL_ENV)
            .with_context(|| format!("{COMMUNITY_NODE_RENDEZVOUS_REDIS_URL_ENV} is required"))?;
        let rendezvous_key_prefix = std::env::var(COMMUNITY_NODE_RENDEZVOUS_KEY_PREFIX_ENV)
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "cn:rendezvous:v1".to_string());
        let base_url = normalize_http_url(
            std::env::var("COMMUNITY_NODE_BASE_URL")
                .context("COMMUNITY_NODE_BASE_URL is required")?
                .as_str(),
        )?;
        let public_base_url = normalize_http_url(
            std::env::var("COMMUNITY_NODE_PUBLIC_BASE_URL")
                .ok()
                .as_deref()
                .unwrap_or(base_url.as_str()),
        )?;
        let connectivity_urls =
            normalize_http_url_list(parse_csv_env("COMMUNITY_NODE_CONNECTIVITY_URLS"))?;
        let operator_config_path = std::env::var("COMMUNITY_NODE_OPERATOR_CONFIG")
            .context("COMMUNITY_NODE_OPERATOR_CONFIG is required")?;
        if operator_config_path.trim().is_empty() {
            anyhow::bail!("COMMUNITY_NODE_OPERATOR_CONFIG must not be empty");
        }
        let operator_config_path = Some(PathBuf::from(operator_config_path));
        let channel_secret_key = std::env::var("COMMUNITY_NODE_CHANNEL_SECRET_KEY")
            .ok()
            .filter(|value| !value.trim().is_empty());
        let legal_data_key = std::env::var("COMMUNITY_NODE_LEGAL_DATA_KEY")
            .ok()
            .filter(|value| !value.trim().is_empty());
        let index_query_enabled = parse_bool_env("COMMUNITY_NODE_INDEX_QUERY_ENABLED", false)?;
        let trust_read_enabled = parse_bool_env("COMMUNITY_NODE_TRUST_READ_ENABLED", false)?;
        let relation_distance_optout_min_proximity = parse_relation_distance_optout_min_proximity(
            std::env::var(RELATION_DISTANCE_OPTOUT_MIN_PROXIMITY_ENV)
                .ok()
                .as_deref(),
            index_query_enabled || trust_read_enabled,
        )?;
        let deployment_revision = std::env::var("COMMUNITY_NODE_DEPLOYMENT_REVISION")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_default();
        if (index_query_enabled || trust_read_enabled) && deployment_revision.is_empty() {
            anyhow::bail!(
                "COMMUNITY_NODE_DEPLOYMENT_REVISION is required when index/trust surfaces are enabled"
            );
        }
        let readiness_activation_max_age_secs =
            parse_u64_env("COMMUNITY_NODE_READINESS_ACTIVATION_MAX_AGE_SECS", 900)?.max(1);
        // 署名鍵(または明示識別子)は cn-indexer と同じ env_file で届く。値そのものは保持せず、
        // 公開鍵 hex(発行元識別子)だけを設定に残す。
        let expected_issuer_node_id = kukuri_cn_safety_runtime::expected_issuer_node_id_from_env()
            .context("invalid COMMUNITY_NODE_SAFETY_SIGNING_KEY")?;
        Ok(Self {
            bind_addr,
            database_url,
            rendezvous_redis_url,
            rendezvous_key_prefix,
            base_url,
            public_base_url,
            connectivity_urls,
            jwt_config: JwtConfig::from_env()?,
            operator_config_path,
            channel_secret_key,
            legal_data_key,
            index_query_enabled,
            trust_read_enabled,
            relation_distance_optout_min_proximity,
            deployment_revision,
            readiness_activation_max_age_secs,
            expected_issuer_node_id,
        })
    }
}

fn parse_relation_distance_optout_min_proximity(
    raw: Option<&str>,
    required: bool,
) -> Result<Option<f64>> {
    let raw = raw.map(str::trim).filter(|value| !value.is_empty());
    let Some(raw) = raw else {
        if required {
            anyhow::bail!(
                "{RELATION_DISTANCE_OPTOUT_MIN_PROXIMITY_ENV} is required when index/trust surfaces are enabled"
            );
        }
        return Ok(None);
    };
    let value: f64 = raw.parse().with_context(|| {
        format!("{RELATION_DISTANCE_OPTOUT_MIN_PROXIMITY_ENV} must be a number within (0, 1]")
    })?;
    if !value.is_finite() || value <= 0.0 || value > 1.0 {
        anyhow::bail!(
            "{RELATION_DISTANCE_OPTOUT_MIN_PROXIMITY_ENV} must be within (0, 1] (got {raw})"
        );
    }
    Ok(Some(value))
}

/// Optional per-client rate limit for the public HTTP surface.
///
/// Disabled by default in code so unit/contract tests and library embeddings are
/// never throttled; the shipped `.env.community-node.example` turns it on. Behind a
/// trusted reverse proxy set `trust_forwarded_for` so each real client is limited
/// individually instead of sharing the proxy's connection IP. Leave it `false` when
/// the API is directly exposed, since `X-Forwarded-For` is attacker-controlled there.
#[derive(Clone, Copy, Debug)]
pub struct RateLimitConfig {
    pub enabled: bool,
    pub per_second: u64,
    pub burst: u32,
    pub trust_forwarded_for: bool,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            per_second: 10,
            burst: 30,
            trust_forwarded_for: false,
        }
    }
}

impl RateLimitConfig {
    pub fn from_env() -> Result<Self> {
        let defaults = Self::default();
        Ok(Self {
            enabled: parse_bool_env("COMMUNITY_NODE_RATE_LIMIT_ENABLED", defaults.enabled)?,
            per_second: parse_u64_env("COMMUNITY_NODE_RATE_LIMIT_PER_SECOND", defaults.per_second)?
                .max(1),
            burst: parse_u32_env("COMMUNITY_NODE_RATE_LIMIT_BURST", defaults.burst)?.max(1),
            trust_forwarded_for: parse_bool_env(
                "COMMUNITY_NODE_RATE_LIMIT_TRUST_FORWARDED_FOR",
                defaults.trust_forwarded_for,
            )?,
        })
    }

    pub(crate) fn request_policy(&self) -> kukuri_transport::RequestRatePolicy {
        kukuri_transport::RequestRatePolicy {
            limit: self.per_second,
            window: std::time::Duration::from_secs(1),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_relation_distance_optout_min_proximity;

    #[test]
    fn distance_optout_policy_is_required_for_read_surfaces() {
        assert!(parse_relation_distance_optout_min_proximity(None, true).is_err());
        assert!(parse_relation_distance_optout_min_proximity(Some("0"), true).is_err());
        assert!(parse_relation_distance_optout_min_proximity(Some("1.1"), true).is_err());
        assert!(parse_relation_distance_optout_min_proximity(Some("NaN"), true).is_err());
        assert_eq!(
            parse_relation_distance_optout_min_proximity(Some("0.5"), true).unwrap(),
            Some(0.5)
        );
        assert_eq!(
            parse_relation_distance_optout_min_proximity(None, false).unwrap(),
            None
        );
    }
}
