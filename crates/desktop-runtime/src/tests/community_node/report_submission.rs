use super::super::*;
use axum::response::{IntoResponse, Response};
use kukuri_cn_protocol::{ApiErrorBody, CONSENT_REQUIRED_CODE};
use serde_json::{Value, json};

#[derive(Clone, Default)]
struct MockReportState {
    received: Arc<Mutex<Vec<Value>>>,
    invalid_appeal: bool,
    policies_hits: Arc<AtomicUsize>,
    simulate_snapshot_update: Arc<AtomicBool>,
}

async fn mock_report(State(state): State<MockReportState>, Json(payload): Json<Value>) -> Response {
    state.received.lock().await.push(payload);
    if state.invalid_appeal {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiErrorBody {
                code: "INVALID_APPEAL".to_string(),
                message: "異議申し立ての対象を確認できません".to_string(),
            }),
        )
            .into_response();
    }
    Json(json!({
        "reference_id": "report-1",
        "disputed_risk_signal_id": "signal-1"
    }))
    .into_response()
}

async fn mock_report_policies(
    State(state): State<MockReportState>,
) -> Json<kukuri_cn_protocol::CommunityNodePoliciesResponse> {
    state.policies_hits.fetch_add(1, Ordering::SeqCst);
    let snapshot_revision = state
        .simulate_snapshot_update
        .load(Ordering::SeqCst)
        .then(|| "snapshot-2".to_string());
    Json(kukuri_cn_protocol::CommunityNodePoliciesResponse {
        policies: vec![kukuri_cn_protocol::CommunityNodePolicyDocument {
            policy_kind: None,
            policy_slug: MOCK_MANAGED_POLICY_SLUG.to_string(),
            policy_version: 1,
            title: "Builder Preview".to_string(),
            body_markdown: "Builder preview policy body.".to_string(),
            required: true,
            effective_date: Some("2026-09-02".to_string()),
            language: Some("ja".to_string()),
            policy_snapshot_revision: snapshot_revision.clone(),
            authoritative_language: Some("ja".to_string()),
            reference_translation: false,
            translation_revision: None,
            translation_of_version: None,
            fallback: false,
            requested_language: None,
            material_change: false,
            requires_reconsent: false,
            is_current: true,
            publication_status: Some("current".to_string()),
            published_at: None,
            retired_at: None,
            previous_policy_version: None,
            previous_policy_snapshot_revision: None,
            next_policy_version: None,
            next_policy_snapshot_revision: None,
        }],
        policy_snapshot_revision: snapshot_revision,
    })
}

async fn report_runtime(
    invalid_appeal: bool,
) -> (
    DesktopRuntime,
    String,
    MockReportState,
    tokio::task::JoinHandle<()>,
    tempfile::TempDir,
) {
    let dir = tempdir().expect("tempdir");
    let runtime = DesktopRuntime::new_with_config_and_identity(
        dir.path().join("community-report.db"),
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime");
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
    let base_url = format!("http://{}", listener.local_addr().expect("local addr"));
    let state = MockReportState {
        received: Arc::new(Mutex::new(Vec::new())),
        invalid_appeal,
        policies_hits: Arc::new(AtomicUsize::new(0)),
        simulate_snapshot_update: Arc::new(AtomicBool::new(false)),
    };
    let app = Router::new()
        .route("/v1/policies", get(mock_report_policies))
        .route("/v1/report", post(mock_report))
        .route("/v1/report-moved", post(mock_report_redirect))
        .with_state(state.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("server");
    });
    // #703: 通報先は構成済みノードに限るため、モックノードを構成に登録しておく。
    *runtime.community_node_config.lock().await = CommunityNodeConfig {
        trust_node_priority: Vec::new(),
        nodes: vec![CommunityNodeNodeConfig {
            content_advisory_enabled: true,
            base_url: base_url.clone(),
            resolved_urls: None,
        }],
    };
    (runtime, base_url, state, server, dir)
}

/// 転送応答を返す受付先(転送先は別オリジン)。
async fn mock_report_redirect() -> Response {
    (
        StatusCode::TEMPORARY_REDIRECT,
        [(
            axum::http::header::LOCATION,
            "https://elsewhere.example/v1/report",
        )],
    )
        .into_response()
}

fn appeal_request(base_url: &str) -> SubmitCommunityNodeReportRequest {
    SubmitCommunityNodeReportRequest {
        node_base_url: base_url.to_string(),
        report_endpoint: format!("{base_url}/v1/report"),
        subject_kind: "profile".to_string(),
        subject_id: "author-pubkey".to_string(),
        capability: "trust_signal".to_string(),
        reason: "other".to_string(),
        details: Some("誤検知として再確認を求めます".to_string()),
        reporter_contact: None,
        appeal: Some(CommunityNodeReportAppeal {
            risk_signal_id: "signal-1".to_string(),
        }),
    }
}

#[tokio::test]
async fn community_node_report_client_sends_anonymous_appeal_and_reads_disputed_signal() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let (runtime, base_url, state, server, _dir) = report_runtime(false).await;
    seed_local_community_node_consents(&runtime, base_url.as_str(), 1);

    let result = runtime
        .submit_community_node_report(appeal_request(base_url.as_str()))
        .await
        .expect("appeal submitted");
    assert_eq!(result.reference_id.as_deref(), Some("report-1"));
    assert_eq!(result.disputed_risk_signal_id.as_deref(), Some("signal-1"));
    let received = state.received.lock().await;
    assert_eq!(received.len(), 1);
    assert_eq!(received[0]["appeal"]["risk_signal_id"], "signal-1");
    assert!(received[0].get("reporter_contact").is_none());

    runtime.shutdown().await;
    server.abort();
}

#[tokio::test]
async fn community_node_report_client_preserves_invalid_appeal_code() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let (runtime, base_url, _state, server, _dir) = report_runtime(true).await;
    seed_local_community_node_consents(&runtime, base_url.as_str(), 1);

    let error = runtime
        .submit_community_node_report(appeal_request(base_url.as_str()))
        .await
        .expect_err("invalid appeal");
    assert_eq!(error.code, "INVALID_APPEAL");
    assert_eq!(error.status, Some(400));

    runtime.shutdown().await;
    server.abort();
}

// #703: 構成情報にないノード、別オリジンの受付先、転送応答へは通報本文を送らない。
#[tokio::test]
async fn community_node_report_client_rejects_unconfigured_node_before_http() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let (runtime, base_url, state, server, _dir) = report_runtime(false).await;
    *runtime.community_node_config.lock().await = CommunityNodeConfig::default();

    let error = runtime
        .submit_community_node_report(appeal_request(base_url.as_str()))
        .await
        .expect_err("unconfigured node must be rejected");
    assert_eq!(error.code, "REPORT_TARGET_NOT_CONFIGURED");
    assert!(state.received.lock().await.is_empty());

    runtime.shutdown().await;
    server.abort();
}

#[tokio::test]
async fn community_node_report_client_rejects_endpoint_on_another_origin() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let (runtime, base_url, state, server, _dir) = report_runtime(false).await;

    let mut request = appeal_request(base_url.as_str());
    request.report_endpoint = "https://attacker.example/v1/report".to_string();
    let error = runtime
        .submit_community_node_report(request)
        .await
        .expect_err("foreign origin must be rejected");
    assert_eq!(error.code, "REPORT_ENDPOINT_MISMATCH");
    assert!(state.received.lock().await.is_empty());

    runtime.shutdown().await;
    server.abort();
}

#[tokio::test]
async fn community_node_report_client_does_not_follow_redirects() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let (runtime, base_url, state, server, _dir) = report_runtime(false).await;
    seed_local_community_node_consents(&runtime, base_url.as_str(), 1);

    let mut request = appeal_request(base_url.as_str());
    request.report_endpoint = format!("{base_url}/v1/report-moved");
    let error = runtime
        .submit_community_node_report(request)
        .await
        .expect_err("redirects must not be followed");
    assert_eq!(error.code, "REPORT_REDIRECT_REJECTED");
    assert_eq!(error.status, Some(307));
    assert!(
        state.received.lock().await.is_empty(),
        "the report body must not reach the redirect target"
    );

    runtime.shutdown().await;
    server.abort();
}

#[tokio::test]
async fn community_node_report_client_stops_before_http_without_active_local_consent() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let (runtime, base_url, state, server, _dir) = report_runtime(false).await;

    let error = runtime
        .submit_community_node_report(appeal_request(base_url.as_str()))
        .await
        .expect_err("report without local consent must be rejected");

    assert_eq!(error.code, CONSENT_REQUIRED_CODE);
    assert_eq!(state.policies_hits.load(Ordering::SeqCst), 0);
    assert!(state.received.lock().await.is_empty());

    runtime.shutdown().await;
    server.abort();
}

#[tokio::test]
async fn community_node_report_client_stops_before_http_after_consent_withdrawal() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let (runtime, base_url, state, server, _dir) = report_runtime(false).await;
    seed_local_community_node_consents(&runtime, base_url.as_str(), 1);
    runtime
        .withdraw_community_node_consents(crate::CommunityNodeTargetRequest {
            base_url: base_url.clone(),
        })
        .await
        .expect("withdraw consents");

    let error = runtime
        .submit_community_node_report(appeal_request(base_url.as_str()))
        .await
        .expect_err("report after consent withdrawal must be rejected");

    assert_eq!(error.code, CONSENT_REQUIRED_CODE);
    assert_eq!(state.policies_hits.load(Ordering::SeqCst), 0);
    assert!(state.received.lock().await.is_empty());

    runtime.shutdown().await;
    server.abort();
}

#[tokio::test]
async fn community_node_report_client_stops_before_body_post_on_snapshot_update() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let (runtime, base_url, state, server, _dir) = report_runtime(false).await;
    seed_local_community_node_consents_with_snapshot(
        &runtime,
        base_url.as_str(),
        1,
        Some("snapshot-1"),
    );
    state.simulate_snapshot_update.store(true, Ordering::SeqCst);

    let error = runtime
        .submit_community_node_report(appeal_request(base_url.as_str()))
        .await
        .expect_err("report after policy snapshot update must be rejected");

    assert_eq!(error.code, CONSENT_REQUIRED_CODE);
    assert_eq!(state.policies_hits.load(Ordering::SeqCst), 1);
    assert!(state.received.lock().await.is_empty());

    runtime.shutdown().await;
    server.abort();
}

#[tokio::test]
async fn community_node_report_client_stops_before_http_when_reconsent_is_pending() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let (runtime, base_url, state, server, _dir) = report_runtime(false).await;
    seed_local_community_node_consents(&runtime, base_url.as_str(), 1);
    runtime
        .set_community_node_local_consent_update_pending(base_url.as_str(), true)
        .await;

    let error = runtime
        .submit_community_node_report(appeal_request(base_url.as_str()))
        .await
        .expect_err("report while reconsent is pending must be rejected");

    assert_eq!(error.code, CONSENT_REQUIRED_CODE);
    assert!(state.received.lock().await.is_empty());

    runtime.shutdown().await;
    server.abort();
}
