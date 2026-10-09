//! #1061 ブロック / ミュート観測と、trust 絶対値 T・閲覧者別 relation 値 R の CN 側合算の contract test。
//!
//! `POST|DELETE /v1/trust/observations`、`GET /v1/trust/users/{pubkey}`、
//! `POST /v1/trust/evaluations` を ADR 0026 §8 に沿って固定する。拒否時は応答コードだけでなく
//! 観測 DB の行数が 0 / 不変であることを確認する。
//!
//! Postgres + Redis を要するため `KUKURI_CN_RUN_INTEGRATION_TESTS=1` で gate する。

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use kukuri_cn_core::{
    JwtConfig, TestDatabase, cleanup_trust_observations, connect_postgres,
    list_active_relation_observations, persist_risk_signal, store_trust_observations,
};
use kukuri_cn_protocol::{
    AcceptConsentsRequest, CommunityNodePoliciesResponse, TRUST_OBSERVATION_SHARING_POLICY_SLUG,
    TrustEvaluationReason, TrustEvaluationsResponse, TrustObservationsRevokeResponse,
    TrustObservationsSubmitResponse, TrustUserReadResponse, build_auth_envelope_json,
};
use kukuri_cn_safety::{
    Basis, RiskSignalTarget, SafetyCategory, SafetyRiskSignal, Severity, Visibility,
};
use kukuri_cn_trust::{
    EdgeFeatures, FEATURE_SHARED_TOPICS, MemoryRelationStore, RelationStore, TrustParams,
    TrustReadView, compose_trust,
};
use kukuri_cn_user_api::{
    RelationVisibilityState, TrustReadState, UserApiConfig, app_router, build_state,
};
use kukuri_core::{
    BlockEdgeStatus, EnvelopeId, KukuriEnvelope, KukuriKeys, MuteObservationStatus, Pubkey,
    TrustObservation, TrustObservationKind, build_block_edge_envelope,
    build_mute_observation_envelope, generate_keys,
};
use reqwest::{Client, StatusCode};
use sqlx::PgPool;

mod support;
use support::{
    accept_required_consents, integration_test_admin_database_url,
    integration_test_rendezvous_redis_url,
};

/// SAMPLE_CONFIG に観測提供の任意文書を加えた operator config。
fn config_with_sharing_document() -> String {
    kukuri_cn_operator::SAMPLE_CONFIG.replace(
        "    - kind: rights_infringement\n      slug: rights_infringement\n      version: 1\n      effective_date: 2026-09-02\n      language: ja\n",
        "    - kind: rights_infringement\n      slug: rights_infringement\n      version: 1\n      effective_date: 2026-09-02\n      language: ja\n    - kind: trust_observation_sharing\n      slug: trust_observation_sharing\n      version: 1\n      effective_date: 2026-09-18\n      language: ja\n",
    )
}

struct TestServer {
    task: tokio::task::JoinHandle<()>,
    database: TestDatabase,
    base_url: String,
    pool: PgPool,
}

impl TestServer {
    async fn spawn(
        admin_database_url: &str,
        prefix: &str,
        operator_config: &str,
        trust: Arc<TrustReadState>,
    ) -> Result<Self> {
        let database = TestDatabase::create(admin_database_url, prefix).await?;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .context("failed to bind test trust observation listener")?;
        let addr = listener.local_addr()?;
        let base_url = format!("http://{addr}");
        let operator_config_dir = tempfile::tempdir()?;
        let operator_config_path = operator_config_dir.path().join("operator-config.yaml");
        std::fs::write(&operator_config_path, operator_config.as_bytes())?;
        let state = build_state(&UserApiConfig {
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
        let relation_visibility =
            Arc::new(RelationVisibilityState::new(trust.relation.clone(), 0.5)?);
        let state = state
            .with_trust_read(trust)
            .with_relation_visibility(relation_visibility);
        let app = app_router(state);
        let task = tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .expect("trust observation server");
        });
        let pool = connect_postgres(database.database_url.as_str()).await?;
        Ok(Self {
            task,
            database,
            base_url,
            pool,
        })
    }

    async fn shutdown(self) -> Result<()> {
        self.task.abort();
        self.pool.close().await;
        self.database.cleanup().await
    }

    async fn observation_rows(&self) -> Result<i64> {
        Ok(
            sqlx::query_scalar("SELECT COUNT(*) FROM cn_trust.observations")
                .fetch_one(&self.pool)
                .await?,
        )
    }
}

fn memory_trust_state() -> (Arc<TrustReadState>, Arc<MemoryRelationStore>) {
    let relation = Arc::new(MemoryRelationStore::new());
    let state = Arc::new(TrustReadState {
        params: TrustParams::default(),
        relation: relation.clone(),
    });
    (state, relation)
}

async fn authenticate(client: &Client, base_url: &str, keys: &KukuriKeys) -> Result<String> {
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

async fn authenticate_and_consent(
    client: &Client,
    base_url: &str,
    keys: &KukuriKeys,
) -> Result<String> {
    let token = authenticate(client, base_url, keys).await?;
    accept_required_consents(client, base_url, token.as_str()).await?;
    Ok(token)
}

async fn accept_sharing_consent(client: &Client, base_url: &str, token: &str) -> Result<()> {
    let policies = client
        .get(format!("{base_url}/v1/policies"))
        .send()
        .await?
        .error_for_status()?
        .json::<CommunityNodePoliciesResponse>()
        .await?;
    let document = policies
        .policies
        .iter()
        .find(|policy| policy.policy_slug == TRUST_OBSERVATION_SHARING_POLICY_SLUG)
        .context("sharing document is published in the catalog")?;
    assert!(!document.required, "sharing document is optional");
    client
        .post(format!("{base_url}/v1/consents"))
        .bearer_auth(token)
        .json(&AcceptConsentsRequest {
            policy_slugs: vec![TRUST_OBSERVATION_SHARING_POLICY_SLUG.to_string()],
            policy_snapshot_revision: policies.policy_snapshot_revision,
        })
        .send()
        .await?
        .error_for_status()?;
    Ok(())
}

async fn submit(
    client: &Client,
    base_url: &str,
    token: Option<&str>,
    envelopes: &[KukuriEnvelope],
) -> Result<reqwest::Response> {
    let mut request = client
        .post(format!("{base_url}/v1/trust/observations"))
        .json(&serde_json::json!({ "envelopes": envelopes }));
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    Ok(request.send().await?)
}

async fn assert_error_code(response: reqwest::Response, status: StatusCode, code: &str) {
    assert_eq!(response.status(), status);
    let body: serde_json::Value = response.json().await.expect("error body");
    assert_eq!(body["code"], code);
}

fn mute(keys: &KukuriKeys, target: &Pubkey, active: bool) -> KukuriEnvelope {
    let status = if active {
        MuteObservationStatus::Active
    } else {
        MuteObservationStatus::Revoked
    };
    build_mute_observation_envelope(keys, target, status).expect("sign mute observation")
}

fn block(keys: &KukuriKeys, target: &Pubkey, active: bool) -> KukuriEnvelope {
    let status = if active {
        BlockEdgeStatus::Active
    } else {
        BlockEdgeStatus::Revoked
    };
    build_block_edge_envelope(keys, target, status).expect("sign block edge")
}

/// envelope の created_at（ミリ秒）が前の envelope と重ならないよう待つ。
async fn next_millisecond() {
    tokio::time::sleep(std::time::Duration::from_millis(3)).await;
}

#[test]
fn sharing_policy_slug_matches_operator_catalog() {
    assert_eq!(
        TRUST_OBSERVATION_SHARING_POLICY_SLUG,
        kukuri_cn_operator::TRUST_OBSERVATION_SHARING_SLUG
    );
}

#[tokio::test]
async fn observation_intake_requires_matching_signer_and_sharing_consent() -> Result<()> {
    let Some(admin_database_url) = integration_test_admin_database_url() else {
        eprintln!(
            "skipping cn-user-api trust observation test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1"
        );
        return Ok(());
    };
    let (trust, _relation) = memory_trust_state();
    let server = TestServer::spawn(
        admin_database_url.as_str(),
        "cn_trust_obs_intake",
        config_with_sharing_document().as_str(),
        trust,
    )
    .await?;
    let client = Client::new();
    let base_url = server.base_url.as_str();
    let observer = generate_keys();
    let other = generate_keys();
    let target = generate_keys().public_key();
    let envelope = mute(&observer, &target, true);

    // 未認証は拒否。
    let response = submit(&client, base_url, None, std::slice::from_ref(&envelope)).await?;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    // 認証 + 必須同意だけでは、任意文書に同意していないので保存しない。
    let token = authenticate_and_consent(&client, base_url, &observer).await?;
    let response = submit(
        &client,
        base_url,
        Some(token.as_str()),
        std::slice::from_ref(&envelope),
    )
    .await?;
    assert_error_code(
        response,
        StatusCode::FORBIDDEN,
        "TRUST_OBSERVATION_SHARING_CONSENT_REQUIRED",
    )
    .await;
    assert_eq!(server.observation_rows().await?, 0);

    accept_sharing_consent(&client, base_url, token.as_str()).await?;

    // 他人が署名した観測、改ざんされた観測、観測以外の envelope は拒否し、1 件も保存しない。
    let foreign = mute(&other, &target, true);
    let mut tampered = mute(&observer, &target, true);
    tampered.content = tampered.content.replace("active", "revoked");
    let not_observation = kukuri_core::build_follow_edge_envelope(
        &observer,
        &target,
        kukuri_core::FollowEdgeStatus::Active,
    )?;
    for batch in [
        vec![envelope.clone(), foreign],
        vec![tampered],
        vec![not_observation],
        Vec::new(),
    ] {
        let response = submit(&client, base_url, Some(token.as_str()), &batch).await?;
        assert_error_code(
            response,
            StatusCode::BAD_REQUEST,
            "INVALID_TRUST_OBSERVATION",
        )
        .await;
        assert_eq!(server.observation_rows().await?, 0);
    }

    // 同意後の本人署名の観測だけを保存する。
    let stored = submit(
        &client,
        base_url,
        Some(token.as_str()),
        std::slice::from_ref(&envelope),
    )
    .await?
    .error_for_status()?
    .json::<TrustObservationsSubmitResponse>()
    .await?;
    assert_eq!(stored.stored, 1);
    assert_eq!(server.observation_rows().await?, 1);
    server.shutdown().await?;

    // 任意文書を公開していない node は観測を受け付けない。
    let (trust, _relation) = memory_trust_state();
    let server = TestServer::spawn(
        admin_database_url.as_str(),
        "cn_trust_obs_not_offered",
        kukuri_cn_operator::SAMPLE_CONFIG,
        trust,
    )
    .await?;
    let base_url = server.base_url.as_str();
    let token = authenticate_and_consent(&client, base_url, &observer).await?;
    let response = submit(
        &client,
        base_url,
        Some(token.as_str()),
        std::slice::from_ref(&envelope),
    )
    .await?;
    assert_error_code(
        response,
        StatusCode::NOT_FOUND,
        "TRUST_OBSERVATION_SHARING_NOT_OFFERED",
    )
    .await;
    assert_eq!(server.observation_rows().await?, 0);
    server.shutdown().await
}

#[tokio::test]
async fn observation_upsert_is_idempotent_and_ignores_stale() -> Result<()> {
    let Some(admin_database_url) = integration_test_admin_database_url() else {
        eprintln!(
            "skipping cn-user-api trust observation test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1"
        );
        return Ok(());
    };
    let (trust, _relation) = memory_trust_state();
    let server = TestServer::spawn(
        admin_database_url.as_str(),
        "cn_trust_obs_upsert",
        config_with_sharing_document().as_str(),
        trust,
    )
    .await?;
    let client = Client::new();
    let base_url = server.base_url.as_str();
    let observer = generate_keys();
    let target = generate_keys().public_key();
    let token = authenticate_and_consent(&client, base_url, &observer).await?;
    accept_sharing_consent(&client, base_url, token.as_str()).await?;

    let muted = mute(&observer, &target, true);
    next_millisecond().await;
    let blocked = block(&observer, &target, true);
    next_millisecond().await;
    let unmuted = mute(&observer, &target, false);

    let post = |envelopes: Vec<KukuriEnvelope>| {
        let client = client.clone();
        let token = token.clone();
        async move {
            submit(&client, base_url, Some(token.as_str()), &envelopes)
                .await?
                .error_for_status()?
                .json::<TrustObservationsSubmitResponse>()
                .await
                .map_err(anyhow::Error::from)
        }
    };

    // block と mute は別の行。同じ envelope の再送（複数端末・retry）は増幅しない。
    let first = post(vec![muted.clone(), blocked.clone()]).await?;
    assert_eq!((first.stored, first.ignored), (2, 0));
    let resent = post(vec![muted.clone(), blocked.clone()]).await?;
    assert_eq!((resent.stored, resent.ignored), (0, 2));
    assert_eq!(server.observation_rows().await?, 2);

    // 解除は新しい方が勝ち、古い active の再送（順序逆転）では復活しない。
    let revoked = post(vec![unmuted]).await?;
    assert_eq!(revoked.stored, 1);
    let stale = post(vec![muted]).await?;
    assert_eq!((stale.stored, stale.ignored), (0, 1));
    let now = chrono::Utc::now();
    let active =
        list_active_relation_observations(&server.pool, &[target.as_str().to_string()], now)
            .await?;
    let kinds: Vec<_> = active
        .get(target.as_str())
        .map(|items| items.iter().map(|item| item.kind).collect())
        .unwrap_or_default();
    assert_eq!(kinds, vec![kukuri_cn_trust::RelationObservationKind::Block]);
    server.shutdown().await
}

#[tokio::test]
async fn observation_revocation_deletes_rows_and_blocks_intake() -> Result<()> {
    let Some(admin_database_url) = integration_test_admin_database_url() else {
        eprintln!(
            "skipping cn-user-api trust observation test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1"
        );
        return Ok(());
    };
    let (trust, _relation) = memory_trust_state();
    let server = TestServer::spawn(
        admin_database_url.as_str(),
        "cn_trust_obs_revoke",
        config_with_sharing_document().as_str(),
        trust,
    )
    .await?;
    let client = Client::new();
    let base_url = server.base_url.as_str();
    let observer = generate_keys();
    let bystander = generate_keys();
    let target = generate_keys().public_key();
    let token = authenticate_and_consent(&client, base_url, &observer).await?;
    accept_sharing_consent(&client, base_url, token.as_str()).await?;
    let bystander_token = authenticate_and_consent(&client, base_url, &bystander).await?;
    accept_sharing_consent(&client, base_url, bystander_token.as_str()).await?;
    submit(
        &client,
        base_url,
        Some(token.as_str()),
        &[
            mute(&observer, &target, true),
            block(&observer, &target, true),
        ],
    )
    .await?
    .error_for_status()?;
    submit(
        &client,
        base_url,
        Some(bystander_token.as_str()),
        &[mute(&bystander, &target, true)],
    )
    .await?
    .error_for_status()?;
    assert_eq!(server.observation_rows().await?, 3);

    // 取消は本人の観測だけを全削除する。
    let revoked = client
        .delete(format!("{base_url}/v1/trust/observations"))
        .bearer_auth(token.as_str())
        .send()
        .await?
        .error_for_status()?
        .json::<TrustObservationsRevokeResponse>()
        .await?;
    assert_eq!(revoked.deleted, 2);
    assert_eq!(server.observation_rows().await?, 1);

    // 取消後は再同意まで受け付けない。
    next_millisecond().await;
    let response = submit(
        &client,
        base_url,
        Some(token.as_str()),
        &[mute(&observer, &target, true)],
    )
    .await?;
    assert_error_code(
        response,
        StatusCode::FORBIDDEN,
        "TRUST_OBSERVATION_SHARING_CONSENT_REQUIRED",
    )
    .await;
    assert_eq!(server.observation_rows().await?, 1);

    // 再同意すれば受け付ける。
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    accept_sharing_consent(&client, base_url, token.as_str()).await?;
    submit(
        &client,
        base_url,
        Some(token.as_str()),
        &[mute(&observer, &target, true)],
    )
    .await?
    .error_for_status()?;
    assert_eq!(server.observation_rows().await?, 2);

    // 取消は必須同意が未成立でも本人認証だけで実行できる。未認証は拒否する。
    let unconsented = authenticate(&client, base_url, &generate_keys()).await?;
    client
        .delete(format!("{base_url}/v1/trust/observations"))
        .bearer_auth(unconsented.as_str())
        .send()
        .await?
        .error_for_status()?;
    let anonymous = client
        .delete(format!("{base_url}/v1/trust/observations"))
        .send()
        .await?;
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(server.observation_rows().await?, 2);
    server.shutdown().await
}

#[tokio::test]
async fn observation_retention_purges_expired() -> Result<()> {
    let Some(admin_database_url) = integration_test_admin_database_url() else {
        eprintln!(
            "skipping cn-user-api trust observation test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1"
        );
        return Ok(());
    };
    let (trust, _relation) = memory_trust_state();
    let server = TestServer::spawn(
        admin_database_url.as_str(),
        "cn_trust_obs_retention",
        config_with_sharing_document().as_str(),
        trust,
    )
    .await?;
    let client = Client::new();
    let base_url = server.base_url.as_str();
    let observer = generate_keys();
    let token = authenticate_and_consent(&client, base_url, &observer).await?;
    accept_sharing_consent(&client, base_url, token.as_str()).await?;
    let now = chrono::Utc::now();
    let old_target = generate_keys().public_key();
    let fresh_target = generate_keys().public_key();
    let observation =
        |target: &Pubkey, active: bool, observed_at: chrono::DateTime<chrono::Utc>| {
            TrustObservation {
                observer_pubkey: observer.public_key(),
                target_pubkey: target.clone(),
                kind: TrustObservationKind::Block,
                active,
                observed_at: observed_at.timestamp_millis(),
                envelope_id: EnvelopeId(format!("{:064x}", observed_at.timestamp_millis())),
            }
        };
    store_trust_observations(
        &server.pool,
        observer.public_key_hex().as_str(),
        &[
            observation(&old_target, true, now - chrono::Duration::days(181)),
            observation(&fresh_target, true, now - chrono::Duration::days(10)),
        ],
        now,
    )
    .await?;
    // 保持期間を過ぎた active 観測は、削除前でも評価に使わない。
    let active = list_active_relation_observations(
        &server.pool,
        &[
            old_target.as_str().to_string(),
            fresh_target.as_str().to_string(),
        ],
        now,
    )
    .await?;
    assert!(!active.contains_key(old_target.as_str()));
    assert!(active.contains_key(fresh_target.as_str()));

    // revoked は受信から 30 日で削除する。
    let revoked_target = generate_keys().public_key();
    store_trust_observations(
        &server.pool,
        observer.public_key_hex().as_str(),
        &[observation(
            &revoked_target,
            false,
            now - chrono::Duration::days(32),
        )],
        now - chrono::Duration::days(31),
    )
    .await?;
    assert_eq!(server.observation_rows().await?, 3);
    let deleted = cleanup_trust_observations(&server.pool, now).await?;
    assert_eq!(deleted, 2);
    assert_eq!(server.observation_rows().await?, 1);
    server.shutdown().await
}

fn edge(topics: f64) -> EdgeFeatures {
    EdgeFeatures::new().with(FEATURE_SHARED_TOPICS, topics)
}

async fn read_trust(
    client: &Client,
    base_url: &str,
    token: &str,
    target: &str,
) -> Result<(TrustUserReadResponse, String)> {
    let body = client
        .get(format!("{base_url}/v1/trust/users/{target}"))
        .bearer_auth(token)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    Ok((serde_json::from_str(&body)?, body))
}

#[tokio::test]
async fn trust_read_sums_absolute_and_viewer_relation() -> Result<()> {
    let Some(admin_database_url) = integration_test_admin_database_url() else {
        eprintln!(
            "skipping cn-user-api trust observation test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1"
        );
        return Ok(());
    };
    let (trust, relation) = memory_trust_state();
    let server = TestServer::spawn(
        admin_database_url.as_str(),
        "cn_trust_obs_compose",
        config_with_sharing_document().as_str(),
        trust,
    )
    .await?;
    let client = Client::new();
    let base_url = server.base_url.as_str();
    let viewer_a = generate_keys();
    let viewer_c = generate_keys();
    let observer = generate_keys();
    let target = generate_keys();
    let target_hex = target.public_key_hex();

    // A→U は高 relation（0.9）、C→U は低 relation（0.2）。
    relation
        .upsert_edge(
            viewer_a.public_key_hex().as_str(),
            observer.public_key_hex().as_str(),
            &edge(9.0),
        )
        .await?;
    relation
        .upsert_edge(
            viewer_c.public_key_hex().as_str(),
            observer.public_key_hex().as_str(),
            &edge(0.25),
        )
        .await?;
    // B には閲覧者によらない T の根拠（spam）がある。
    persist_risk_signal(
        &server.pool,
        "issuer-node",
        &SafetyRiskSignal {
            target: RiskSignalTarget::UserPubkey,
            target_id: target_hex.clone(),
            category: SafetyCategory::Spam,
            severity: Severity::Low,
            basis: Basis::ClassifierScore,
            confidence: Some(100),
            visibility: Visibility::Local,
            expires_at: None,
            appeal_status: None,
        },
    )
    .await?;
    let token_a = authenticate_and_consent(&client, base_url, &viewer_a).await?;
    let token_c = authenticate_and_consent(&client, base_url, &viewer_c).await?;
    let token_u = authenticate_and_consent(&client, base_url, &observer).await?;

    let (before_a, _) = read_trust(&client, base_url, token_a.as_str(), &target_hex).await?;
    let (before_c, _) = read_trust(&client, base_url, token_c.as_str(), &target_hex).await?;
    // T は応答ごとの内訳から求める（相対成分は照会の時刻まで減衰するので、別の照会とは比べない。
    // JSON の数値の読み取りは最後の桁がずれうる）。
    let t_of = |view: &TrustReadView| {
        compose_trust(&TrustParams::default(), view.absolute, view.relative).trust
    };
    let is_t = |view: &TrustReadView| (view.trust - t_of(view)).abs() < 1e-12;
    let t_value = t_of(&before_a.view);
    assert!(t_value < 0.0);
    for before in [&before_a, &before_c] {
        assert!(is_t(&before.view), "観測が無ければ閲覧者によらず S = T");
    }
    let before_eval = before_a.view.evaluation.clone().expect("evaluation");
    assert_eq!(
        before_eval.reasons,
        vec![TrustEvaluationReason::RiskSignals]
    );
    assert!(!before_eval.hide_recommended);

    // U が B をブロック（提供同意あり）。
    accept_sharing_consent(&client, base_url, token_u.as_str()).await?;
    submit(
        &client,
        base_url,
        Some(token_u.as_str()),
        &[block(&observer, &target.public_key(), true)],
    )
    .await?
    .error_for_status()?;

    let (after_a, body_a) = read_trust(&client, base_url, token_a.as_str(), &target_hex).await?;
    let (after_c, body_c) = read_trust(&client, base_url, token_c.as_str(), &target_hex).await?;
    // 同じ観測でも、U との relation が高い A の方が大きく下がる。
    assert!(after_a.view.trust < after_c.view.trust);
    assert!(after_c.view.trust < t_of(&after_c.view));
    assert!((after_a.view.trust - (t_of(&after_a.view) - 0.9).max(-1.0)).abs() < 1e-9);
    // T の内訳は閲覧者・観測で変わらない（照会の時刻の差の減衰だけが残る）。
    for after in [&after_a, &after_c] {
        assert_eq!(after.view.absolute, before_a.view.absolute);
        assert!((after.view.relative - before_a.view.relative).abs() < 1e-6);
        assert_eq!(after.view.basis.len(), before_a.view.basis.len());
        let eval = after.view.evaluation.as_ref().expect("evaluation");
        assert_eq!(eval.trust_version, before_eval.trust_version);
        assert_ne!(eval.relation_version, before_eval.relation_version);
    }
    let eval_a = after_a.view.evaluation.as_ref().unwrap();
    assert!(eval_a.hide_recommended);
    assert_eq!(
        eval_a.reasons,
        vec![
            TrustEvaluationReason::RiskSignals,
            TrustEvaluationReason::RelatedUsersBlockOrMute
        ]
    );
    assert!(!after_c.view.evaluation.as_ref().unwrap().hide_recommended);
    // 応答に observer を出さない。
    let observer_hex = observer.public_key_hex();
    assert!(!body_a.contains(observer_hex.as_str()));
    assert!(!body_c.contains(observer_hex.as_str()));

    // 一括評価も同じ S と metadata を返す。
    let batch_body = client
        .post(format!("{base_url}/v1/trust/evaluations"))
        .bearer_auth(token_a.as_str())
        .json(&serde_json::json!({ "targets": [target_hex, target_hex] }))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    assert!(!batch_body.contains(observer_hex.as_str()));
    let batch: TrustEvaluationsResponse = serde_json::from_str(&batch_body)?;
    assert_eq!(batch.viewer_pubkey, viewer_a.public_key_hex());
    assert_eq!(batch.evaluations.len(), 1, "重複 target は 1 件にまとめる");
    assert!((batch.evaluations[0].trust - after_a.view.trust).abs() < 1e-6);
    assert!(batch.evaluations[0].evaluation.hide_recommended);
    for invalid in [serde_json::json!([]), serde_json::json!(["not-a-pubkey"])] {
        let response = client
            .post(format!("{base_url}/v1/trust/evaluations"))
            .bearer_auth(token_a.as_str())
            .json(&serde_json::json!({ "targets": invalid }))
            .send()
            .await?;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    let unauthenticated = client
        .post(format!("{base_url}/v1/trust/evaluations"))
        .json(&serde_json::json!({ "targets": [target_hex] }))
        .send()
        .await?;
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

    // cross-node pull には観測・R を返さない（spam の相対成分も返らない）。
    let pull_body = client
        .get(format!("{base_url}/v1/trust/pull/{target_hex}"))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    assert!(!pull_body.contains(observer_hex.as_str()));
    let pull: serde_json::Value = serde_json::from_str(&pull_body)?;
    assert_eq!(pull["absolute"], serde_json::json!(0.0));
    assert_eq!(pull["basis"], serde_json::json!([]));
    assert!(pull.get("trust").is_none());
    assert!(pull.get("evaluation").is_none());

    // U が解除すると、R の寄与は消えて S は T に戻る。
    next_millisecond().await;
    submit(
        &client,
        base_url,
        Some(token_u.as_str()),
        &[block(&observer, &target.public_key(), false)],
    )
    .await?
    .error_for_status()?;
    let (restored, _) = read_trust(&client, base_url, token_a.as_str(), &target_hex).await?;
    assert!(is_t(&restored.view));
    let restored_eval = restored.view.evaluation.unwrap();
    assert_eq!(
        restored_eval.reasons,
        vec![TrustEvaluationReason::RiskSignals]
    );
    assert_ne!(
        restored_eval.relation_version,
        after_a.view.evaluation.unwrap().relation_version
    );

    // 提供同意を取り消した observer の観測は、行が残っていても評価に使わない。
    next_millisecond().await;
    submit(
        &client,
        base_url,
        Some(token_u.as_str()),
        &[block(&observer, &target.public_key(), true)],
    )
    .await?
    .error_for_status()?;
    sqlx::query(
        "INSERT INTO cn_trust.observation_sharing_revocations (observer_pubkey, revoked_at)
         VALUES ($1, NOW())",
    )
    .bind(observer_hex.as_str())
    .execute(&server.pool)
    .await?;
    let (withdrawn, _) = read_trust(&client, base_url, token_a.as_str(), &target_hex).await?;
    assert!(is_t(&withdrawn.view));
    server.shutdown().await
}
