//! #415 trust / relation read surface（`CommunityLocalTrust`）の contract test。
//!
//! `GET /v1/trust/users/{pubkey}` / `GET /v1/trust/pull/{pubkey}` /
//! `GET /v1/relation/users/{target}` / `GET /v1/relation/neighbors` /
//! `PUT|DELETE /v1/relation/optout` を ADR 0026 の contract に沿って固定する。
//! 機能未構成（既定。`CommunityLocalTrust` = `Availability::Planned`）の node は 404。
//!
//! Postgres + Redis を要するため `KUKURI_CN_RUN_INTEGRATION_TESTS=1` で gate する。
//! relation graph は in-memory（`MemoryRelationStore`）を `with_trust_read` で注入する
//! （proximity の合成は ArcadeDB 実装と共通の `proximity_from_features`）。

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use kukuri_cn_core::{
    JwtConfig, NewCommunityNodeReport, TestDatabase, connect_postgres, dispute_risk_signal,
    insert_community_node_report, persist_risk_signal, update_risk_signal_appeal_status,
};
use kukuri_cn_protocol::build_auth_envelope_json;
use kukuri_cn_safety::{
    AppealStatus, Basis, RiskSignalTarget, SafetyCategory, SafetyRiskSignal, Severity, Visibility,
};
use kukuri_cn_trust::{
    EdgeFeatures, FEATURE_SHARED_TOPICS, MemoryRelationStore, RelationStore, TrustParams,
};
use kukuri_cn_user_api::{
    RelationVisibilityState, TrustReadState, UserApiConfig, app_router, build_state,
};
use kukuri_core::{KukuriKeys, generate_keys};
use reqwest::{Client, StatusCode};

mod support;
use support::{
    accept_required_consents, integration_test_admin_database_url,
    integration_test_rendezvous_redis_url,
};

struct TestServer {
    task: tokio::task::JoinHandle<()>,
    database: TestDatabase,
    base_url: String,
}

impl TestServer {
    /// trust / relation read（in-memory relation）を注入した user-api server を起動する。
    /// `trust` が None の場合は機能未構成の node（`/v1/trust/*` / `/v1/relation/*` は 404）。
    async fn spawn(
        admin_database_url: &str,
        prefix: &str,
        trust: Option<Arc<TrustReadState>>,
    ) -> Result<Self> {
        let database = TestDatabase::create(admin_database_url, prefix).await?;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .context("failed to bind test trust read listener")?;
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
        if let Some(trust) = trust {
            let relation_visibility =
                Arc::new(RelationVisibilityState::new(trust.relation.clone(), 0.5)?);
            state = state
                .with_trust_read(trust)
                .with_relation_visibility(relation_visibility);
        }
        let app = app_router(state);
        let task = tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .expect("trust read server");
        });
        Ok(Self {
            task,
            database,
            base_url,
        })
    }

    async fn shutdown(self) -> Result<()> {
        self.task.abort();
        self.database.cleanup().await
    }
}

async fn authenticate_only(client: &Client, base_url: &str, keys: &KukuriKeys) -> Result<String> {
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
    Ok(verify.access_token)
}

/// 認証 + consent を通し、bearer access token を返す。
async fn authenticate_and_consent(
    client: &Client,
    base_url: &str,
    keys: &KukuriKeys,
) -> Result<String> {
    let access_token = authenticate_only(client, base_url, keys).await?;
    accept_required_consents(client, base_url, access_token.as_str()).await?;
    Ok(access_token)
}

#[allow(clippy::unwrap_used)] // test fixture helper
/// 別の照会どうしの値を比べる。相対成分は照会の時刻まで減衰するので、その差だけを許す（ADR 0026 §10）。
fn assert_close(left: &serde_json::Value, right: &serde_json::Value) {
    let (left, right) = (left.as_f64().unwrap(), right.as_f64().unwrap());
    assert!((left - right).abs() < 1e-6, "{left} vs {right}");
}

fn memory_trust_state() -> (Arc<TrustReadState>, Arc<MemoryRelationStore>) {
    let relation = Arc::new(MemoryRelationStore::new());
    let state = Arc::new(TrustReadState {
        params: TrustParams::default(),
        relation: relation.clone(),
    });
    (state, relation)
}

fn risk_signal(
    target_id: &str,
    category: SafetyCategory,
    severity: Severity,
    basis: Basis,
    visibility: Visibility,
) -> SafetyRiskSignal {
    SafetyRiskSignal {
        target: RiskSignalTarget::UserPubkey,
        target_id: target_id.to_string(),
        category,
        severity,
        basis,
        confidence: Some(100),
        visibility,
        expires_at: None,
        appeal_status: None,
    }
}

#[tokio::test]
async fn trust_read_is_not_found_when_not_configured() -> Result<()> {
    // 既定（機能無効 = `CommunityLocalTrust` Planned 相当）の node は trust / relation read を
    // 公開しない。
    let Some(admin_database_url) = integration_test_admin_database_url() else {
        eprintln!("skipping cn-user-api trust read test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let server = TestServer::spawn(admin_database_url.as_str(), "cn_trust_off", None).await?;
    let client = Client::new();
    let keys = generate_keys();
    let token = authenticate_and_consent(&client, server.base_url.as_str(), &keys).await?;
    let target = generate_keys().public_key_hex();

    for path in [
        format!("/v1/trust/users/{target}"),
        format!("/v1/trust/pull/{target}"),
        format!("/v1/relation/users/{target}"),
        "/v1/relation/neighbors".to_string(),
    ] {
        let response = client
            .get(format!("{}{path}", server.base_url))
            .bearer_auth(token.as_str())
            .send()
            .await?;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        // 安定コードは通信契約(#712)。名前の変更はこの試験で検知する。
        let body: serde_json::Value = response.json().await?;
        assert_eq!(body["code"], "TRUST_READ_NOT_CONFIGURED", "{path}");
    }

    // #1061: 一括評価と観測の提供・取消も、機能未構成の node では公開しない。
    for request in [
        client
            .post(format!("{}/v1/trust/evaluations", server.base_url))
            .json(&serde_json::json!({ "targets": [target] })),
        client
            .post(format!("{}/v1/trust/observations", server.base_url))
            .json(&serde_json::json!({ "envelopes": [] })),
        client.delete(format!("{}/v1/trust/observations", server.base_url)),
    ] {
        let response = request.bearer_auth(token.as_str()).send().await?;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body: serde_json::Value = response.json().await?;
        assert_eq!(body["code"], "TRUST_READ_NOT_CONFIGURED");
    }

    // 距離利用停止の面は独立した安定コードで縮退を判別できる(#712)。
    let optout = client
        .get(format!("{}/v1/relation/optout", server.base_url))
        .bearer_auth(token.as_str())
        .send()
        .await?;
    assert_eq!(optout.status(), StatusCode::NOT_FOUND);
    let body: serde_json::Value = optout.json().await?;
    assert_eq!(body["code"], "RELATION_VISIBILITY_NOT_CONFIGURED");
    server.shutdown().await
}

#[tokio::test]
async fn viewer_relative_read_requires_authenticated_viewer() -> Result<()> {
    let Some(admin_database_url) = integration_test_admin_database_url() else {
        eprintln!("skipping cn-user-api trust read test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let (trust, _relation) = memory_trust_state();
    let server =
        TestServer::spawn(admin_database_url.as_str(), "cn_trust_auth", Some(trust)).await?;
    let client = Client::new();
    let target = generate_keys().public_key_hex();

    // 未認証の viewer 相対 read（trust / relation）は拒否される。
    for path in [
        format!("/v1/trust/users/{target}"),
        format!("/v1/relation/users/{target}"),
        "/v1/relation/neighbors".to_string(),
    ] {
        let response = client
            .get(format!("{}{path}", server.base_url))
            .send()
            .await?;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{path}");
    }

    // 認証済み read の viewer は bearer identity（鍵署名で検証済みの pubkey）に固定される:
    // 「viewer=A」を騙って read する手段が存在しない（なりすまし防止, §6.3）。
    let viewer_keys = generate_keys();
    let token = authenticate_and_consent(&client, server.base_url.as_str(), &viewer_keys).await?;
    let body: serde_json::Value = client
        .get(format!("{}/v1/trust/users/{target}", server.base_url))
        .bearer_auth(token.as_str())
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(
        body["viewer_pubkey"].as_str(),
        Some(viewer_keys.public_key_hex().as_str())
    );
    server.shutdown().await
}

#[tokio::test]
async fn trust_read_returns_components_with_basis_and_ignores_reports() -> Result<()> {
    let Some(admin_database_url) = integration_test_admin_database_url() else {
        eprintln!("skipping cn-user-api trust read test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let (trust, _relation) = memory_trust_state();
    let server =
        TestServer::spawn(admin_database_url.as_str(), "cn_trust_read", Some(trust)).await?;
    let pool = connect_postgres(server.database.database_url.as_str()).await?;
    let client = Client::new();
    let viewer_keys = generate_keys();
    let token = authenticate_and_consent(&client, server.base_url.as_str(), &viewer_keys).await?;
    let target = generate_keys().public_key_hex();

    // CSAM（critical safety, known-hash confirmed）→ 絶対成分。spam → 相対成分
    // （nsfw / objectionable は ADR 0026 §7 で advisory-only = 寄与 0。
    //   `trust_read_lists_advisory_only_basis_with_zero_contribution` を参照）。
    persist_risk_signal(
        &pool,
        "issuer-node",
        &risk_signal(
            target.as_str(),
            SafetyCategory::Csam,
            Severity::Critical,
            Basis::KnownHashMatch,
            Visibility::Public,
        ),
    )
    .await?;
    persist_risk_signal(
        &pool,
        "issuer-node",
        &risk_signal(
            target.as_str(),
            SafetyCategory::Spam,
            Severity::High,
            Basis::ClassifierScore,
            Visibility::Local,
        ),
    )
    .await?;

    let read_trust = |token: String| {
        let client = client.clone();
        let url = format!("{}/v1/trust/users/{target}", server.base_url);
        async move {
            anyhow::Ok(
                client
                    .get(url)
                    .bearer_auth(token)
                    .send()
                    .await?
                    .error_for_status()?
                    .json::<serde_json::Value>()
                    .await?,
            )
        }
    };
    let body = read_trust(token.clone()).await?;

    // 単一絶対スカラーではなく、絶対 / 相対成分 + 合成 + 根拠を返す。
    let absolute = body["absolute"].as_f64().unwrap();
    let relative = body["relative"].as_f64().unwrap();
    let trust_value = body["trust"].as_f64().unwrap();
    assert!(absolute <= -1.0 + 1e-9, "CSAM confirmed で絶対成分は最低値");
    assert!(relative < 0.0);
    assert_eq!(body["w_abs_applied"].as_f64(), Some(2.0));
    assert!((-1.0..=1.0).contains(&trust_value), "最終クランプ");
    assert!(trust_value <= -0.5, "絶対マイナスは相対で薄まらない");
    let basis = body["basis"].as_array().unwrap();
    assert_eq!(basis.len(), 2);
    for entry in basis {
        // 根拠つき advisory: issuer / basis / confidence / visibility を説明できる。
        assert!(entry["issuer_node_id"].is_string());
        assert!(entry["basis"].is_string());
        assert!(entry["visibility"].is_string());
        assert!(entry["contribution"].is_number());
    }

    // 通報（reporter identity を持たない, #370）は何件積まれても trust に影響しない
    // （report-bombing 耐性の構造的保証。trust の入力は verdict / risk signal のみで、
    //  `cn_admin.reports` から trust への経路が存在しない）。intake 経路（HTTP / 管理操作）に
    //  依存しない性質なので、通報の真実源へ直接大量投入して固定する。
    for i in 0..25 {
        insert_community_node_report(
            &pool,
            &NewCommunityNodeReport {
                subject_kind: "profile".to_string(),
                subject_id: target.clone(),
                capability: "community_index".to_string(),
                reason: format!("mass-report-{i}"),
                details: None,
                reporter_contact: None,
                appeal_risk_signal_id: None,
            },
            None,
        )
        .await?;
    }
    let after = read_trust(token.clone()).await?;
    assert_eq!(after["absolute"], body["absolute"]);
    assert_close(&after["relative"], &body["relative"]);
    assert_close(&after["trust"], &body["trust"]);

    server.shutdown().await
}

#[tokio::test]
async fn trust_read_keeps_cleared_basis_with_zero_contribution() -> Result<()> {
    let Some(admin_database_url) = integration_test_admin_database_url() else {
        eprintln!("skipping cn-user-api trust read test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let (trust, _relation) = memory_trust_state();
    let server = TestServer::spawn(
        admin_database_url.as_str(),
        "cn_trust_read_cleared",
        Some(trust),
    )
    .await?;
    let pool = connect_postgres(server.database.database_url.as_str()).await?;
    let client = Client::new();
    let viewer_keys = generate_keys();
    let token = authenticate_and_consent(&client, server.base_url.as_str(), &viewer_keys).await?;
    let target = generate_keys().public_key_hex();
    let stored = persist_risk_signal(
        &pool,
        "issuer-node",
        &risk_signal(
            target.as_str(),
            SafetyCategory::Nsfw,
            Severity::High,
            Basis::ClassifierScore,
            Visibility::Local,
        ),
    )
    .await?;
    dispute_risk_signal(&pool, stored.id.as_str()).await?;
    update_risk_signal_appeal_status(&pool, stored.id.as_str(), AppealStatus::Cleared).await?;

    let body = client
        .get(format!("{}/v1/trust/users/{target}", server.base_url))
        .bearer_auth(token)
        .send()
        .await?
        .error_for_status()?
        .json::<serde_json::Value>()
        .await?;
    assert_eq!(body["relative"].as_f64(), Some(0.0));
    assert_eq!(body["trust"].as_f64(), Some(0.0));
    let basis = body["basis"].as_array().expect("basis");
    assert_eq!(basis.len(), 1);
    assert_eq!(basis[0]["signal_id"], stored.id);
    assert_eq!(basis[0]["target"], "user_pubkey");
    assert_eq!(basis[0]["target_id"], target);
    assert_eq!(basis[0]["appeal_status"], "cleared");
    assert_eq!(basis[0]["contribution"].as_f64(), Some(0.0));

    server.shutdown().await
}

#[tokio::test]
async fn cross_node_pull_discloses_only_confirmed_absolute_component() -> Result<()> {
    let Some(admin_database_url) = integration_test_admin_database_url() else {
        eprintln!("skipping cn-user-api trust read test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let (trust, _relation) = memory_trust_state();
    let server =
        TestServer::spawn(admin_database_url.as_str(), "cn_trust_pull", Some(trust)).await?;
    let pool = connect_postgres(server.database.database_url.as_str()).await?;
    let client = Client::new();
    let target = generate_keys().public_key_hex();

    // Public + confirmed（開示される）/ Local + confirmed（返さない）/
    // Public + suspected（返さない）/ 相対成分（返さない）/ SubscribedNodes + confirmed
    // （subscriber 認証時のみ）。
    let public_confirmed = persist_risk_signal(
        &pool,
        "issuer-node",
        &risk_signal(
            target.as_str(),
            SafetyCategory::Csam,
            Severity::Critical,
            Basis::KnownHashMatch,
            Visibility::Public,
        ),
    )
    .await?;
    persist_risk_signal(
        &pool,
        "issuer-node",
        &risk_signal(
            target.as_str(),
            SafetyCategory::Cse,
            Severity::Critical,
            Basis::ProviderVerdict,
            Visibility::Local,
        ),
    )
    .await?;
    persist_risk_signal(
        &pool,
        "issuer-node",
        &risk_signal(
            target.as_str(),
            SafetyCategory::Grooming,
            Severity::High,
            Basis::ClassifierScore,
            Visibility::Public,
        ),
    )
    .await?;
    persist_risk_signal(
        &pool,
        "issuer-node",
        &risk_signal(
            target.as_str(),
            SafetyCategory::Nsfw,
            Severity::High,
            Basis::KnownHashMatch,
            Visibility::Public,
        ),
    )
    .await?;
    let subscribed_confirmed = persist_risk_signal(
        &pool,
        "issuer-node",
        &risk_signal(
            target.as_str(),
            SafetyCategory::Csam,
            Severity::Critical,
            Basis::ProviderVerdict,
            Visibility::SubscribedNodes,
        ),
    )
    .await?;

    let pull_url = format!("{}/v1/trust/pull/{target}", server.base_url);
    let signal_ids = |body: &serde_json::Value| -> Vec<String> {
        body["basis"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["signal_id"].as_str().unwrap_or_default().to_string())
            .collect()
    };

    // 匿名 pull: Public + confirmed + 絶対成分のみ。
    let anonymous: serde_json::Value = client
        .get(pull_url.as_str())
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(signal_ids(&anonymous), vec![public_confirmed.id.clone()]);
    assert!(anonymous.get("relative").is_none(), "相対成分は返さない");
    assert!(anonymous.get("trust").is_none(), "合成値も返さない");

    // 有効なbearerでも同意前はSubscribedNodesへ昇格しない。
    let subscriber_keys = generate_keys();
    let token = authenticate_only(&client, server.base_url.as_str(), &subscriber_keys).await?;
    let no_consent = client
        .get(pull_url.as_str())
        .bearer_auth(token.as_str())
        .send()
        .await?;
    assert_eq!(no_consent.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        no_consent.json::<serde_json::Value>().await?["code"],
        "CONSENT_REQUIRED"
    );

    // current exact consent後だけSubscribedNodes visibilityのconfirmedも開示される。
    accept_required_consents(&client, server.base_url.as_str(), token.as_str()).await?;
    let subscribed: serde_json::Value = client
        .get(pull_url.as_str())
        .bearer_auth(token.as_str())
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let mut ids = signal_ids(&subscribed);
    ids.sort();
    let mut expected = vec![public_confirmed.id.clone(), subscribed_confirmed.id];
    expected.sort();
    assert_eq!(ids, expected);

    // policy更新で同意が旧snapshotになったbearerは再び拒否する。
    support::advance_policy_snapshot(&pool, "issue-860-trust-next-snapshot").await?;
    let old_snapshot = client
        .get(pull_url.as_str())
        .bearer_auth(token.as_str())
        .send()
        .await?;
    assert_eq!(old_snapshot.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        old_snapshot.json::<serde_json::Value>().await?["code"],
        "CONSENT_REQUIRED"
    );
    let anonymous_after_rotation: serde_json::Value = client
        .get(pull_url.as_str())
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(
        signal_ids(&anonymous_after_rotation),
        vec![public_confirmed.id]
    );

    server.shutdown().await
}

#[tokio::test]
async fn relation_distance_optout_is_explicit_symmetric_and_reversible() -> Result<()> {
    let Some(admin_database_url) = integration_test_admin_database_url() else {
        eprintln!("skipping cn-user-api trust read test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let (trust, relation) = memory_trust_state();
    let server = TestServer::spawn(admin_database_url.as_str(), "cn_relation", Some(trust)).await?;
    let pool = connect_postgres(server.database.database_url.as_str()).await?;
    let client = Client::new();

    let viewer_keys = generate_keys();
    let target_keys = generate_keys();
    let other_keys = generate_keys();
    let viewer = viewer_keys.public_key_hex();
    let target = target_keys.public_key_hex();
    let other = other_keys.public_key_hex();
    let viewer_token =
        authenticate_and_consent(&client, server.base_url.as_str(), &viewer_keys).await?;
    let target_token =
        authenticate_and_consent(&client, server.base_url.as_str(), &target_keys).await?;

    // target は遠距離、other は閾値以上として relation graph を seedする。
    relation
        .upsert_edge(
            viewer.as_str(),
            target.as_str(),
            &EdgeFeatures::new().with(FEATURE_SHARED_TOPICS, 0.1),
        )
        .await?;
    relation
        .upsert_edge(
            viewer.as_str(),
            other.as_str(),
            &EdgeFeatures::new().with(FEATURE_SHARED_TOPICS, 3.0),
        )
        .await?;

    // pairwise read: 根拠つき proximity が返る。
    let relation_url = format!("{}/v1/relation/users/{target}", server.base_url);
    let body: serde_json::Value = client
        .get(relation_url.as_str())
        .bearer_auth(viewer_token.as_str())
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert!(body["score"].as_f64().unwrap() > 0.0);
    assert!(!body["basis"].as_array().unwrap().is_empty(), "根拠つき");

    // neighbors: proximity 降順で返る。
    let neighbors_url = format!("{}/v1/relation/neighbors", server.base_url);
    let neighbors: serde_json::Value = client
        .get(neighbors_url.as_str())
        .bearer_auth(viewer_token.as_str())
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(
        neighbors["neighbors"].as_array().unwrap().len(),
        2,
        "opt-out 前は両方見える"
    );

    // 未選択時は遠距離でも自動抑制しない。
    let initial_status: serde_json::Value = client
        .get(format!("{}/v1/relation/optout", server.base_url))
        .bearer_auth(target_token.as_str())
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(initial_status["opted_out"].as_bool(), Some(false));
    assert_eq!(initial_status["min_proximity"].as_f64(), Some(0.5));

    // target がdistance opt-outを選ぶ（自分自身のみ・user-controlled）。
    let optout: serde_json::Value = client
        .put(format!("{}/v1/relation/optout", server.base_url))
        .bearer_auth(target_token.as_str())
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(optout["opted_out"].as_bool(), Some(true));
    assert_eq!(optout["pubkey"].as_str(), Some(target.as_str()));
    assert_eq!(optout["min_proximity"].as_f64(), Some(0.5));

    // 遠距離のtargetだけがrelation read / neighborsから消え、近距離のotherは残る。
    let hidden = client
        .get(relation_url.as_str())
        .bearer_auth(viewer_token.as_str())
        .send()
        .await?;
    assert_eq!(hidden.status(), StatusCode::NOT_FOUND);
    // 距離利用停止の抑制は存在秘匿と同じ RELATION_NOT_FOUND を返す(#712)。
    assert_eq!(
        hidden.json::<serde_json::Value>().await?["code"],
        "RELATION_NOT_FOUND"
    );
    let neighbors_after: serde_json::Value = client
        .get(neighbors_url.as_str())
        .bearer_auth(viewer_token.as_str())
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let visible: Vec<&str> = neighbors_after["neighbors"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert_eq!(visible, vec![other.as_str()]);

    // opt-out は trust には影響しない（troll 判定回避の手段にしない）: opt-out した target の
    // trust read は変わらず取得でき、risk signal の寄与も変わらない。
    persist_risk_signal(
        &pool,
        "issuer-node",
        &risk_signal(
            target.as_str(),
            SafetyCategory::Spam,
            Severity::Medium,
            Basis::ClassifierScore,
            Visibility::Local,
        ),
    )
    .await?;
    let trust_body: serde_json::Value = client
        .get(format!("{}/v1/trust/users/{target}", server.base_url))
        .bearer_auth(viewer_token.as_str())
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert!(trust_body["relative"].as_f64().unwrap() < 0.0);

    // 可逆: 解除すれば見え直す（canonical 削除ではない node-local の選択）。
    client
        .delete(format!("{}/v1/relation/optout", server.base_url))
        .bearer_auth(target_token.as_str())
        .send()
        .await?
        .error_for_status()?;
    let restored = client
        .get(relation_url.as_str())
        .bearer_auth(viewer_token.as_str())
        .send()
        .await?;
    assert_eq!(restored.status(), StatusCode::OK);

    // viewer側の選択も同じpairを抑制する（相互・向きに依存しない）。
    client
        .put(format!("{}/v1/relation/optout", server.base_url))
        .bearer_auth(viewer_token.as_str())
        .send()
        .await?
        .error_for_status()?;
    let hidden_by_viewer = client
        .get(relation_url.as_str())
        .bearer_auth(viewer_token.as_str())
        .send()
        .await?;
    assert_eq!(hidden_by_viewer.status(), StatusCode::NOT_FOUND);
    client
        .delete(format!("{}/v1/relation/optout", server.base_url))
        .bearer_auth(viewer_token.as_str())
        .send()
        .await?
        .error_for_status()?;

    // edge の無い相手へのreadは従来どおりrelation未観測の404。
    let stranger = generate_keys().public_key_hex();
    let cross = client
        .get(format!("{}/v1/relation/users/{stranger}", server.base_url))
        .bearer_auth(viewer_token.as_str())
        .send()
        .await?;
    assert_eq!(cross.status(), StatusCode::NOT_FOUND);

    server.shutdown().await
}

/// ADR 0026 §7.2 contract: `trust_read_lists_advisory_only_basis_with_zero_contribution`。
///
/// nsfw / objectionable の signal は利用者向け read の basis に `raw_contribution = 0` /
/// `contribution = 0` で残り（判定と appeal 状態を確認できる）、`relative` / `trust` は動かない。
/// cross-node pull には（visibility が Public でも）出ない。
#[tokio::test]
async fn trust_read_lists_advisory_only_basis_with_zero_contribution() -> Result<()> {
    let Some(admin_database_url) = integration_test_admin_database_url() else {
        eprintln!("skipping cn-user-api trust read test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let (trust, _relation) = memory_trust_state();
    let server = TestServer::spawn(
        admin_database_url.as_str(),
        "cn_trust_read_advisory",
        Some(trust),
    )
    .await?;
    let pool = connect_postgres(server.database.database_url.as_str()).await?;
    let client = Client::new();
    let viewer_keys = generate_keys();
    let token = authenticate_and_consent(&client, server.base_url.as_str(), &viewer_keys).await?;
    let target = generate_keys().public_key_hex();

    let mut nsfw = risk_signal(
        target.as_str(),
        SafetyCategory::Nsfw,
        Severity::Low,
        Basis::ClassifierScore,
        Visibility::Public,
    );
    nsfw.confidence = Some(84);
    let nsfw = persist_risk_signal(&pool, "issuer-node", &nsfw).await?;
    let objectionable = persist_risk_signal(
        &pool,
        "issuer-node",
        &risk_signal(
            target.as_str(),
            SafetyCategory::Objectionable,
            Severity::Low,
            Basis::ClassifierScore,
            Visibility::Local,
        ),
    )
    .await?;

    let read_url = format!("{}/v1/trust/users/{target}", server.base_url);
    let body = client
        .get(read_url.as_str())
        .bearer_auth(token.as_str())
        .send()
        .await?
        .error_for_status()?
        .json::<serde_json::Value>()
        .await?;
    assert_eq!(body["absolute"].as_f64(), Some(0.0));
    assert_eq!(
        body["relative"].as_f64(),
        Some(0.0),
        "advisory-only は相対成分を動かさない"
    );
    assert_eq!(body["trust"].as_f64(), Some(0.0));
    let basis = body["basis"].as_array().expect("basis");
    assert_eq!(basis.len(), 2, "basis には判定として残る");
    for entry in basis {
        assert_eq!(entry["component"], "relative");
        assert_eq!(entry["basis"], "classifier_score");
        assert_eq!(entry["severity"], "low");
        assert_eq!(entry["appeal_status"], "none");
        assert_eq!(entry["raw_contribution"].as_f64(), Some(0.0));
        assert_eq!(entry["contribution"].as_f64(), Some(0.0));
        assert!(entry["issuer_node_id"].is_string());
        assert!(entry["confidence"].is_number());
    }
    let categories: Vec<&str> = basis
        .iter()
        .map(|entry| entry["category"].as_str().unwrap())
        .collect();
    assert!(categories.contains(&"nsfw"));
    assert!(categories.contains(&"objectionable"));

    // spam（相対成分）を足すと値が動くが、advisory-only の 2 件は引き続き 0 のまま。
    persist_risk_signal(
        &pool,
        "issuer-node",
        &risk_signal(
            target.as_str(),
            SafetyCategory::Spam,
            Severity::High,
            Basis::ClassifierScore,
            Visibility::Local,
        ),
    )
    .await?;
    let with_spam = client
        .get(read_url.as_str())
        .bearer_auth(token.as_str())
        .send()
        .await?
        .error_for_status()?
        .json::<serde_json::Value>()
        .await?;
    assert!(with_spam["relative"].as_f64().unwrap() < 0.0);
    for entry in with_spam["basis"].as_array().unwrap() {
        let id = entry["signal_id"].as_str().unwrap();
        if id == nsfw.id || id == objectionable.id {
            assert_eq!(entry["contribution"].as_f64(), Some(0.0));
        } else {
            assert!(entry["contribution"].as_f64().unwrap() < 0.0);
        }
    }

    // Disputed → Cleared と遷移しても評価値は動かず、basis の状態表示だけが変わる（§7.3）。
    dispute_risk_signal(&pool, nsfw.id.as_str()).await?;
    let disputed = client
        .get(read_url.as_str())
        .bearer_auth(token.as_str())
        .send()
        .await?
        .error_for_status()?
        .json::<serde_json::Value>()
        .await?;
    assert_close(&disputed["relative"], &with_spam["relative"]);
    update_risk_signal_appeal_status(&pool, nsfw.id.as_str(), AppealStatus::Cleared).await?;
    let cleared = client
        .get(read_url.as_str())
        .bearer_auth(token.as_str())
        .send()
        .await?
        .error_for_status()?
        .json::<serde_json::Value>()
        .await?;
    assert_close(&cleared["relative"], &with_spam["relative"]);
    let nsfw_entry = cleared["basis"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["signal_id"] == nsfw.id)
        .expect("cleared nsfw basis stays listed");
    assert_eq!(nsfw_entry["appeal_status"], "cleared");
    assert_eq!(nsfw_entry["contribution"].as_f64(), Some(0.0));

    // cross-node pull（匿名）: advisory-only は Public visibility でも出ない（INVAR-4）。
    let pulled = client
        .get(format!("{}/v1/trust/pull/{target}", server.base_url))
        .send()
        .await?
        .error_for_status()?
        .json::<serde_json::Value>()
        .await?;
    assert!(pulled["basis"].as_array().unwrap().is_empty());
    assert_eq!(pulled["absolute"].as_f64(), Some(0.0));

    server.shutdown().await
}

/// 単体照会と pull の basis を cursor で辿る（ADR 0026 §10、#1702 AC-1 (d)）。
#[allow(clippy::unwrap_used)] // test fixture helper
async fn basis_pages(
    client: &Client,
    url: &str,
    token: Option<&str>,
) -> Result<(Vec<String>, Vec<usize>)> {
    let (mut ids, mut sizes, mut cursor) = (Vec::new(), Vec::new(), None::<String>);
    loop {
        let mut request = client.get(url);
        if let Some(cursor) = &cursor {
            request = request.query(&[("cursor", cursor.as_str())]);
        }
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        let body: serde_json::Value = request.send().await?.error_for_status()?.json().await?;
        let page = body["basis"].as_array().context("basis")?;
        sizes.push(page.len());
        ids.extend(
            page.iter()
                .map(|entry| entry["signal_id"].as_str().unwrap().to_string()),
        );
        match body["basis_next_cursor"].as_str() {
            Some(next) => cursor = Some(next.to_string()),
            None => return Ok((ids, sizes)),
        }
    }
}

#[tokio::test]
async fn trust_read_and_pull_page_basis_with_cursor() -> Result<()> {
    let Some(admin_database_url) = integration_test_admin_database_url() else {
        eprintln!("skipping cn-user-api trust read test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let (trust, _relation) = memory_trust_state();
    let server =
        TestServer::spawn(admin_database_url.as_str(), "cn_trust_pages", Some(trust)).await?;
    let pool = connect_postgres(server.database.database_url.as_str()).await?;
    let client = Client::new();
    let token =
        authenticate_and_consent(&client, server.base_url.as_str(), &generate_keys()).await?;
    let target = generate_keys().public_key_hex();

    // 対象が著者の内容に、開示できる絶対成分（csam・known-hash・public）60 件と相対成分（spam）60 件。
    sqlx::query(
        "INSERT INTO cn_safety.risk_signal_subject_authors (target, target_id, author_pubkey)
         SELECT 'post_id', 'post-' || g, $1 FROM generate_series(1, 120) g",
    )
    .bind(target.as_str())
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO cn_safety.risk_signals
            (id, issuer_node_id, target, target_id, category, severity, basis, visibility,
             confidence, appeal_status, persisted_at)
         SELECT 'sig-' || lpad(g::text, 3, '0'), 'issuer-node', 'post_id', 'post-' || g,
                CASE WHEN g <= 60 THEN 'csam' ELSE 'spam' END, 'low',
                CASE WHEN g <= 60 THEN 'known_hash_match' ELSE 'classifier_score' END,
                CASE WHEN g <= 60 THEN 'public' ELSE 'local' END, 1, 'none',
                NOW() - make_interval(secs => g)
         FROM generate_series(1, 120) g",
    )
    .execute(&pool)
    .await?;
    // 絶対成分が先、各成分は新しい順（persisted_at は g が小さいほど新しい）。
    let expected: Vec<String> = (1..=120).map(|g| format!("sig-{g:03}")).collect();

    let read_url = format!("{}/v1/trust/users/{target}", server.base_url);
    let (ids, sizes) = basis_pages(&client, read_url.as_str(), Some(token.as_str())).await?;
    assert_eq!(sizes, vec![50, 50, 20]);
    assert_eq!(ids, expected);

    let pull_url = format!("{}/v1/trust/pull/{target}", server.base_url);
    let (ids, sizes) = basis_pages(&client, pull_url.as_str(), None).await?;
    assert_eq!(sizes, vec![50, 10]);
    assert_eq!(ids, expected[..60]);

    // 読めない cursor は 400。
    let invalid = client
        .get(read_url.as_str())
        .query(&[("cursor", "not-a-cursor")])
        .bearer_auth(token.as_str())
        .send()
        .await?;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        invalid.json::<serde_json::Value>().await?["code"],
        "INVALID_TRUST_QUERY"
    );
    server.shutdown().await
}
