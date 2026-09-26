use super::*;

#[tokio::test]
async fn tracking_multiple_topics_updates_sync_status() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(FakeTransport::new("app", FakeNetwork::default()));
    let app = AppService::new(store, transport);

    for topic in ["kukuri:topic:one", "kukuri:topic:two"] {
        display_topic(&app, topic)
            .await
            .expect("open the topic column");
    }
    let status = app.get_sync_status().await.expect("sync status");

    assert!(
        status
            .subscribed_topics
            .iter()
            .any(|topic| topic == "kukuri:topic:one")
    );
    assert!(
        status
            .subscribed_topics
            .iter()
            .any(|topic| topic == "kukuri:topic:two")
    );
    assert!(
        status
            .topic_diagnostics
            .iter()
            .any(|topic| topic.topic == "kukuri:topic:one")
    );
    assert!(
        status
            .topic_diagnostics
            .iter()
            .any(|topic| topic.topic == "kukuri:topic:two")
    );
    assert_eq!(status.status_detail, "No peers configured");
    assert!(
        status
            .topic_diagnostics
            .iter()
            .all(|topic| !topic.status_detail.is_empty())
    );
    assert!(
        status
            .topic_diagnostics
            .iter()
            .all(|topic| topic.last_error.is_none())
    );
}

#[tokio::test]
async fn local_only_bootstrap_reads_return_empty_without_remote_docs() {
    let docs_sync = Arc::new(HangingRemoteOnMissDocsSync::default());
    let blob_service = Arc::new(MemoryBlobService::default());
    let store = Arc::new(MemoryStore::default());
    let app = app_with_hanging_remote_docs(store, docs_sync, blob_service, generate_keys());
    let topic = "kukuri:topic:local-only-empty";

    let timeline = timeout(
        Duration::from_secs(2),
        app.list_timeline_scoped(topic, TimelineScope::Public, None, 20),
    )
    .await
    .expect("timeline should not wait for remote docs")
    .expect("timeline");
    assert!(timeline.items.is_empty());

    let thread = timeout(
        Duration::from_secs(2),
        app.list_thread(topic, "missing-root", None, 20),
    )
    .await
    .expect("thread should not wait for remote docs")
    .expect("thread");
    assert!(thread.items.is_empty());

    let joined = timeout(
        Duration::from_secs(2),
        app.list_joined_private_channels(topic),
    )
    .await
    .expect("joined channels should not wait for remote docs")
    .expect("joined channels");
    assert!(joined.is_empty());

    timeout(
        Duration::from_secs(2),
        app.reconcile_blocked_dome_connections_at_start(),
    )
    .await
    .expect("startup reconcile should not wait for remote docs")
    .expect("startup reconcile");

    app.shutdown().await;
}

#[tokio::test]
async fn local_only_bootstrap_reads_return_cached_content_without_remote_docs() {
    let docs_sync = Arc::new(HangingRemoteOnMissDocsSync::default());
    let blob_service = Arc::new(MemoryBlobService::default());
    let keys = generate_keys();
    let writer = app_with_hanging_remote_docs(
        Arc::new(MemoryStore::default()),
        docs_sync.clone(),
        blob_service.clone(),
        keys.clone(),
    );
    let topic = "kukuri:topic:local-only-cached";
    let followed_pubkey = generate_keys().public_key_hex();

    let root_id = writer
        .create_post(topic, "cached root", None)
        .await
        .expect("create cached root");
    let reply_id = writer
        .create_post(topic, "cached reply", Some(root_id.as_str()))
        .await
        .expect("create cached reply");
    let channel = writer
        .create_private_channel(CreatePrivateChannelInput {
            topic_id: TopicId::new(topic),
            label: "cached".into(),
            audience_kind: ChannelAudienceKind::InviteOnly,
        })
        .await
        .expect("create private channel");
    let capability = writer
        .get_private_channel_capability(topic, channel.channel_id.as_str())
        .await
        .expect("get capability")
        .expect("capability");
    writer
        .follow_author(followed_pubkey.as_str())
        .await
        .expect("follow author");

    let reader = app_with_hanging_remote_docs(
        Arc::new(MemoryStore::default()),
        docs_sync,
        blob_service,
        keys,
    );
    reader
        .restore_private_channel_capability(capability)
        .await
        .expect("restore capability");

    let timeline = timeout(
        Duration::from_secs(2),
        reader.list_timeline_scoped(topic, TimelineScope::Public, None, 20),
    )
    .await
    .expect("timeline should use cached local docs")
    .expect("timeline");
    assert!(timeline.items.iter().any(|post| post.object_id == root_id));

    let thread = timeout(
        Duration::from_secs(2),
        reader.list_thread(topic, root_id.as_str(), None, 20),
    )
    .await
    .expect("thread should use cached local docs")
    .expect("thread");
    assert!(thread.items.iter().any(|post| post.object_id == root_id));
    assert!(thread.items.iter().any(|post| post.object_id == reply_id));

    let joined = timeout(
        Duration::from_secs(2),
        reader.list_joined_private_channels(topic),
    )
    .await
    .expect("joined channels should use cached local docs")
    .expect("joined channels");
    assert_eq!(joined.len(), 1);
    assert_eq!(joined[0].channel_id, channel.channel_id);

    // 自分の profile の列を開くと、自分の author replica の follow が手元の docs から反映される。
    timeout(
        Duration::from_secs(2),
        display_author(&reader, reader.current_author_pubkey().as_str()),
    )
    .await
    .expect("profile display should use cached local docs")
    .expect("profile display");
    timeout(Duration::from_secs(2), async {
        loop {
            let view = reader
                .get_author_social_view(followed_pubkey.as_str())
                .await
                .expect("author social view");
            if view.following {
                return;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("follow relationship should hydrate from cached local docs");

    writer.shutdown().await;
    reader.shutdown().await;
}

#[tokio::test]
async fn discovery_status_separates_bootstrap_seed_peers_from_manual_tickets() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(FakeTransport::new("app", FakeNetwork::default()));
    transport
        .configure_discovery(
            DiscoveryMode::StaticPeer,
            false,
            vec![SeedPeer {
                endpoint_id: "configured-peer".into(),
                addr_hint: None,
            }],
            vec![SeedPeer {
                endpoint_id: "bootstrap-peer".into(),
                addr_hint: None,
            }],
        )
        .await
        .expect("configure discovery");
    transport
        .import_ticket("manual-ticket-peer")
        .await
        .expect("import ticket");
    let app = AppService::new(store, transport);

    let discovery = app.get_discovery_status().await.expect("discovery status");

    assert_eq!(
        discovery.configured_seed_peer_ids,
        vec!["configured-peer".to_string()]
    );
    assert_eq!(
        discovery.bootstrap_seed_peer_ids,
        vec!["bootstrap-peer".to_string()]
    );
    assert_eq!(
        discovery.manual_ticket_peer_ids,
        vec!["manual-ticket-peer".to_string()]
    );
    assert!(discovery.docs_assist_peer_ids.is_empty());
    assert!(discovery.blob_assist_peer_ids.is_empty());
}

#[tokio::test]
async fn docs_assisted_peers_do_not_mark_live_sync_connected() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot {
        connected: false,
        peer_count: 0,
        connected_peers: Vec::new(),
        configured_peers: vec!["peer-a".into(), "peer-b".into()],
        subscribed_topics: vec!["kukuri:topic:relay-assisted".into()],
        active_path: Default::default(),
        fallback_peer_ids: Vec::new(),
        pending_events: 0,
        status_detail: "No peers configured".into(),
        last_error: None,
        topic_diagnostics: vec![TopicPeerSnapshot {
            topic: "kukuri:topic:relay-assisted".into(),
            joined: false,
            peer_count: 0,
            connected_peers: Vec::new(),
            configured_peer_ids: vec!["peer-a".into(), "peer-b".into()],
            missing_peer_ids: vec!["peer-a".into(), "peer-b".into()],
            active_path: Default::default(),
            rendezvous_peer_ids: Vec::new(),
            fallback_peer_ids: Vec::new(),
            last_received_at: None,
            status_detail: "No peers configured".into(),
            last_error: None,
        }],
    }));
    let docs_sync = Arc::new(AssistedDocsSync::new(vec!["peer-a", "peer-b"]));
    let blob_service = Arc::new(AssistedBlobService::new(vec!["peer-b", "peer-c"]));
    let app = app_service_from_dependencies(
        store.clone(),
        store,
        transport.clone(),
        transport,
        docs_sync,
        blob_service,
        generate_keys(),
    );

    let status = app.get_sync_status().await.expect("sync status");

    assert!(!status.connected);
    assert_eq!(status.delivery_state, DeliveryState::DurableRecovering);
    assert_eq!(status.peer_count, 0);
    assert_eq!(
        status.status_detail,
        "docs-assisted recovery is in progress via 2 peer(s); live topic delivery is unavailable"
    );
    assert_eq!(
        status.discovery.docs_assist_peer_ids,
        vec!["peer-a".to_string(), "peer-b".to_string()]
    );
    assert_eq!(
        status.discovery.blob_assist_peer_ids,
        vec!["peer-b".to_string(), "peer-c".to_string()]
    );
    assert_eq!(status.topic_diagnostics.len(), 1);
    assert!(!status.topic_diagnostics[0].joined);
    assert_eq!(
        status.topic_diagnostics[0].delivery_state,
        DeliveryState::DurableRecovering
    );
    assert_eq!(status.topic_diagnostics[0].peer_count, 0);
    assert_eq!(
        status.topic_diagnostics[0].docs_assist_peer_ids,
        vec!["peer-a".to_string(), "peer-b".to_string()]
    );
    assert_eq!(
        status.topic_diagnostics[0].status_detail,
        "docs-assisted recovery is in progress via 2 peer(s); live topic delivery is unavailable"
    );
}

#[tokio::test]
async fn blob_only_assist_peers_do_not_mark_sync_healthy() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot {
        connected: false,
        peer_count: 0,
        connected_peers: Vec::new(),
        configured_peers: vec!["peer-a".into()],
        subscribed_topics: vec!["kukuri:topic:relay-assisted".into()],
        active_path: Default::default(),
        fallback_peer_ids: Vec::new(),
        pending_events: 0,
        status_detail: "No peers configured".into(),
        last_error: None,
        topic_diagnostics: vec![TopicPeerSnapshot {
            topic: "kukuri:topic:relay-assisted".into(),
            joined: false,
            peer_count: 0,
            connected_peers: Vec::new(),
            configured_peer_ids: vec!["peer-a".into()],
            missing_peer_ids: vec!["peer-a".into()],
            active_path: Default::default(),
            rendezvous_peer_ids: Vec::new(),
            fallback_peer_ids: Vec::new(),
            last_received_at: None,
            status_detail: "No peers configured".into(),
            last_error: None,
        }],
    }));
    let app = app_service_from_dependencies(
        store.clone(),
        store,
        transport.clone(),
        transport,
        Arc::new(AssistedDocsSync::default()),
        Arc::new(AssistedBlobService::new(vec!["peer-b"])),
        generate_keys(),
    );

    let status = app.get_sync_status().await.expect("sync status");

    assert!(!status.connected);
    assert_eq!(status.delivery_state, DeliveryState::Offline);
    assert_eq!(status.peer_count, 0);
    assert_eq!(status.status_detail, "No peers configured");
    assert!(status.discovery.docs_assist_peer_ids.is_empty());
    assert_eq!(
        status.discovery.blob_assist_peer_ids,
        vec!["peer-b".to_string()]
    );
    assert_eq!(status.topic_diagnostics.len(), 1);
    assert!(!status.topic_diagnostics[0].joined);
    assert_eq!(
        status.topic_diagnostics[0].delivery_state,
        DeliveryState::Offline
    );
    assert_eq!(status.topic_diagnostics[0].peer_count, 0);
    assert!(status.topic_diagnostics[0].docs_assist_peer_ids.is_empty());
    assert_eq!(
        status.topic_diagnostics[0].status_detail,
        "No peers configured"
    );
}
