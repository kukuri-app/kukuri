//! contract 系テストの共有ヘルパ(WP-H4 で contract.rs から抽出)。
//! 各テストバイナリは使う部分だけ参照するため、未使用警告は許容する。
#![allow(dead_code)]

use std::net::SocketAddr;

use anyhow::{Context, Result};
use kukuri_cn_core::{JwtConfig, TestDatabase};
use kukuri_cn_protocol::{
    AcceptConsentsRequest, CommunityNodePoliciesResponse, build_auth_envelope_json,
};
use kukuri_cn_user_api::{UserApiConfig, UserApiState, app_router, build_state};
use kukuri_core::KukuriKeys;
use redis::AsyncCommands;
use reqwest::{Client, StatusCode};
use sqlx::postgres::PgPool;

pub const DEFAULT_ADMIN_DATABASE_URL: &str = "postgres://cn:cn_password@127.0.0.1:15432/cn";
pub const DEFAULT_RENDEZVOUS_REDIS_URL: &str = "redis://127.0.0.1:16379/";

pub struct TestServer {
    pub task: tokio::task::JoinHandle<()>,
    pub database: TestDatabase,
    pub base_url: String,
    pub rendezvous_redis_url: String,
    pub rendezvous_key_prefix: String,
}

impl TestServer {
    pub async fn spawn(admin_database_url: &str, prefix: &str) -> Result<Self> {
        Self::spawn_with_operator_config(
            admin_database_url,
            prefix,
            kukuri_cn_operator::SAMPLE_CONFIG,
        )
        .await
    }

    pub async fn spawn_with_operator_config(
        admin_database_url: &str,
        prefix: &str,
        operator_config: &str,
    ) -> Result<Self> {
        Self::spawn_with_state(admin_database_url, prefix, operator_config, |state| state).await
    }

    /// 組み立てた state を `customize` で差し替えてから起動する（機能の差し込み）。
    pub async fn spawn_with_state(
        admin_database_url: &str,
        prefix: &str,
        operator_config: &str,
        customize: impl FnOnce(UserApiState) -> UserApiState,
    ) -> Result<Self> {
        let database = TestDatabase::create(admin_database_url, prefix).await?;
        Self::spawn_on(database, prefix, operator_config, customize).await
    }

    /// 用意した DB で起動する（起動の前に置いた行を確かめるため）。
    pub async fn spawn_on(
        database: TestDatabase,
        prefix: &str,
        operator_config: &str,
        customize: impl FnOnce(UserApiState) -> UserApiState,
    ) -> Result<Self> {
        let rendezvous_redis_url = integration_test_rendezvous_redis_url();
        let rendezvous_key_prefix = format!("cn:test:{prefix}");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .context("failed to bind test user-api listener")?;
        let addr = listener.local_addr()?;
        let base_url = format!("http://{addr}");
        let operator_config_dir = tempfile::tempdir()?;
        let operator_config_path = operator_config_dir.path().join("operator-config.yaml");
        std::fs::write(&operator_config_path, operator_config.as_bytes())?;
        let state = build_state(&UserApiConfig {
            bind_addr: addr,
            database_url: database.database_url.clone(),
            rendezvous_redis_url: rendezvous_redis_url.clone(),
            rendezvous_key_prefix: rendezvous_key_prefix.clone(),
            base_url: base_url.clone(),
            public_base_url: base_url.clone(),
            connectivity_urls: vec!["http://127.0.0.1:13340".to_string()],
            jwt_config: JwtConfig::new("kukuri-cn-tests", "test-secret", 3600),
            operator_config_path: Some(operator_config_path),
            channel_secret_key: None,
            legal_data_key: Some("unit-test-legal-data-key-0123456789abcdef".to_string()),
            index_query_enabled: false,
            indexer_data_dir: Default::default(),
            trust_read_enabled: false,
            relation_distance_optout_min_proximity: None,
            deployment_revision: "test-deployment-v1".to_string(),
            readiness_activation_max_age_secs: 3600,
            expected_issuer_node_id: None,
        })
        .await?;
        let app = app_router(customize(state));
        let task = tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .expect("user-api server");
        });
        Ok(Self {
            task,
            database,
            base_url,
            rendezvous_redis_url,
            rendezvous_key_prefix,
        })
    }

    pub async fn shutdown(self) -> Result<()> {
        self.task.abort();
        self.database.cleanup().await
    }
}

pub async fn accept_required_consents(
    client: &Client,
    base_url: &str,
    access_token: &str,
) -> Result<kukuri_cn_protocol::CommunityNodeConsentStatus> {
    let policies = client
        .get(format!("{base_url}/v1/policies"))
        .send()
        .await?
        .error_for_status()?
        .json::<CommunityNodePoliciesResponse>()
        .await?;
    let policy_snapshot_revision = policies.policy_snapshot_revision.clone();
    let policy_snapshots = policies
        .policies
        .iter()
        .map(|policy| {
            (
                policy.policy_slug.clone(),
                policy.policy_snapshot_revision.clone(),
            )
        })
        .collect::<Vec<_>>();
    let response = client
        .post(format!("{base_url}/v1/consents"))
        .bearer_auth(access_token)
        .json(&AcceptConsentsRequest {
            policy_slugs: Vec::new(),
            policy_snapshot_revision: policies.policy_snapshot_revision,
        })
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        anyhow::bail!(
            "consent acceptance failed with {status}: {body}; catalog snapshot={policy_snapshot_revision:?}, policies={policy_snapshots:?}"
        );
    }
    let accepted = serde_json::from_str::<kukuri_cn_protocol::CommunityNodeConsentStatus>(&body)?;
    assert!(accepted.all_required_accepted);
    Ok(accepted)
}

pub async fn send_bootstrap_heartbeat(
    client: &Client,
    base_url: &str,
    access_token: &str,
    endpoint_id: &str,
    addr_hint: Option<&str>,
) -> Result<kukuri_cn_protocol::BootstrapHeartbeatResponse> {
    Ok(client
        .post(format!("{base_url}/v1/bootstrap/heartbeat"))
        .bearer_auth(access_token)
        .json(&serde_json::json!({
            "endpoint_id": endpoint_id,
            "addr_hint": addr_hint,
        }))
        .send()
        .await?
        .error_for_status()?
        .json::<kukuri_cn_protocol::BootstrapHeartbeatResponse>()
        .await?)
}

pub async fn advance_policy_snapshot(pool: &PgPool, revision: &str) -> Result<()> {
    let mut policies = kukuri_cn_core::list_policies(pool).await?;
    for policy in &mut policies {
        policy.policy_snapshot_revision = Some(revision.to_string());
    }
    kukuri_cn_core::sync_policies(pool, policies.as_slice()).await
}

pub async fn redis_keys(redis_url: &str, pattern: &str) -> Result<Vec<String>> {
    let client = redis::Client::open(redis_url)?;
    let mut connection = client.get_multiplexed_async_connection().await?;
    let mut keys: Vec<String> = connection.keys(pattern).await?;
    keys.sort();
    Ok(keys)
}

pub fn integration_test_admin_database_url() -> Option<String> {
    kukuri_test_support::gated_env_url(
        "KUKURI_CN_RUN_INTEGRATION_TESTS",
        "COMMUNITY_NODE_DATABASE_URL",
        DEFAULT_ADMIN_DATABASE_URL,
    )
}

pub fn integration_test_rendezvous_redis_url() -> String {
    std::env::var("COMMUNITY_NODE_RENDEZVOUS_REDIS_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_RENDEZVOUS_REDIS_URL.to_string())
}

pub async fn authenticate(
    client: &Client,
    base_url: &str,
    keys: &KukuriKeys,
    endpoint_id: &str,
    addr_hint: Option<&str>,
) -> Result<(String, serde_json::Value)> {
    authenticate_with_invite(client, base_url, keys, endpoint_id, addr_hint, None).await
}

pub async fn authenticate_with_invite(
    client: &Client,
    base_url: &str,
    keys: &KukuriKeys,
    endpoint_id: &str,
    addr_hint: Option<&str>,
    invite_code: Option<&str>,
) -> Result<(String, serde_json::Value)> {
    let pubkey = keys.public_key_hex();
    let challenge = client
        .post(format!("{base_url}/v1/auth/challenge"))
        .json(&serde_json::json!({ "pubkey": pubkey }))
        .send()
        .await?
        .error_for_status()?
        .json::<kukuri_cn_protocol::AuthChallengeResponse>()
        .await?;
    let auth_envelope_json =
        build_auth_envelope_json(keys, challenge.challenge.as_str(), base_url)?;
    let verify = client
        .post(format!("{base_url}/v1/auth/verify"))
        .json(&serde_json::json!({
            "auth_envelope_json": auth_envelope_json.clone(),
            "endpoint_id": endpoint_id,
            "addr_hint": addr_hint,
            "invite_code": invite_code,
        }))
        .send()
        .await?
        .error_for_status()?
        .json::<kukuri_cn_protocol::AuthVerifyResponse>()
        .await?;
    Ok((verify.access_token, auth_envelope_json))
}

/// auth/verify を生で叩き、HTTP status とボディ JSON を返す（拒否ケースの検証用）。
pub async fn raw_auth_verify(
    client: &Client,
    base_url: &str,
    keys: &KukuriKeys,
    invite_code: Option<&str>,
) -> Result<(StatusCode, serde_json::Value)> {
    let pubkey = keys.public_key_hex();
    let challenge = client
        .post(format!("{base_url}/v1/auth/challenge"))
        .json(&serde_json::json!({ "pubkey": pubkey }))
        .send()
        .await?
        .error_for_status()?
        .json::<kukuri_cn_protocol::AuthChallengeResponse>()
        .await?;
    let auth_envelope_json =
        build_auth_envelope_json(keys, challenge.challenge.as_str(), base_url)?;
    let response = client
        .post(format!("{base_url}/v1/auth/verify"))
        .json(&serde_json::json!({
            "auth_envelope_json": auth_envelope_json,
            "invite_code": invite_code,
        }))
        .send()
        .await?;
    let status = response.status();
    let body = response.json::<serde_json::Value>().await?;
    Ok((status, body))
}
