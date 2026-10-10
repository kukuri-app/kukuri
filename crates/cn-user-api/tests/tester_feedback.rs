//! テスターフィードバック受付エンドポイント(#802 / ADR 0039)の contract test。
//!
//! `POST /v1/tester-feedback` は `tester_feedback` capability を有効化した node でのみ、
//! 認証済み + consent 済み user から受理する。
//! - 3 つの自由記述は必須で、各 2000 コードポイント以内。
//! - capability 無効 node は 404(`TESTER_FEEDBACK_NOT_CONFIGURED`)で fail-closed。
//! - 未認証は 401。
//!
//! Postgres を要するため `KUKURI_CN_RUN_INTEGRATION_TESTS=1` で gate する。

use std::net::SocketAddr;

use anyhow::{Context, Result};
use kukuri_cn_core::{JwtConfig, TestDatabase, connect_postgres, list_tester_feedback};
use kukuri_cn_protocol::build_auth_envelope_json;
use kukuri_cn_user_api::{UserApiConfig, app_router, build_state};
use kukuri_core::{KukuriKeys, generate_keys};
use reqwest::{Client, StatusCode};

mod support;
use support::{
    accept_required_consents, integration_test_admin_database_url,
    integration_test_rendezvous_redis_url,
};

/// tester_feedback capability を有効化した operator config。
const FEEDBACK_ENABLED_YAML: &str = r#"server:
  domain: example-kukuri.net
  operator_name: Example Operator
  country: JP
features:
  tester_feedback: true
acknowledge_planned_capabilities: true
"#;

/// tester_feedback capability を有効化しない operator config。
const FEEDBACK_DISABLED_YAML: &str = r#"server:
  domain: example-kukuri.net
  operator_name: Example Operator
  country: JP
features:
  iroh_relay: true
"#;

struct TestServer {
    task: tokio::task::JoinHandle<()>,
    database: TestDatabase,
    base_url: String,
    // operator config の一時ファイルを test 期間中保持する。
    _operator_config: tempfile::NamedTempFile,
}

impl TestServer {
    async fn spawn(admin_database_url: &str, prefix: &str, operator_yaml: &str) -> Result<Self> {
        use std::io::Write;

        let database = TestDatabase::create(admin_database_url, prefix).await?;
        let mut operator_config = tempfile::Builder::new()
            .suffix(".yaml")
            .tempfile()
            .context("create temp operator config")?;
        operator_config
            .write_all(operator_yaml.as_bytes())
            .context("write temp operator config")?;
        operator_config.flush().ok();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .context("failed to bind test tester-feedback listener")?;
        let addr = listener.local_addr()?;
        let base_url = format!("http://{addr}");
        let state = build_state(&UserApiConfig {
            bind_addr: addr,
            database_url: database.database_url.clone(),
            rendezvous_redis_url: integration_test_rendezvous_redis_url(),
            rendezvous_key_prefix: format!("cn:test:{prefix}"),
            base_url: base_url.clone(),
            public_base_url: base_url.clone(),
            connectivity_urls: vec!["http://127.0.0.1:13340".to_string()],
            jwt_config: JwtConfig::new("kukuri-cn-tests", "test-secret", 3600),
            operator_config_path: Some(operator_config.path().to_path_buf()),
            channel_secret_key: None,
            legal_data_key: None,
            index_query_enabled: false,
            indexer_data_dir: Default::default(),
            trust_read_enabled: false,
            relation_distance_optout_min_proximity: None,
            deployment_revision: "test-deployment-v1".to_string(),
            readiness_activation_max_age_secs: 3600,
            expected_issuer_node_id: None,
        })
        .await;
        let state = match state {
            Ok(state) => state,
            Err(error) => {
                database.cleanup().await?;
                return Err(error);
            }
        };
        let app = app_router(state);
        let task = tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .expect("tester feedback server");
        });
        Ok(Self {
            task,
            database,
            base_url,
            _operator_config: operator_config,
        })
    }

    async fn shutdown(self) -> Result<()> {
        self.task.abort();
        self.database.cleanup().await
    }
}

/// 認証 + consent を通し、bearer access token を返す。
async fn authenticate_and_consent(
    client: &Client,
    base_url: &str,
    keys: &KukuriKeys,
) -> Result<String> {
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
            "auth_envelope_json": auth_envelope_json,
            "endpoint_id": "peer-a",
        }))
        .send()
        .await?
        .error_for_status()?
        .json::<kukuri_cn_protocol::AuthVerifyResponse>()
        .await?;
    accept_required_consents(client, base_url, verify.access_token.as_str()).await?;
    Ok(verify.access_token)
}

fn valid_body() -> serde_json::Value {
    serde_json::json!({
        "what_attempted": "投稿を作成しようとした",
        "what_happened": "送信ボタンを押しても反応がなかった",
        "what_seemed_wrong": "エラーも成功も表示されないのが変だと思った",
        "client_version": "0.1.7",
        "os": "linux",
    })
}

#[tokio::test]
async fn tester_feedback_accepts_stores_and_validates() -> Result<()> {
    let Some(admin_database_url) = integration_test_admin_database_url() else {
        eprintln!(
            "skipping cn-user-api tester feedback test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1"
        );
        return Ok(());
    };
    let server = TestServer::spawn(
        admin_database_url.as_str(),
        "cn_tester_feedback",
        FEEDBACK_ENABLED_YAML,
    )
    .await?;
    let client = Client::new();
    let keys = generate_keys();
    let token = authenticate_and_consent(&client, &server.base_url, &keys).await?;

    // 受理: reference_id が返り、row が保存される。
    let accepted = client
        .post(format!("{}/v1/tester-feedback", server.base_url))
        .bearer_auth(token.as_str())
        .json(&valid_body())
        .send()
        .await?;
    assert_eq!(accepted.status(), StatusCode::OK);
    let body = accepted.json::<serde_json::Value>().await?;
    let reference_id = body["reference_id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(!reference_id.is_empty());

    let pool = connect_postgres(server.database.database_url.as_str()).await?;
    let stored = list_tester_feedback(&pool, 50, 0).await?;
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].id, reference_id);
    assert_eq!(stored[0].what_attempted, "投稿を作成しようとした");
    assert_eq!(
        stored[0].what_happened,
        "送信ボタンを押しても反応がなかった"
    );
    assert_eq!(
        stored[0].what_seemed_wrong,
        "エラーも成功も表示されないのが変だと思った"
    );
    assert_eq!(stored[0].client_version, "0.1.7");
    assert_eq!(stored[0].os, "linux");

    // 空欄は 400。
    let mut missing = valid_body();
    missing["what_happened"] = serde_json::json!("   ");
    let rejected = client
        .post(format!("{}/v1/tester-feedback", server.base_url))
        .bearer_auth(token.as_str())
        .json(&missing)
        .send()
        .await?;
    assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
    let body = rejected.json::<serde_json::Value>().await?;
    assert_eq!(body["code"], "INVALID_TESTER_FEEDBACK");

    // 2000 コードポイント超は 400(2000 ちょうどは受理)。
    let mut at_limit = valid_body();
    at_limit["what_attempted"] = serde_json::json!("あ".repeat(2000));
    let accepted = client
        .post(format!("{}/v1/tester-feedback", server.base_url))
        .bearer_auth(token.as_str())
        .json(&at_limit)
        .send()
        .await?;
    assert_eq!(accepted.status(), StatusCode::OK);

    let mut over_limit = valid_body();
    over_limit["what_attempted"] = serde_json::json!("あ".repeat(2001));
    let rejected = client
        .post(format!("{}/v1/tester-feedback", server.base_url))
        .bearer_auth(token.as_str())
        .json(&over_limit)
        .send()
        .await?;
    assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
    let body = rejected.json::<serde_json::Value>().await?;
    assert_eq!(body["code"], "INVALID_TESTER_FEEDBACK");

    server.shutdown().await
}

#[tokio::test]
async fn tester_feedback_requires_auth() -> Result<()> {
    let Some(admin_database_url) = integration_test_admin_database_url() else {
        eprintln!(
            "skipping cn-user-api tester feedback test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1"
        );
        return Ok(());
    };
    let server = TestServer::spawn(
        admin_database_url.as_str(),
        "cn_tester_feedback_auth",
        FEEDBACK_ENABLED_YAML,
    )
    .await?;
    let client = Client::new();

    let unauthenticated = client
        .post(format!("{}/v1/tester-feedback", server.base_url))
        .json(&valid_body())
        .send()
        .await?;
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

    server.shutdown().await
}

#[tokio::test]
async fn tester_feedback_rejects_when_capability_disabled() -> Result<()> {
    let Some(admin_database_url) = integration_test_admin_database_url() else {
        eprintln!(
            "skipping cn-user-api tester feedback test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1"
        );
        return Ok(());
    };
    let server = TestServer::spawn(
        admin_database_url.as_str(),
        "cn_tester_feedback_disabled",
        FEEDBACK_DISABLED_YAML,
    )
    .await?;
    let client = Client::new();

    // capability を有効化していない node は受け付けない(404 fail-closed)。認証より先に閉じる。
    let rejected = client
        .post(format!("{}/v1/tester-feedback", server.base_url))
        .json(&valid_body())
        .send()
        .await?;
    assert_eq!(rejected.status(), StatusCode::NOT_FOUND);
    let body = rejected.json::<serde_json::Value>().await?;
    assert_eq!(body["code"], "TESTER_FEEDBACK_NOT_CONFIGURED");

    server.shutdown().await
}
