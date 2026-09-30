//! #1056 タイムライン向け content advisory 一括照会(`POST /v1/advisories/lookup`)の contract test。
//!
//! ADR 0046 §6.3 / ADR 0028 §8.12 の contract を固定する:
//! - `advisory_lookup_returns_only_configured_node_signals`: 自 node が発行した nsfw / objectionable の
//!   advisory だけを返し、`Cleared`・失効・他 issuer・非 advisory category・user 対象は返さない。
//! - `advisory_lookup_reads_do_not_mutate_state`: 読み取りのみ。
//! - 未認証 / 未同意は `AUTH_REQUIRED` / `CONSENT_REQUIRED`(INVAR-4)。
//!
//! Postgres + Redis を要するため `KUKURI_CN_RUN_INTEGRATION_TESTS=1` で gate する。

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use kukuri_cn_core::{
    JwtConfig, MemoryIndexEntryStore, TestDatabase, connect_postgres, dispute_risk_signal,
    persist_risk_signal, update_risk_signal_appeal_status,
};
use kukuri_cn_indexer::projection::MemoryIndexProjection;
use kukuri_cn_indexer::query::FailClosedIndexQuery;
use kukuri_cn_protocol::{ADVISORY_LOOKUP_MAX_SUBJECTS, build_auth_envelope_json};
use kukuri_cn_safety::{
    AppealStatus, Basis, RiskSignalTarget, SafetyCategory, SafetyRiskSignal, Severity, Visibility,
};
use kukuri_cn_safety_runtime::MemorySafetyArtifactStore;
use kukuri_cn_user_api::{UserApiConfig, app_router, build_state};
use kukuri_core::{KukuriKeys, generate_keys};
use reqwest::{Client, StatusCode};
use sqlx::PgPool;

mod support;
use support::{
    accept_required_consents, integration_test_admin_database_url,
    integration_test_rendezvous_redis_url,
};

/// `SAMPLE_CONFIG` の `server.node_id`(= この node の発行元識別子)。
const SELF_NODE_ID: &str = "0000000000000000000000000000000000000000000000000000000000000000";
const OTHER_NODE_ID: &str = "3333333333333333333333333333333333333333333333333333333333333333";

struct TestServer {
    task: tokio::task::JoinHandle<()>,
    database: TestDatabase,
    base_url: String,
    _operator_config_dir: tempfile::TempDir,
}

impl TestServer {
    /// `index_enabled = false` は索引未提供の node(一括照会も 404)。
    async fn spawn(admin_database_url: &str, prefix: &str, index_enabled: bool) -> Result<Self> {
        let database = TestDatabase::create(admin_database_url, prefix).await?;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .context("failed to bind advisory lookup listener")?;
        let addr = listener.local_addr()?;
        let base_url = format!("http://{addr}");
        let operator_config_dir = tempfile::tempdir()?;
        let operator_config_path = operator_config_dir.path().join("operator-config.yaml");
        std::fs::write(
            &operator_config_path,
            kukuri_cn_operator::SAMPLE_CONFIG.as_bytes(),
        )?;
        let mut state = build_state(&UserApiConfig {
            bind_addr: addr,
            database_url: database.database_url.clone(),
            rendezvous_redis_url: integration_test_rendezvous_redis_url(),
            rendezvous_key_prefix: format!("cn:test:{prefix}"),
            base_url: base_url.clone(),
            public_base_url: base_url.clone(),
            connectivity_urls: vec!["http://127.0.0.1:13340".to_string()],
            jwt_config: JwtConfig::new("kukuri-cn-tests", "test-secret", 3600),
            operator_config_path: Some(operator_config_path),
            channel_secret_key: None,
            legal_data_key: Some("unit-test-legal-data-key-0123456789abcdef".to_string()),
            index_query_enabled: false,
            trust_read_enabled: false,
            relation_distance_optout_min_proximity: None,
            deployment_revision: "test-deployment-v1".to_string(),
            readiness_activation_max_age_secs: 3600,
            expected_issuer_node_id: None,
        })
        .await?;
        if index_enabled {
            let store = Arc::new(MemorySafetyArtifactStore::new());
            let entries = Arc::new(MemoryIndexEntryStore::new(store));
            let projection = Arc::new(MemoryIndexProjection::new());
            state =
                state.with_index_query(Arc::new(FailClosedIndexQuery::new(projection, entries)));
        }
        let app = app_router(state);
        let task = tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .expect("advisory lookup server");
        });
        Ok(Self {
            task,
            database,
            base_url,
            _operator_config_dir: operator_config_dir,
        })
    }

    fn lookup_url(&self) -> String {
        format!("{}/v1/advisories/lookup", self.base_url)
    }

    async fn shutdown(self) -> Result<()> {
        self.task.abort();
        self.database.cleanup().await
    }
}

async fn authenticate_only(client: &Client, base_url: &str, keys: &KukuriKeys) -> Result<String> {
    let challenge = client
        .post(format!("{base_url}/v1/auth/challenge"))
        .json(&serde_json::json!({ "pubkey": keys.public_key_hex() }))
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
    Ok(verify.access_token)
}

fn signal(
    target: RiskSignalTarget,
    target_id: &str,
    category: SafetyCategory,
    basis: Basis,
) -> SafetyRiskSignal {
    SafetyRiskSignal {
        target,
        target_id: target_id.to_string(),
        category,
        severity: Severity::Low,
        basis,
        confidence: Some(84),
        visibility: Visibility::Local,
        expires_at: None,
        appeal_status: None,
    }
}

fn hash(fill: char) -> String {
    fill.to_string().repeat(64)
}

fn body_code(body: &serde_json::Value) -> &str {
    body["code"].as_str().unwrap_or_default()
}

/// 対象テーブルの内容を 1 つの指紋にまとめる(読み取り前後の比較用)。
async fn state_fingerprint(pool: &PgPool) -> Result<Vec<String>> {
    let mut fingerprints = Vec::new();
    for table in [
        "cn_safety.risk_signals",
        "cn_safety.scan_verdicts",
        "cn_safety.signed_moderation_events",
        "cn_index.index_entries",
        "cn_index.supported_topics",
        "cn_index.indexing_requests",
    ] {
        let fingerprint: Option<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT md5(COALESCE(string_agg(t::text, '|' ORDER BY t::text), '')) FROM {table} t"
        )))
        .fetch_one(pool)
        .await?;
        fingerprints.push(format!("{table}:{}", fingerprint.unwrap_or_default()));
    }
    Ok(fingerprints)
}

/// AC-1 / TR-1 / INVAR-3: `advisory_lookup_returns_only_configured_node_signals`。
#[tokio::test]
async fn advisory_lookup_returns_only_configured_node_signals() -> Result<()> {
    let Some(admin_database_url) = integration_test_admin_database_url() else {
        eprintln!("skipping advisory lookup test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let server = TestServer::spawn(admin_database_url.as_str(), "cn_advisory_lookup", true).await?;
    let pool = connect_postgres(server.database.database_url.as_str()).await?;
    let client = Client::new();
    let keys = generate_keys();
    let token = authenticate_only(&client, server.base_url.as_str(), &keys).await?;
    accept_required_consents(&client, server.base_url.as_str(), token.as_str()).await?;

    let nsfw_blob = hash('a');
    let cleared_blob = hash('c');
    let expired_blob = hash('e');
    let other_issuer_blob = hash('f');
    let spam_blob = hash('d');

    // 返るもの: 自 node の nsfw(blob)と objectionable(post)。
    let nsfw = persist_risk_signal(
        &pool,
        SELF_NODE_ID,
        &signal(
            RiskSignalTarget::BlobCid,
            nsfw_blob.as_str(),
            SafetyCategory::Nsfw,
            Basis::ClassifierScore,
        ),
    )
    .await?;
    let objectionable = persist_risk_signal(
        &pool,
        SELF_NODE_ID,
        &signal(
            RiskSignalTarget::PostId,
            "post-visible",
            SafetyCategory::Objectionable,
            Basis::ClassifierScore,
        ),
    )
    .await?;
    // 返らないもの: Cleared、失効、他 issuer、非 advisory category、user 対象、未要求 subject。
    let cleared = persist_risk_signal(
        &pool,
        SELF_NODE_ID,
        &signal(
            RiskSignalTarget::BlobCid,
            cleared_blob.as_str(),
            SafetyCategory::Nsfw,
            Basis::ClassifierScore,
        ),
    )
    .await?;
    dispute_risk_signal(&pool, cleared.id.as_str()).await?;
    update_risk_signal_appeal_status(&pool, cleared.id.as_str(), AppealStatus::Cleared).await?;
    let mut expired = signal(
        RiskSignalTarget::BlobCid,
        expired_blob.as_str(),
        SafetyCategory::Nsfw,
        Basis::ClassifierScore,
    );
    expired.expires_at = Some("2020-01-01T00:00:00Z".to_string());
    persist_risk_signal(&pool, SELF_NODE_ID, &expired).await?;
    persist_risk_signal(
        &pool,
        OTHER_NODE_ID,
        &signal(
            RiskSignalTarget::BlobCid,
            other_issuer_blob.as_str(),
            SafetyCategory::Nsfw,
            Basis::ClassifierScore,
        ),
    )
    .await?;
    persist_risk_signal(
        &pool,
        SELF_NODE_ID,
        &signal(
            RiskSignalTarget::BlobCid,
            spam_blob.as_str(),
            SafetyCategory::Spam,
            Basis::ClassifierScore,
        ),
    )
    .await?;
    persist_risk_signal(
        &pool,
        SELF_NODE_ID,
        &signal(
            RiskSignalTarget::UserPubkey,
            "post-visible",
            SafetyCategory::Nsfw,
            Basis::ClassifierScore,
        ),
    )
    .await?;
    persist_risk_signal(
        &pool,
        SELF_NODE_ID,
        &signal(
            RiskSignalTarget::PostId,
            "post-not-requested",
            SafetyCategory::Nsfw,
            Basis::ClassifierScore,
        ),
    )
    .await?;

    let response = client
        .post(server.lookup_url())
        .bearer_auth(token.as_str())
        .json(&serde_json::json!({
            "post_ids": ["post-visible", " post-visible ", "post-unknown"],
            "blob_hashes": [
                nsfw_blob.to_ascii_uppercase(),
                cleared_blob,
                expired_blob,
                other_issuer_blob,
                spam_blob,
            ],
        }))
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.json::<serde_json::Value>().await?;
    let advisories = body["advisories"].as_array().expect("advisories");
    assert_eq!(advisories.len(), 2, "unexpected advisories: {body}");
    let mut by_subject: Vec<(String, serde_json::Value)> = advisories
        .iter()
        .map(|advisory| {
            (
                advisory["subject_id"].as_str().unwrap().to_string(),
                advisory.clone(),
            )
        })
        .collect();
    by_subject.sort_by(|left, right| left.0.cmp(&right.0));
    let (blob_subject, blob_advisory) = &by_subject[0];
    assert_eq!(blob_subject, &nsfw_blob);
    assert_eq!(blob_advisory["subject_kind"], "blob_cid");
    assert_eq!(blob_advisory["category"], "nsfw");
    assert_eq!(blob_advisory["label"], "adult");
    assert_eq!(blob_advisory["issuer_node_id"], SELF_NODE_ID);
    assert_eq!(blob_advisory["signal_id"], nsfw.id.as_str());
    assert_eq!(blob_advisory["basis"], "classifier_score");
    assert_eq!(blob_advisory["confidence"], 84);
    let (post_subject, post_advisory) = &by_subject[1];
    assert_eq!(post_subject, "post-visible");
    assert_eq!(post_advisory["subject_kind"], "post_id");
    assert_eq!(post_advisory["label"], "sensitive");
    assert_eq!(post_advisory["signal_id"], objectionable.id.as_str());
    // INVAR-3: 相対 trust / relation を返さない(応答は advisories だけ)。
    assert_eq!(
        body.as_object().unwrap().keys().collect::<Vec<_>>(),
        vec!["advisories"]
    );

    server.shutdown().await
}

/// AC-2: `advisory_lookup_reads_do_not_mutate_state`。
#[tokio::test]
async fn advisory_lookup_reads_do_not_mutate_state() -> Result<()> {
    let Some(admin_database_url) = integration_test_admin_database_url() else {
        eprintln!("skipping advisory lookup test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let server =
        TestServer::spawn(admin_database_url.as_str(), "cn_advisory_lookup_ro", true).await?;
    let pool = connect_postgres(server.database.database_url.as_str()).await?;
    let client = Client::new();
    let keys = generate_keys();
    let token = authenticate_only(&client, server.base_url.as_str(), &keys).await?;
    accept_required_consents(&client, server.base_url.as_str(), token.as_str()).await?;
    let blob = hash('a');
    persist_risk_signal(
        &pool,
        SELF_NODE_ID,
        &signal(
            RiskSignalTarget::BlobCid,
            blob.as_str(),
            SafetyCategory::Nsfw,
            Basis::ClassifierScore,
        ),
    )
    .await?;

    let before = state_fingerprint(&pool).await?;
    for _ in 0..2 {
        let response = client
            .post(server.lookup_url())
            .bearer_auth(token.as_str())
            .json(&serde_json::json!({ "post_ids": ["post-1"], "blob_hashes": [blob] }))
            .send()
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
    }
    let after = state_fingerprint(&pool).await?;
    assert_eq!(
        before, after,
        "lookup must not mutate index / verdict / signal state"
    );

    server.shutdown().await
}

/// INVAR-4 / TR-2: 未認証は 401 `AUTH_REQUIRED`、未同意は 403 `CONSENT_REQUIRED`。
#[tokio::test]
async fn advisory_lookup_requires_auth_and_consent() -> Result<()> {
    let Some(admin_database_url) = integration_test_admin_database_url() else {
        eprintln!("skipping advisory lookup test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let server =
        TestServer::spawn(admin_database_url.as_str(), "cn_advisory_lookup_gate", true).await?;
    let pool = connect_postgres(server.database.database_url.as_str()).await?;
    let client = Client::new();
    let blob = hash('a');
    persist_risk_signal(
        &pool,
        SELF_NODE_ID,
        &signal(
            RiskSignalTarget::BlobCid,
            blob.as_str(),
            SafetyCategory::Nsfw,
            Basis::ClassifierScore,
        ),
    )
    .await?;
    let payload = serde_json::json!({ "blob_hashes": [blob] });

    let anonymous = client
        .post(server.lookup_url())
        .json(&payload)
        .send()
        .await?;
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
    let anonymous_body = anonymous.json::<serde_json::Value>().await?;
    assert_eq!(body_code(&anonymous_body), "AUTH_REQUIRED");
    assert!(anonymous_body.get("advisories").is_none());

    let keys = generate_keys();
    let token = authenticate_only(&client, server.base_url.as_str(), &keys).await?;
    let unconsented = client
        .post(server.lookup_url())
        .bearer_auth(token.as_str())
        .json(&payload)
        .send()
        .await?;
    assert_eq!(unconsented.status(), StatusCode::FORBIDDEN);
    let unconsented_body = unconsented.json::<serde_json::Value>().await?;
    assert_eq!(body_code(&unconsented_body), "CONSENT_REQUIRED");
    assert!(unconsented_body.get("advisories").is_none());

    accept_required_consents(&client, server.base_url.as_str(), token.as_str()).await?;
    let consented = client
        .post(server.lookup_url())
        .bearer_auth(token.as_str())
        .json(&payload)
        .send()
        .await?;
    assert_eq!(consented.status(), StatusCode::OK);

    server.shutdown().await
}

/// 空・上限超過・形式不正の要求は 400 `INVALID_ADVISORY_LOOKUP`。
#[tokio::test]
async fn advisory_lookup_rejects_oversized_or_empty_batch() -> Result<()> {
    let Some(admin_database_url) = integration_test_admin_database_url() else {
        eprintln!("skipping advisory lookup test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let server =
        TestServer::spawn(admin_database_url.as_str(), "cn_advisory_lookup_bad", true).await?;
    let client = Client::new();
    let keys = generate_keys();
    let token = authenticate_only(&client, server.base_url.as_str(), &keys).await?;
    accept_required_consents(&client, server.base_url.as_str(), token.as_str()).await?;

    let oversized: Vec<String> = (0..=ADVISORY_LOOKUP_MAX_SUBJECTS)
        .map(|index| format!("post-{index}"))
        .collect();
    for payload in [
        serde_json::json!({}),
        serde_json::json!({ "post_ids": ["  "] }),
        serde_json::json!({ "blob_hashes": ["not-a-hash"] }),
        serde_json::json!({ "post_ids": oversized }),
    ] {
        let response = client
            .post(server.lookup_url())
            .bearer_auth(token.as_str())
            .json(&payload)
            .send()
            .await?;
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "payload: {payload}"
        );
        let body = response.json::<serde_json::Value>().await?;
        assert_eq!(body_code(&body), "INVALID_ADVISORY_LOOKUP");
    }

    server.shutdown().await
}

/// 索引を提供しない node は一括照会も提供しない(404 `INDEX_QUERY_NOT_CONFIGURED`)。
#[tokio::test]
async fn advisory_lookup_is_not_found_when_index_query_is_not_configured() -> Result<()> {
    let Some(admin_database_url) = integration_test_admin_database_url() else {
        eprintln!("skipping advisory lookup test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let server =
        TestServer::spawn(admin_database_url.as_str(), "cn_advisory_lookup_off", false).await?;
    let client = Client::new();
    let response = client
        .post(server.lookup_url())
        .json(&serde_json::json!({ "post_ids": ["post-1"] }))
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = response.json::<serde_json::Value>().await?;
    assert_eq!(body_code(&body), "INDEX_QUERY_NOT_CONFIGURED");

    server.shutdown().await
}
