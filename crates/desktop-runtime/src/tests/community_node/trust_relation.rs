use super::super::*;
use axum::extract::{Path as AxumPath, Query};
use axum::http::{Method, Uri};
use axum::response::{IntoResponse, Response};
use kukuri_cn_protocol::{
    ApiErrorBody, Proximity, ProximityBasisEntry, RelationNeighborsResponse,
    RelationOptoutResponse, RelationReadResponse, TrustEvaluation, TrustEvaluationReason,
    TrustReadView, TrustUserReadResponse,
};
use std::collections::HashMap;

#[derive(Clone)]
struct MockTrustRelationState {
    expected_token: Arc<Mutex<String>>,
    requests: Arc<Mutex<Vec<(Method, String)>>>,
    forced_error: Arc<Mutex<Option<(StatusCode, ApiErrorBody)>>>,
    /// 設定すると、応答本文の対象識別子を要求対象ではなくこの値にする(規約に従わないノードの再現)。
    mismatch_target: Arc<Mutex<Option<String>>>,
}

impl MockTrustRelationState {
    async fn authorize_or_error(&self, headers: &HeaderMap) -> Option<Response> {
        let expected = format!("Bearer {}", self.expected_token.lock().await.as_str());
        if headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            != Some(expected.as_str())
        {
            return Some(
                (
                    StatusCode::UNAUTHORIZED,
                    Json(ApiErrorBody {
                        code: "AUTH_REQUIRED".to_string(),
                        message: "community node authentication is required".to_string(),
                    }),
                )
                    .into_response(),
            );
        }
        self.forced_error
            .lock()
            .await
            .clone()
            .map(|(status, body)| (status, Json(body)).into_response())
    }
}

async fn mock_trust_user(
    State(state): State<MockTrustRelationState>,
    AxumPath(target): AxumPath<String>,
    Query(params): Query<HashMap<String, String>>,
    uri: Uri,
    method: Method,
    headers: HeaderMap,
) -> Response {
    state.requests.lock().await.push((method, uri.to_string()));
    if let Some(error) = state.authorize_or_error(&headers).await {
        return error;
    }
    let target = state.mismatch_target.lock().await.clone().unwrap_or(target);
    Json(TrustUserReadResponse {
        viewer_pubkey: "viewer".to_string(),
        view: TrustReadView {
            target_id: target,
            absolute: -0.4,
            relative: -0.2,
            trust: -0.3,
            w_abs_applied: 0.5,
            computed_at: "2026-08-13T00:00:00Z".to_string(),
            basis: Vec::new(),
            // #1702: 受け取った cursor の続きを示す cursor を返す（受け渡しの確認用）。
            basis_next_cursor: params.get("cursor").map(|cursor| format!("{cursor}-next")),
            evaluation: Some(TrustEvaluation {
                policy_version: "v1-policy".to_string(),
                trust_version: "t-version".to_string(),
                relation_version: "r-1-2".to_string(),
                computed_at: "2026-08-13T00:00:00Z".to_string(),
                expires_at: "2026-08-13T00:10:00Z".to_string(),
                hide_recommended: false,
                reasons: vec![TrustEvaluationReason::RiskSignals],
            }),
        },
    })
    .into_response()
}

async fn mock_relation_user(
    State(state): State<MockTrustRelationState>,
    AxumPath(target): AxumPath<String>,
    uri: Uri,
    method: Method,
    headers: HeaderMap,
) -> Response {
    state
        .requests
        .lock()
        .await
        .push((method, uri.path().to_string()));
    if let Some(error) = state.authorize_or_error(&headers).await {
        return error;
    }
    let target = state.mismatch_target.lock().await.clone().unwrap_or(target);
    Json(RelationReadResponse {
        viewer_pubkey: "viewer".to_string(),
        target_pubkey: target,
        proximity: Proximity {
            score: 0.75,
            basis: vec![ProximityBasisEntry {
                feature: "shared_topics".to_string(),
                value: 3.0,
                weight: 1.0,
                contribution: 0.75,
            }],
        },
    })
    .into_response()
}

async fn mock_relation_neighbors(
    State(state): State<MockTrustRelationState>,
    Query(params): Query<HashMap<String, String>>,
    uri: Uri,
    method: Method,
    headers: HeaderMap,
) -> Response {
    state
        .requests
        .lock()
        .await
        .push((method, format!("{}?limit={}", uri.path(), params["limit"])));
    if let Some(error) = state.authorize_or_error(&headers).await {
        return error;
    }
    Json(RelationNeighborsResponse {
        viewer_pubkey: "viewer".to_string(),
        neighbors: vec!["b".repeat(64)],
    })
    .into_response()
}

async fn mock_relation_optout(
    State(state): State<MockTrustRelationState>,
    uri: Uri,
    method: Method,
    headers: HeaderMap,
) -> Response {
    state
        .requests
        .lock()
        .await
        .push((method.clone(), uri.path().to_string()));
    if let Some(error) = state.authorize_or_error(&headers).await {
        return error;
    }
    Json(RelationOptoutResponse {
        pubkey: "viewer".to_string(),
        opted_out: method == Method::PUT,
        opted_out_at: (method == Method::PUT).then(|| "2026-08-13T00:00:00Z".to_string()),
        min_proximity: 0.25,
    })
    .into_response()
}

async fn mock_relation_rendezvous() -> Json<kukuri_cn_protocol::TopicRendezvousHeartbeatResponse> {
    Json(kukuri_cn_protocol::TopicRendezvousHeartbeatResponse {
        expires_in_seconds: 45,
        topics: Vec::new(),
    })
}

async fn trust_relation_runtime(
    forced_error: Option<(StatusCode, ApiErrorBody)>,
) -> (
    DesktopRuntime,
    String,
    MockTrustRelationState,
    Arc<MockManagedCommunityNodeState>,
    tokio::task::JoinHandle<()>,
    tempfile::TempDir,
) {
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("community-trust-relation.db");
    let runtime = DesktopRuntime::new_with_config_and_identity(
        &db_path,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime");
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
    let base_url = format!("http://{}", listener.local_addr().expect("local addr"));
    let expected_token = Arc::new(Mutex::new("trust-token".to_string()));
    let managed = Arc::new(MockManagedCommunityNodeState::new(
        base_url.clone(),
        Vec::new(),
        true,
        Arc::clone(&expected_token),
    ));
    let trust_relation = MockTrustRelationState {
        expected_token,
        requests: Arc::new(Mutex::new(Vec::new())),
        forced_error: Arc::new(Mutex::new(forced_error)),
        mismatch_target: Arc::new(Mutex::new(None)),
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
    let trust_relation_router = Router::new()
        .route("/v1/trust/users/{target}", get(mock_trust_user))
        .route("/v1/relation/users/{target}", get(mock_relation_user))
        .route("/v1/relation/neighbors", get(mock_relation_neighbors))
        .route(
            "/v1/relation/optout",
            get(mock_relation_optout)
                .put(mock_relation_optout)
                .delete(mock_relation_optout),
        )
        .route(
            "/v1/rendezvous/topics/heartbeat",
            post(mock_relation_rendezvous),
        )
        .with_state(trust_relation.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, managed_router.merge(trust_relation_router))
            .await
            .expect("server");
    });
    persist_community_node_token(
        &db_path,
        IdentityStorageMode::FileOnly,
        base_url.as_str(),
        &StoredCommunityNodeToken {
            access_token: "trust-token".to_string(),
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
    (runtime, base_url, trust_relation, managed, server, dir)
}

#[tokio::test]
async fn community_node_trust_relation_client_preserves_wire_contract_and_methods() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let (runtime, base_url, state, _managed, server, _dir) = trust_relation_runtime(None).await;
    let target = "a".repeat(64);

    let trust = runtime
        .read_community_node_trust_user(CommunityNodeUserAdvisoryRequest {
            base_url: base_url.clone(),
            target_pubkey: target.clone(),
            cursor: None,
        })
        .await
        .expect("trust read");
    assert_eq!(trust.view.target_id, target);
    assert_eq!(trust.view.trust, -0.3);
    assert_eq!(trust.view.basis_next_cursor, None);
    // #1061: CN が合算した評価の版・期限・表示 policy をそのまま運ぶ。
    let evaluation = trust.view.evaluation.as_ref().expect("evaluation");
    assert_eq!(evaluation.relation_version, "r-1-2");
    assert!(!evaluation.hide_recommended);
    // #1702: basis の続きは cursor を CN の `?cursor=` へ渡して読み、CN の次の cursor を返す。
    let next = runtime
        .read_community_node_trust_user(CommunityNodeUserAdvisoryRequest {
            base_url: base_url.clone(),
            target_pubkey: target.clone(),
            cursor: Some("1.1760000000000000.signal-50".to_string()),
        })
        .await
        .expect("trust basis page");
    assert_eq!(
        next.view.basis_next_cursor.as_deref(),
        Some("1.1760000000000000.signal-50-next")
    );

    let relation = runtime
        .read_community_node_relation_user(CommunityNodeUserAdvisoryRequest {
            base_url: base_url.clone(),
            target_pubkey: "a".repeat(64),
            cursor: None,
        })
        .await
        .expect("relation read");
    assert_eq!(relation.proximity.score, 0.75);
    let neighbors = runtime
        .list_community_node_relation_neighbors(CommunityNodeRelationNeighborsRequest {
            base_url: base_url.clone(),
            limit: Some(12),
        })
        .await
        .expect("neighbors");
    assert_eq!(neighbors.neighbors, vec!["b".repeat(64)]);

    let target_request = || CommunityNodeTargetRequest {
        base_url: base_url.clone(),
    };
    assert!(
        !runtime
            .get_community_node_relation_optout(target_request())
            .await
            .expect("get optout")
            .opted_out
    );
    assert!(
        runtime
            .set_community_node_relation_optout(target_request())
            .await
            .expect("set optout")
            .opted_out
    );
    assert!(
        !runtime
            .clear_community_node_relation_optout(target_request())
            .await
            .expect("clear optout")
            .opted_out
    );

    let requests = state.requests.lock().await.clone();
    assert!(requests.contains(&(Method::GET, format!("/v1/trust/users/{}", "a".repeat(64)))));
    assert!(requests.contains(&(
        Method::GET,
        format!("/v1/trust/users/{target}?cursor=1.1760000000000000.signal-50")
    )));
    assert!(requests.contains(&(Method::GET, "/v1/relation/neighbors?limit=12".to_string())));
    assert!(requests.contains(&(Method::PUT, "/v1/relation/optout".to_string())));
    assert!(requests.contains(&(Method::DELETE, "/v1/relation/optout".to_string())));

    runtime.shutdown().await;
    server.abort();
}

#[tokio::test]
async fn community_node_trust_relation_client_preserves_stable_unavailable_codes() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let forced = ApiErrorBody {
        code: "RELATION_NOT_FOUND".to_string(),
        message: "no relation observed for this pair".to_string(),
    };
    let (runtime, base_url, _state, _managed, server, _dir) =
        trust_relation_runtime(Some((StatusCode::NOT_FOUND, forced))).await;

    let error = runtime
        .read_community_node_relation_user(CommunityNodeUserAdvisoryRequest {
            base_url,
            target_pubkey: "a".repeat(64),
            cursor: None,
        })
        .await
        .expect_err("stable unavailable error");
    assert_eq!(error.code, "RELATION_NOT_FOUND");
    assert_eq!(error.status, Some(404));

    runtime.shutdown().await;
    server.abort();
}

// #705: 必須同意が未承認のノードへは、対象利用者の公開鍵を含む要求も距離利用停止の操作も送らない。
#[tokio::test]
async fn community_node_trust_relation_client_stops_before_http_when_consent_is_pending() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let (runtime, base_url, state, managed, server, _dir) = trust_relation_runtime(None).await;
    managed.consent_accepted.store(false, Ordering::SeqCst);
    // #857: ローカル同意(v1)がカバーしない新版(v2)への更新 = 再同意待ち。
    managed
        .simulate_pending_update
        .store(true, Ordering::SeqCst);

    let trust_error = runtime
        .read_community_node_trust_user(CommunityNodeUserAdvisoryRequest {
            base_url: base_url.clone(),
            target_pubkey: "a".repeat(64),
            cursor: None,
        })
        .await
        .expect_err("pending consent must stop the trust read");
    assert_eq!(trust_error.code, "CONSENT_REQUIRED");

    let optout_error = runtime
        .set_community_node_relation_optout(CommunityNodeTargetRequest {
            base_url: base_url.clone(),
        })
        .await
        .expect_err("pending consent must stop the opt-out change");
    assert_eq!(optout_error.code, "CONSENT_REQUIRED");

    assert!(
        state.requests.lock().await.is_empty(),
        "no trust / relation / opt-out request may reach the node while consent is pending"
    );
    runtime.shutdown().await;
    server.abort();
}

#[tokio::test]
async fn retrying_session_stops_trust_relation_request_before_http() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let (runtime, base_url, state, managed, server, _dir) = trust_relation_runtime(None).await;
    runtime
        .set_community_node_retry_state(
            base_url.as_str(),
            anyhow::anyhow!("temporary session failure"),
        )
        .await;

    let error = runtime
        .read_community_node_trust_user(CommunityNodeUserAdvisoryRequest {
            base_url,
            target_pubkey: "a".repeat(64),
            cursor: None,
        })
        .await
        .expect_err("retrying session must defer the trust request");

    assert_eq!(error.code, "COMMUNITY_NODE_SESSION_DEFERRED");
    assert_eq!(managed.policies_hits.load(Ordering::SeqCst), 1);
    assert!(state.requests.lock().await.is_empty());
    runtime.shutdown().await;
    server.abort();
}

// #699: 応答本文の対象識別子が要求対象と違う応答は採用しない。
#[tokio::test]
async fn community_node_trust_relation_client_rejects_responses_for_another_target() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let (runtime, base_url, state, _managed, server, _dir) = trust_relation_runtime(None).await;
    *state.mismatch_target.lock().await = Some("b".repeat(64));

    let trust_error = runtime
        .read_community_node_trust_user(CommunityNodeUserAdvisoryRequest {
            base_url: base_url.clone(),
            target_pubkey: "A".repeat(64),
            cursor: None,
        })
        .await
        .expect_err("trust response for another target must be rejected");
    assert_eq!(trust_error.code, "TRUST_RELATION_RESPONSE_MISMATCH");

    let relation_error = runtime
        .read_community_node_relation_user(CommunityNodeUserAdvisoryRequest {
            base_url: base_url.clone(),
            target_pubkey: "a".repeat(64),
            cursor: None,
        })
        .await
        .expect_err("relation response for another target must be rejected");
    assert_eq!(relation_error.code, "TRUST_RELATION_RESPONSE_MISMATCH");

    // 大文字混じりで要求しても、正規化した同じ対象の応答は受理される。
    *state.mismatch_target.lock().await = Some("a".repeat(64));
    let trust = runtime
        .read_community_node_trust_user(CommunityNodeUserAdvisoryRequest {
            base_url,
            target_pubkey: "A".repeat(64),
            cursor: None,
        })
        .await
        .expect("normalized target matches");
    assert_eq!(trust.view.target_id, "a".repeat(64));

    runtime.shutdown().await;
    server.abort();
}
