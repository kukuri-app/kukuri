use super::*;

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

    display_topic(&app, topic).await.expect("display topic");

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
        configured_peer_count: 1,
        subscribed_topics: vec!["hint/kukuri:topic:demo".into()],
        active_path: Default::default(),
        fallback_peer_count: 0,
        pending_events: 0,
        status_detail: "Connected".into(),
        last_error: None,
        topic_diagnostics: vec![TopicPeerSnapshot {
            topic: "hint/kukuri:topic:demo".into(),
            joined: true,
            peer_count: 1,
            configured_peer_count: 1,
            missing_peer_count: 0,
            active_path: Default::default(),
            rendezvous_peer_count: 0,
            fallback_peer_count: 0,
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

    // unsubscribe_topic が外すのは desired(#1221 R2-C)。
    for topic in ["kukuri:topic:one", "kukuri:topic:two"] {
        app.set_desired_scope(topic, &TimelineScope::Public, true)
            .await
            .expect("desired");
    }
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
