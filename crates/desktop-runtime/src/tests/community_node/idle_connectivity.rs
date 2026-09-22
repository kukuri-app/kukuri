use super::super::*;
use crate::community_node::load_community_node_token;

#[tokio::test]
async fn idle_actor_repair_does_not_replace_an_unreadable_canonical_store() {
    let dir = tempdir().expect("tempdir");
    let runtime = DesktopRuntime::new_with_config_and_identity(
        dir.path().join("unreadable.db"),
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime");
    let node = runtime
        .iroh_stack
        .current
        .lock()
        .await
        .as_ref()
        .expect("stack")
        .node
        .clone();
    node.clone()
        .shutdown()
        .await
        .expect("close before fault injection");
    let root = &runtime.iroh_stack.root;
    let docs = root.join("docs.redb");
    let original = std::fs::read(&docs).expect("original docs");
    let damaged = b"unreadable canonical store";
    std::fs::write(&docs, damaged).expect("inject unreadable store");
    let result = runtime.apply_runtime_connectivity_assist().await;
    runtime.shutdown().await;
    let retained = std::fs::read(&docs).expect("retained store");
    // Restore only the test fixture so cleanup/retry can use the same profile.
    std::fs::write(&docs, &original).expect("restore fixture");
    assert!(
        result.is_err(),
        "runtime repair must report store failure, not silently initialize docs"
    );
    assert_eq!(retained, damaged);
    assert!(!std::fs::read_dir(root).expect("root").any(|entry| {
        entry
            .expect("entry")
            .file_name()
            .to_string_lossy()
            .starts_with("iroh-docs-recovery-")
    }));
}

#[tokio::test]
async fn idle_repair_of_closed_actor_restores_private_capability_with_unchanged_seeds() {
    use kukuri_docs_sync::{DocFetchPolicy, DocOp, DocQuery, DocsSync};
    let dir = tempdir().expect("tempdir");
    let runtime = DesktopRuntime::new_with_config_and_identity(
        dir.path().join("actor.db"),
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime");
    let channel = runtime
        .create_private_channel(CreatePrivateChannelRequest {
            topic: "kukuri:topic:idle-private".into(),
            label: "private".into(),
            audience_kind: ChannelAudienceKind::InviteOnly,
        })
        .await
        .expect("private channel");
    let replica = kukuri_docs_sync::private_channel_epoch_replica_id(
        &channel.channel_id,
        &channel.current_epoch_id,
    );
    runtime
        .iroh_stack
        .docs_sync
        .apply_doc_op(
            &replica,
            DocOp::SetBytes {
                key: "test/retained".into(),
                value: b"private retained".to_vec(),
            },
        )
        .await
        .expect("private write");
    let before = runtime
        .iroh_stack
        .current
        .lock()
        .await
        .as_ref()
        .expect("stack")
        .node
        .clone();
    let endpoint = before.endpoint().id();
    before
        .clone()
        .shutdown()
        .await
        .expect("inject closed actor");
    runtime
        .apply_runtime_connectivity_assist()
        .await
        .expect("repair without input changes");
    runtime
        .apply_effective_seed_peers()
        .await
        .expect("restore subscriptions despite unchanged seeds");
    let after = runtime
        .iroh_stack
        .current
        .lock()
        .await
        .as_ref()
        .expect("new stack")
        .node
        .clone();
    assert!(!Arc::ptr_eq(&before, &after));
    assert_eq!(endpoint, after.endpoint().id());
    let rows = runtime
        .iroh_stack
        .docs_sync
        .query_replica_with_policy(
            &replica,
            DocQuery::Exact("test/retained".into()),
            DocFetchPolicy::LocalOnly,
        )
        .await
        .expect("private capability restored");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].value, b"private retained");
    runtime.shutdown().await;
}

#[tokio::test]
async fn idle_peer_repair_preserves_healthy_docs_actor() {
    let dir = tempdir().expect("tempdir");
    let runtime = DesktopRuntime::new_with_config_and_identity(
        dir.path().join("idle.db"),
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime");
    let before = runtime
        .iroh_stack
        .current
        .lock()
        .await
        .as_ref()
        .expect("current stack")
        .node
        .clone();
    runtime
        .repair_community_node_connectivity()
        .await
        .expect("repair");
    let after = runtime
        .iroh_stack
        .current
        .lock()
        .await
        .as_ref()
        .expect("current stack")
        .node
        .clone();
    let same = Arc::ptr_eq(&before, &after);
    runtime.shutdown().await;
    assert!(
        same,
        "an offline peer does not justify shutting down the healthy local docs actor"
    );
}

#[tokio::test]
async fn cancelled_stack_rebuild_marks_the_current_stack_unavailable_before_shutdown() {
    let dir = tempdir().expect("tempdir");
    let runtime = DesktopRuntime::new_with_config_and_identity(
        dir.path().join("cancel-rebuild.db"),
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime");
    let config = runtime.discovery_config.lock().await.clone();
    let (shutdown_reached, _resume_shutdown) = runtime
        .iroh_stack
        .pause_next_rebuild_before_shutdown()
        .await;
    {
        let rebuild = runtime.iroh_stack.rebuild(&config, &[], Default::default());
        tokio::pin!(rebuild);
        tokio::select! {
            result = &mut rebuild => panic!("rebuild reached the test gate before completing: {result:?}"),
            reached = shutdown_reached => reached.expect("rebuild shutdown gate"),
        }
    }

    let available = timeout(
        Duration::from_millis(250),
        runtime.iroh_stack.local_docs_available(),
    )
    .await
    .expect("stopped-stack probe must return immediately")
    .expect("stopped-stack availability");
    assert!(
        !available,
        "cancelling rebuild at the shutdown boundary must leave the unavailable marker set"
    );
    runtime.shutdown().await;
}

#[tokio::test]
async fn failed_stack_rebuild_can_retry_without_losing_local_docs() {
    use kukuri_docs_sync::{DocOp, DocQuery, DocsSync};
    let dir = tempdir().expect("tempdir");
    let runtime = DesktopRuntime::new_with_config_and_identity(
        dir.path().join("retry.db"),
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime");
    let replica = kukuri_docs_sync::topic_replica_id("kukuri:topic:idle-rebuild");
    runtime
        .iroh_stack
        .docs_sync
        .apply_doc_op(
            &replica,
            DocOp::SetBytes {
                key: "test/retained".into(),
                value: b"retained".to_vec(),
            },
        )
        .await
        .expect("local write");
    let config = runtime.discovery_config.lock().await.clone();
    assert!(
        runtime
            .iroh_stack
            .rebuild(
                &config,
                &[],
                kukuri_transport::TransportRelayConfig {
                    iroh_relay_urls: vec!["not a relay URL".into()],
                }
            )
            .await
            .is_err()
    );
    let available_after_failure = timeout(
        Duration::from_millis(250),
        runtime.iroh_stack.local_docs_available(),
    )
    .await
    .expect("failed-rebuild probe must return immediately")
    .expect("failed-rebuild availability");
    assert!(
        !available_after_failure,
        "a failed rebuild must leave the stopped stack marked unavailable"
    );
    let retried = runtime
        .iroh_stack
        .apply_runtime_connectivity(&config, &[], Default::default())
        .await;
    assert!(
        retried.is_ok(),
        "failed rebuild must remain recoverable: {retried:?}"
    );
    assert!(
        runtime
            .iroh_stack
            .local_docs_available()
            .await
            .expect("successful-rebuild availability"),
        "a successful rebuild must clear the marker and probe the replacement docs actor"
    );
    let rows = runtime
        .iroh_stack
        .docs_sync
        .query_replica_with_policy(
            &replica,
            DocQuery::Exact("test/retained".into()),
            kukuri_docs_sync::DocFetchPolicy::LocalOnly,
        )
        .await
        .expect("retained docs");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].value, b"retained");
    runtime.shutdown().await;
}

#[tokio::test]
async fn shutdown_after_a_failed_rebuild_reports_the_missing_active_stack() {
    let dir = tempdir().expect("tempdir");
    let runtime = DesktopRuntime::new_with_config_and_identity(
        dir.path().join("failed-rebuild-shutdown.db"),
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime");
    let config = runtime.discovery_config.lock().await.clone();
    runtime
        .iroh_stack
        .rebuild(
            &config,
            &[],
            kukuri_transport::TransportRelayConfig {
                iroh_relay_urls: vec!["not a relay URL".into()],
            },
        )
        .await
        .expect_err("invalid relay URL must fail rebuild");
    runtime
        .iroh_stack
        .shutdown_checked()
        .await
        .expect("shutdown stopped stack");

    let error = runtime
        .iroh_stack
        .local_docs_available()
        .await
        .expect_err("a shut down runtime has no active stack");
    assert_eq!(error.to_string(), "missing active iroh stack");
    runtime.shutdown().await;
}

#[tokio::test]
async fn idle_maintenance_merges_node_metadata_and_retains_consent_boundaries() {
    let dir = tempdir().expect("tempdir");
    let runtime = DesktopRuntime::new_with_config_and_identity(
        dir.path().join("multi.db"),
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime");
    let mut servers = Vec::new();
    let mut states = Vec::new();
    let mut nodes = Vec::new();
    for id in ["1", "2"] {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
        let base_url = format!("http://{}", listener.local_addr().expect("address"));
        let state = Arc::new(MockCommunityNodeState {
            base_url: base_url.clone(),
            seed_peers: Arc::new(Mutex::new(vec![
                CommunityNodeSeedPeer::new(id.repeat(64), None).expect("seed"),
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
        servers.push(tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        }));
        persist_community_node_token(
            &runtime.db_path,
            IdentityStorageMode::FileOnly,
            &base_url,
            &StoredCommunityNodeToken {
                access_token: "fake-token".into(),
                expires_at: Utc::now().timestamp() + 3600,
            },
        )
        .expect("token");
        seed_local_community_node_consents(&runtime, &base_url, 1);
        nodes.push(CommunityNodeNodeConfig::new(base_url, None));
        states.push(state);
    }
    // Configured but never consented: count every forbidden HTTP request.
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
    let unconsented = format!("http://{}", listener.local_addr().expect("address"));
    let forbidden = Arc::new(AtomicUsize::new(0));
    let hits = forbidden.clone();
    let app = Router::new().fallback(move || {
        let hits = hits.clone();
        async move {
            hits.fetch_add(1, Ordering::SeqCst);
            StatusCode::FORBIDDEN
        }
    });
    servers.push(tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    }));
    nodes.push(CommunityNodeNodeConfig::new(unconsented.clone(), None));
    *runtime.community_node_config.lock().await = CommunityNodeConfig {
        nodes,
        trust_node_priority: Vec::new(),
    };
    runtime.run_community_node_session_maintenance_once().await;
    let config = runtime.community_node_config.lock().await.clone();
    for state in &states {
        assert_eq!(state.heartbeat_hits.load(Ordering::SeqCst), 1);
        let node = config
            .nodes
            .iter()
            .find(|node| node.base_url == state.base_url)
            .expect("node");
        assert_eq!(
            node.resolved_urls.as_ref().expect("metadata").seed_peers,
            *state.seed_peers.lock().await
        );
    }
    let other = config
        .nodes
        .iter()
        .find(|node| node.base_url == unconsented)
        .expect("unconsented node");
    assert!(other.resolved_urls.is_none());
    assert!(
        load_community_node_token(&runtime.db_path, runtime.identity_mode, &unconsented)
            .expect("read token")
            .is_none()
    );
    assert_eq!(forbidden.load(Ordering::SeqCst), 0);
    runtime.shutdown().await;
    for server in servers {
        server.abort();
    }
}
