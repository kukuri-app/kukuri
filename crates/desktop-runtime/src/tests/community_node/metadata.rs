use super::super::*;

#[tokio::test]
async fn community_node_status_refresh_updates_bootstrap_seed_peers() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("community-heartbeat-refresh.db");
    let runtime = DesktopRuntime::new_with_config_and_identity(
        &db_path,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime");

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let base_url = format!("http://{}", listener.local_addr().expect("local addr"));
    let state = Arc::new(MockCommunityNodeState {
        base_url: base_url.clone(),
        seed_peers: Arc::new(Mutex::new(vec![
            CommunityNodeSeedPeer::new(
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
                None,
            )
            .expect("seed peer"),
        ])),
        heartbeat_seed_peers: Arc::new(Mutex::new(None)),
        heartbeat_hits: Arc::new(AtomicUsize::new(0)),
        bootstrap_hits: Arc::new(AtomicUsize::new(0)),
    });
    let app = Router::new()
        .route("/v1/policies", get(mock_current_policies))
        .route("/v1/consents/status", get(mock_bootstrap_consent_status))
        .route("/v1/bootstrap/heartbeat", post(mock_bootstrap_heartbeat))
        .route("/v1/bootstrap/nodes", get(mock_bootstrap_nodes))
        .with_state(state.clone());
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    persist_community_node_token(
        &db_path,
        IdentityStorageMode::FileOnly,
        base_url.as_str(),
        &StoredCommunityNodeToken {
            access_token: "fake-token".to_string(),
            expires_at: Utc::now().timestamp() + 3600,
        },
    )
    .expect("persist community-node token");
    *runtime.community_node_config.lock().await = CommunityNodeConfig {
        trust_node_priority: Vec::new(),
        nodes: vec![CommunityNodeNodeConfig::new(
            base_url.clone(),
            Some(
                CommunityNodeResolvedUrls::new(base_url.clone(), Vec::new(), Vec::new())
                    .expect("resolved urls"),
            ),
        )],
    };
    seed_local_community_node_consents(&runtime, base_url.as_str(), 1);

    // WP-Q2: registration refresh はスケジューラ tick が駆動し、getter は読み取り専用。
    runtime.run_community_node_session_maintenance_once().await;
    let statuses = runtime
        .get_community_node_statuses()
        .await
        .expect("community node statuses");
    assert_eq!(state.heartbeat_hits.load(Ordering::SeqCst), 1);
    assert_eq!(state.bootstrap_hits.load(Ordering::SeqCst), 1);
    assert_eq!(statuses.len(), 1);
    assert_eq!(
        statuses[0]
            .resolved_urls
            .as_ref()
            .expect("resolved urls")
            .seed_peers,
        state.seed_peers.lock().await.clone()
    );
    assert_eq!(
        runtime.community_node_config.lock().await.nodes[0]
            .resolved_urls
            .as_ref()
            .expect("resolved urls")
            .seed_peers,
        state.seed_peers.lock().await.clone()
    );

    runtime.shutdown().await;
    server.abort();
}

#[tokio::test]
async fn community_node_session_maintenance_updates_bootstrap_seed_peers() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("community-sync-status-refresh.db");
    let runtime = DesktopRuntime::new_with_config_and_identity(
        &db_path,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime");

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let base_url = format!("http://{}", listener.local_addr().expect("local addr"));
    let state = Arc::new(MockCommunityNodeState {
        base_url: base_url.clone(),
        seed_peers: Arc::new(Mutex::new(vec![
            CommunityNodeSeedPeer::new(
                "1111111111111111111111111111111111111111111111111111111111111111",
                None,
            )
            .expect("seed peer"),
        ])),
        heartbeat_seed_peers: Arc::new(Mutex::new(None)),
        heartbeat_hits: Arc::new(AtomicUsize::new(0)),
        bootstrap_hits: Arc::new(AtomicUsize::new(0)),
    });
    let app = Router::new()
        .route("/v1/policies", get(mock_current_policies))
        .route("/v1/consents/status", get(mock_bootstrap_consent_status))
        .route("/v1/bootstrap/heartbeat", post(mock_bootstrap_heartbeat))
        .route("/v1/bootstrap/nodes", get(mock_bootstrap_nodes))
        .with_state(state.clone());
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    persist_community_node_token(
        &db_path,
        IdentityStorageMode::FileOnly,
        base_url.as_str(),
        &StoredCommunityNodeToken {
            access_token: "fake-token".to_string(),
            expires_at: Utc::now().timestamp() + 3600,
        },
    )
    .expect("persist community-node token");
    *runtime.community_node_config.lock().await = CommunityNodeConfig {
        trust_node_priority: Vec::new(),
        nodes: vec![CommunityNodeNodeConfig::new(
            base_url.clone(),
            Some(
                CommunityNodeResolvedUrls::new(base_url.clone(), Vec::new(), Vec::new())
                    .expect("resolved urls"),
            ),
        )],
    };
    seed_local_community_node_consents(&runtime, base_url.as_str(), 1);

    runtime.run_community_node_session_maintenance_once().await;

    assert_eq!(state.heartbeat_hits.load(Ordering::SeqCst), 1);
    assert_eq!(state.bootstrap_hits.load(Ordering::SeqCst), 1);
    assert_eq!(
        runtime.community_node_config.lock().await.nodes[0]
            .resolved_urls
            .as_ref()
            .expect("resolved urls")
            .seed_peers,
        state.seed_peers.lock().await.clone()
    );

    runtime.shutdown().await;
    server.abort();
}

#[tokio::test]
async fn community_node_metadata_refresh_heartbeats_before_bootstrap_sync_even_when_metadata_is_unchanged()
 {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("community-metadata-refresh-no-churn.db");
    let runtime = DesktopRuntime::new_with_config_and_identity(
        &db_path,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime");
    let seed_peer_runtime = DesktopRuntime::new_with_config_and_identity(
        dir.path()
            .join("community-metadata-refresh-no-churn-peer.db"),
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("seed peer runtime");
    let seed_peer = seed_peer_runtime
        .local_community_node_seed_peer("metadata-refresh-test")
        .await
        .expect("seed peer");
    seed_peer_runtime.shutdown().await;

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let base_url = format!("http://{}", listener.local_addr().expect("local addr"));
    let state = Arc::new(MockCommunityNodeState {
        base_url: base_url.clone(),
        seed_peers: Arc::new(Mutex::new(vec![seed_peer.clone()])),
        heartbeat_seed_peers: Arc::new(Mutex::new(None)),
        heartbeat_hits: Arc::new(AtomicUsize::new(0)),
        bootstrap_hits: Arc::new(AtomicUsize::new(0)),
    });
    let app = Router::new()
        .route("/v1/policies", get(mock_current_policies))
        .route("/v1/consents/status", get(mock_bootstrap_consent_status))
        .route("/v1/bootstrap/heartbeat", post(mock_bootstrap_heartbeat))
        .route("/v1/bootstrap/nodes", get(mock_bootstrap_nodes))
        .with_state(state.clone());
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    persist_community_node_token(
        &db_path,
        IdentityStorageMode::FileOnly,
        base_url.as_str(),
        &StoredCommunityNodeToken {
            access_token: "fake-token".to_string(),
            expires_at: Utc::now().timestamp() + 3600,
        },
    )
    .expect("persist community-node token");
    *runtime.community_node_config.lock().await = CommunityNodeConfig {
        trust_node_priority: Vec::new(),
        nodes: vec![CommunityNodeNodeConfig::new(
            base_url.clone(),
            Some(
                CommunityNodeResolvedUrls::new(base_url.clone(), Vec::new(), Vec::new())
                    .expect("resolved urls"),
            ),
        )],
    };
    seed_local_community_node_consents(&runtime, base_url.as_str(), 1);

    runtime.run_community_node_session_maintenance_once().await;
    assert_eq!(state.heartbeat_hits.load(Ordering::SeqCst), 1);
    assert_eq!(state.bootstrap_hits.load(Ordering::SeqCst), 1);
    let generation = runtime.iroh_stack.generation();

    if let Some(entry) = runtime
        .community_node_sessions
        .lock()
        .await
        .get_mut(base_url.as_str())
    {
        entry.ready_refresh_pending = false;
    }

    let refreshed = runtime
        .refresh_community_node_metadata(CommunityNodeTargetRequest {
            base_url: base_url.clone(),
        })
        .await
        .expect("refresh metadata");

    assert_eq!(state.heartbeat_hits.load(Ordering::SeqCst), 2);
    assert_eq!(state.bootstrap_hits.load(Ordering::SeqCst), 2);
    assert_eq!(
        refreshed
            .resolved_urls
            .as_ref()
            .expect("resolved urls")
            .seed_peers,
        vec![seed_peer]
    );
    assert_eq!(runtime.iroh_stack.generation(), generation);

    runtime.shutdown().await;
    server.abort();
}

#[tokio::test]
async fn community_node_ready_transition_refreshes_bootstrap_metadata_before_next_heartbeat_due() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("community-ready-refresh.db");
    let runtime = DesktopRuntime::new_with_config_and_identity(
        &db_path,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime");
    let initial_seed_peer_runtime = DesktopRuntime::new_with_config_and_identity(
        dir.path().join("community-ready-refresh-initial-peer.db"),
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("initial seed peer runtime");
    let initial_seed_peer = initial_seed_peer_runtime
        .local_community_node_seed_peer("ready-refresh-initial")
        .await
        .expect("initial seed peer");
    initial_seed_peer_runtime.shutdown().await;
    let refreshed_seed_peer_runtime = DesktopRuntime::new_with_config_and_identity(
        dir.path().join("community-ready-refresh-refreshed-peer.db"),
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("refreshed seed peer runtime");
    let refreshed_seed_peer = refreshed_seed_peer_runtime
        .local_community_node_seed_peer("ready-refresh-updated")
        .await
        .expect("refreshed seed peer");
    refreshed_seed_peer_runtime.shutdown().await;

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let base_url = format!("http://{}", listener.local_addr().expect("local addr"));
    let state = Arc::new(MockCommunityNodeState {
        base_url: base_url.clone(),
        seed_peers: Arc::new(Mutex::new(vec![initial_seed_peer.clone()])),
        heartbeat_seed_peers: Arc::new(Mutex::new(None)),
        heartbeat_hits: Arc::new(AtomicUsize::new(0)),
        bootstrap_hits: Arc::new(AtomicUsize::new(0)),
    });
    let app = Router::new()
        .route("/v1/policies", get(mock_current_policies))
        .route("/v1/consents/status", get(mock_bootstrap_consent_status))
        .route("/v1/bootstrap/heartbeat", post(mock_bootstrap_heartbeat))
        .route("/v1/bootstrap/nodes", get(mock_bootstrap_nodes))
        .with_state(state.clone());
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    persist_community_node_token(
        &db_path,
        IdentityStorageMode::FileOnly,
        base_url.as_str(),
        &StoredCommunityNodeToken {
            access_token: "fake-token".to_string(),
            expires_at: Utc::now().timestamp() + 3600,
        },
    )
    .expect("persist community-node token");
    *runtime.community_node_config.lock().await = CommunityNodeConfig {
        trust_node_priority: Vec::new(),
        nodes: vec![CommunityNodeNodeConfig::new(
            base_url.clone(),
            Some(
                CommunityNodeResolvedUrls::new(base_url.clone(), Vec::new(), Vec::new())
                    .expect("resolved urls"),
            ),
        )],
    };
    seed_local_community_node_consents(&runtime, base_url.as_str(), 1);

    runtime.run_community_node_session_maintenance_once().await;
    assert_eq!(state.heartbeat_hits.load(Ordering::SeqCst), 1);
    assert_eq!(state.bootstrap_hits.load(Ordering::SeqCst), 1);
    assert!(
        runtime
            .community_node_sessions
            .lock()
            .await
            .get(base_url.as_str())
            .map(|s| s.heartbeat_deadline)
            .expect("heartbeat deadline")
            > Utc::now().timestamp()
    );
    assert_eq!(
        runtime.community_node_config.lock().await.nodes[0]
            .resolved_urls
            .as_ref()
            .expect("resolved urls")
            .seed_peers,
        vec![initial_seed_peer]
    );

    *state.seed_peers.lock().await = vec![refreshed_seed_peer.clone()];

    runtime.run_community_node_session_maintenance_once().await;

    assert_eq!(state.heartbeat_hits.load(Ordering::SeqCst), 1);
    assert_eq!(state.bootstrap_hits.load(Ordering::SeqCst), 2);
    assert_eq!(
        runtime.community_node_config.lock().await.nodes[0]
            .resolved_urls
            .as_ref()
            .expect("resolved urls")
            .seed_peers,
        vec![refreshed_seed_peer]
    );

    runtime.shutdown().await;
    server.abort();
}

#[tokio::test]
async fn community_node_ready_transition_refreshes_bootstrap_metadata_only_once_before_next_heartbeat_due()
 {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("community-ready-refresh-once.db");
    let runtime = DesktopRuntime::new_with_config_and_identity(
        &db_path,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime");
    let seed_peer_runtime = DesktopRuntime::new_with_config_and_identity(
        dir.path().join("community-ready-refresh-once-peer.db"),
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("seed peer runtime");
    let seed_peer = seed_peer_runtime
        .local_community_node_seed_peer("ready-refresh-once")
        .await
        .expect("seed peer");
    seed_peer_runtime.shutdown().await;

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let base_url = format!("http://{}", listener.local_addr().expect("local addr"));
    let state = Arc::new(MockCommunityNodeState {
        base_url: base_url.clone(),
        seed_peers: Arc::new(Mutex::new(vec![seed_peer.clone()])),
        heartbeat_seed_peers: Arc::new(Mutex::new(None)),
        heartbeat_hits: Arc::new(AtomicUsize::new(0)),
        bootstrap_hits: Arc::new(AtomicUsize::new(0)),
    });
    let app = Router::new()
        .route("/v1/policies", get(mock_current_policies))
        .route("/v1/consents/status", get(mock_bootstrap_consent_status))
        .route("/v1/bootstrap/heartbeat", post(mock_bootstrap_heartbeat))
        .route("/v1/bootstrap/nodes", get(mock_bootstrap_nodes))
        .with_state(state.clone());
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    persist_community_node_token(
        &db_path,
        IdentityStorageMode::FileOnly,
        base_url.as_str(),
        &StoredCommunityNodeToken {
            access_token: "fake-token".to_string(),
            expires_at: Utc::now().timestamp() + 3600,
        },
    )
    .expect("persist community-node token");
    *runtime.community_node_config.lock().await = CommunityNodeConfig {
        trust_node_priority: Vec::new(),
        nodes: vec![CommunityNodeNodeConfig::new(
            base_url.clone(),
            Some(
                CommunityNodeResolvedUrls::new(base_url.clone(), Vec::new(), Vec::new())
                    .expect("resolved urls"),
            ),
        )],
    };
    seed_local_community_node_consents(&runtime, base_url.as_str(), 1);

    runtime.run_community_node_session_maintenance_once().await;
    assert_eq!(state.heartbeat_hits.load(Ordering::SeqCst), 1);
    assert_eq!(state.bootstrap_hits.load(Ordering::SeqCst), 1);

    runtime.run_community_node_session_maintenance_once().await;
    assert_eq!(state.heartbeat_hits.load(Ordering::SeqCst), 1);
    assert_eq!(state.bootstrap_hits.load(Ordering::SeqCst), 2);

    runtime.run_community_node_session_maintenance_once().await;
    assert_eq!(state.heartbeat_hits.load(Ordering::SeqCst), 1);
    assert_eq!(state.bootstrap_hits.load(Ordering::SeqCst), 2);

    runtime.shutdown().await;
    server.abort();
}

#[tokio::test]
async fn community_node_status_retries_bootstrap_metadata_when_seed_peers_are_empty() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("community-metadata-retry.db");
    let runtime = DesktopRuntime::new_with_config_and_identity(
        &db_path,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime");

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let base_url = format!("http://{}", listener.local_addr().expect("local addr"));
    let seed_peer = CommunityNodeSeedPeer::new(
        "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
        None,
    )
    .expect("seed peer");
    let state = Arc::new(MockCommunityNodeState {
        base_url: base_url.clone(),
        seed_peers: Arc::new(Mutex::new(Vec::new())),
        heartbeat_seed_peers: Arc::new(Mutex::new(None)),
        heartbeat_hits: Arc::new(AtomicUsize::new(0)),
        bootstrap_hits: Arc::new(AtomicUsize::new(0)),
    });
    let app = Router::new()
        .route("/v1/policies", get(mock_current_policies))
        .route("/v1/consents/status", get(mock_bootstrap_consent_status))
        .route("/v1/bootstrap/heartbeat", post(mock_bootstrap_heartbeat))
        .route("/v1/bootstrap/nodes", get(mock_bootstrap_nodes))
        .with_state(state.clone());
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    persist_community_node_token(
        &db_path,
        IdentityStorageMode::FileOnly,
        base_url.as_str(),
        &StoredCommunityNodeToken {
            access_token: "fake-token".to_string(),
            expires_at: Utc::now().timestamp() + 3600,
        },
    )
    .expect("persist community-node token");
    *runtime.community_node_config.lock().await = CommunityNodeConfig {
        trust_node_priority: Vec::new(),
        nodes: vec![CommunityNodeNodeConfig::new(
            base_url.clone(),
            Some(
                CommunityNodeResolvedUrls::new(base_url.clone(), Vec::new(), Vec::new())
                    .expect("resolved urls"),
            ),
        )],
    };
    seed_local_community_node_consents(&runtime, base_url.as_str(), 1);

    // WP-Q2: metadata refresh はスケジューラ tick が駆動し、getter は読み取り専用。
    runtime.run_community_node_session_maintenance_once().await;
    let initial_statuses = runtime
        .get_community_node_statuses()
        .await
        .expect("initial community node statuses");
    assert_eq!(state.heartbeat_hits.load(Ordering::SeqCst), 1);
    assert_eq!(state.bootstrap_hits.load(Ordering::SeqCst), 1);
    assert_eq!(
        initial_statuses[0]
            .resolved_urls
            .as_ref()
            .expect("resolved urls")
            .seed_peers,
        Vec::<CommunityNodeSeedPeer>::new()
    );
    assert!(
        runtime
            .community_node_sessions
            .lock()
            .await
            .get(base_url.as_str())
            .map(|s| s.metadata_refresh_deadline > 0)
            .unwrap_or(false),
        "empty bootstrap metadata should schedule a retry"
    );

    *state.seed_peers.lock().await = vec![seed_peer.clone()];
    if let Some(entry) = runtime
        .community_node_sessions
        .lock()
        .await
        .get_mut(base_url.as_str())
    {
        entry.metadata_refresh_deadline = Utc::now().timestamp() - 1;
    }

    runtime.run_community_node_session_maintenance_once().await;
    let refreshed_statuses = runtime
        .get_community_node_statuses()
        .await
        .expect("refreshed community node statuses");
    assert_eq!(state.heartbeat_hits.load(Ordering::SeqCst), 1);
    assert_eq!(state.bootstrap_hits.load(Ordering::SeqCst), 2);
    assert_eq!(
        refreshed_statuses[0]
            .resolved_urls
            .as_ref()
            .expect("resolved urls")
            .seed_peers,
        vec![seed_peer]
    );

    runtime.shutdown().await;
    server.abort();
}

#[tokio::test]
async fn refresh_community_node_metadata_refreshes_registration_before_bootstrap_sync() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("community-refresh-heartbeat.db");
    let runtime = DesktopRuntime::new_with_config_and_identity(
        &db_path,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime");

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let base_url = format!("http://{}", listener.local_addr().expect("local addr"));
    let refreshed_seed_peer = CommunityNodeSeedPeer::new(
        "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210",
        Some("127.0.0.1:44003".into()),
    )
    .expect("refreshed seed peer");
    let state = Arc::new(MockCommunityNodeState {
        base_url: base_url.clone(),
        seed_peers: Arc::new(Mutex::new(Vec::new())),
        heartbeat_seed_peers: Arc::new(Mutex::new(Some(vec![refreshed_seed_peer.clone()]))),
        heartbeat_hits: Arc::new(AtomicUsize::new(0)),
        bootstrap_hits: Arc::new(AtomicUsize::new(0)),
    });
    let app = Router::new()
        .route("/v1/policies", get(mock_current_policies))
        .route("/v1/consents/status", get(mock_bootstrap_consent_status))
        .route("/v1/bootstrap/heartbeat", post(mock_bootstrap_heartbeat))
        .route("/v1/bootstrap/nodes", get(mock_bootstrap_nodes))
        .with_state(state.clone());
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    persist_community_node_token(
        &db_path,
        IdentityStorageMode::FileOnly,
        base_url.as_str(),
        &StoredCommunityNodeToken {
            access_token: "fake-token".to_string(),
            expires_at: Utc::now().timestamp() + 3600,
        },
    )
    .expect("persist community-node token");
    *runtime.community_node_config.lock().await = CommunityNodeConfig {
        trust_node_priority: Vec::new(),
        nodes: vec![CommunityNodeNodeConfig::new(
            base_url.clone(),
            Some(
                CommunityNodeResolvedUrls::new(base_url.clone(), Vec::new(), Vec::new())
                    .expect("resolved urls"),
            ),
        )],
    };
    seed_local_community_node_consents(&runtime, base_url.as_str(), 1);

    let status = runtime
        .refresh_community_node_metadata(CommunityNodeTargetRequest {
            base_url: base_url.clone(),
        })
        .await
        .expect("refresh metadata");

    assert_eq!(state.heartbeat_hits.load(Ordering::SeqCst), 1);
    assert!(
        state.bootstrap_hits.load(Ordering::SeqCst) >= 1,
        "metadata refresh should fetch bootstrap nodes"
    );
    assert_eq!(
        status
            .resolved_urls
            .as_ref()
            .expect("resolved urls")
            .seed_peers,
        vec![refreshed_seed_peer.clone()]
    );
    assert_eq!(
        runtime.community_node_config.lock().await.nodes[0]
            .resolved_urls
            .as_ref()
            .expect("resolved urls")
            .seed_peers,
        vec![refreshed_seed_peer]
    );

    runtime.shutdown().await;
    server.abort();
}

#[tokio::test]
async fn refresh_community_node_metadata_requeues_heartbeat_when_runtime_connectivity_changes_local_seed_peer()
 {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let (_relay_map, relay_url, _guard) = iroh::test_utils::run_relay_server()
        .await
        .expect("relay server");
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("community-refresh-requeue-heartbeat.db");
    let runtime = DesktopRuntime::new_with_config_and_identity(
        &db_path,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime");

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let base_url = format!("http://{}", listener.local_addr().expect("local addr"));
    let state = Arc::new(MockHeartbeatEchoCommunityNodeState {
        base_url: base_url.clone(),
        connectivity_urls: vec![relay_url.to_string()],
        seed_peers: Arc::new(Mutex::new(Vec::new())),
        heartbeat_hits: Arc::new(AtomicUsize::new(0)),
        bootstrap_hits: Arc::new(AtomicUsize::new(0)),
    });
    let app = Router::new()
        .route("/v1/policies", get(mock_current_policies))
        .route("/v1/consents/status", get(mock_bootstrap_consent_status))
        .route(
            "/v1/bootstrap/heartbeat",
            post(mock_heartbeat_echo_bootstrap_heartbeat),
        )
        .route(
            "/v1/bootstrap/nodes",
            get(mock_heartbeat_echo_bootstrap_nodes),
        )
        .with_state(state.clone());
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    persist_community_node_token(
        &db_path,
        IdentityStorageMode::FileOnly,
        base_url.as_str(),
        &StoredCommunityNodeToken {
            access_token: "fake-token".to_string(),
            expires_at: Utc::now().timestamp() + 3600,
        },
    )
    .expect("persist community-node token");
    *runtime.community_node_config.lock().await = CommunityNodeConfig {
        trust_node_priority: Vec::new(),
        nodes: vec![CommunityNodeNodeConfig::new(
            base_url.clone(),
            Some(
                CommunityNodeResolvedUrls::new(base_url.clone(), Vec::new(), Vec::new())
                    .expect("resolved urls"),
            ),
        )],
    };
    seed_local_community_node_consents(&runtime, base_url.as_str(), 1);

    let initial_seed_peer = runtime
        .local_community_node_seed_peer("initial")
        .await
        .expect("initial seed peer");
    let _status = runtime
        .refresh_community_node_metadata(CommunityNodeTargetRequest {
            base_url: base_url.clone(),
        })
        .await
        .expect("refresh metadata");
    assert_eq!(state.heartbeat_hits.load(Ordering::SeqCst), 1);
    assert!(
        state.bootstrap_hits.load(Ordering::SeqCst) >= 1,
        "metadata refresh should fetch bootstrap nodes"
    );

    let refreshed_seed_peer = runtime
        .local_community_node_seed_peer("after-refresh")
        .await
        .expect("refreshed seed peer");
    if refreshed_seed_peer == initial_seed_peer {
        runtime.shutdown().await;
        server.abort();
        return;
    }

    runtime.run_community_node_session_maintenance_once().await;

    assert_eq!(
        state.heartbeat_hits.load(Ordering::SeqCst),
        2,
        "runtime should heartbeat again after relay rebuild changes the local seed peer"
    );
    assert_eq!(
        runtime.community_node_config.lock().await.nodes[0]
            .resolved_urls
            .as_ref()
            .expect("resolved urls")
            .seed_peers,
        vec![refreshed_seed_peer]
    );

    runtime.shutdown().await;
    server.abort();
}

#[tokio::test]
async fn manual_refresh_stops_before_protected_requests_on_snapshot_update() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("community-refresh-snapshot-update.db");
    let runtime = DesktopRuntime::new_with_config_and_identity(
        &db_path,
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
        vec![],
        true,
        Arc::new(Mutex::new("saved-token".into())),
    ));
    state.simulate_snapshot_update.store(true, Ordering::SeqCst);
    let app = Router::new()
        .route("/v1/auth/challenge", post(mock_managed_auth_challenge))
        .route("/v1/auth/verify", post(mock_managed_auth_verify))
        .route("/v1/consents/status", get(mock_managed_consent_status))
        .route("/v1/consents", post(mock_managed_accept_consents))
        .route("/v1/policies", get(mock_managed_policies))
        .route(
            "/v1/bootstrap/heartbeat",
            post(mock_managed_bootstrap_heartbeat),
        )
        .route("/v1/bootstrap/nodes", get(mock_managed_bootstrap_nodes))
        .with_state(state.clone());
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    persist_community_node_token(
        &db_path,
        IdentityStorageMode::FileOnly,
        base_url.as_str(),
        &StoredCommunityNodeToken {
            access_token: "saved-token".into(),
            expires_at: Utc::now().timestamp() + 3600,
        },
    )
    .expect("persist token");
    *runtime.community_node_config.lock().await = CommunityNodeConfig {
        trust_node_priority: Vec::new(),
        nodes: vec![CommunityNodeNodeConfig::new(
            base_url.clone(),
            Some(
                CommunityNodeResolvedUrls::new(base_url.clone(), Vec::new(), Vec::new())
                    .expect("resolved urls"),
            ),
        )],
    };
    seed_local_community_node_consents_with_snapshot(
        &runtime,
        base_url.as_str(),
        1,
        Some("snapshot-1"),
    );

    let status = runtime
        .refresh_community_node_metadata(crate::CommunityNodeTargetRequest {
            base_url: base_url.clone(),
        })
        .await
        .expect("refresh returns consent status");

    assert_eq!(state.policies_hits.load(Ordering::SeqCst), 1);
    assert_eq!(state.challenge_hits.load(Ordering::SeqCst), 0);
    assert_eq!(state.verify_hits.load(Ordering::SeqCst), 0);
    assert_eq!(state.consent_status_hits.load(Ordering::SeqCst), 0);
    assert_eq!(state.heartbeat_hits.load(Ordering::SeqCst), 0);
    assert_eq!(state.bootstrap_hits.load(Ordering::SeqCst), 0);
    assert_eq!(status.session_phase, crate::CommunityNodeSessionPhase::Idle);
    assert!(status.consent_update_pending);

    runtime.shutdown().await;
    server.abort();
}
