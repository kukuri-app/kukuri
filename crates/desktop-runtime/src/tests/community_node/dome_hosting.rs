use super::super::*;
use kukuri_cn_protocol::CONSENT_REQUIRED_CODE;

mod transfer_contract;

async fn dome_runtime() -> (
    DesktopRuntime,
    String,
    Arc<MockManagedCommunityNodeState>,
    tokio::task::JoinHandle<()>,
    tempfile::TempDir,
) {
    dome_runtime_with_routes(Router::new()).await
}

async fn dome_runtime_with_routes(
    extra_routes: Router,
) -> (
    DesktopRuntime,
    String,
    Arc<MockManagedCommunityNodeState>,
    tokio::task::JoinHandle<()>,
    tempfile::TempDir,
) {
    let dir = tempdir().expect("tempdir");
    let runtime = DesktopRuntime::new_with_config_and_identity(
        dir.path().join("community-node-dome-consent.db"),
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime");
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let base_url = format!("http://{}", listener.local_addr().expect("local addr"));
    let state = Arc::new(MockManagedCommunityNodeState::new(
        base_url.clone(),
        Vec::new(),
        true,
        Arc::new(Mutex::new("unused-token".to_string())),
    ));
    let app = Router::new()
        .route("/v1/auth/challenge", post(mock_managed_auth_challenge))
        .route("/v1/auth/verify", post(mock_managed_auth_verify))
        .route("/v1/policies", get(mock_managed_policies))
        .route(
            "/v1/dome-hosting/status/{instance_id}",
            get(mock_managed_dome_status),
        )
        .route(
            "/v1/dome-hosting/session/resync",
            post(mock_managed_dome_resync),
        )
        .with_state(state.clone())
        .merge(extra_routes);
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("mock server");
    });
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

#[tokio::test]
async fn dome_requests_stop_before_auth_and_http_without_active_local_consent() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let (runtime, base_url, state, server, _dir) = dome_runtime().await;

    let get_error = runtime
        .get_dome_hosting_status_from_community_node(base_url.as_str(), "instance-1")
        .await
        .expect_err("Dome GET without consent must be rejected");
    let get_error = get_error
        .downcast_ref::<crate::DomeHostingRequestError>()
        .expect("typed Dome error");
    assert_eq!(get_error.code, CONSENT_REQUIRED_CODE);

    let post_error = runtime
        .resync_dome_snapshots_from_community_node(
            base_url.as_str(),
            &kukuri_cn_protocol::DomeHostingSnapshotResyncRequest {
                instance_id: "instance-1".to_string(),
                after_sequence: 0,
            },
        )
        .await
        .expect_err("Dome POST without consent must be rejected");
    let post_error = post_error
        .downcast_ref::<crate::DomeHostingRequestError>()
        .expect("typed Dome error");
    assert_eq!(post_error.code, CONSENT_REQUIRED_CODE);

    assert_eq!(state.policies_hits.load(Ordering::SeqCst), 0);
    assert_eq!(state.challenge_hits.load(Ordering::SeqCst), 0);
    assert_eq!(state.verify_hits.load(Ordering::SeqCst), 0);
    assert_eq!(state.dome_get_hits.load(Ordering::SeqCst), 0);
    assert_eq!(state.dome_post_hits.load(Ordering::SeqCst), 0);
    assert!(
        crate::community_node::load_community_node_token(
            &runtime.db_path,
            IdentityStorageMode::FileOnly,
            base_url.as_str(),
        )
        .await
        .expect("load token state")
        .is_none(),
        "Dome consent rejection must not persist a JWT"
    );

    runtime.shutdown().await;
    server.abort();
}

#[tokio::test]
async fn dome_requests_stop_before_auth_and_http_after_consent_withdrawal() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let (runtime, base_url, state, server, _dir) = dome_runtime().await;
    seed_local_community_node_consents(&runtime, base_url.as_str(), 1).await;
    runtime
        .withdraw_community_node_consents(crate::CommunityNodeTargetRequest {
            base_url: base_url.clone(),
        })
        .await
        .expect("withdraw consent");

    let error = runtime
        .get_dome_hosting_status_from_community_node(base_url.as_str(), "instance-1")
        .await
        .expect_err("Dome GET after withdrawal must be rejected");
    assert_eq!(
        error
            .downcast_ref::<crate::DomeHostingRequestError>()
            .expect("typed Dome error")
            .code,
        CONSENT_REQUIRED_CODE
    );
    assert_eq!(state.policies_hits.load(Ordering::SeqCst), 0);
    assert_eq!(state.challenge_hits.load(Ordering::SeqCst), 0);
    assert_eq!(state.verify_hits.load(Ordering::SeqCst), 0);
    assert_eq!(state.dome_get_hits.load(Ordering::SeqCst), 0);

    runtime.shutdown().await;
    server.abort();
}

#[tokio::test]
async fn dome_requests_stop_before_auth_and_http_when_current_policy_changed() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let (runtime, base_url, state, server, _dir) = dome_runtime().await;
    seed_local_community_node_consents(&runtime, base_url.as_str(), 1).await;
    state.simulate_pending_update.store(true, Ordering::SeqCst);

    let error = runtime
        .resync_dome_snapshots_from_community_node(
            base_url.as_str(),
            &kukuri_cn_protocol::DomeHostingSnapshotResyncRequest {
                instance_id: "instance-1".to_string(),
                after_sequence: 0,
            },
        )
        .await
        .expect_err("Dome POST while reconsent is pending must be rejected");
    assert_eq!(
        error
            .downcast_ref::<crate::DomeHostingRequestError>()
            .expect("typed Dome error")
            .code,
        CONSENT_REQUIRED_CODE
    );
    assert_eq!(state.policies_hits.load(Ordering::SeqCst), 1);
    assert_eq!(state.challenge_hits.load(Ordering::SeqCst), 0);
    assert_eq!(state.verify_hits.load(Ordering::SeqCst), 0);
    assert_eq!(state.dome_post_hits.load(Ordering::SeqCst), 0);
    let statuses = runtime
        .get_community_node_statuses()
        .await
        .expect("community node statuses");
    assert!(statuses[0].consent_update_pending);

    runtime.shutdown().await;
    server.abort();
}

#[tokio::test]
async fn dome_requests_reject_unconfigured_nodes_before_any_http() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let (runtime, base_url, state, server, _dir) = dome_runtime().await;
    *runtime.community_node_config.lock().await = CommunityNodeConfig::default();

    let error = runtime
        .get_dome_hosting_status_from_community_node(base_url.as_str(), "instance-1")
        .await
        .expect_err("unconfigured Dome target must be rejected");
    assert_eq!(
        error
            .downcast_ref::<crate::DomeHostingRequestError>()
            .expect("typed Dome error")
            .code,
        "DOME_HOSTING_TARGET_NOT_CONFIGURED"
    );
    assert_eq!(state.policies_hits.load(Ordering::SeqCst), 0);
    assert_eq!(state.challenge_hits.load(Ordering::SeqCst), 0);
    assert_eq!(state.verify_hits.load(Ordering::SeqCst), 0);
    assert_eq!(state.dome_get_hits.load(Ordering::SeqCst), 0);

    runtime.shutdown().await;
    server.abort();
}

#[tokio::test]
async fn dome_get_and_post_succeed_only_after_current_local_consent() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let (runtime, base_url, state, server, _dir) = dome_runtime().await;
    seed_local_community_node_consents(&runtime, base_url.as_str(), 1).await;

    let status = runtime
        .get_dome_hosting_status_from_community_node(base_url.as_str(), "instance-1")
        .await
        .expect("consented Dome GET");
    assert_eq!(status.instance_id, "instance-1");
    let snapshots = runtime
        .resync_dome_snapshots_from_community_node(
            base_url.as_str(),
            &kukuri_cn_protocol::DomeHostingSnapshotResyncRequest {
                instance_id: "instance-1".to_string(),
                after_sequence: 0,
            },
        )
        .await
        .expect("consented Dome POST");
    assert!(snapshots.snapshots.is_empty());

    assert_eq!(state.policies_hits.load(Ordering::SeqCst), 2);
    assert_eq!(state.challenge_hits.load(Ordering::SeqCst), 1);
    assert_eq!(state.verify_hits.load(Ordering::SeqCst), 1);
    assert_eq!(state.dome_get_hits.load(Ordering::SeqCst), 1);
    assert_eq!(state.dome_post_hits.load(Ordering::SeqCst), 1);

    runtime.shutdown().await;
    server.abort();
}

#[tokio::test]
async fn dome_get_reauthenticates_after_unauthorized_only_with_current_consent() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let (runtime, base_url, state, server, _dir) = dome_runtime().await;
    seed_local_community_node_consents(&runtime, base_url.as_str(), 1).await;
    persist_community_node_token(
        &runtime.db_path,
        IdentityStorageMode::FileOnly,
        base_url.as_str(),
        &StoredCommunityNodeToken {
            access_token: "stale-client-token".to_string(),
            expires_at: Utc::now().timestamp() + 3600,
        },
    )
    .await
    .expect("persist stale token");

    let status = runtime
        .get_dome_hosting_status_from_community_node(base_url.as_str(), "instance-1")
        .await
        .expect("Dome GET retries after authentication");
    assert_eq!(status.instance_id, "instance-1");
    assert_eq!(state.policies_hits.load(Ordering::SeqCst), 1);
    assert_eq!(state.challenge_hits.load(Ordering::SeqCst), 1);
    assert_eq!(state.verify_hits.load(Ordering::SeqCst), 1);
    assert_eq!(state.dome_get_hits.load(Ordering::SeqCst), 1);

    runtime.shutdown().await;
    server.abort();
}
