//! テスターフィードバック送信(#802 / ADR 0039)の client contract。
//!
//! UI からは 3 つの自由記述と base_url だけを渡しても、wire request に client version /
//! OS が自動付与され、bearer 付きで POST されることを固定する。

use super::super::*;
use axum::response::IntoResponse;
use kukuri_cn_protocol::ApiErrorBody;
use serde_json::{Value, json};

#[derive(Clone)]
struct MockTesterFeedbackState {
    expected_token: Arc<Mutex<String>>,
    received: Arc<Mutex<Vec<Value>>>,
    unauthorized_remaining: Arc<AtomicUsize>,
}

async fn mock_tester_feedback(
    State(state): State<MockTesterFeedbackState>,
    headers: HeaderMap,
    Json(payload): Json<Value>,
) -> axum::response::Response {
    if state
        .unauthorized_remaining
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |value| {
            value.checked_sub(1)
        })
        .is_ok()
    {
        return (
            StatusCode::UNAUTHORIZED,
            Json(ApiErrorBody {
                code: "AUTH_REQUIRED".to_string(),
                message: "community node authentication is required".to_string(),
            }),
        )
            .into_response();
    }
    let expected = state.expected_token.lock().await.clone();
    let expected_header = format!("Bearer {expected}");
    if headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        != Some(expected_header.as_str())
    {
        return (
            StatusCode::UNAUTHORIZED,
            Json(ApiErrorBody {
                code: "AUTH_REQUIRED".to_string(),
                message: "community node authentication is required".to_string(),
            }),
        )
            .into_response();
    }
    state.received.lock().await.push(payload);
    Json(json!({ "reference_id": "feedback-1" })).into_response()
}

async fn tester_feedback_runtime(
    unauthorized_first: bool,
) -> (
    DesktopRuntime,
    String,
    MockTesterFeedbackState,
    tokio::task::JoinHandle<()>,
    tempfile::TempDir,
) {
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("community-tester-feedback.db");
    let runtime = DesktopRuntime::new_with_config_and_identity(
        &db_path,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime");
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
    let base_url = format!("http://{}", listener.local_addr().expect("local addr"));
    let expected_token = Arc::new(Mutex::new("feedback-token".to_string()));
    let managed = Arc::new(MockManagedCommunityNodeState::new(
        base_url.clone(),
        Vec::new(),
        true,
        Arc::clone(&expected_token),
    ));
    let feedback = MockTesterFeedbackState {
        expected_token,
        received: Arc::new(Mutex::new(Vec::new())),
        unauthorized_remaining: Arc::new(AtomicUsize::new(usize::from(unauthorized_first))),
    };
    let managed_router = Router::new()
        .route("/v1/auth/challenge", post(mock_managed_auth_challenge))
        .route("/v1/auth/verify", post(mock_managed_auth_verify))
        .route("/v1/policies", get(mock_managed_policies))
        .route("/v1/consents/status", get(mock_managed_consent_status))
        .route("/v1/consents", post(mock_managed_accept_consents))
        .route(
            "/v1/bootstrap/heartbeat",
            post(mock_managed_bootstrap_heartbeat),
        )
        .route("/v1/bootstrap/nodes", get(mock_managed_bootstrap_nodes))
        .with_state(Arc::clone(&managed));
    let feedback_router = Router::new()
        .route("/v1/tester-feedback", post(mock_tester_feedback))
        .with_state(feedback.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, managed_router.merge(feedback_router))
            .await
            .expect("server");
    });
    persist_community_node_token(
        &db_path,
        IdentityStorageMode::FileOnly,
        base_url.as_str(),
        &StoredCommunityNodeToken {
            access_token: "feedback-token".to_string(),
            expires_at: Utc::now().timestamp() + 3600,
        },
    )
    .await
    .expect("persist token");
    *runtime.community_node_config.lock().await = CommunityNodeConfig {
        trust_node_priority: Vec::new(),
        nodes: vec![CommunityNodeNodeConfig {
            content_advisory_enabled: true,
            base_url: base_url.clone(),
            resolved_urls: Some(
                CommunityNodeResolvedUrls::new(base_url.clone(), Vec::new(), Vec::new())
                    .expect("resolved urls"),
            ),
        }],
    };
    seed_local_community_node_consents(&runtime, base_url.as_str(), 1).await;
    (runtime, base_url, feedback, server, dir)
}

fn submission(base_url: &str) -> CommunityNodeTesterFeedbackSubmission {
    CommunityNodeTesterFeedbackSubmission {
        base_url: base_url.to_string(),
        what_attempted: "投稿を作成しようとした".to_string(),
        what_happened: "送信ボタンを押しても反応がなかった".to_string(),
        what_seemed_wrong: "エラーも成功も表示されないのが変だと思った".to_string(),
    }
}

#[tokio::test]
async fn tester_feedback_client_attaches_version_and_os_and_sends_bearer() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let (runtime, base_url, state, server, _dir) = tester_feedback_runtime(false).await;

    let result = runtime
        .submit_community_node_tester_feedback(submission(base_url.as_str()))
        .await
        .expect("tester feedback submitted");
    assert_eq!(result.reference_id.as_deref(), Some("feedback-1"));

    let received = state.received.lock().await;
    assert_eq!(received.len(), 1);
    assert_eq!(received[0]["what_attempted"], "投稿を作成しようとした");
    assert_eq!(
        received[0]["what_happened"],
        "送信ボタンを押しても反応がなかった"
    );
    assert_eq!(
        received[0]["what_seemed_wrong"],
        "エラーも成功も表示されないのが変だと思った"
    );
    // client version / OS は runtime 層が自動付与する(UI からは渡していない)。
    assert_eq!(received[0]["client_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(received[0]["os"], std::env::consts::OS);
    // 送信者 identity はペイロードに含めない。
    assert!(received[0].get("pubkey").is_none());
    drop(received);

    runtime.shutdown().await;
    server.abort();
}

#[tokio::test]
async fn tester_feedback_client_rejects_invalid_input_without_network() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let (runtime, base_url, state, server, _dir) = tester_feedback_runtime(false).await;

    let mut empty = submission(base_url.as_str());
    empty.what_happened = "   ".to_string();
    let error = runtime
        .submit_community_node_tester_feedback(empty)
        .await
        .expect_err("empty field must be rejected");
    assert_eq!(error.code, "INVALID_TESTER_FEEDBACK");

    let mut over_limit = submission(base_url.as_str());
    over_limit.what_attempted = "あ".repeat(2001);
    let error = runtime
        .submit_community_node_tester_feedback(over_limit)
        .await
        .expect_err("over-limit field must be rejected");
    assert_eq!(error.code, "INVALID_TESTER_FEEDBACK");

    assert!(state.received.lock().await.is_empty());

    runtime.shutdown().await;
    server.abort();
}

#[tokio::test]
async fn retrying_session_stops_tester_feedback_before_http() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let (runtime, base_url, state, server, _dir) = tester_feedback_runtime(false).await;
    runtime
        .set_community_node_retry_state(
            base_url.as_str(),
            anyhow::anyhow!("temporary session failure"),
        )
        .await;

    let error = runtime
        .submit_community_node_tester_feedback(submission(base_url.as_str()))
        .await
        .expect_err("retrying session must defer tester feedback");

    assert_eq!(error.code, "COMMUNITY_NODE_SESSION_DEFERRED");
    assert!(state.received.lock().await.is_empty());
    runtime.shutdown().await;
    server.abort();
}

#[tokio::test]
async fn tester_feedback_client_reauthenticates_once_on_unauthorized() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let (runtime, base_url, state, server, _dir) = tester_feedback_runtime(true).await;

    let result = runtime
        .submit_community_node_tester_feedback(submission(base_url.as_str()))
        .await
        .expect("tester feedback submitted after re-authentication");
    assert_eq!(result.reference_id.as_deref(), Some("feedback-1"));
    assert_eq!(state.received.lock().await.len(), 1);

    runtime.shutdown().await;
    server.abort();
}
