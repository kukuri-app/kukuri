use super::super::*;

pub(crate) async fn apply_relay_backed_community_node_seed_peers(
    runtime: &DesktopRuntime,
    base_url: &str,
    relay_url: &str,
    seed_peers: Vec<CommunityNodeSeedPeer>,
) {
    seed_local_community_node_consents(runtime, base_url, 1);
    mark_community_node_session_ready_for_test(runtime, base_url).await;
    *runtime.community_node_config.lock().await = CommunityNodeConfig {
        trust_node_priority: Vec::new(),
        nodes: vec![CommunityNodeNodeConfig {
            content_advisory_enabled: true,
            base_url: base_url.to_string(),
            resolved_urls: Some(
                CommunityNodeResolvedUrls::new(base_url, vec![relay_url.to_string()], seed_peers)
                    .expect("resolved urls"),
            ),
        }],
    };
    timeout(
        Duration::from_secs(30),
        runtime.apply_runtime_connectivity_assist(),
    )
    .await
    .expect("apply assist timeout")
    .expect("apply assist");
    timeout(
        Duration::from_secs(15),
        runtime.apply_effective_seed_peers(),
    )
    .await
    .expect("apply seed peers timeout")
    .expect("apply seed peers");
}

pub(crate) async fn mark_community_node_session_ready_for_test(
    runtime: &DesktopRuntime,
    base_url: &str,
) {
    let local_consent = crate::community_node::load_community_node_local_consents(
        &runtime.db_path,
        runtime.identity_mode,
        base_url,
    )
    .expect("load local consent");
    runtime
        .set_community_node_session_ready(base_url, false, local_consent)
        .await;
}
#[derive(Clone)]
pub(crate) struct MockCommunityNodeState {
    pub(crate) base_url: String,
    pub(crate) seed_peers: Arc<Mutex<Vec<CommunityNodeSeedPeer>>>,
    pub(crate) heartbeat_seed_peers: Arc<Mutex<Option<Vec<CommunityNodeSeedPeer>>>>,
    pub(crate) heartbeat_hits: Arc<AtomicUsize>,
    pub(crate) bootstrap_hits: Arc<AtomicUsize>,
}

#[derive(Clone)]
pub(crate) struct MockHeartbeatEchoCommunityNodeState {
    pub(crate) base_url: String,
    pub(crate) connectivity_urls: Vec<String>,
    pub(crate) seed_peers: Arc<Mutex<Vec<CommunityNodeSeedPeer>>>,
    pub(crate) heartbeat_hits: Arc<AtomicUsize>,
    pub(crate) bootstrap_hits: Arc<AtomicUsize>,
}

pub(crate) async fn mock_bootstrap_heartbeat(
    State(state): State<Arc<MockCommunityNodeState>>,
    Json(_request): Json<serde_json::Value>,
) -> Json<BootstrapHeartbeatResponse> {
    state.heartbeat_hits.fetch_add(1, Ordering::SeqCst);
    if let Some(seed_peers) = state.heartbeat_seed_peers.lock().await.take() {
        *state.seed_peers.lock().await = seed_peers;
    }
    Json(BootstrapHeartbeatResponse {
        expires_at: Utc::now().timestamp() + 300,
    })
}

pub(crate) async fn mock_bootstrap_nodes(
    State(state): State<Arc<MockCommunityNodeState>>,
) -> Json<BootstrapNodesResponse> {
    state.bootstrap_hits.fetch_add(1, Ordering::SeqCst);
    let seed_peers = state.seed_peers.lock().await.clone();
    Json(BootstrapNodesResponse {
        nodes: vec![kukuri_cn_protocol::CommunityNodeBootstrapNode {
            base_url: state.base_url.clone(),
            resolved_urls: CommunityNodeResolvedUrls::new(
                state.base_url.clone(),
                Vec::new(),
                seed_peers,
            )
            .expect("resolved urls"),
        }],
    })
}

pub(crate) async fn mock_bootstrap_consent_status() -> Json<CommunityNodeConsentStatus> {
    Json(managed_community_node_consent_status(true))
}

pub(crate) async fn mock_current_policies()
-> Json<kukuri_cn_protocol::CommunityNodePoliciesResponse> {
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

pub(crate) async fn mock_heartbeat_echo_bootstrap_heartbeat(
    State(state): State<Arc<MockHeartbeatEchoCommunityNodeState>>,
    Json(request): Json<serde_json::Value>,
) -> Json<BootstrapHeartbeatResponse> {
    state.heartbeat_hits.fetch_add(1, Ordering::SeqCst);
    let endpoint_id = request
        .get("endpoint_id")
        .and_then(serde_json::Value::as_str)
        .expect("heartbeat endpoint id")
        .to_string();
    let addr_hint = request
        .get("addr_hint")
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned);
    *state.seed_peers.lock().await =
        vec![CommunityNodeSeedPeer::new(endpoint_id, addr_hint).expect("heartbeat seed peer")];
    Json(BootstrapHeartbeatResponse {
        expires_at: Utc::now().timestamp() + 300,
    })
}

pub(crate) async fn mock_heartbeat_echo_bootstrap_nodes(
    State(state): State<Arc<MockHeartbeatEchoCommunityNodeState>>,
) -> Json<BootstrapNodesResponse> {
    state.bootstrap_hits.fetch_add(1, Ordering::SeqCst);
    let seed_peers = state.seed_peers.lock().await.clone();
    Json(BootstrapNodesResponse {
        nodes: vec![kukuri_cn_protocol::CommunityNodeBootstrapNode {
            base_url: state.base_url.clone(),
            resolved_urls: CommunityNodeResolvedUrls::new(
                state.base_url.clone(),
                state.connectivity_urls.clone(),
                seed_peers,
            )
            .expect("resolved urls"),
        }],
    })
}

// topic rendezvous heartbeat を観測するシナリオ用 state(#572)。
// 既存 MockCommunityNodeState の構築箇所を増やさないため、シナリオ別 state として分ける
// (MockHeartbeatEchoCommunityNodeState と同じ前例)。
#[derive(Clone)]
pub(crate) struct MockRendezvousCommunityNodeState {
    pub(crate) base_url: String,
    pub(crate) seed_peers: Vec<CommunityNodeSeedPeer>,
    pub(crate) heartbeat_hits: Arc<AtomicUsize>,
    pub(crate) bootstrap_hits: Arc<AtomicUsize>,
    pub(crate) rendezvous_hits: Arc<AtomicUsize>,
    pub(crate) rendezvous_requests: Arc<Mutex<Vec<kukuri_cn_protocol::TopicRendezvousHeartbeat>>>,
}

pub(crate) async fn mock_rendezvous_bootstrap_heartbeat(
    State(state): State<Arc<MockRendezvousCommunityNodeState>>,
    Json(_request): Json<serde_json::Value>,
) -> Json<BootstrapHeartbeatResponse> {
    state.heartbeat_hits.fetch_add(1, Ordering::SeqCst);
    Json(BootstrapHeartbeatResponse {
        expires_at: Utc::now().timestamp() + 300,
    })
}

pub(crate) async fn mock_rendezvous_bootstrap_nodes(
    State(state): State<Arc<MockRendezvousCommunityNodeState>>,
) -> Json<BootstrapNodesResponse> {
    state.bootstrap_hits.fetch_add(1, Ordering::SeqCst);
    Json(BootstrapNodesResponse {
        nodes: vec![kukuri_cn_protocol::CommunityNodeBootstrapNode {
            base_url: state.base_url.clone(),
            resolved_urls: CommunityNodeResolvedUrls::new(
                state.base_url.clone(),
                Vec::new(),
                state.seed_peers.clone(),
            )
            .expect("resolved urls"),
        }],
    })
}

pub(crate) async fn mock_rendezvous_topics_heartbeat(
    State(state): State<Arc<MockRendezvousCommunityNodeState>>,
    Json(request): Json<kukuri_cn_protocol::TopicRendezvousHeartbeat>,
) -> Json<kukuri_cn_protocol::TopicRendezvousHeartbeatResponse> {
    assert!(
        !request.refreshes.is_empty() || !request.joins.is_empty(),
        "rendezvous heartbeat without topics"
    );
    state.rendezvous_hits.fetch_add(1, Ordering::SeqCst);
    state.rendezvous_requests.lock().await.push(request);
    // expires_in_seconds はクライアントのマージン(20 秒)より小さくし、deadline が
    // 毎 maintenance pass で即時 due になるようにする(wall-clock 待ちなしの決定的テスト用)。
    Json(kukuri_cn_protocol::TopicRendezvousHeartbeatResponse {
        expires_in_seconds: 5,
        topics: Vec::new(),
    })
}

#[derive(Clone)]
pub(crate) struct MockManagedCommunityNodeState {
    pub(crate) base_url: String,
    pub(crate) seed_peers: Vec<CommunityNodeSeedPeer>,
    pub(crate) consent_accepted: Arc<AtomicBool>,
    pub(crate) current_token: Arc<Mutex<String>>,
    pub(crate) challenge_hits: Arc<AtomicUsize>,
    pub(crate) verify_hits: Arc<AtomicUsize>,
    pub(crate) consent_status_hits: Arc<AtomicUsize>,
    pub(crate) consent_accept_hits: Arc<AtomicUsize>,
    pub(crate) heartbeat_hits: Arc<AtomicUsize>,
    pub(crate) bootstrap_hits: Arc<AtomicUsize>,
    // #857: 認証不要の公開 policy カタログ(GET /v1/policies)の観測カウンタ。
    pub(crate) policies_hits: Arc<AtomicUsize>,
    pub(crate) manifest_hits: Arc<AtomicUsize>,
    pub(crate) dome_get_hits: Arc<AtomicUsize>,
    pub(crate) dome_post_hits: Arc<AtomicUsize>,
    // true の場合、未同意状態を「版が上がった更新（旧版は同意済み）」として返す。
    // ローカル同意が旧版のままの node で黙って再受諾しない挙動の検証に使う（#384 / #857）。
    pub(crate) simulate_pending_update: Arc<AtomicBool>,
    // true の場合、文書版は据え置いたまま snapshot revision だけを更新する。
    pub(crate) simulate_snapshot_update: Arc<AtomicBool>,
}

impl MockManagedCommunityNodeState {
    pub(crate) fn new(
        base_url: String,
        seed_peers: Vec<CommunityNodeSeedPeer>,
        consent_accepted: bool,
        current_token: Arc<Mutex<String>>,
    ) -> Self {
        Self {
            base_url,
            seed_peers,
            consent_accepted: Arc::new(AtomicBool::new(consent_accepted)),
            current_token,
            challenge_hits: Arc::new(AtomicUsize::new(0)),
            verify_hits: Arc::new(AtomicUsize::new(0)),
            consent_status_hits: Arc::new(AtomicUsize::new(0)),
            consent_accept_hits: Arc::new(AtomicUsize::new(0)),
            heartbeat_hits: Arc::new(AtomicUsize::new(0)),
            bootstrap_hits: Arc::new(AtomicUsize::new(0)),
            policies_hits: Arc::new(AtomicUsize::new(0)),
            manifest_hits: Arc::new(AtomicUsize::new(0)),
            dome_get_hits: Arc::new(AtomicUsize::new(0)),
            dome_post_hits: Arc::new(AtomicUsize::new(0)),
            simulate_pending_update: Arc::new(AtomicBool::new(false)),
            simulate_snapshot_update: Arc::new(AtomicBool::new(false)),
        }
    }
}

pub(crate) async fn mock_managed_dome_status(
    State(state): State<Arc<MockManagedCommunityNodeState>>,
    axum::extract::Path(instance_id): axum::extract::Path<String>,
    headers: HeaderMap,
) -> std::result::Result<Json<kukuri_cn_protocol::DomeHostingStatusResponse>, StatusCode> {
    authorize_managed_community_node_request(&headers, state.as_ref()).await?;
    state.dome_get_hits.fetch_add(1, Ordering::SeqCst);
    Ok(Json(kukuri_cn_protocol::DomeHostingStatusResponse {
        instance_id,
        state: kukuri_core::DomeHostingStateKindV1::CommunityNodeHosted,
        lease_epoch: 1,
        session_id: Some("mock-dome-session".to_string()),
        participants: 1,
        sleeping: false,
        signed_heartbeat: None,
        expires_at: Utc::now().timestamp_millis() + 60_000,
        resource_budget: kukuri_core::MetaverseResourceBudgetConfig::default(),
        resource_metrics: kukuri_core::MetaverseResourceMetricsV1::default(),
    }))
}

pub(crate) async fn mock_managed_dome_resync(
    State(state): State<Arc<MockManagedCommunityNodeState>>,
    headers: HeaderMap,
    Json(_request): Json<kukuri_cn_protocol::DomeHostingSnapshotResyncRequest>,
) -> std::result::Result<Json<kukuri_cn_protocol::DomeHostingSnapshotResyncResponse>, StatusCode> {
    authorize_managed_community_node_request(&headers, state.as_ref()).await?;
    state.dome_post_hits.fetch_add(1, Ordering::SeqCst);
    Ok(Json(
        kukuri_cn_protocol::DomeHostingSnapshotResyncResponse {
            snapshots: Vec::new(),
        },
    ))
}

/// #857: mock CN の policy カタログの現行版。simulate_pending_update 時は版が上がる。
pub(crate) const MOCK_MANAGED_POLICY_SLUG: &str = "builder-preview";

/// #857: ユーザーが同意モーダルで受諾した状態をローカル同意記録として直接シードする。
/// version は同意した版(mock カタログの現行版は 1、pending update 時は 2)。
pub(crate) fn seed_local_community_node_consents(
    runtime: &DesktopRuntime,
    base_url: &str,
    version: i32,
) {
    seed_local_community_node_consents_with_snapshot(runtime, base_url, version, None);
}

pub(crate) fn seed_local_community_node_consents_with_snapshot(
    runtime: &DesktopRuntime,
    base_url: &str,
    version: i32,
    policy_snapshot_revision: Option<&str>,
) {
    let mut state = crate::CommunityNodeLocalConsentState::default();
    crate::community_node::record_community_node_local_consents(
        &mut state,
        &[crate::CommunityNodeConsentDocumentRef {
            policy_slug: MOCK_MANAGED_POLICY_SLUG.to_string(),
            policy_version: version,
            policy_snapshot_revision: policy_snapshot_revision.map(str::to_string),
        }],
        "ja",
        "test-app",
        Utc::now().timestamp(),
    );
    crate::community_node::persist_community_node_local_consents(
        &runtime.db_path,
        runtime.identity_mode,
        base_url,
        &state,
    )
    .expect("persist local community-node consents");
}

/// #857: 認証不要の公開 policy カタログ。consent status と同じ slug / 版を返す。
pub(crate) async fn mock_managed_policies(
    State(state): State<Arc<MockManagedCommunityNodeState>>,
) -> Json<kukuri_cn_protocol::CommunityNodePoliciesResponse> {
    state.policies_hits.fetch_add(1, Ordering::SeqCst);
    let pending_update = state.simulate_pending_update.load(Ordering::SeqCst);
    let snapshot_update = state.simulate_snapshot_update.load(Ordering::SeqCst);
    Json(kukuri_cn_protocol::CommunityNodePoliciesResponse {
        policies: vec![kukuri_cn_protocol::CommunityNodePolicyDocument {
            policy_kind: None,
            policy_slug: MOCK_MANAGED_POLICY_SLUG.to_string(),
            policy_version: if pending_update { 2 } else { 1 },
            title: "Builder Preview".to_string(),
            body_markdown: "Builder preview policy body.".to_string(),
            required: true,
            effective_date: Some("2026-09-02".to_string()),
            language: Some("ja".to_string()),
            policy_snapshot_revision: snapshot_update.then(|| "snapshot-2".to_string()),
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
        policy_snapshot_revision: snapshot_update.then(|| "snapshot-2".to_string()),
    })
}

pub(crate) async fn mock_managed_manifest(
    State(state): State<Arc<MockManagedCommunityNodeState>>,
) -> StatusCode {
    state.manifest_hits.fetch_add(1, Ordering::SeqCst);
    StatusCode::NOT_FOUND
}

pub(crate) fn managed_community_node_consent_status(accepted: bool) -> CommunityNodeConsentStatus {
    managed_community_node_consent_status_with_update(accepted, false)
}

pub(crate) fn managed_community_node_consent_status_with_update(
    accepted: bool,
    pending_update: bool,
) -> CommunityNodeConsentStatus {
    // 未同意でも「更新（pending_update=true）」のときは過去版の同意を示す previously_accepted_version
    // を返し、初回未同意（previously_accepted_version=None）と区別できるようにする。
    let previously_accepted_version = if accepted || pending_update {
        Some(1)
    } else {
        None
    };
    CommunityNodeConsentStatus {
        all_required_accepted: accepted,
        items: vec![kukuri_cn_protocol::CommunityNodeConsentItem {
            policy_slug: "builder-preview".into(),
            policy_version: if pending_update { 2 } else { 1 },
            title: "Builder Preview".into(),
            body: "Builder preview policy body.".into(),
            required: true,
            effective_date: Some("2026-09-02".to_string()),
            language: Some("ja".to_string()),
            policy_snapshot_revision: None,
            accepted_at: accepted.then(|| Utc::now().timestamp()),
            previously_accepted_version,
        }],
        policy_snapshot_revision: None,
    }
}

pub(crate) async fn authorize_managed_community_node_request(
    headers: &HeaderMap,
    state: &MockManagedCommunityNodeState,
) -> std::result::Result<(), StatusCode> {
    let Some(value) = headers.get(AUTHORIZATION) else {
        return Err(StatusCode::UNAUTHORIZED);
    };
    let Ok(value) = value.to_str() else {
        return Err(StatusCode::UNAUTHORIZED);
    };
    let Some(token) = value.strip_prefix("Bearer ") else {
        return Err(StatusCode::UNAUTHORIZED);
    };
    let current_token = state.current_token.lock().await.clone();
    if token == current_token {
        Ok(())
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}

pub(crate) async fn mock_managed_auth_challenge(
    State(state): State<Arc<MockManagedCommunityNodeState>>,
    Json(_request): Json<serde_json::Value>,
) -> Json<kukuri_cn_protocol::AuthChallengeResponse> {
    state.challenge_hits.fetch_add(1, Ordering::SeqCst);
    Json(kukuri_cn_protocol::AuthChallengeResponse {
        challenge: format!("challenge-{}", state.challenge_hits.load(Ordering::SeqCst)),
        expires_at: Utc::now().timestamp() + 300,
    })
}

pub(crate) async fn mock_managed_auth_verify(
    State(state): State<Arc<MockManagedCommunityNodeState>>,
    Json(_request): Json<serde_json::Value>,
) -> Json<kukuri_cn_protocol::AuthVerifyResponse> {
    let next = state.verify_hits.fetch_add(1, Ordering::SeqCst) + 1;
    let token = format!("managed-token-{next}");
    *state.current_token.lock().await = token.clone();
    Json(kukuri_cn_protocol::AuthVerifyResponse {
        access_token: token,
        token_type: "Bearer".into(),
        expires_at: Utc::now().timestamp() + 3600,
        pubkey: "f".repeat(64),
    })
}

pub(crate) async fn mock_managed_consent_status(
    State(state): State<Arc<MockManagedCommunityNodeState>>,
    headers: HeaderMap,
) -> std::result::Result<Json<CommunityNodeConsentStatus>, StatusCode> {
    authorize_managed_community_node_request(&headers, state.as_ref()).await?;
    state.consent_status_hits.fetch_add(1, Ordering::SeqCst);
    let accepted = state.consent_accepted.load(Ordering::SeqCst);
    let mut status = managed_community_node_consent_status_with_update(
        accepted,
        !accepted && state.simulate_pending_update.load(Ordering::SeqCst),
    );
    if state.simulate_snapshot_update.load(Ordering::SeqCst) {
        status.policy_snapshot_revision = Some("snapshot-2".to_string());
        for item in &mut status.items {
            item.policy_snapshot_revision = Some("snapshot-2".to_string());
        }
    }
    Ok(Json(status))
}

pub(crate) async fn mock_managed_accept_consents(
    State(state): State<Arc<MockManagedCommunityNodeState>>,
    headers: HeaderMap,
    Json(_request): Json<serde_json::Value>,
) -> std::result::Result<Json<CommunityNodeConsentStatus>, StatusCode> {
    authorize_managed_community_node_request(&headers, state.as_ref()).await?;
    state.consent_accept_hits.fetch_add(1, Ordering::SeqCst);
    state.consent_accepted.store(true, Ordering::SeqCst);
    Ok(Json(managed_community_node_consent_status(true)))
}

pub(crate) async fn mock_managed_bootstrap_heartbeat(
    State(state): State<Arc<MockManagedCommunityNodeState>>,
    headers: HeaderMap,
    Json(_request): Json<serde_json::Value>,
) -> std::result::Result<Json<BootstrapHeartbeatResponse>, StatusCode> {
    authorize_managed_community_node_request(&headers, state.as_ref()).await?;
    if !state.consent_accepted.load(Ordering::SeqCst) {
        return Err(StatusCode::FORBIDDEN);
    }
    state.heartbeat_hits.fetch_add(1, Ordering::SeqCst);
    Ok(Json(BootstrapHeartbeatResponse {
        expires_at: Utc::now().timestamp() + 300,
    }))
}

pub(crate) async fn mock_managed_bootstrap_nodes(
    State(state): State<Arc<MockManagedCommunityNodeState>>,
    headers: HeaderMap,
) -> std::result::Result<Json<BootstrapNodesResponse>, StatusCode> {
    authorize_managed_community_node_request(&headers, state.as_ref()).await?;
    if !state.consent_accepted.load(Ordering::SeqCst) {
        return Err(StatusCode::FORBIDDEN);
    }
    state.bootstrap_hits.fetch_add(1, Ordering::SeqCst);
    Ok(Json(BootstrapNodesResponse {
        nodes: vec![kukuri_cn_protocol::CommunityNodeBootstrapNode {
            base_url: state.base_url.clone(),
            resolved_urls: CommunityNodeResolvedUrls::new(
                state.base_url.clone(),
                Vec::new(),
                state.seed_peers.clone(),
            )
            .expect("resolved urls"),
        }],
    }))
}
