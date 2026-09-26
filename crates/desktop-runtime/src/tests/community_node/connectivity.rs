use super::super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn community_node_connectivity_assist_relay_backed_seed_peers_ignore_stale_addr_hints() {
    let _resource = lock_test_resource(TestResource::IrohNetwork).await;
    let relay = kukuri_cn_iroh_relay::spawn_server(kukuri_cn_iroh_relay::IrohRelayConfig {
        http_bind_addr: "127.0.0.1:0".parse().expect("relay bind addr"),
        tls: None,
        client_rx_limit: None,
    })
    .await
    .expect("relay server");
    let relay_url = format!("http://{}", relay.http_addr());
    let dir = tempdir().expect("tempdir");
    let db_a = dir.path().join("community-relay-a.db");
    let db_b = dir.path().join("community-relay-b.db");
    let runtime_a = DesktopRuntime::new_with_config_and_identity(
        &db_a,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime a");
    let runtime_b = DesktopRuntime::new_with_config_and_identity(
        &db_b,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime b");

    let endpoint_a = runtime_a
        .get_sync_status()
        .await
        .expect("status a")
        .discovery
        .local_endpoint_id;
    let endpoint_b = runtime_b
        .get_sync_status()
        .await
        .expect("status b")
        .discovery
        .local_endpoint_id;
    let base_url = "https://community.example.com";
    let stale_addr_hint = "127.0.0.1:9".to_string();

    apply_relay_backed_community_node_seed_peers(
        &runtime_a,
        base_url,
        relay_url.as_str(),
        vec![
            CommunityNodeSeedPeer::new(endpoint_b.as_str(), Some(stale_addr_hint.clone()))
                .expect("seed peer b"),
        ],
    )
    .await;
    apply_relay_backed_community_node_seed_peers(
        &runtime_b,
        base_url,
        relay_url.as_str(),
        vec![
            CommunityNodeSeedPeer::new(endpoint_a.as_str(), Some(stale_addr_hint))
                .expect("seed peer a"),
        ],
    )
    .await;

    let topic = "kukuri:topic:community-node-relay-assist";
    let scope = TimelineScope::Public;
    let _ = timeout(
        Duration::from_secs(15),
        runtime_a.list_timeline(ListTimelineRequest {
            topic: topic.to_string(),
            scope: scope.clone(),
            cursor: None,
            limit: Some(20),
        }),
    )
    .await
    .expect("subscribe a timeout")
    .expect("subscribe a");
    let _ = timeout(
        Duration::from_secs(15),
        runtime_b.list_timeline(ListTimelineRequest {
            topic: topic.to_string(),
            scope: scope.clone(),
            cursor: None,
            limit: Some(20),
        }),
    )
    .await
    .expect("subscribe b timeout")
    .expect("subscribe b");

    wait_for_public_pair_delivery_with_refresh(
        &runtime_a,
        &runtime_b,
        topic,
        1,
        runtime_replication_timeout(),
    )
    .await
    .expect("relay-backed stale addr hints pair delivery");

    let status_a = runtime_a
        .get_sync_status()
        .await
        .expect("status a after warmup");
    let status_b = runtime_b
        .get_sync_status()
        .await
        .expect("status b after warmup");
    let (publisher, subscriber) = if !topic_has_direct_peer(&status_a, topic, 1)
        && topic_has_direct_peer(&status_b, topic, 1)
    {
        (&runtime_b, &runtime_a)
    } else {
        (&runtime_a, &runtime_b)
    };
    let _ = replicate_public_post_with_retry(
        publisher,
        subscriber,
        topic,
        "relay-backed stale addr hints",
        "relay-backed stale addr hints should replicate",
    )
    .await;

    timeout(runtime_shutdown_timeout(), runtime_a.shutdown())
        .await
        .expect("runtime a shutdown timeout");
    timeout(runtime_shutdown_timeout(), runtime_b.shutdown())
        .await
        .expect("runtime b shutdown timeout");
    drop(relay);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn community_node_connectivity_assist_backfills_public_timeline_with_relay_only_seed_peers() {
    let _resource = lock_test_resource(TestResource::IrohNetwork).await;
    let relay = kukuri_cn_iroh_relay::spawn_server(kukuri_cn_iroh_relay::IrohRelayConfig {
        http_bind_addr: "127.0.0.1:0".parse().expect("relay bind addr"),
        tls: None,
        client_rx_limit: None,
    })
    .await
    .expect("relay server");
    let relay_url = format!("http://{}", relay.http_addr());
    let dir = tempdir().expect("tempdir");
    let db_a = dir.path().join("community-relay-only-a.db");
    let db_b = dir.path().join("community-relay-only-b.db");
    let runtime_a = DesktopRuntime::new_with_config_and_identity(
        &db_a,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime a");
    let runtime_b = DesktopRuntime::new_with_config_and_identity(
        &db_b,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime b");

    let endpoint_a = runtime_a
        .get_sync_status()
        .await
        .expect("status a")
        .discovery
        .local_endpoint_id;
    let endpoint_b = runtime_b
        .get_sync_status()
        .await
        .expect("status b")
        .discovery
        .local_endpoint_id;
    let base_url = "https://community.example.com";

    *runtime_a.community_node_config.lock().await = CommunityNodeConfig {
        trust_node_priority: Vec::new(),
        nodes: vec![CommunityNodeNodeConfig::new(
            base_url.to_string(),
            Some(
                CommunityNodeResolvedUrls::new(
                    base_url,
                    vec![relay_url.to_string()],
                    vec![
                        CommunityNodeSeedPeer::new(endpoint_b.as_str(), None).expect("seed peer b"),
                    ],
                )
                .expect("resolved urls a"),
            ),
        )],
    };
    seed_local_community_node_consents(&runtime_a, base_url, 1);
    mark_community_node_session_ready_for_test(&runtime_a, base_url).await;
    *runtime_b.community_node_config.lock().await = CommunityNodeConfig {
        trust_node_priority: Vec::new(),
        nodes: vec![CommunityNodeNodeConfig::new(
            base_url.to_string(),
            Some(
                CommunityNodeResolvedUrls::new(
                    base_url,
                    vec![relay_url.to_string()],
                    vec![
                        CommunityNodeSeedPeer::new(endpoint_a.as_str(), None).expect("seed peer a"),
                    ],
                )
                .expect("resolved urls b"),
            ),
        )],
    };
    seed_local_community_node_consents(&runtime_b, base_url, 1);
    mark_community_node_session_ready_for_test(&runtime_b, base_url).await;
    timeout(
        Duration::from_secs(30),
        runtime_a.apply_runtime_connectivity_assist(),
    )
    .await
    .expect("apply assist a timeout")
    .expect("apply assist a");
    timeout(
        Duration::from_secs(15),
        runtime_a.apply_effective_seed_peers(),
    )
    .await
    .expect("apply seed peers a timeout")
    .expect("apply seed peers a");
    timeout(
        Duration::from_secs(30),
        runtime_b.apply_runtime_connectivity_assist(),
    )
    .await
    .expect("apply assist b timeout")
    .expect("apply assist b");
    timeout(
        Duration::from_secs(15),
        runtime_b.apply_effective_seed_peers(),
    )
    .await
    .expect("apply seed peers b timeout")
    .expect("apply seed peers b");

    let topic = "kukuri:topic:community-node-relay-only";
    let scope = TimelineScope::Public;
    let _ = timeout(
        Duration::from_secs(15),
        runtime_a.list_timeline(ListTimelineRequest {
            topic: topic.to_string(),
            scope: scope.clone(),
            cursor: None,
            limit: Some(20),
        }),
    )
    .await
    .expect("subscribe a timeout")
    .expect("subscribe a");
    let _ = timeout(
        Duration::from_secs(15),
        runtime_b.list_timeline(ListTimelineRequest {
            topic: topic.to_string(),
            scope: scope.clone(),
            cursor: None,
            limit: Some(20),
        }),
    )
    .await
    .expect("subscribe b timeout")
    .expect("subscribe b");

    wait_for_public_pair_delivery_with_refresh(
        &runtime_a,
        &runtime_b,
        topic,
        1,
        runtime_replication_timeout(),
    )
    .await
    .expect("relay-only pair delivery");

    let _ = replicate_public_post_with_retry(
        &runtime_a,
        &runtime_b,
        topic,
        "relay-only bootstrap",
        "relay-only bootstrap should replicate",
    )
    .await;

    timeout(runtime_shutdown_timeout(), runtime_a.shutdown())
        .await
        .expect("runtime a shutdown timeout");
    timeout(runtime_shutdown_timeout(), runtime_b.shutdown())
        .await
        .expect("runtime b shutdown timeout");
    drop(relay);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn external_relay_endpoint_only_seed_peers_backfill_desktop_public_timeline() {
    let Some(relay_url) = std::env::var("KUKURI_TEST_EXTERNAL_IROH_RELAY_URL")
        .ok()
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())
    else {
        return;
    };

    let _resource = lock_test_resource(TestResource::IrohNetwork).await;
    let dir = tempdir().expect("tempdir");
    let db_a = dir.path().join("external-relay-only-a.db");
    let db_b = dir.path().join("external-relay-only-b.db");
    let runtime_a = DesktopRuntime::new_with_config_and_identity(
        &db_a,
        TransportNetworkConfig::default(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime a");
    let runtime_b = DesktopRuntime::new_with_config_and_identity(
        &db_b,
        TransportNetworkConfig::default(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime b");

    let endpoint_a = runtime_a
        .get_sync_status()
        .await
        .expect("status a")
        .discovery
        .local_endpoint_id;
    let endpoint_b = runtime_b
        .get_sync_status()
        .await
        .expect("status b")
        .discovery
        .local_endpoint_id;
    let base_url = "https://community.example.com";

    *runtime_a.community_node_config.lock().await = CommunityNodeConfig {
        trust_node_priority: Vec::new(),
        nodes: vec![CommunityNodeNodeConfig::new(
            base_url.to_string(),
            Some(
                CommunityNodeResolvedUrls::new(
                    base_url,
                    vec![relay_url.clone()],
                    vec![
                        CommunityNodeSeedPeer::new(endpoint_b.as_str(), None).expect("seed peer b"),
                    ],
                )
                .expect("resolved urls a"),
            ),
        )],
    };
    seed_local_community_node_consents(&runtime_a, base_url, 1);
    *runtime_b.community_node_config.lock().await = CommunityNodeConfig {
        trust_node_priority: Vec::new(),
        nodes: vec![CommunityNodeNodeConfig::new(
            base_url.to_string(),
            Some(
                CommunityNodeResolvedUrls::new(
                    base_url,
                    vec![relay_url],
                    vec![
                        CommunityNodeSeedPeer::new(endpoint_a.as_str(), None).expect("seed peer a"),
                    ],
                )
                .expect("resolved urls b"),
            ),
        )],
    };
    seed_local_community_node_consents(&runtime_b, base_url, 1);

    timeout(
        Duration::from_secs(30),
        runtime_a.apply_runtime_connectivity_assist(),
    )
    .await
    .expect("apply assist a timeout")
    .expect("apply assist a");
    timeout(
        Duration::from_secs(15),
        runtime_a.apply_effective_seed_peers(),
    )
    .await
    .expect("apply seed peers a timeout")
    .expect("apply seed peers a");
    timeout(
        Duration::from_secs(30),
        runtime_b.apply_runtime_connectivity_assist(),
    )
    .await
    .expect("apply assist b timeout")
    .expect("apply assist b");
    timeout(
        Duration::from_secs(15),
        runtime_b.apply_effective_seed_peers(),
    )
    .await
    .expect("apply seed peers b timeout")
    .expect("apply seed peers b");

    let topic = "kukuri:topic:external-relay-desktop";
    let scope = TimelineScope::Public;
    open_topic_column(&runtime_a, topic, scope.clone())
        .await
        .expect("subscribe a");
    let _ = runtime_b
        .list_timeline(ListTimelineRequest {
            topic: topic.to_string(),
            scope,
            cursor: None,
            limit: Some(20),
        })
        .await
        .expect("subscribe b");

    wait_for_public_pair_delivery_with_refresh(
        &runtime_a,
        &runtime_b,
        topic,
        1,
        runtime_replication_timeout(),
    )
    .await
    .expect("external relay endpoint-only pair delivery");

    let _ = replicate_public_post_with_retry(
        &runtime_a,
        &runtime_b,
        topic,
        "external relay desktop",
        "external relay desktop should replicate",
    )
    .await;

    timeout(runtime_shutdown_timeout(), runtime_a.shutdown())
        .await
        .expect("runtime a shutdown timeout");
    timeout(runtime_shutdown_timeout(), runtime_b.shutdown())
        .await
        .expect("runtime b shutdown timeout");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn community_node_connectivity_assist_backfills_three_client_public_timeline_with_stale_addr_hints()
 {
    let _resource = lock_test_resource(TestResource::IrohNetwork).await;
    let relay = kukuri_cn_iroh_relay::spawn_server(kukuri_cn_iroh_relay::IrohRelayConfig {
        http_bind_addr: "127.0.0.1:0".parse().expect("relay bind addr"),
        tls: None,
        client_rx_limit: None,
    })
    .await
    .expect("relay server");
    let relay_url = format!("http://{}", relay.http_addr());
    let dir = tempdir().expect("tempdir");
    let db_a = dir.path().join("community-relay-three-a.db");
    let db_b = dir.path().join("community-relay-three-b.db");
    let db_c = dir.path().join("community-relay-three-c.db");
    let runtime_a = DesktopRuntime::new_with_config_and_identity(
        &db_a,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime a");
    let runtime_b = DesktopRuntime::new_with_config_and_identity(
        &db_b,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime b");
    let runtime_c = DesktopRuntime::new_with_config_and_identity(
        &db_c,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime c");

    let endpoint_a = runtime_a
        .get_sync_status()
        .await
        .expect("status a")
        .discovery
        .local_endpoint_id;
    let endpoint_b = runtime_b
        .get_sync_status()
        .await
        .expect("status b")
        .discovery
        .local_endpoint_id;
    let endpoint_c = runtime_c
        .get_sync_status()
        .await
        .expect("status c")
        .discovery
        .local_endpoint_id;
    let base_url = "https://community.example.com";
    let stale_addr_hint = "127.0.0.1:9".to_string();

    apply_relay_backed_community_node_seed_peers(
        &runtime_a,
        base_url,
        relay_url.as_str(),
        vec![
            CommunityNodeSeedPeer::new(endpoint_b.as_str(), Some(stale_addr_hint.clone()))
                .expect("seed peer b"),
            CommunityNodeSeedPeer::new(endpoint_c.as_str(), Some(stale_addr_hint.clone()))
                .expect("seed peer c"),
        ],
    )
    .await;
    apply_relay_backed_community_node_seed_peers(
        &runtime_b,
        base_url,
        relay_url.as_str(),
        vec![
            CommunityNodeSeedPeer::new(endpoint_a.as_str(), Some(stale_addr_hint.clone()))
                .expect("seed peer a"),
            CommunityNodeSeedPeer::new(endpoint_c.as_str(), Some(stale_addr_hint.clone()))
                .expect("seed peer c"),
        ],
    )
    .await;
    apply_relay_backed_community_node_seed_peers(
        &runtime_c,
        base_url,
        relay_url.as_str(),
        vec![
            CommunityNodeSeedPeer::new(endpoint_a.as_str(), Some(stale_addr_hint.clone()))
                .expect("seed peer a"),
            CommunityNodeSeedPeer::new(endpoint_b.as_str(), Some(stale_addr_hint))
                .expect("seed peer b"),
        ],
    )
    .await;

    let topic = "kukuri:topic:community-node-relay-three-clients";
    let scope = TimelineScope::Public;
    for runtime in [&runtime_a, &runtime_b, &runtime_c] {
        let _ = timeout(
            Duration::from_secs(15),
            runtime.list_timeline(ListTimelineRequest {
                topic: topic.to_string(),
                scope: scope.clone(),
                cursor: None,
                limit: Some(20),
            }),
        )
        .await
        .expect("subscribe timeout")
        .expect("subscribe");
    }

    wait_for_public_runtime_delivery_with_refresh(
        &runtime_a,
        topic,
        1,
        runtime_replication_timeout(),
    )
    .await
    .expect("runtime a three-client delivery");
    wait_for_public_runtime_delivery_with_refresh(
        &runtime_b,
        topic,
        1,
        runtime_replication_timeout(),
    )
    .await
    .expect("runtime b three-client delivery");
    wait_for_public_runtime_delivery_with_refresh(
        &runtime_c,
        topic,
        1,
        runtime_replication_timeout(),
    )
    .await
    .expect("runtime c three-client delivery");

    let object_id_a = runtime_a
        .create_post(CreatePostRequest {
            topic: topic.into(),
            content: "three-client relay post a".into(),
            reply_to: None,
            channel_ref: ChannelRef::Public,
            attachments: vec![],
            content_labels: Vec::new(),
        })
        .await
        .expect("create post a");
    wait_for_topic_doc_index_entry_result(
        &runtime_a,
        topic,
        object_id_a.as_str(),
        runtime_replication_timeout(),
    )
    .await
    .expect("runtime a should persist post a into docs index");
    wait_for_timeline_post_result(
        &runtime_b,
        topic,
        &scope,
        object_id_a.as_str(),
        runtime_replication_timeout(),
    )
    .await
    .expect("runtime b should receive runtime a post");
    wait_for_timeline_post_result(
        &runtime_c,
        topic,
        &scope,
        object_id_a.as_str(),
        runtime_replication_timeout(),
    )
    .await
    .expect("runtime c should receive runtime a post");

    let object_id_c = runtime_c
        .create_post(CreatePostRequest {
            topic: topic.into(),
            content: "three-client relay post c".into(),
            reply_to: None,
            channel_ref: ChannelRef::Public,
            attachments: vec![],
            content_labels: Vec::new(),
        })
        .await
        .expect("create post c");
    wait_for_topic_doc_index_entry_result(
        &runtime_c,
        topic,
        object_id_c.as_str(),
        runtime_replication_timeout(),
    )
    .await
    .expect("runtime c should persist post c into docs index");
    wait_for_timeline_post_result(
        &runtime_a,
        topic,
        &scope,
        object_id_c.as_str(),
        runtime_replication_timeout(),
    )
    .await
    .expect("runtime a should receive runtime c post");
    wait_for_timeline_post_result(
        &runtime_b,
        topic,
        &scope,
        object_id_c.as_str(),
        runtime_replication_timeout(),
    )
    .await
    .expect("runtime b should receive runtime c post");

    timeout(runtime_shutdown_timeout(), runtime_a.shutdown())
        .await
        .expect("runtime a shutdown timeout");
    timeout(runtime_shutdown_timeout(), runtime_b.shutdown())
        .await
        .expect("runtime b shutdown timeout");
    timeout(runtime_shutdown_timeout(), runtime_c.shutdown())
        .await
        .expect("runtime c shutdown timeout");
    drop(relay);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn runtime_starts_with_unreachable_community_node_and_recovers_via_manual_peer() {
    let _resource = lock_test_resource(TestResource::IrohNetwork).await;
    let dir = tempdir().expect("tempdir");
    let db_a = dir.path().join("community-unreachable-a.db");
    let db_b = dir.path().join("community-unreachable-b.db");
    let community_base_url = "http://127.0.0.1:1";
    let relay_url = "https://127.0.0.1:1";
    save_community_node_config(
        &db_a,
        &CommunityNodeConfig {
            trust_node_priority: Vec::new(),
            nodes: vec![CommunityNodeNodeConfig::new(
                community_base_url.to_string(),
                Some(
                    CommunityNodeResolvedUrls::new(
                        community_base_url,
                        vec![relay_url.to_string()],
                        Vec::new(),
                    )
                    .expect("resolved urls"),
                ),
            )],
        },
    )
    .expect("save community-node config");

    let runtime_a = timeout(
        Duration::from_secs(40),
        DesktopRuntime::new_with_config_and_identity(
            &db_a,
            TransportNetworkConfig::loopback(),
            IdentityStorageMode::FileOnly,
        ),
    )
    .await
    .expect("runtime a init timeout")
    .expect("runtime a");
    let runtime_b = DesktopRuntime::new_with_config_and_identity(
        &db_b,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime b");

    let status_before = runtime_a
        .get_sync_status()
        .await
        .expect("sync status before recovery");
    assert_eq!(
        status_before.discovery.connect_mode,
        ConnectMode::DirectOnly
    );
    assert!(!status_before.connected);
    assert!(matches!(
        status_before.delivery_state,
        kukuri_app_api::DeliveryState::Offline | kukuri_app_api::DeliveryState::DurableRecovering
    ));

    // WP-Q2: 到達不能ノードへのregistration refresh試行はscheduler tickが駆動し、getterは読み取り専用。
    // #857: local consentだけでは保存済みrelayを有効化しない。到達不能Nodeはpreflightを完了できないためDirectOnlyを維持する。
    seed_local_community_node_consents(&runtime_a, community_base_url, 1);
    runtime_a
        .apply_runtime_connectivity_assist()
        .await
        .expect("apply consented connectivity");
    runtime_a
        .apply_effective_seed_peers()
        .await
        .expect("apply consented seeds");
    assert_eq!(
        runtime_a
            .get_sync_status()
            .await
            .expect("sync status after consent")
            .discovery
            .connect_mode,
        ConnectMode::DirectOnly
    );
    runtime_a
        .run_community_node_session_maintenance_once()
        .await;
    let community_statuses = runtime_a
        .get_community_node_statuses()
        .await
        .expect("community node statuses");
    assert_eq!(community_statuses.len(), 1);
    assert!(
        community_statuses[0].last_error.is_some(),
        "expected unreachable community node error, got {community_statuses:?}"
    );

    let ticket_a = runtime_a
        .local_peer_ticket()
        .await
        .expect("ticket a")
        .expect("ticket a value");
    let ticket_b = runtime_b
        .local_peer_ticket()
        .await
        .expect("ticket b")
        .expect("ticket b value");
    runtime_a
        .import_peer_ticket(ImportPeerTicketRequest {
            ticket: ticket_b.clone(),
        })
        .await
        .expect("import b into a");
    runtime_b
        .import_peer_ticket(ImportPeerTicketRequest {
            ticket: ticket_a.clone(),
        })
        .await
        .expect("import a into b");

    let topic = "kukuri:topic:community-node-unreachable-direct";
    let scope = TimelineScope::Public;
    open_topic_column(&runtime_a, topic, scope.clone())
        .await
        .expect("subscribe a");
    open_topic_column(&runtime_b, topic, scope.clone())
        .await
        .expect("subscribe b");
    wait_for_public_runtime_delivery_with_refresh(&runtime_a, topic, 1, Duration::from_secs(30))
        .await
        .expect("runtime a manual peer recovery");
    wait_for_public_runtime_delivery_with_refresh(&runtime_b, topic, 1, Duration::from_secs(30))
        .await
        .expect("runtime b manual peer recovery");

    let object_id = runtime_b
        .create_post(CreatePostRequest {
            topic: topic.into(),
            content: "direct recovery".into(),
            reply_to: None,
            channel_ref: ChannelRef::Public,
            attachments: vec![],
            content_labels: Vec::new(),
        })
        .await
        .expect("create recovery post");
    timeout(Duration::from_secs(30), async {
        loop {
            let timeline = runtime_a
                .list_timeline(ListTimelineRequest {
                    topic: topic.into(),
                    scope: scope.clone(),
                    cursor: None,
                    limit: Some(20),
                })
                .await
                .expect("timeline a");
            if timeline
                .items
                .iter()
                .any(|post| post.object_id == object_id)
            {
                return;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("runtime a manual peer propagation timeout");

    let status_after = runtime_a
        .get_sync_status()
        .await
        .expect("sync status after recovery");
    assert!(status_after.connected || topic_has_durable_delivery(&status_after, topic));
    assert!(
        topic_has_direct_peer(&status_after, topic, 1)
            || topic_has_durable_delivery(&status_after, topic)
    );

    runtime_a.shutdown().await;
    runtime_b.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn community_node_connectivity_assist_relay_backed_seed_peers_ignore_stale_addr_hints_with_shared_identity()
 {
    let _resource = lock_test_resource(TestResource::IrohNetwork).await;
    let relay = kukuri_cn_iroh_relay::spawn_server(kukuri_cn_iroh_relay::IrohRelayConfig {
        http_bind_addr: "127.0.0.1:0".parse().expect("relay bind addr"),
        tls: None,
        client_rx_limit: None,
    })
    .await
    .expect("relay server");
    let relay_url = format!("http://{}", relay.http_addr());
    let dir = tempdir().expect("tempdir");
    let db_a = dir.path().join("community-relay-shared-a.db");
    let db_b = dir.path().join("community-relay-shared-b.db");
    let shared_keys = KukuriKeys::generate();
    let shared_secret = shared_keys.export_secret_hex();
    fs::write(
        db_a.with_extension("identity-key"),
        shared_secret.as_bytes(),
    )
    .expect("persist shared identity key a");
    fs::write(db_a.with_extension("identity-store"), b"file")
        .expect("persist shared identity backend a");
    fs::write(
        db_b.with_extension("identity-key"),
        shared_secret.as_bytes(),
    )
    .expect("persist shared identity key b");
    fs::write(db_b.with_extension("identity-store"), b"file")
        .expect("persist shared identity backend b");

    let runtime_a = DesktopRuntime::new_with_config_and_identity(
        &db_a,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime a");
    let runtime_b = DesktopRuntime::new_with_config_and_identity(
        &db_b,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime b");

    let status_a = runtime_a.get_sync_status().await.expect("status a");
    let status_b = runtime_b.get_sync_status().await.expect("status b");
    assert_eq!(status_a.local_author_pubkey, status_b.local_author_pubkey);

    let endpoint_a = status_a.discovery.local_endpoint_id;
    let endpoint_b = status_b.discovery.local_endpoint_id;
    let base_url = "https://community.example.com";
    let stale_addr_hint = "127.0.0.1:9".to_string();

    apply_relay_backed_community_node_seed_peers(
        &runtime_a,
        base_url,
        relay_url.as_str(),
        vec![
            CommunityNodeSeedPeer::new(endpoint_b.as_str(), Some(stale_addr_hint.clone()))
                .expect("seed peer b"),
        ],
    )
    .await;
    apply_relay_backed_community_node_seed_peers(
        &runtime_b,
        base_url,
        relay_url.as_str(),
        vec![
            CommunityNodeSeedPeer::new(endpoint_a.as_str(), Some(stale_addr_hint))
                .expect("seed peer a"),
        ],
    )
    .await;

    let topic = "kukuri:topic:community-node-relay-assist-shared";
    let scope = TimelineScope::Public;
    let _ = timeout(
        Duration::from_secs(15),
        runtime_a.list_timeline(ListTimelineRequest {
            topic: topic.to_string(),
            scope: scope.clone(),
            cursor: None,
            limit: Some(20),
        }),
    )
    .await
    .expect("subscribe a timeout")
    .expect("subscribe a");
    let _ = timeout(
        Duration::from_secs(15),
        runtime_b.list_timeline(ListTimelineRequest {
            topic: topic.to_string(),
            scope: scope.clone(),
            cursor: None,
            limit: Some(20),
        }),
    )
    .await
    .expect("subscribe b timeout")
    .expect("subscribe b");

    wait_for_public_pair_delivery_with_refresh(
        &runtime_a,
        &runtime_b,
        topic,
        1,
        runtime_replication_timeout(),
    )
    .await
    .expect("shared-identity relay-backed stale addr hints pair delivery");

    let _ = replicate_public_post_with_retry(
        &runtime_a,
        &runtime_b,
        topic,
        "shared identity relay-backed stale addr hints",
        "shared identity relay-backed stale addr hints should replicate",
    )
    .await;

    timeout(runtime_shutdown_timeout(), runtime_a.shutdown())
        .await
        .expect("runtime a shutdown timeout");
    timeout(runtime_shutdown_timeout(), runtime_b.shutdown())
        .await
        .expect("runtime b shutdown timeout");
    drop(relay);
}
