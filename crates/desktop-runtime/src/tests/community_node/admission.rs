use super::super::*;
use axum::response::{IntoResponse, Response};
use kukuri_cn_protocol::{ApiErrorBody, AuthVerifyRequest, AuthVerifyResponse};

use crate::community_node::{load_community_node_invite_code, persist_community_node_invite_code};

#[derive(Clone)]
struct AdmissionMockState {
    base_url: String,
    required_invite: Arc<Mutex<Option<String>>>,
    forced_rejection: Arc<Mutex<Option<ApiErrorBody>>>,
    received_invites: Arc<Mutex<Vec<Option<String>>>>,
    verify_hits: Arc<AtomicUsize>,
    /// 真にすると心拍を 401 で拒否する(参加禁止後の既存トークンをサーバが失効させた状態)。
    heartbeat_unauthorized: Arc<AtomicBool>,
}

async fn admission_challenge() -> Json<kukuri_cn_protocol::AuthChallengeResponse> {
    Json(kukuri_cn_protocol::AuthChallengeResponse {
        challenge: "admission-challenge".into(),
        expires_at: Utc::now().timestamp() + 300,
    })
}

async fn admission_verify(
    State(state): State<Arc<AdmissionMockState>>,
    Json(request): Json<AuthVerifyRequest>,
) -> Response {
    state.verify_hits.fetch_add(1, Ordering::SeqCst);
    state
        .received_invites
        .lock()
        .await
        .push(request.invite_code.clone());
    if let Some(body) = state.forced_rejection.lock().await.clone() {
        return (StatusCode::FORBIDDEN, Json(body)).into_response();
    }
    if let Some(required) = state.required_invite.lock().await.clone()
        && request.invite_code.as_deref() != Some(required.as_str())
    {
        let code = if request.invite_code.is_some() {
            "INVITE_INVALID"
        } else {
            "INVITE_REQUIRED"
        };
        return (
            StatusCode::FORBIDDEN,
            Json(ApiErrorBody {
                code: code.into(),
                message: "admission denied".into(),
            }),
        )
            .into_response();
    }
    Json(AuthVerifyResponse {
        access_token: "admission-token".into(),
        token_type: "Bearer".into(),
        expires_at: Utc::now().timestamp() + 3600,
        pubkey: "a".repeat(64),
    })
    .into_response()
}

async fn admission_consent_status() -> Json<CommunityNodeConsentStatus> {
    Json(CommunityNodeConsentStatus {
        all_required_accepted: true,
        items: Vec::new(),
        policy_snapshot_revision: None,
    })
}

// #857: 認証前の公開カタログ照合に応答する。テスト側でシード済みのローカル同意
// (builder-preview v1)と一致させる。
async fn admission_policies() -> Json<kukuri_cn_protocol::CommunityNodePoliciesResponse> {
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
            policy_snapshot_revision: None,
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
        policy_snapshot_revision: None,
    })
}

async fn admission_heartbeat(State(state): State<Arc<AdmissionMockState>>) -> Response {
    if state.heartbeat_unauthorized.load(Ordering::SeqCst) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(ApiErrorBody {
                code: "AUTH_REQUIRED".into(),
                message: "subscriber is banned".into(),
            }),
        )
            .into_response();
    }
    Json(BootstrapHeartbeatResponse {
        expires_at: Utc::now().timestamp() + 300,
    })
    .into_response()
}

async fn admission_bootstrap_nodes(
    State(state): State<Arc<AdmissionMockState>>,
) -> Json<BootstrapNodesResponse> {
    Json(BootstrapNodesResponse {
        nodes: vec![kukuri_cn_protocol::CommunityNodeBootstrapNode {
            base_url: state.base_url.clone(),
            resolved_urls: CommunityNodeResolvedUrls::new(
                state.base_url.clone(),
                Vec::new(),
                Vec::new(),
            )
            .expect("resolved urls"),
        }],
    })
}

async fn spawn_admission_mock(
    required_invite: Option<&str>,
) -> (String, Arc<AdmissionMockState>, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind admission listener");
    let base_url = format!("http://{}", listener.local_addr().expect("local addr"));
    let state = Arc::new(AdmissionMockState {
        base_url: base_url.clone(),
        required_invite: Arc::new(Mutex::new(required_invite.map(str::to_string))),
        forced_rejection: Arc::new(Mutex::new(None)),
        received_invites: Arc::new(Mutex::new(Vec::new())),
        verify_hits: Arc::new(AtomicUsize::new(0)),
        heartbeat_unauthorized: Arc::new(AtomicBool::new(false)),
    });
    let app = Router::new()
        .route("/v1/auth/challenge", post(admission_challenge))
        .route("/v1/auth/verify", post(admission_verify))
        .route("/v1/consents/status", get(admission_consent_status))
        .route("/v1/policies", get(admission_policies))
        .route("/v1/bootstrap/heartbeat", post(admission_heartbeat))
        .route("/v1/bootstrap/nodes", get(admission_bootstrap_nodes))
        .with_state(state.clone());
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (base_url, state, server)
}

async fn admission_runtime(db_path: &Path, nodes: Vec<String>) -> DesktopRuntime {
    let runtime = DesktopRuntime::new_with_config_and_identity(
        db_path,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime");
    for base_url in &nodes {
        seed_local_community_node_consents(&runtime, base_url.as_str(), 1);
    }
    *runtime.community_node_config.lock().await = CommunityNodeConfig {
        trust_node_priority: Vec::new(),
        nodes: nodes
            .into_iter()
            .map(|base_url| CommunityNodeNodeConfig {
                content_advisory_enabled: true,
                base_url,
                resolved_urls: None,
            })
            .collect(),
    };
    runtime
}

#[test]
fn invite_code_storage_is_scoped_by_normalized_node_url() {
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("invite-storage.db");
    persist_community_node_invite_code(
        &db_path,
        IdentityStorageMode::FileOnly,
        "https://a.example",
        "invite-a",
    )
    .expect("persist invite code");

    assert_eq!(
        load_community_node_invite_code(
            &db_path,
            IdentityStorageMode::FileOnly,
            "https://a.example"
        )
        .expect("load invite code")
        .as_deref(),
        Some("invite-a")
    );
    assert_eq!(
        load_community_node_invite_code(
            &db_path,
            IdentityStorageMode::FileOnly,
            "https://b.example"
        )
        .expect("load other invite code"),
        None
    );
}

#[tokio::test]
async fn removing_or_clearing_node_config_deletes_its_invite_code() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("invite-config-cleanup.db");
    let base_url = "https://invite.example".to_string();
    let runtime = admission_runtime(&db_path, vec![base_url.clone()]).await;
    persist_community_node_invite_code(
        &db_path,
        IdentityStorageMode::FileOnly,
        base_url.as_str(),
        "first-code",
    )
    .expect("persist invite code before removal");

    runtime
        .set_community_node_config(SetCommunityNodeConfigRequest {
            trust_node_priority: None,
            nodes: Vec::new(),
        })
        .await
        .expect("remove node config");
    assert_eq!(
        load_community_node_invite_code(&db_path, IdentityStorageMode::FileOnly, base_url.as_str())
            .expect("load removed invite code"),
        None
    );

    *runtime.community_node_config.lock().await = CommunityNodeConfig {
        trust_node_priority: Vec::new(),
        nodes: vec![CommunityNodeNodeConfig {
            content_advisory_enabled: true,
            base_url: base_url.clone(),
            resolved_urls: None,
        }],
    };
    seed_local_community_node_consents(&runtime, base_url.as_str(), 1);
    persist_community_node_invite_code(
        &db_path,
        IdentityStorageMode::FileOnly,
        base_url.as_str(),
        "second-code",
    )
    .expect("persist invite code before clear");
    runtime
        .clear_community_node_config()
        .await
        .expect("clear node config");
    assert_eq!(
        load_community_node_invite_code(&db_path, IdentityStorageMode::FileOnly, base_url.as_str())
            .expect("load cleared invite code"),
        None
    );

    runtime.shutdown().await;
}

#[tokio::test]
async fn authentication_sends_invite_only_to_its_node() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("invite-node-scope.db");
    let (base_url_a, state_a, server_a) = spawn_admission_mock(None).await;
    let (base_url_b, state_b, server_b) = spawn_admission_mock(None).await;
    let runtime = admission_runtime(&db_path, vec![base_url_a.clone(), base_url_b.clone()]).await;
    persist_community_node_invite_code(
        &db_path,
        IdentityStorageMode::FileOnly,
        base_url_a.as_str(),
        "invite-a",
    )
    .expect("persist invite code");

    runtime
        .request_community_node_authentication_token(base_url_a.as_str())
        .await
        .expect("authenticate node a");
    runtime
        .request_community_node_authentication_token(base_url_b.as_str())
        .await
        .expect("authenticate node b");

    assert_eq!(
        state_a.received_invites.lock().await.as_slice(),
        &[Some("invite-a".into())]
    );
    assert_eq!(state_b.received_invites.lock().await.as_slice(), &[None]);

    runtime.shutdown().await;
    server_a.abort();
    server_b.abort();
}

#[tokio::test]
async fn auth_verify_preserves_all_stable_admission_rejection_codes() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("admission-codes.db");
    let (base_url, state, server) = spawn_admission_mock(None).await;
    let runtime = admission_runtime(&db_path, vec![base_url.clone()]).await;
    let cases = [
        (
            "INVITE_REQUIRED",
            CommunityNodeAdmissionRejectionCode::InviteRequired,
        ),
        (
            "INVITE_INVALID",
            CommunityNodeAdmissionRejectionCode::InviteInvalid,
        ),
        (
            "INVITE_EXPIRED",
            CommunityNodeAdmissionRejectionCode::InviteExpired,
        ),
        (
            "INVITE_EXHAUSTED",
            CommunityNodeAdmissionRejectionCode::InviteExhausted,
        ),
        (
            "INVITE_REVOKED",
            CommunityNodeAdmissionRejectionCode::InviteRevoked,
        ),
        (
            "NOT_ALLOWLISTED",
            CommunityNodeAdmissionRejectionCode::NotAllowlisted,
        ),
        ("BANNED", CommunityNodeAdmissionRejectionCode::Banned),
    ];

    for (wire_code, expected) in cases {
        *state.forced_rejection.lock().await = Some(ApiErrorBody {
            code: wire_code.into(),
            message: format!("message for {wire_code}"),
        });
        let error = runtime
            .request_community_node_authentication_token(base_url.as_str())
            .await
            .expect_err("admission rejection");
        let rejection = error
            .downcast_ref::<CommunityNodeAdmissionRejection>()
            .expect("typed admission rejection");
        assert_eq!(rejection.code, expected);
        assert_eq!(rejection.message, format!("message for {wire_code}"));
    }

    runtime.shutdown().await;
    server.abort();
}

#[tokio::test]
async fn admission_rejection_waits_for_user_and_saved_invite_recovers_session() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("admission-session.db");
    let (base_url, state, server) = spawn_admission_mock(Some("join-code")).await;
    let runtime = admission_runtime(&db_path, vec![base_url.clone()]).await;

    runtime.run_community_node_session_maintenance_once().await;
    runtime.run_community_node_session_maintenance_once().await;
    let waiting = runtime
        .get_community_node_statuses()
        .await
        .expect("waiting status")
        .remove(0);
    assert_eq!(state.verify_hits.load(Ordering::SeqCst), 1);
    assert_eq!(
        waiting.session_phase,
        CommunityNodeSessionPhase::AwaitingAdmission
    );
    assert_eq!(waiting.retry_after, None);
    assert!(!waiting.invite_code_saved);
    assert_eq!(
        waiting
            .admission_rejection
            .as_ref()
            .expect("admission rejection")
            .code,
        CommunityNodeAdmissionRejectionCode::InviteRequired
    );

    let ready = runtime
        .set_community_node_invite_code(SetCommunityNodeInviteCodeRequest {
            base_url: base_url.clone(),
            invite_code: Some("join-code".into()),
        })
        .await
        .expect("save invite and authenticate");
    assert_eq!(ready.session_phase, CommunityNodeSessionPhase::Ready);
    assert!(ready.auth_state.authenticated);
    assert!(ready.invite_code_saved);
    assert_eq!(ready.admission_rejection, None);

    runtime
        .clear_community_node_token(CommunityNodeTargetRequest {
            base_url: base_url.clone(),
        })
        .await
        .expect("clear token");
    let reconnected = runtime
        .authenticate_community_node(CommunityNodeTargetRequest {
            base_url: base_url.clone(),
        })
        .await
        .expect("reauthenticate with saved invite");
    assert_eq!(reconnected.session_phase, CommunityNodeSessionPhase::Ready);
    assert_eq!(
        state.received_invites.lock().await.as_slice(),
        &[None, Some("join-code".into()), Some("join-code".into())]
    );

    runtime.shutdown().await;
    server.abort();
}

#[tokio::test]
async fn banned_node_does_not_schedule_retries() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("admission-banned.db");
    let (base_url, state, server) = spawn_admission_mock(None).await;
    *state.forced_rejection.lock().await = Some(ApiErrorBody {
        code: "BANNED".into(),
        message: "node-local support denied".into(),
    });
    let runtime = admission_runtime(&db_path, vec![base_url.clone()]).await;

    runtime.run_community_node_session_maintenance_once().await;
    runtime.run_community_node_session_maintenance_once().await;
    let status = runtime
        .get_community_node_statuses()
        .await
        .expect("banned status")
        .remove(0);

    assert_eq!(state.verify_hits.load(Ordering::SeqCst), 1);
    assert_eq!(
        status.session_phase,
        CommunityNodeSessionPhase::AwaitingAdmission
    );
    assert_eq!(status.retry_after, None);
    assert_eq!(
        status
            .admission_rejection
            .as_ref()
            .expect("banned rejection")
            .code,
        CommunityNodeAdmissionRejectionCode::Banned
    );

    runtime.shutdown().await;
    server.abort();
}

#[tokio::test]
async fn banned_member_with_stored_token_stops_self_heal_reauthentication() {
    // #708: 参加済み利用者が禁止された場合、端末側トークンが未失効でも「認証済み」と扱わず、
    // 自己修復経路(refresh_community_node_metadata)からも再認証を繰り返さない。
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("admission-banned-member.db");
    let (base_url, state, server) = spawn_admission_mock(None).await;
    let runtime = admission_runtime(&db_path, vec![base_url.clone()]).await;

    // まず正常に認証・接続する。
    let ready = runtime
        .authenticate_community_node(CommunityNodeTargetRequest {
            base_url: base_url.clone(),
        })
        .await
        .expect("authenticate");
    assert!(ready.auth_state.authenticated);
    assert_eq!(state.verify_hits.load(Ordering::SeqCst), 1);

    // サーバ側で参加禁止: 既存トークンの心拍は 401、再認証は 403 BANNED。
    state.heartbeat_unauthorized.store(true, Ordering::SeqCst);
    *state.forced_rejection.lock().await = Some(ApiErrorBody {
        code: "BANNED".into(),
        message: "node-local support denied".into(),
    });

    // 自己修復経路が呼ぶ metadata 更新: 心拍 401 → 再認証 → 403 で参加拒否に落ちる。
    let rejected = runtime
        .refresh_community_node_metadata(CommunityNodeTargetRequest {
            base_url: base_url.clone(),
        })
        .await
        .expect("rejection is folded into session state, not an error");
    assert_eq!(state.verify_hits.load(Ordering::SeqCst), 2);
    assert!(
        !rejected.auth_state.authenticated,
        "banned member must not stay authenticated"
    );
    assert_eq!(
        rejected.session_phase,
        CommunityNodeSessionPhase::AwaitingAdmission
    );
    assert_eq!(rejected.retry_after, None);
    assert_eq!(
        rejected
            .admission_rejection
            .as_ref()
            .expect("banned rejection")
            .code,
        CommunityNodeAdmissionRejectionCode::Banned
    );
    assert!(
        crate::community_node::load_community_node_token(
            &db_path,
            IdentityStorageMode::FileOnly,
            base_url.as_str()
        )
        .expect("load token")
        .is_none(),
        "stored token must be discarded on admission rejection"
    );

    // 以後、定期処理でも自己修復判定でも認証要求は増えない。
    runtime.run_community_node_session_maintenance_once().await;
    runtime.run_community_node_session_maintenance_once().await;
    assert_eq!(state.verify_hits.load(Ordering::SeqCst), 2);
    assert!(
        runtime
            .ready_community_node_base_urls()
            .await
            .expect("ready base urls")
            .is_empty(),
        "rejected node must not be a self-heal target"
    );
    let status = runtime
        .get_community_node_statuses()
        .await
        .expect("status")
        .remove(0);
    assert_eq!(
        status.session_phase,
        CommunityNodeSessionPhase::AwaitingAdmission
    );
    assert!(!status.auth_state.authenticated);

    runtime.shutdown().await;
    server.abort();
}
