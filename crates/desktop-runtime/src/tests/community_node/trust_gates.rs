//! #1061 採用 CN の信頼値による著者の表示判断（ADR 0026 §8.4）。
//!
//! 未選択の CN へ照会しないこと、優先順位の上位から採ること、失敗を非表示に変換しないこと、
//! 対象・閲覧者・期限の照合、設定変更での破棄、restart 後の復元、表示例外、
//! そして判断が mute / block や観測を作らないことを確認する。

use super::super::*;
use axum::response::{IntoResponse, Response};
use kukuri_cn_protocol::{
    ApiErrorBody, TrustEvaluation, TrustEvaluationItem, TrustEvaluationReason,
    TrustEvaluationsResponse,
};

use crate::{AuthorTrustGateRequest, SetAuthorTrustDisplayExceptionRequest};

#[derive(Clone)]
struct MockTrustGateNode {
    managed: Arc<MockManagedCommunityNodeState>,
    /// 受け取った照会の対象（node ごと）。
    requests: Arc<Mutex<Vec<Vec<String>>>>,
    /// 非表示にする対象。
    hidden: Arc<Mutex<Vec<String>>>,
    error: Arc<Mutex<Option<StatusCode>>>,
    /// 応答の viewer を差し替える（別 account の結果の再現）。
    viewer_override: Arc<Mutex<Option<String>>>,
    /// 評価の期限（既定は 600 秒後）。
    expires_in_seconds: Arc<Mutex<i64>>,
}

fn evaluation(hide: bool, expires_at: String) -> TrustEvaluation {
    TrustEvaluation {
        policy_version: "v1-policy".to_string(),
        trust_version: "t-1".to_string(),
        relation_version: "r-1-1".to_string(),
        computed_at: Utc::now().to_rfc3339(),
        expires_at,
        hide_recommended: hide,
        reasons: if hide {
            vec![TrustEvaluationReason::RelatedUsersBlockOrMute]
        } else {
            Vec::new()
        },
    }
}

async fn mock_evaluations(
    State(state): State<MockTrustGateNode>,
    headers: HeaderMap,
    Json(request): Json<serde_json::Value>,
) -> Response {
    if authorize_managed_community_node_request(&headers, state.managed.as_ref())
        .await
        .is_err()
    {
        return (
            StatusCode::UNAUTHORIZED,
            Json(ApiErrorBody {
                code: "AUTH_REQUIRED".to_string(),
                message: "auth".to_string(),
            }),
        )
            .into_response();
    }
    if let Some(status) = *state.error.lock().await {
        return (
            status,
            Json(ApiErrorBody {
                code: "TRUST_RELATION_UNAVAILABLE".to_string(),
                message: "unavailable".to_string(),
            }),
        )
            .into_response();
    }
    let targets: Vec<String> = serde_json::from_value(request["targets"].clone()).expect("targets");
    state.requests.lock().await.push(targets.clone());
    let hidden = state.hidden.lock().await.clone();
    let expires_at = (Utc::now()
        + chrono::Duration::seconds(*state.expires_in_seconds.lock().await))
    .to_rfc3339();
    let viewer = state
        .viewer_override
        .lock()
        .await
        .clone()
        .unwrap_or_else(|| "viewer".to_string());
    Json(TrustEvaluationsResponse {
        viewer_pubkey: viewer,
        evaluations: targets
            .into_iter()
            .map(|target| {
                let hide = hidden.contains(&target);
                TrustEvaluationItem {
                    trust: if hide { -0.9 } else { 0.0 },
                    evaluation: evaluation(hide, expires_at.clone()),
                    target_pubkey: target,
                }
            })
            .collect(),
    })
    .into_response()
}

struct GateNode {
    base_url: String,
    state: MockTrustGateNode,
    server: tokio::task::JoinHandle<()>,
}

impl GateNode {
    async fn requested(&self) -> Vec<Vec<String>> {
        self.state.requests.lock().await.clone()
    }

    async fn request_count(&self) -> usize {
        self.state.requests.lock().await.len()
    }
}

async fn spawn_node(db_path: &std::path::Path, token: &str) -> GateNode {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
    let base_url = format!("http://{}", listener.local_addr().expect("addr"));
    let managed = Arc::new(MockManagedCommunityNodeState::new(
        base_url.clone(),
        Vec::new(),
        true,
        Arc::new(Mutex::new(token.to_string())),
    ));
    let state = MockTrustGateNode {
        managed: Arc::clone(&managed),
        requests: Arc::new(Mutex::new(Vec::new())),
        hidden: Arc::new(Mutex::new(Vec::new())),
        error: Arc::new(Mutex::new(None)),
        viewer_override: Arc::new(Mutex::new(None)),
        expires_in_seconds: Arc::new(Mutex::new(600)),
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
        .with_state(managed);
    let gate_router = Router::new()
        .route("/v1/trust/evaluations", post(mock_evaluations))
        .route("/v1/rendezvous/topics/heartbeat", post(mock_rendezvous))
        .with_state(state.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, managed_router.merge(gate_router))
            .await
            .expect("server");
    });
    persist_community_node_token(
        db_path,
        IdentityStorageMode::FileOnly,
        base_url.as_str(),
        &StoredCommunityNodeToken {
            access_token: token.to_string(),
            expires_at: Utc::now().timestamp() + 3600,
        },
    )
    .await
    .expect("persist token");
    GateNode {
        base_url,
        state,
        server,
    }
}

async fn open_runtime(
    db_path: &std::path::Path,
    nodes: &[&GateNode],
    priority: &[&GateNode],
) -> DesktopRuntime {
    let runtime = DesktopRuntime::new_with_config_and_identity(
        db_path,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime");
    *runtime.community_node_config.lock().await = CommunityNodeConfig {
        trust_node_priority: priority.iter().map(|node| node.base_url.clone()).collect(),
        nodes: nodes
            .iter()
            .map(|node| CommunityNodeNodeConfig {
                content_advisory_enabled: true,
                base_url: node.base_url.clone(),
                resolved_urls: Some(
                    CommunityNodeResolvedUrls::new(node.base_url.clone(), Vec::new(), Vec::new())
                        .expect("resolved urls"),
                ),
            })
            .collect(),
    };
    for node in nodes {
        seed_local_community_node_consents(&runtime, node.base_url.as_str(), 1).await;
        // mock は既定でこの runtime の閲覧者として応答する（照合の対象は別テストで確認する）。
        *node.state.viewer_override.lock().await = Some(runtime.author_keys.public_key_hex());
    }
    runtime
}

fn author(seed: usize) -> String {
    static AUTHORS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    AUTHORS.get_or_init(|| {
        (0..4)
            .map(|_| kukuri_core::generate_keys().public_key_hex())
            .collect()
    })[seed]
        .clone()
}

async fn gates(runtime: &DesktopRuntime, authors: &[String]) -> Vec<crate::AuthorTrustGate> {
    runtime
        .evaluate_author_trust_gates(AuthorTrustGateRequest {
            author_pubkeys: authors.to_vec(),
        })
        .await
        .expect("gates")
        .gates
}

#[tokio::test]
async fn gate_uses_the_first_valid_node_in_priority_and_never_queries_unselected_nodes() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("trust-gates.db");
    let first = spawn_node(&db_path, "first-token").await;
    let second = spawn_node(&db_path, "second-token").await;
    let unselected = spawn_node(&db_path, "unselected-token").await;
    first.state.hidden.lock().await.push(author(0));
    second.state.hidden.lock().await.push(author(1));
    unselected.state.hidden.lock().await.push(author(2));
    let runtime = open_runtime(
        &db_path,
        &[&first, &second, &unselected],
        &[&first, &second],
    )
    .await;

    let gates = gates(&runtime, &[author(0), author(1), author(2)]).await;
    let hidden: Vec<&str> = gates
        .iter()
        .filter(|gate| gate.hidden)
        .map(|gate| gate.author_pubkey.as_str())
        .collect();
    assert_eq!(hidden, vec![author(0).as_str()]);
    // 上位で解決した対象は下位へ照会しない。未選択の CN へは一度も送らない。
    assert_eq!(first.request_count().await, 1);
    assert_eq!(second.request_count().await, 0);
    assert_eq!(unselected.request_count().await, 0);
    let gate = gates
        .iter()
        .find(|gate| gate.author_pubkey == author(0))
        .expect("gate");
    assert_eq!(gate.node_base_url.as_deref(), Some(first.base_url.as_str()));
    assert_eq!(
        gate.reasons,
        vec![TrustEvaluationReason::RelatedUsersBlockOrMute]
    );

    runtime.shutdown().await;
    first.server.abort();
    second.server.abort();
    unselected.server.abort();
}

#[tokio::test]
async fn failures_fall_back_within_the_selection_and_never_hide() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("trust-gates.db");
    let first = spawn_node(&db_path, "first-token").await;
    let second = spawn_node(&db_path, "second-token").await;
    *first.state.error.lock().await = Some(StatusCode::SERVICE_UNAVAILABLE);
    second.state.hidden.lock().await.push(author(0));
    let runtime = open_runtime(&db_path, &[&first, &second], &[&first, &second]).await;

    // 上位が失敗したら、選択済みの下位へ進む。
    let fallback = gates(&runtime, &[author(0)]).await;
    assert!(fallback[0].hidden);
    assert_eq!(
        fallback[0].node_base_url.as_deref(),
        Some(second.base_url.as_str())
    );

    // すべて失敗すると未評価（非表示にしない）。
    *second.state.error.lock().await = Some(StatusCode::SERVICE_UNAVAILABLE);
    runtime.invalidate_author_trust_gate_cache().await;
    let unevaluated = gates(&runtime, &[author(0)]).await;
    assert!(!unevaluated[0].hidden);
    assert!(unevaluated[0].node_base_url.is_none());
    assert!(unevaluated[0].expires_at.is_none());

    runtime.shutdown().await;
    first.server.abort();
    second.server.abort();
}

#[tokio::test]
async fn mismatched_viewer_or_expired_evaluation_is_rejected() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("trust-gates.db");
    let node = spawn_node(&db_path, "token").await;
    node.state.hidden.lock().await.push(author(0));
    let runtime = open_runtime(&db_path, &[&node], &[&node]).await;

    // 別 account の viewer を返す応答は採らない。
    let viewer = runtime.author_keys.public_key_hex();
    *node.state.viewer_override.lock().await = Some("f".repeat(64));
    let rejected = gates(&runtime, &[author(0)]).await;
    assert!(!rejected[0].hidden);
    assert!(rejected[0].node_base_url.is_none());

    // 期限切れの評価も採らない。
    *node.state.viewer_override.lock().await = Some(viewer);
    *node.state.expires_in_seconds.lock().await = -1;
    let expired = gates(&runtime, &[author(0)]).await;
    assert!(!expired[0].hidden);

    // 期限内なら採る。
    *node.state.expires_in_seconds.lock().await = 600;
    let fresh = gates(&runtime, &[author(0)]).await;
    assert!(fresh[0].hidden);

    runtime.shutdown().await;
    node.server.abort();
}

#[tokio::test]
async fn cached_evaluation_is_reused_and_dropped_when_the_configuration_changes() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("trust-gates.db");
    let node = spawn_node(&db_path, "token").await;
    node.state.hidden.lock().await.push(author(0));
    let runtime = open_runtime(&db_path, &[&node], &[&node]).await;

    assert!(gates(&runtime, &[author(0)]).await[0].hidden);
    assert!(gates(&runtime, &[author(0)]).await[0].hidden);
    assert_eq!(node.request_count().await, 1, "期限内は cache を使う");

    // 設定の変更で cache を破棄し、照会し直す。
    runtime
        .set_community_node_config(SetCommunityNodeConfigRequest {
            trust_node_priority: Some(vec![node.base_url.clone()]),
            nodes: vec![SetCommunityNodeConfigNode::new(node.base_url.clone())],
        })
        .await
        .expect("set config");
    assert!(gates(&runtime, &[author(0)]).await[0].hidden);
    assert_eq!(node.request_count().await, 2);

    runtime.shutdown().await;
    node.server.abort();
}

#[tokio::test]
async fn priority_and_display_exceptions_restore_after_restart() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("trust-gates.db");
    let node = spawn_node(&db_path, "token").await;
    node.state.hidden.lock().await.push(author(0));
    let runtime = open_runtime(&db_path, &[&node], &[&node]).await;
    // 優先順位を保存する（設定ファイルへ書く）。
    runtime
        .set_community_node_config(SetCommunityNodeConfigRequest {
            trust_node_priority: Some(vec![node.base_url.clone()]),
            nodes: vec![SetCommunityNodeConfigNode::new(node.base_url.clone())],
        })
        .await
        .expect("set config");
    let gate = runtime
        .set_author_trust_display_exception(SetAuthorTrustDisplayExceptionRequest {
            author_pubkey: author(0),
            always_visible: true,
        })
        .await
        .expect("set exception");
    assert!(gate.always_visible);
    assert!(!gate.hidden, "例外を設定した著者は折りたたまない");
    runtime.shutdown().await;

    // restart 後も優先順位と例外を復元する。
    let restarted = DesktopRuntime::new_with_config_and_identity(
        &db_path,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime");
    seed_local_community_node_consents(&restarted, node.base_url.as_str(), 1).await;
    assert_eq!(
        restarted
            .community_node_config
            .lock()
            .await
            .trust_node_priority,
        vec![node.base_url.clone()]
    );
    assert_eq!(
        restarted
            .list_author_trust_display_exceptions()
            .await
            .expect("exceptions"),
        vec![author(0)]
    );
    assert!(!gates(&restarted, &[author(0)]).await[0].hidden);

    // 例外を解除すると折りたたみへ戻る。
    let gate = restarted
        .set_author_trust_display_exception(SetAuthorTrustDisplayExceptionRequest {
            author_pubkey: author(0),
            always_visible: false,
        })
        .await
        .expect("clear exception");
    assert!(gate.hidden);

    restarted.shutdown().await;
    node.server.abort();
}

/// 表示設定にすぎないため、読めない優先順位があっても起動を止めず、有効な分だけ復元する。
#[tokio::test]
async fn unusable_priority_entries_are_dropped_without_blocking_startup() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("trust-gates.db");
    let node = spawn_node(&db_path, "token").await;
    node.state.hidden.lock().await.push(author(0));

    // 手編集・部分書き込みで壊れた設定を置く（空文字・不正 URL・未設定 node・重複）。
    let config_path = crate::paths::community_node_config_path(&db_path);
    let config = serde_json::json!({
        "nodes": [{ "base_url": node.base_url, "content_advisory_enabled": true }],
        "trust_node_priority": [
            "",
            "not a url",
            "https://unconfigured.example",
            node.base_url,
            node.base_url,
        ],
    });
    std::fs::write(
        &config_path,
        serde_json::to_string(&config).expect("config json"),
    )
    .expect("write config");

    let runtime = DesktopRuntime::new_with_config_and_identity(
        &db_path,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime starts with a broken priority");
    seed_local_community_node_consents(&runtime, node.base_url.as_str(), 1).await;
    *node.state.viewer_override.lock().await = Some(runtime.author_keys.public_key_hex());

    assert_eq!(
        runtime
            .community_node_config
            .lock()
            .await
            .trust_node_priority,
        vec![node.base_url.clone()],
        "読めない値と未設定 node と重複は落とし、有効な分だけ残す"
    );
    assert!(gates(&runtime, &[author(0)]).await[0].hidden);

    runtime.shutdown().await;
    node.server.abort();
}

#[tokio::test]
async fn empty_priority_does_not_query_any_node() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("trust-gates.db");
    let node = spawn_node(&db_path, "token").await;
    node.state.hidden.lock().await.push(author(0));
    let runtime = open_runtime(&db_path, &[&node], &[]).await;

    let gates = gates(&runtime, &[author(0)]).await;
    assert!(!gates[0].hidden);
    assert_eq!(node.request_count().await, 0);

    runtime.shutdown().await;
    node.server.abort();
}

#[tokio::test]
async fn gating_never_changes_social_state_or_queues_observations() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("trust-gates.db");
    let node = spawn_node(&db_path, "token").await;
    node.state.hidden.lock().await.push(author(0));
    let runtime = open_runtime(&db_path, &[&node], &[&node]).await;

    assert!(gates(&runtime, &[author(0)]).await[0].hidden);
    // 非表示の判断は mute / block を作らず、観測も積まない。
    let view = runtime
        .get_author_social_view(AuthorRequest { pubkey: author(0) })
        .await
        .expect("social view");
    assert!(!view.muted);
    assert!(!view.blocking);
    assert_eq!(
        crate::community_node::load_trust_observation_pending_count(
            &runtime,
            node.base_url.as_str()
        )
        .await,
        0
    );
    // 要求した対象だけを照会する。
    let requested = node.requested().await;
    assert_eq!(requested, vec![vec![author(0)]]);

    runtime.shutdown().await;
    node.server.abort();
}
