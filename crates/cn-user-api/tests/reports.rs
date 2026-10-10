//! 通報受信エンドポイント (#370) の contract test。
//!
//! `POST /v1/report` は report_endpoint capability を有効化した node でのみ受理し、authority scope
//! 内の対象に対する通報を保存する。受理（200 + reference_id）・必須欠落の拒否（400）・capability 無効
//! node での拒否（404）を再現する。Postgres を要するため `KUKURI_CN_RUN_INTEGRATION_TESTS=1` で gate する。

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use kukuri_cn_core::{
    AppealReviewOperation, JwtConfig, RiskSignalCorrection, TestDatabase,
    apply_appeal_review_action, get_appeal_review,
};
use kukuri_cn_protocol::{CommunityNodeReportAppeal, CommunityNodeReportRequest};
use kukuri_cn_trust::{MemoryRelationStore, TrustParams};
use kukuri_cn_user_api::{TrustReadState, UserApiConfig, app_router, build_state};
use kukuri_core::{KukuriKeys, generate_keys};
use reqwest::{Client, StatusCode};

mod support;
use support::{
    accept_required_consents, integration_test_admin_database_url,
    integration_test_rendezvous_redis_url,
};

/// report_endpoint capability を有効化した operator config。
const REPORT_ENABLED_YAML: &str = r#"server:
  domain: example-kukuri.net
  node_id: issuer-node-1
  operator_name: Example Operator
  country: JP
features:
  report_endpoint: true
acknowledge_planned_capabilities: true
"#;

/// report_endpoint capability を有効化しない operator config。
const REPORT_DISABLED_YAML: &str = r#"server:
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
        Self::spawn_inner(admin_database_url, prefix, operator_yaml, None, None).await
    }

    /// 発行元識別子(署名鍵の公開鍵 hex 等)を与えて起動する。公開ノード情報の node_id と
    /// 一致しない場合は起動が失敗する(#706)。
    async fn spawn_with_expected_issuer(
        admin_database_url: &str,
        prefix: &str,
        operator_yaml: &str,
        expected_issuer_node_id: &str,
    ) -> Result<Self> {
        Self::spawn_inner(
            admin_database_url,
            prefix,
            operator_yaml,
            None,
            Some(expected_issuer_node_id.to_string()),
        )
        .await
    }

    async fn spawn_with_trust(
        admin_database_url: &str,
        prefix: &str,
        operator_yaml: &str,
        trust: Arc<TrustReadState>,
    ) -> Result<Self> {
        Self::spawn_inner(admin_database_url, prefix, operator_yaml, Some(trust), None).await
    }

    async fn spawn_inner(
        admin_database_url: &str,
        prefix: &str,
        operator_yaml: &str,
        trust: Option<Arc<TrustReadState>>,
        expected_issuer_node_id: Option<String>,
    ) -> Result<Self> {
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
            .context("failed to bind test report listener")?;
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
            legal_data_key: Some("unit-test-legal-data-key-0123456789abcdef".to_string()),
            index_query_enabled: false,
            indexer_data_dir: Default::default(),
            trust_read_enabled: false,
            relation_distance_optout_min_proximity: None,
            deployment_revision: "test-deployment-v1".to_string(),
            readiness_activation_max_age_secs: 3600,
            expected_issuer_node_id,
        })
        .await;
        let mut state = match state {
            Ok(state) => state,
            Err(error) => {
                database.cleanup().await?;
                return Err(error);
            }
        };
        if let Some(trust) = trust {
            state = state.with_trust_read(trust);
        }
        let app = app_router(state);
        let task = tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .expect("report server");
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

async fn authenticate_and_consent(
    client: &Client,
    base_url: &str,
    keys: &KukuriKeys,
) -> Result<String> {
    let challenge = client
        .post(format!("{base_url}/v1/auth/challenge"))
        .json(&serde_json::json!({ "pubkey": keys.public_key_hex() }))
        .send()
        .await?
        .error_for_status()?
        .json::<kukuri_cn_protocol::AuthChallengeResponse>()
        .await?;
    let auth_envelope_json =
        kukuri_cn_protocol::build_auth_envelope_json(keys, challenge.challenge.as_str(), base_url)?;
    let verify = client
        .post(format!("{base_url}/v1/auth/verify"))
        .json(&serde_json::json!({
            "auth_envelope_json": auth_envelope_json,
            "endpoint_id": "appeal-test-peer",
        }))
        .send()
        .await?
        .error_for_status()?
        .json::<kukuri_cn_protocol::AuthVerifyResponse>()
        .await?;
    accept_required_consents(client, base_url, verify.access_token.as_str()).await?;
    Ok(verify.access_token)
}

#[tokio::test]
async fn report_endpoint_accepts_stores_and_validates() -> Result<()> {
    let Some(admin_database_url) = integration_test_admin_database_url() else {
        eprintln!("skipping cn-user-api report test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let server = TestServer::spawn(
        admin_database_url.as_str(),
        "cn_user_api_report",
        REPORT_ENABLED_YAML,
    )
    .await?;
    let client = Client::new();

    // 受理：authority scope 内の対象への通報を保存し reference_id を返す。
    let accepted = client
        .post(format!("{}/v1/report", server.base_url))
        .json(&serde_json::json!({
            "subject_kind": "post",
            "subject_id": "object-123",
            "capability": "community_index",
            "reason": "spam",
            "details": "repeated spam",
        }))
        .send()
        .await?;
    assert_eq!(accepted.status(), StatusCode::OK);
    let body = accepted.json::<serde_json::Value>().await?;
    let reference_id = body["reference_id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(!reference_id.is_empty(), "reference_id must be returned");

    // 匿名（reporter_contact 無し）でも受理する。
    let anonymous = client
        .post(format!("{}/v1/report", server.base_url))
        .json(&serde_json::json!({
            "subject_kind": "profile",
            "subject_id": "pubkey-abc",
            "capability": "moderation",
            "reason": "harassment",
        }))
        .send()
        .await?;
    assert_eq!(anonymous.status(), StatusCode::OK);

    // 権利侵害の申出内容や連絡先は一般通報へ保存せず、専用受付へ案内する。
    let rights_request = client
        .post(format!("{}/v1/report", server.base_url))
        .json(&serde_json::json!({
            "subject_kind": "post",
            "subject_id": "object-rights",
            "capability": "community_index",
            "reason": "rights_infringement",
            "details": "一般通報へ保存してはならない権利主張",
            "reporter_contact": "rights-holder@example.com",
        }))
        .send()
        .await?;
    assert_eq!(rights_request.status(), StatusCode::CONFLICT);
    let rights_request_body = rights_request.json::<serde_json::Value>().await?;
    assert_eq!(
        rights_request_body["code"],
        "RIGHTS_REQUEST_REQUIRES_DEDICATED_INTAKE"
    );

    // 必須欠落は 400。
    let invalid = client
        .post(format!("{}/v1/report", server.base_url))
        .json(&serde_json::json!({
            "subject_kind": "post",
            "subject_id": "",
            "capability": "community_index",
            "reason": "spam",
        }))
        .send()
        .await?;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    let invalid_body = invalid.json::<serde_json::Value>().await?;
    assert_eq!(invalid_body["code"], "INVALID_REPORT");

    server.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn report_endpoint_accepts_appeal_and_disputes_advisory() -> Result<()> {
    // #420 / ADR 0028 §2.8: issuer node への異議申し立て導線。appeal 参照付きの通報を
    // 受理すると、対象 risk signal の AppealStatus が None → Disputed に遷移する。
    let Some(admin_database_url) = integration_test_admin_database_url() else {
        eprintln!("skipping cn-user-api report test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let server = TestServer::spawn(
        admin_database_url.as_str(),
        "cn_user_api_appeal",
        REPORT_ENABLED_YAML,
    )
    .await?;
    let client = Client::new();

    // issuer node（= この node）が発行済みの suspected advisory を用意する。
    let pool = kukuri_cn_core::connect_postgres(server.database.database_url.as_str()).await?;
    let stored = kukuri_cn_core::persist_risk_signal(
        &pool,
        "issuer-node-1",
        &kukuri_cn_safety::SafetyRiskSignal {
            target: kukuri_cn_safety::RiskSignalTarget::PostId,
            target_id: "post-appealed".to_string(),
            category: kukuri_cn_safety::SafetyCategory::Nsfw,
            severity: kukuri_cn_safety::Severity::High,
            basis: kukuri_cn_safety::Basis::ClassifierScore,
            confidence: Some(90),
            visibility: kukuri_cn_safety::Visibility::Local,
            expires_at: None,
            appeal_status: Some(kukuri_cn_safety::AppealStatus::None),
        },
    )
    .await?;

    // 判定識別子と対象識別子だけが一致しても、元の対象種別と異なる申立ては拒否する。
    let wrong_kind = client
        .post(format!("{}/v1/report", server.base_url))
        .json(&serde_json::json!({
            "subject_kind": "profile",
            "subject_id": "post-appealed",
            "capability": "moderation",
            "reason": "false_positive",
            "appeal": { "risk_signal_id": stored.id },
        }))
        .send()
        .await?;
    assert_eq!(wrong_kind.status(), StatusCode::BAD_REQUEST);
    let wrong_kind_body = wrong_kind.json::<serde_json::Value>().await?;
    assert_eq!(wrong_kind_body["code"], "INVALID_APPEAL");

    let accepted = client
        .post(format!("{}/v1/report", server.base_url))
        .json(&serde_json::json!({
            "subject_kind": "post",
            "subject_id": "post-appealed",
            "capability": "moderation",
            "reason": "false_positive",
            "details": "this was misclassified",
            "reporter_contact": "must-not-be-stored@example.com",
            "appeal": { "risk_signal_id": stored.id },
        }))
        .send()
        .await?;
    assert_eq!(accepted.status(), StatusCode::OK);
    let body = accepted.json::<serde_json::Value>().await?;
    assert_eq!(body["disputed_risk_signal_id"], stored.id.as_str());
    let reference_id = body["reference_id"].as_str().expect("reference id");
    let linked = kukuri_cn_core::get_community_node_report(&pool, reference_id)
        .await?
        .expect("linked appeal report");
    assert_eq!(
        linked.appeal_risk_signal_id.as_deref(),
        Some(stored.id.as_str())
    );
    assert!(linked.reporter_contact.is_none());

    let disputed = kukuri_cn_core::get_risk_signal(&pool, &stored.id)
        .await?
        .expect("signal exists");
    assert_eq!(
        disputed.signal.appeal_status,
        Some(kukuri_cn_safety::AppealStatus::Disputed)
    );

    // 利用者対象の判定はプロフィールを対象として受理する。
    let user_signal = kukuri_cn_core::persist_risk_signal(
        &pool,
        "issuer-node-1",
        &kukuri_cn_safety::SafetyRiskSignal {
            target: kukuri_cn_safety::RiskSignalTarget::UserPubkey,
            target_id: "author-pubkey".to_string(),
            category: kukuri_cn_safety::SafetyCategory::Spam,
            severity: kukuri_cn_safety::Severity::Medium,
            basis: kukuri_cn_safety::Basis::ClassifierScore,
            confidence: Some(80),
            visibility: kukuri_cn_safety::Visibility::Local,
            expires_at: None,
            appeal_status: Some(kukuri_cn_safety::AppealStatus::None),
        },
    )
    .await?;
    let accepted_user = client
        .post(format!("{}/v1/report", server.base_url))
        .json(&serde_json::json!({
            "subject_kind": "profile",
            "subject_id": "author-pubkey",
            "capability": "moderation",
            "reason": "false_positive",
            "appeal": { "risk_signal_id": user_signal.id },
        }))
        .send()
        .await?;
    assert_eq!(accepted_user.status(), StatusCode::OK);

    // 存在しない advisory への appeal は 400 で拒否し、report も保存しない。
    let unknown = client
        .post(format!("{}/v1/report", server.base_url))
        .json(&serde_json::json!({
            "subject_kind": "post",
            "subject_id": "post-appealed",
            "capability": "moderation",
            "reason": "false_positive",
            "appeal": { "risk_signal_id": "no-such-signal" },
        }))
        .send()
        .await?;
    assert_eq!(unknown.status(), StatusCode::BAD_REQUEST);
    let unknown_body = unknown.json::<serde_json::Value>().await?;
    assert_eq!(unknown_body["code"], "INVALID_APPEAL");

    let foreign = kukuri_cn_core::persist_risk_signal(
        &pool,
        "another-node",
        &kukuri_cn_safety::SafetyRiskSignal {
            target: kukuri_cn_safety::RiskSignalTarget::PostId,
            target_id: "post-foreign".to_string(),
            category: kukuri_cn_safety::SafetyCategory::Nsfw,
            severity: kukuri_cn_safety::Severity::High,
            basis: kukuri_cn_safety::Basis::ClassifierScore,
            confidence: Some(80),
            visibility: kukuri_cn_safety::Visibility::Local,
            expires_at: None,
            appeal_status: Some(kukuri_cn_safety::AppealStatus::None),
        },
    )
    .await?;
    let rejected = client
        .post(format!("{}/v1/report", server.base_url))
        .json(&serde_json::json!({
            "subject_kind": "post",
            "subject_id": "post-foreign",
            "capability": "moderation",
            "reason": "false_positive",
            "appeal": { "risk_signal_id": foreign.id },
        }))
        .send()
        .await?;
    assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        kukuri_cn_core::get_risk_signal(&pool, &foreign.id)
            .await?
            .expect("foreign signal")
            .signal
            .appeal_status,
        Some(kukuri_cn_safety::AppealStatus::None)
    );

    server.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn post_appeal_acceptance_is_visible_after_trust_refetch() -> Result<()> {
    let Some(admin_database_url) = integration_test_admin_database_url() else {
        eprintln!("skipping cn-user-api report test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let trust = Arc::new(TrustReadState {
        params: TrustParams::default(),
        relation: Arc::new(MemoryRelationStore::new()),
    });
    let server = TestServer::spawn_with_trust(
        admin_database_url.as_str(),
        "cn_user_api_appeal_refetch",
        REPORT_ENABLED_YAML,
        trust,
    )
    .await?;
    let pool = kukuri_cn_core::connect_postgres(server.database.database_url.as_str()).await?;
    let client = Client::new();
    let viewer = generate_keys();
    let author = generate_keys();
    let author_pubkey = author.public_key_hex();
    let token = authenticate_and_consent(&client, server.base_url.as_str(), &viewer).await?;
    let stored = kukuri_cn_core::persist_risk_signal_with_author(
        &pool,
        "issuer-node-1",
        &kukuri_cn_safety::SafetyRiskSignal {
            target: kukuri_cn_safety::RiskSignalTarget::PostId,
            target_id: "post-appeal-refetch".to_string(),
            category: kukuri_cn_safety::SafetyCategory::Csam,
            severity: kukuri_cn_safety::Severity::Critical,
            basis: kukuri_cn_safety::Basis::KnownHashMatch,
            confidence: Some(100),
            visibility: kukuri_cn_safety::Visibility::Local,
            expires_at: None,
            appeal_status: Some(kukuri_cn_safety::AppealStatus::None),
        },
        Some(author_pubkey.as_str()),
    )
    .await?;
    let trust_url = format!("{}/v1/trust/users/{author_pubkey}", server.base_url);
    let before = client
        .get(trust_url.as_str())
        .bearer_auth(token.as_str())
        .send()
        .await?
        .error_for_status()?
        .json::<serde_json::Value>()
        .await?;
    assert!(before["absolute"].as_f64().is_some_and(|value| value < 0.0));
    assert_eq!(before["basis"][0]["target"], "post_id");
    assert_eq!(before["basis"][0]["target_id"], "post-appeal-refetch");

    let appeal = CommunityNodeReportRequest {
        subject_kind: "post".to_string(),
        subject_id: "post-appeal-refetch".to_string(),
        capability: "moderation".to_string(),
        reason: "false_positive".to_string(),
        details: Some("誤判定として再確認を求めます".to_string()),
        reporter_contact: None,
        appeal: Some(CommunityNodeReportAppeal {
            risk_signal_id: stored.id.clone(),
        }),
    };
    let submitted = client
        .post(format!("{}/v1/report", server.base_url))
        .json(&appeal)
        .send()
        .await?
        .error_for_status()?
        .json::<kukuri_cn_protocol::CommunityNodeReportResponse>()
        .await?;
    assert_eq!(
        submitted.disputed_risk_signal_id.as_deref(),
        Some(stored.id.as_str())
    );

    let expected = get_appeal_review(&pool, stored.id.as_str())
        .await?
        .expect("appeal review")
        .version();
    apply_appeal_review_action(
        &pool,
        "ops@kukuri.app",
        stored.id.as_str(),
        &AppealReviewOperation::Accept { expected },
        true,
    )
    .await?;

    let after = client
        .get(trust_url)
        .bearer_auth(token)
        .send()
        .await?
        .error_for_status()?
        .json::<serde_json::Value>()
        .await?;
    assert_eq!(after["absolute"].as_f64(), Some(0.0));
    assert_eq!(after["trust"].as_f64(), Some(0.0));
    assert_eq!(after["basis"][0]["signal_id"], stored.id);
    assert_eq!(after["basis"][0]["target"], "post_id");
    assert_eq!(after["basis"][0]["appeal_status"], "cleared");
    assert_eq!(after["basis"][0]["contribution"].as_f64(), Some(0.0));

    server.shutdown().await
}

#[tokio::test]
async fn report_endpoint_rejects_when_capability_disabled() -> Result<()> {
    let Some(admin_database_url) = integration_test_admin_database_url() else {
        eprintln!("skipping cn-user-api report test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let server = TestServer::spawn(
        admin_database_url.as_str(),
        "cn_user_api_report_off",
        REPORT_DISABLED_YAML,
    )
    .await?;
    let client = Client::new();

    // report_endpoint capability を有効化していない node は通報を受け付けない（404）。
    let rejected = client
        .post(format!("{}/v1/report", server.base_url))
        .json(&serde_json::json!({
            "subject_kind": "post",
            "subject_id": "object-123",
            "capability": "community_index",
            "reason": "spam",
        }))
        .send()
        .await?;
    assert_eq!(rejected.status(), StatusCode::NOT_FOUND);
    let body = rejected.json::<serde_json::Value>().await?;
    assert_eq!(body["code"], "REPORT_NOT_CONFIGURED");

    server.shutdown().await?;
    Ok(())
}

/// テスト専用の決定論的な署名鍵(本番鍵ではない)。
const TEST_SIGNING_KEY: &str = "0000000000000000000000000000000000000000000000000000000000000001";

#[tokio::test]
async fn appeal_is_accepted_when_manifest_node_id_matches_signing_key_issuer() -> Result<()> {
    // #706: リスク判定の issuer_node_id は署名鍵の公開鍵 hex。公開ノード情報の node_id を同じ値に
    // した構成で異議申し立てが端から端まで通ること、別値の構成では起動時に拒否されることを固定する。
    let Some(admin_database_url) = integration_test_admin_database_url() else {
        eprintln!("skipping cn-user-api report test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let signer =
        kukuri_cn_safety_runtime::Secp256k1ModerationEventSigner::from_secret(TEST_SIGNING_KEY)?;
    let issuer = kukuri_cn_safety::ModerationEventSigner::issuer_node_id(&signer).to_string();
    let matching_yaml =
        REPORT_ENABLED_YAML.replace("node_id: issuer-node-1", &format!("node_id: {issuer}"));

    // node_id が発行元識別子と一致しない構成(既存 fixture の issuer-node-1)は起動を拒否する。
    let mismatch = TestServer::spawn_with_expected_issuer(
        admin_database_url.as_str(),
        "cn_user_api_issuer_mismatch",
        REPORT_ENABLED_YAML,
        issuer.as_str(),
    )
    .await;
    let error = mismatch
        .err()
        .expect("mismatched node_id must refuse to start");
    let message = format!("{error:#}");
    assert!(message.contains("server.node_id"), "{message}");
    assert!(message.contains(issuer.as_str()), "{message}");

    // 一致する構成では、署名鍵由来の issuer で保存した判定への異議申し立てが受理される。
    let server = TestServer::spawn_with_expected_issuer(
        admin_database_url.as_str(),
        "cn_user_api_issuer_match",
        matching_yaml.as_str(),
        issuer.as_str(),
    )
    .await?;
    let manifest = Client::new()
        .get(format!("{}/v1/node/manifest", server.base_url))
        .send()
        .await?
        .json::<serde_json::Value>()
        .await?;
    assert_eq!(manifest["node_id"], issuer);

    let pool = kukuri_cn_core::connect_postgres(server.database.database_url.as_str()).await?;
    let stored = kukuri_cn_core::persist_risk_signal(
        &pool,
        issuer.as_str(),
        &kukuri_cn_safety::SafetyRiskSignal {
            target: kukuri_cn_safety::RiskSignalTarget::PostId,
            target_id: "post-signed-issuer".to_string(),
            category: kukuri_cn_safety::SafetyCategory::Nsfw,
            severity: kukuri_cn_safety::Severity::High,
            basis: kukuri_cn_safety::Basis::ClassifierScore,
            confidence: Some(90),
            visibility: kukuri_cn_safety::Visibility::Local,
            expires_at: None,
            appeal_status: Some(kukuri_cn_safety::AppealStatus::None),
        },
    )
    .await?;
    let accepted = Client::new()
        .post(format!("{}/v1/report", server.base_url))
        .json(&serde_json::json!({
            "subject_kind": "post",
            "subject_id": "post-signed-issuer",
            "capability": "moderation",
            "reason": "false_positive",
            "appeal": { "risk_signal_id": stored.id },
        }))
        .send()
        .await?;
    assert_eq!(accepted.status(), StatusCode::OK);
    let body = accepted.json::<serde_json::Value>().await?;
    assert_eq!(body["disputed_risk_signal_id"], stored.id);
    let refreshed = kukuri_cn_core::get_risk_signal(&pool, &stored.id)
        .await?
        .expect("signal exists");
    assert_eq!(
        refreshed.signal.appeal_status,
        Some(kukuri_cn_safety::AppealStatus::Disputed)
    );

    server.shutdown().await
}

#[tokio::test]
async fn attachment_appeal_is_accepted_as_media_and_disputes_the_signal() -> Result<()> {
    // #707: 添付(blob_cid)由来の判定は対象著者の信頼評価根拠に寄与する。クライアントは
    // subject_kind=media / subject_id=<添付ハッシュ> で異議申し立てを送り、係争中になる。
    let Some(admin_database_url) = integration_test_admin_database_url() else {
        eprintln!("skipping cn-user-api report test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let trust = Arc::new(TrustReadState {
        params: TrustParams::default(),
        relation: Arc::new(MemoryRelationStore::new()),
    });
    let server = TestServer::spawn_with_trust(
        admin_database_url.as_str(),
        "cn_user_api_blob_appeal",
        REPORT_ENABLED_YAML,
        trust,
    )
    .await?;
    let pool = kukuri_cn_core::connect_postgres(server.database.database_url.as_str()).await?;
    let client = Client::new();
    let viewer = generate_keys();
    let author = generate_keys();
    let author_pubkey = author.public_key_hex();
    let token = authenticate_and_consent(&client, server.base_url.as_str(), &viewer).await?;
    let stored = kukuri_cn_core::persist_risk_signal_with_author(
        &pool,
        "issuer-node-1",
        &kukuri_cn_safety::SafetyRiskSignal {
            target: kukuri_cn_safety::RiskSignalTarget::BlobCid,
            target_id: "blob-hash-appeal".to_string(),
            category: kukuri_cn_safety::SafetyCategory::Nsfw,
            severity: kukuri_cn_safety::Severity::High,
            basis: kukuri_cn_safety::Basis::ProviderVerdict,
            confidence: Some(90),
            visibility: kukuri_cn_safety::Visibility::Local,
            expires_at: None,
            appeal_status: Some(kukuri_cn_safety::AppealStatus::None),
        },
        Some(author_pubkey.as_str()),
    )
    .await?;

    // 添付判定が対象著者の信頼評価根拠に現れる(クライアントはこの basis から申し立てる)。
    let trust_url = format!("{}/v1/trust/users/{author_pubkey}", server.base_url);
    let before = client
        .get(trust_url.as_str())
        .bearer_auth(token.as_str())
        .send()
        .await?
        .error_for_status()?
        .json::<serde_json::Value>()
        .await?;
    assert_eq!(before["basis"][0]["signal_id"], stored.id);
    assert_eq!(before["basis"][0]["target"], "blob_cid");
    assert_eq!(before["basis"][0]["target_id"], "blob-hash-appeal");
    assert_eq!(before["basis"][0]["appeal_status"], "none");

    // 元の対象種別(添付)と異なる post での申し立ては拒否される。
    let wrong_kind = client
        .post(format!("{}/v1/report", server.base_url))
        .json(&serde_json::json!({
            "subject_kind": "post",
            "subject_id": "blob-hash-appeal",
            "capability": "trust_signal",
            "reason": "other",
            "appeal": { "risk_signal_id": stored.id },
        }))
        .send()
        .await?;
    assert_eq!(wrong_kind.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        wrong_kind.json::<serde_json::Value>().await?["code"],
        "INVALID_APPEAL"
    );

    let submitted = client
        .post(format!("{}/v1/report", server.base_url))
        .json(&CommunityNodeReportRequest {
            subject_kind: "media".to_string(),
            subject_id: "blob-hash-appeal".to_string(),
            capability: "trust_signal".to_string(),
            reason: "other".to_string(),
            details: Some("添付の誤判定として再確認を求めます".to_string()),
            reporter_contact: None,
            appeal: Some(CommunityNodeReportAppeal {
                risk_signal_id: stored.id.clone(),
            }),
        })
        .send()
        .await?
        .error_for_status()?
        .json::<kukuri_cn_protocol::CommunityNodeReportResponse>()
        .await?;
    assert_eq!(
        submitted.disputed_risk_signal_id.as_deref(),
        Some(stored.id.as_str())
    );

    // 係争中は寄与を維持したまま、根拠の状態だけが disputed になる。
    let after = client
        .get(trust_url)
        .bearer_auth(token)
        .send()
        .await?
        .error_for_status()?
        .json::<serde_json::Value>()
        .await?;
    assert_eq!(after["basis"][0]["signal_id"], stored.id);
    assert_eq!(after["basis"][0]["target"], "blob_cid");
    assert_eq!(after["basis"][0]["appeal_status"], "disputed");
    assert_eq!(
        after["basis"][0]["contribution"],
        before["basis"][0]["contribution"]
    );

    server.shutdown().await
}

#[tokio::test]
async fn post_appeal_reissue_closure_is_visible_after_trust_refetch() -> Result<()> {
    // #710(案A): 訂正版再発行を伴う審査でも、利用者は信頼評価の再取得で終結を確認できる。
    // 旧判定は失効させず cleared(寄与 0)として根拠一覧に残り、訂正版の新判定が現れる。
    let Some(admin_database_url) = integration_test_admin_database_url() else {
        eprintln!("skipping cn-user-api report test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let trust = Arc::new(TrustReadState {
        params: TrustParams::default(),
        relation: Arc::new(MemoryRelationStore::new()),
    });
    let server = TestServer::spawn_with_trust(
        admin_database_url.as_str(),
        "cn_user_api_reissue_refetch",
        REPORT_ENABLED_YAML,
        trust,
    )
    .await?;
    let pool = kukuri_cn_core::connect_postgres(server.database.database_url.as_str()).await?;
    let client = Client::new();
    let viewer = generate_keys();
    let author = generate_keys();
    let author_pubkey = author.public_key_hex();
    let token = authenticate_and_consent(&client, server.base_url.as_str(), &viewer).await?;
    let stored = kukuri_cn_core::persist_risk_signal_with_author(
        &pool,
        "issuer-node-1",
        &kukuri_cn_safety::SafetyRiskSignal {
            target: kukuri_cn_safety::RiskSignalTarget::PostId,
            target_id: "post-reissue-refetch".to_string(),
            category: kukuri_cn_safety::SafetyCategory::Csam,
            severity: kukuri_cn_safety::Severity::Critical,
            basis: kukuri_cn_safety::Basis::KnownHashMatch,
            confidence: Some(100),
            visibility: kukuri_cn_safety::Visibility::Local,
            expires_at: None,
            appeal_status: Some(kukuri_cn_safety::AppealStatus::None),
        },
        Some(author_pubkey.as_str()),
    )
    .await?;

    let appeal = CommunityNodeReportRequest {
        subject_kind: "post".to_string(),
        subject_id: "post-reissue-refetch".to_string(),
        capability: "moderation".to_string(),
        reason: "false_positive".to_string(),
        details: Some("訂正を求めます".to_string()),
        reporter_contact: None,
        appeal: Some(CommunityNodeReportAppeal {
            risk_signal_id: stored.id.clone(),
        }),
    };
    client
        .post(format!("{}/v1/report", server.base_url))
        .json(&appeal)
        .send()
        .await?
        .error_for_status()?;

    let expected = get_appeal_review(&pool, stored.id.as_str())
        .await?
        .expect("appeal review")
        .version();
    apply_appeal_review_action(
        &pool,
        "ops@kukuri.app",
        stored.id.as_str(),
        &AppealReviewOperation::Reissue {
            expected,
            correction: RiskSignalCorrection {
                category: None,
                severity: Some(kukuri_cn_safety::Severity::Low),
                confidence: Some(10),
                visibility: None,
            },
        },
        true,
    )
    .await?;

    let trust_url = format!("{}/v1/trust/users/{author_pubkey}", server.base_url);
    let after = client
        .get(trust_url)
        .bearer_auth(token)
        .send()
        .await?
        .error_for_status()?
        .json::<serde_json::Value>()
        .await?;
    let basis = after["basis"].as_array().expect("basis array");
    assert_eq!(
        basis.len(),
        2,
        "旧判定(cleared)と訂正版の両方が現れる: {after}"
    );
    let old_entry = basis
        .iter()
        .find(|entry| entry["signal_id"] == serde_json::json!(stored.id))
        .expect("旧判定が根拠一覧に残る");
    assert_eq!(old_entry["appeal_status"], "cleared");
    assert_eq!(old_entry["contribution"].as_f64(), Some(0.0));
    let corrected_entry = basis
        .iter()
        .find(|entry| entry["signal_id"] != serde_json::json!(stored.id))
        .expect("訂正版の新判定が現れる");
    assert_eq!(corrected_entry["appeal_status"], "none");
    assert!(
        corrected_entry["contribution"]
            .as_f64()
            .is_some_and(|value| value < 0.0),
        "訂正版は通常の判定として寄与する: {corrected_entry}"
    );

    server.shutdown().await
}
