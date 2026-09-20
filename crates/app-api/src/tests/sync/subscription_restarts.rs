use super::*;

#[tokio::test]
async fn list_timeline_restarts_topic_replica_sync_with_cooldown_when_projection_is_empty() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let docs_sync = Arc::new(TrackingDocsSync::default());
    let blob_service = Arc::new(MemoryBlobService::default());
    let app = app_service_from_dependencies(
        store.clone(),
        store,
        transport.clone(),
        transport,
        docs_sync.clone(),
        blob_service,
        generate_keys(),
    );

    let timeline = app
        .list_timeline("kukuri:topic:replica-restart", None, 20)
        .await
        .expect("timeline");
    assert!(timeline.items.is_empty());

    let second_timeline = app
        .list_timeline("kukuri:topic:replica-restart", None, 20)
        .await
        .expect("second timeline");
    assert!(second_timeline.items.is_empty());
    let third_timeline = app
        .list_timeline("kukuri:topic:replica-restart", None, 20)
        .await
        .expect("third timeline");
    assert!(third_timeline.items.is_empty());

    let restarted = docs_sync.restarted_replicas.lock().await.clone();
    assert_eq!(
        restarted,
        vec![
            topic_replica_id("kukuri:topic:replica-restart")
                .as_str()
                .to_string()
        ]
    );
}

#[tokio::test]
async fn list_timeline_restarts_topic_subscription_with_cooldown_when_projection_is_empty() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let hint_transport = Arc::new(TrackingHintTransport::default());
    let app = app_service_from_dependencies(
        store.clone(),
        store,
        transport,
        hint_transport.clone(),
        Arc::new(TrackingDocsSync::default()),
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    );
    let topic = "kukuri:topic:subscription-restart";

    let timeline = app.list_timeline(topic, None, 20).await.expect("timeline");
    assert!(timeline.items.is_empty());

    let second_timeline = app
        .list_timeline(topic, None, 20)
        .await
        .expect("second timeline");
    assert!(second_timeline.items.is_empty());
    let third_timeline = app
        .list_timeline(topic, None, 20)
        .await
        .expect("third timeline");
    assert!(third_timeline.items.is_empty());

    assert_eq!(*hint_transport.subscribe_count.lock().await, 2);
    assert_eq!(
        hint_transport.unsubscribed_topics.lock().await.clone(),
        vec![topic.to_string()]
    );
}

#[tokio::test]
async fn ensure_topic_subscription_recreates_finished_handle() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let hint_transport = Arc::new(TrackingHintTransport::default());
    let app = app_service_from_dependencies(
        store.clone(),
        store,
        transport,
        hint_transport.clone(),
        Arc::new(TrackingDocsSync::default()),
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    );
    let topic = "kukuri:topic:stale-subscription";

    let timeline = app.list_timeline(topic, None, 20).await.expect("timeline");
    assert!(timeline.items.is_empty());
    assert_eq!(*hint_transport.subscribe_count.lock().await, 1);

    {
        let subscriptions = app.subscription_registry.subscriptions.lock().await;
        subscriptions
            .get(topic)
            .expect("topic subscription handle")
            .abort();
    }
    timeout(Duration::from_secs(5), async {
        loop {
            if !app.has_topic_subscription(topic).await {
                break;
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("subscription should finish after abort");

    let second_timeline = app
        .list_timeline(topic, None, 20)
        .await
        .expect("second timeline");
    assert!(second_timeline.items.is_empty());
    assert_eq!(*hint_transport.subscribe_count.lock().await, 2);
}

#[tokio::test]
async fn set_discovery_seeds_restarts_existing_topic_hint_subscription() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let hint_transport = Arc::new(TrackingHintTransport::default());
    let app = app_service_from_dependencies(
        store.clone(),
        store,
        transport.clone(),
        hint_transport.clone(),
        Arc::new(MemoryDocsSync::default()),
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    );
    let topic = "kukuri:topic:hint-restart";

    let _ = app
        .list_timeline(topic, None, 20)
        .await
        .expect("subscribe timeline");

    app.set_discovery_seeds(
        DiscoveryMode::StaticPeer,
        false,
        vec![SeedPeer {
            endpoint_id: "peer-a".into(),
            addr_hint: None,
        }],
        Vec::new(),
    )
    .await
    .expect("set discovery seeds");

    assert_eq!(*hint_transport.subscribe_count.lock().await, 2);
    assert_eq!(
        hint_transport.unsubscribed_topics.lock().await.clone(),
        vec![topic.to_string()]
    );
}

#[tokio::test]
async fn import_peer_ticket_restarts_existing_topic_hint_subscription() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let hint_transport = Arc::new(TrackingHintTransport::default());
    let docs_sync = Arc::new(TrackingDocsSync::default());
    let app = app_service_from_dependencies(
        store.clone(),
        store,
        transport,
        hint_transport.clone(),
        docs_sync.clone(),
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    );
    let topic = "kukuri:topic:hint-import";

    let _ = app
        .list_timeline(topic, None, 20)
        .await
        .expect("subscribe timeline");

    app.import_peer_ticket("peer-ticket")
        .await
        .expect("import peer ticket");

    assert_eq!(*hint_transport.subscribe_count.lock().await, 2);
    assert_eq!(
        hint_transport.unsubscribed_topics.lock().await.clone(),
        vec![topic.to_string()]
    );
    assert_eq!(docs_sync.subscribe_replicas.lock().await.len(), 2);
}

#[tokio::test]
async fn local_public_post_restarts_replica_sync_after_each_write() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let docs_sync = Arc::new(TrackingDocsSync::default());
    let app = app_service_from_dependencies(
        store.clone(),
        store,
        transport.clone(),
        transport,
        docs_sync.clone(),
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    );
    let topic = "kukuri:topic:post-restart-cooldown";

    let _ = app
        .create_post(topic, "first post", None)
        .await
        .expect("first post");
    let _ = app
        .create_post(topic, "second post", None)
        .await
        .expect("second post");

    let restarted = docs_sync.restarted_replicas.lock().await.clone();
    let expected_replica = topic_replica_id(topic).as_str().to_string();
    assert!(
        restarted.len() >= 2,
        "expected at least one sync restart per local post, got {restarted:?}"
    );
    assert!(
        restarted
            .iter()
            .all(|replica| replica.as_str() == expected_replica),
        "local post restarts should target only the topic replica, got {restarted:?}"
    );
}

#[tokio::test]
async fn hint_miss_coalesces_replica_sync_restarts() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let hint_transport = Arc::new(TrackingHintTransport::default());
    let docs_sync = Arc::new(TrackingDocsSync::default());
    let app = app_service_from_dependencies(
        store.clone(),
        store,
        transport,
        hint_transport.clone(),
        docs_sync.clone(),
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    );
    let topic = "kukuri:topic:hint-miss-cooldown";

    let _ = app
        .list_timeline(topic, None, 20)
        .await
        .expect("subscribe timeline");

    let hint_topic = TopicId::new(topic);
    for suffix in ["one", "two"] {
        hint_transport
            .publish_hint(
                &hint_topic,
                GossipHint::TopicObjectsChanged {
                    topic_id: hint_topic.clone(),
                    objects: vec![HintObjectRef {
                        object_id: format!("missing-{suffix}"),
                        object_kind: "post".into(),
                        docs_author: None,
                    }],
                },
            )
            .await
            .expect("publish hint miss");
    }

    timeout(Duration::from_secs(5), async {
        loop {
            if !docs_sync.restarted_replicas.lock().await.is_empty() {
                return;
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("restart should be requested");

    assert_eq!(
        docs_sync.restarted_replicas.lock().await.clone(),
        vec![topic_replica_id(topic).as_str().to_string()]
    );
}

#[tokio::test]
async fn shutdown_unsubscribes_active_hint_topics() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let hint_transport = Arc::new(TrackingHintTransport::default());
    let app = app_service_from_dependencies(
        store.clone(),
        store,
        transport,
        hint_transport.clone(),
        Arc::new(MemoryDocsSync::default()),
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    );
    let topic = "kukuri:topic:shutdown";

    let _ = app
        .list_timeline(topic, None, 20)
        .await
        .expect("subscribe timeline");

    app.shutdown().await;

    assert_eq!(
        hint_transport.unsubscribed_topics.lock().await.clone(),
        vec![topic.to_string()]
    );
}

#[tokio::test]
async fn sync_status_normalizes_hint_topic_names() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot {
        connected: true,
        peer_count: 1,
        connected_peers: vec!["peer-a".into()],
        configured_peers: vec!["peer-a".into()],
        subscribed_topics: vec!["hint/kukuri:topic:demo".into()],
        active_path: Default::default(),
        fallback_peer_ids: Vec::new(),
        pending_events: 0,
        status_detail: "Connected".into(),
        last_error: None,
        topic_diagnostics: vec![TopicPeerSnapshot {
            topic: "hint/kukuri:topic:demo".into(),
            joined: true,
            peer_count: 1,
            connected_peers: vec!["peer-a".into()],
            configured_peer_ids: vec!["peer-a".into()],
            missing_peer_ids: Vec::new(),
            active_path: Default::default(),
            rendezvous_peer_ids: Vec::new(),
            fallback_peer_ids: Vec::new(),
            last_received_at: Some(1),
            status_detail: "Connected".into(),
            last_error: None,
        }],
    }));
    let app = AppService::new(store, transport);

    let status = app.get_sync_status().await.expect("sync status");

    assert_eq!(status.subscribed_topics, vec!["kukuri:topic:demo"]);
    assert_eq!(status.topic_diagnostics.len(), 1);
    assert_eq!(status.topic_diagnostics[0].topic, "kukuri:topic:demo");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[cfg(feature = "iroh-integration-tests")]
async fn invalid_ticket_updates_sync_status_error_reason() {
    let _guard = iroh_integration_test_lock().lock_owned().await;
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(
        IrohGossipTransport::bind_local()
            .await
            .expect("transport should bind"),
    );
    let app = AppService::new(store, transport);

    let error = app
        .import_peer_ticket("not-a-ticket")
        .await
        .expect_err("invalid ticket should fail");
    let status = app.get_sync_status().await.expect("sync status");

    assert!(error.to_string().contains("failed to import peer ticket"));
    assert!(
        status
            .last_error
            .as_deref()
            .is_some_and(|message| message.contains("failed to import peer ticket"))
    );
}

#[tokio::test]
async fn unsubscribe_topic_removes_subscription_from_sync_status() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(FakeTransport::new("app", FakeNetwork::default()));
    let app = AppService::new(store, transport);

    let _ = app
        .list_timeline("kukuri:topic:one", None, 10)
        .await
        .expect("timeline one");
    let _ = app
        .list_timeline("kukuri:topic:two", None, 10)
        .await
        .expect("timeline two");
    app.unsubscribe_topic("kukuri:topic:two")
        .await
        .expect("unsubscribe topic");
    let status = app.get_sync_status().await.expect("sync status");

    assert!(
        status
            .subscribed_topics
            .iter()
            .any(|topic| topic == "kukuri:topic:one")
    );
    assert!(
        !status
            .subscribed_topics
            .iter()
            .any(|topic| topic == "kukuri:topic:two")
    );
}
