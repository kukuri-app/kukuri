use super::*;
use kukuri_test_support::{PollState, poll_until};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn iroh_transport_syncs_post_between_apps() {
    let _guard = iroh_integration_test_lock().lock_owned().await;
    let dir = tempdir().expect("tempdir");
    let stack_a = TestIrohStack::new(&dir.path().join("post-a")).await;
    let stack_b = TestIrohStack::new(&dir.path().join("post-b")).await;
    let store_a = Arc::new(MemoryStore::default());
    let store_b = Arc::new(MemoryStore::default());
    let app_a = app_with_iroh_services(store_a, &stack_a);
    let app_b = app_with_iroh_services(store_b.clone(), &stack_b);

    let ticket_a = app_a
        .peer_ticket()
        .await
        .expect("ticket a")
        .expect("ticket a value");
    let ticket_b = app_b
        .peer_ticket()
        .await
        .expect("ticket b")
        .expect("ticket b value");
    app_a
        .import_peer_ticket(&ticket_b)
        .await
        .expect("import b into a");
    app_b
        .import_peer_ticket(&ticket_a)
        .await
        .expect("import a into b");

    let topic = "kukuri:topic:app-api-iroh";
    display_topic(&app_b, topic)
        .await
        .expect("app b should subscribe to topic");

    let object_id = app_a
        .create_post(topic, "hello over iroh transport", None)
        .await
        .expect("app a should create post");

    let received = timeout(Duration::from_secs(30), async {
        loop {
            let timeline = app_b
                .list_timeline(topic, None, 20)
                .await
                .expect("timeline should load");
            if let Some(post) = timeline
                .items
                .iter()
                .find(|post| post.object_id == object_id)
            {
                return post.clone();
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("timeline sync timeout");

    assert_eq!(received.content, "hello over iroh transport");
    let status_b = app_b.get_sync_status().await.expect("sync status b");
    assert!(status_b.last_sync_ts.is_some());
    assert!(
        status_b
            .subscribed_topics
            .iter()
            .any(|value| value == topic)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn import_peer_ticket_restarts_existing_topic_subscription_and_resumes_delivery() {
    let _guard = iroh_integration_test_lock().lock_owned().await;
    let dir = tempdir().expect("tempdir");
    let stack_a = TestIrohStack::new(&dir.path().join("rebind-a")).await;
    let stack_b = TestIrohStack::new(&dir.path().join("rebind-b")).await;
    let store_a = Arc::new(MemoryStore::default());
    let store_b = Arc::new(MemoryStore::default());
    let app_a = app_with_iroh_services(store_a, &stack_a);
    let app_b = app_with_iroh_services(store_b, &stack_b);
    let topic = "kukuri:topic:rebind-after-import";

    display_topic(&app_a, topic)
        .await
        .expect("subscribe a before import");
    display_topic(&app_b, topic)
        .await
        .expect("subscribe b before import");

    let ticket_a = app_a
        .peer_ticket()
        .await
        .expect("ticket a")
        .expect("ticket a value");
    let ticket_b = app_b
        .peer_ticket()
        .await
        .expect("ticket b")
        .expect("ticket b value");
    app_a
        .import_peer_ticket(&ticket_b)
        .await
        .expect("import b into a");
    app_b
        .import_peer_ticket(&ticket_a)
        .await
        .expect("import a into b");

    wait_for_topic_delivery(&app_a, topic, 1).await;
    wait_for_topic_delivery(&app_b, topic, 1).await;

    let object_id = app_a
        .create_post(topic, "hello after import", None)
        .await
        .expect("create post");
    let received = timeout(Duration::from_secs(30), async {
        loop {
            let timeline = app_b
                .list_timeline(topic, None, 20)
                .await
                .expect("timeline should load");
            if let Some(post) = timeline
                .items
                .iter()
                .find(|post| post.object_id == object_id)
            {
                return post.clone();
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("timeline sync timeout");

    assert_eq!(received.content, "hello after import");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn seeded_dht_syncs_post_between_apps_without_ticket_import() {
    let _guard = iroh_integration_test_lock().lock_owned().await;
    let dir = tempdir().expect("tempdir");
    let testnet = Testnet::new(5).await.expect("testnet");
    let stack_a = TestIrohStack::new_with_dht(&dir.path().join("seeded-dht-a"), &testnet).await;
    let stack_b = TestIrohStack::new_with_dht(&dir.path().join("seeded-dht-b"), &testnet).await;
    let store_a = Arc::new(MemoryStore::default());
    let store_b = Arc::new(MemoryStore::default());
    let app_a = app_with_iroh_services(store_a, &stack_a);
    let app_b = app_with_iroh_services(store_b, &stack_b);
    let endpoint_a = app_a
        .get_sync_status()
        .await
        .expect("status a")
        .discovery
        .local_endpoint_id;
    let endpoint_b = app_b
        .get_sync_status()
        .await
        .expect("status b")
        .discovery
        .local_endpoint_id;

    configure_seeded_dht(&app_a, endpoint_b.clone()).await;
    configure_seeded_dht(&app_b, endpoint_a.clone()).await;
    let topic = "kukuri:topic:seeded-dht-app";
    display_topic(&app_a, topic)
        .await
        .expect("subscribe a timeline");
    display_topic(&app_b, topic)
        .await
        .expect("subscribe b timeline");
    timeout(Duration::from_secs(90), async {
        loop {
            let status_a = app_a.get_sync_status().await.expect("status a");
            let status_b = app_b.get_sync_status().await.expect("status b");
            let ready_a = status_a
                .topic_diagnostics
                .iter()
                .any(|topic_status| topic_status.topic == topic && topic_status.peer_count > 0);
            let ready_b = status_b
                .topic_diagnostics
                .iter()
                .any(|topic_status| topic_status.topic == topic && topic_status.peer_count > 0);
            if ready_a && ready_b {
                return;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("seeded dht ready timeout");

    let object_id = app_a
        .create_post(topic, "seeded dht app sync", None)
        .await
        .expect("create post");

    let received = timeout(Duration::from_secs(20), async {
        loop {
            let timeline = app_b
                .list_timeline(topic, None, 20)
                .await
                .expect("timeline");
            if let Some(post) = timeline
                .items
                .iter()
                .find(|post| post.object_id == object_id)
            {
                return post.clone();
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("seeded dht sync timeout");

    assert_eq!(received.content, "seeded dht app sync");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relay_seeded_syncs_post_between_apps_without_ticket_import() {
    let _guard = iroh_integration_test_lock().lock_owned().await;
    let (_relay_map, relay_url, _relay_guard) = iroh::test_utils::run_relay_server()
        .await
        .expect("relay server");
    let relay_config = TransportRelayConfig {
        iroh_relay_urls: vec![relay_url.to_string()],
    };
    let dir = tempdir().expect("tempdir");
    let stack_a = TestIrohStack::new_with_options(
        &dir.path().join("relay-seeded-a"),
        DhtDiscoveryOptions::disabled(),
        relay_config.clone(),
    )
    .await;
    let stack_b = TestIrohStack::new_with_options(
        &dir.path().join("relay-seeded-b"),
        DhtDiscoveryOptions::disabled(),
        relay_config,
    )
    .await;
    let store_a = Arc::new(MemoryStore::default());
    let store_b = Arc::new(MemoryStore::default());
    let app_a = app_with_iroh_services(store_a, &stack_a);
    let app_b = app_with_iroh_services(store_b, &stack_b);
    let ticket_a = app_a
        .peer_ticket()
        .await
        .expect("ticket a")
        .expect("ticket a value");
    let ticket_b = app_b
        .peer_ticket()
        .await
        .expect("ticket b")
        .expect("ticket b value");
    let seed_a = {
        let (endpoint_id, addr_hint) = ticket_b.split_once('@').expect("ticket b host");
        SeedPeer {
            endpoint_id: endpoint_id.to_string(),
            addr_hint: Some(addr_hint.to_string()),
        }
    };
    let seed_b = {
        let (endpoint_id, addr_hint) = ticket_a.split_once('@').expect("ticket a host");
        SeedPeer {
            endpoint_id: endpoint_id.to_string(),
            addr_hint: Some(addr_hint.to_string()),
        }
    };
    app_a
        .set_discovery_seeds(DiscoveryMode::StaticPeer, false, Vec::new(), vec![seed_a])
        .await
        .expect("set relay seeds a");
    app_b
        .set_discovery_seeds(DiscoveryMode::StaticPeer, false, Vec::new(), vec![seed_b])
        .await
        .expect("set relay seeds b");

    let topic = "kukuri:topic:relay-seeded";
    display_topic(&app_a, topic).await.expect("subscribe a");
    display_topic(&app_b, topic).await.expect("subscribe b");

    let object_id = app_a
        .create_post(topic, "relay seeded app sync", None)
        .await
        .expect("create post");
    let received = timeout(Duration::from_secs(60), async {
        loop {
            let timeline = app_b
                .list_timeline(topic, None, 20)
                .await
                .expect("timeline");
            if let Some(post) = timeline
                .items
                .iter()
                .find(|post| post.object_id == object_id)
            {
                return post.clone();
            }
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await;
    let received = match received {
        Ok(post) => post,
        Err(error) => {
            let diagnostics =
                iroh_sync_diagnostics(&app_a, &app_b, &stack_a, &stack_b, topic).await;
            panic!("relay seeded sync timeout: {error:?}; {diagnostics}");
        }
    };

    assert_eq!(received.content, "relay seeded app sync");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_relay_endpoint_only_seeds_sync_post_between_apps() {
    let Some(relay_url) = std::env::var("KUKURI_TEST_EXTERNAL_IROH_RELAY_URL")
        .ok()
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())
    else {
        return;
    };

    let _guard = iroh_integration_test_lock().lock_owned().await;
    let relay_config = TransportRelayConfig {
        iroh_relay_urls: vec![relay_url],
    };
    let dir = tempdir().expect("tempdir");
    let stack_a = TestIrohStack::new_with_network_options(
        &dir.path().join("external-relay-a"),
        kukuri_transport::TransportNetworkConfig::default(),
        DhtDiscoveryOptions::disabled(),
        relay_config.clone(),
    )
    .await;
    let stack_b = TestIrohStack::new_with_network_options(
        &dir.path().join("external-relay-b"),
        kukuri_transport::TransportNetworkConfig::default(),
        DhtDiscoveryOptions::disabled(),
        relay_config,
    )
    .await;
    let app_a = app_with_iroh_services(Arc::new(MemoryStore::default()), &stack_a);
    let app_b = app_with_iroh_services(Arc::new(MemoryStore::default()), &stack_b);
    let endpoint_a = app_a
        .get_sync_status()
        .await
        .expect("status a")
        .discovery
        .local_endpoint_id;
    let endpoint_b = app_b
        .get_sync_status()
        .await
        .expect("status b")
        .discovery
        .local_endpoint_id;

    app_a
        .set_discovery_seeds(
            DiscoveryMode::StaticPeer,
            false,
            Vec::new(),
            vec![SeedPeer {
                endpoint_id: endpoint_b,
                addr_hint: None,
            }],
        )
        .await
        .expect("set seed a");
    app_b
        .set_discovery_seeds(
            DiscoveryMode::StaticPeer,
            false,
            Vec::new(),
            vec![SeedPeer {
                endpoint_id: endpoint_a,
                addr_hint: None,
            }],
        )
        .await
        .expect("set seed b");

    let topic = "kukuri:topic:external-relay-endpoint-only";
    display_topic(&app_a, topic).await.expect("subscribe a");
    display_topic(&app_b, topic).await.expect("subscribe b");
    wait_for_topic_delivery(&app_a, topic, 1).await;
    wait_for_topic_delivery(&app_b, topic, 1).await;

    let object_id = app_a
        .create_post(topic, "external relay endpoint-only app sync", None)
        .await
        .expect("create post");
    let received = timeout(Duration::from_secs(120), async {
        loop {
            let timeline = app_b
                .list_timeline(topic, None, 20)
                .await
                .expect("timeline");
            if let Some(post) = timeline
                .items
                .iter()
                .find(|post| post.object_id == object_id)
            {
                return post.clone();
            }
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await;
    let received = match received {
        Ok(post) => post,
        Err(error) => {
            let diagnostics =
                iroh_sync_diagnostics(&app_a, &app_b, &stack_a, &stack_b, topic).await;
            panic!("external relay endpoint-only sync timeout: {error:?}; {diagnostics}");
        }
    };

    assert_eq!(received.content, "external relay endpoint-only app sync");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn seeded_dht_updates_existing_topic_subscription_after_seed_update() {
    let _guard = iroh_integration_test_lock().lock_owned().await;
    let dir = tempdir().expect("tempdir");
    let testnet = Testnet::new(5).await.expect("testnet");
    let stack_a = TestIrohStack::new_with_dht(&dir.path().join("seeded-rebind-a"), &testnet).await;
    let stack_b = TestIrohStack::new_with_dht(&dir.path().join("seeded-rebind-b"), &testnet).await;
    let store_a = Arc::new(MemoryStore::default());
    let store_b = Arc::new(MemoryStore::default());
    let app_a = app_with_iroh_services(store_a, &stack_a);
    let app_b = app_with_iroh_services(store_b, &stack_b);
    let topic = "kukuri:topic:seeded-rebind";

    display_topic(&app_a, topic)
        .await
        .expect("subscribe a before seed update");
    display_topic(&app_b, topic)
        .await
        .expect("subscribe b before seed update");

    let endpoint_a = app_a
        .get_sync_status()
        .await
        .expect("status a")
        .discovery
        .local_endpoint_id;
    let endpoint_b = app_b
        .get_sync_status()
        .await
        .expect("status b")
        .discovery
        .local_endpoint_id;
    configure_seeded_dht(&app_a, endpoint_b.clone()).await;
    configure_seeded_dht(&app_b, endpoint_a.clone()).await;

    poll_until(
        Duration::from_secs(20),
        Duration::from_millis(100),
        3,
        || async {
            let status_a = app_a.get_sync_status().await.expect("status a");
            let status_b = app_b.get_sync_status().await.expect("status b");
            let ready_a = status_a.topic_diagnostics.iter().any(|topic_status| {
                topic_status.topic == topic
                    && topic_status.joined
                    && !topic_status.connected_peers.is_empty()
            });
            let ready_b = status_b.topic_diagnostics.iter().any(|topic_status| {
                topic_status.topic == topic
                    && topic_status.joined
                    && !topic_status.connected_peers.is_empty()
            });
            Ok::<_, std::convert::Infallible>(if ready_a && ready_b {
                PollState::Ready(())
            } else {
                PollState::Pending
            })
        },
    )
    .await
    .expect("seeded dht topic update timeout");

    let object_id = app_a
        .create_post(topic, "seeded dht rebind", None)
        .await
        .expect("create post");

    timeout(Duration::from_secs(90), async {
        loop {
            let timeline = app_b
                .list_timeline(topic, None, 20)
                .await
                .expect("timeline b");
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
    .expect("seeded dht propagation timeout");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn seeded_dht_backfills_docs_and_blobs_with_id_only_seed() {
    let _guard = iroh_integration_test_lock().lock_owned().await;
    let dir = tempdir().expect("tempdir");
    let testnet = Testnet::new(5).await.expect("testnet");
    let stack_a = TestIrohStack::new_with_dht(&dir.path().join("seeded-image-a"), &testnet).await;
    let stack_b = TestIrohStack::new_with_dht(&dir.path().join("seeded-image-b"), &testnet).await;
    let store_a = Arc::new(MemoryStore::default());
    let store_b = Arc::new(MemoryStore::default());
    let app_a = app_with_iroh_services(store_a, &stack_a);
    let app_b = app_with_iroh_services(store_b, &stack_b);
    let endpoint_a = app_a
        .get_sync_status()
        .await
        .expect("status a")
        .discovery
        .local_endpoint_id;
    let endpoint_b = app_b
        .get_sync_status()
        .await
        .expect("status b")
        .discovery
        .local_endpoint_id;
    configure_seeded_dht(&app_a, endpoint_b.clone()).await;
    configure_seeded_dht(&app_b, endpoint_a.clone()).await;
    let topic = "kukuri:topic:seeded-image";
    display_topic(&app_a, topic)
        .await
        .expect("subscribe a timeline");
    display_topic(&app_b, topic)
        .await
        .expect("subscribe b timeline");
    timeout(Duration::from_secs(20), async {
        loop {
            let status_a = app_a.get_sync_status().await.expect("status a");
            let status_b = app_b.get_sync_status().await.expect("status b");
            let ready_a = status_a
                .topic_diagnostics
                .iter()
                .any(|topic_status| topic_status.topic == topic && topic_status.peer_count > 0);
            let ready_b = status_b
                .topic_diagnostics
                .iter()
                .any(|topic_status| topic_status.topic == topic && topic_status.peer_count > 0);
            if ready_a && ready_b {
                return;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("seeded dht image ready timeout");

    let object_id = app_a
        .create_post_with_attachments(
            topic,
            "seeded image",
            None,
            vec![pending_image_attachment("image/png", b"seeded-image-bytes")],
        )
        .await
        .expect("create image post");

    let received = timeout(Duration::from_secs(20), async {
        loop {
            let timeline = app_b
                .list_timeline(topic, None, 20)
                .await
                .expect("timeline b");
            if let Some(post) = timeline
                .items
                .iter()
                .find(|post| post.object_id == object_id)
            {
                return post.clone();
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("seeded dht image backfill timeout");

    assert_eq!(received.attachments.len(), 1);
    // view の状態は remote から取得しない（#1152）。表示要求が取得して local へ保存する。
    assert_eq!(received.attachments[0].status, BlobViewStatus::Missing);
    let hash = received.attachments[0].hash.as_str();
    assert!(
        app_b
            .blob_preview_data_url(hash, "image/png")
            .await
            .expect("preview")
            .is_some()
    );
    assert_eq!(
        timeline_attachment_status(&app_b, topic, &object_id, hash).await,
        BlobViewStatus::Available
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn iroh_transport_syncs_repost_and_notification() {
    let _guard = iroh_integration_test_lock().lock_owned().await;
    let dir = tempdir().expect("tempdir");
    let stack_a = TestIrohStack::new(&dir.path().join("repost-a")).await;
    let stack_b = TestIrohStack::new(&dir.path().join("repost-b")).await;
    let store_a = Arc::new(MemoryStore::default());
    let store_b = Arc::new(MemoryStore::default());
    let app_a = app_with_iroh_services(store_a, &stack_a);
    let app_b = app_with_iroh_services(store_b, &stack_b);
    let topic = "kukuri:topic:repost-notification";

    let ticket_a = app_a
        .peer_ticket()
        .await
        .expect("ticket a")
        .expect("ticket a value");
    let ticket_b = app_b
        .peer_ticket()
        .await
        .expect("ticket b")
        .expect("ticket b value");
    app_a
        .import_peer_ticket(&ticket_b)
        .await
        .expect("import b into a");
    app_b
        .import_peer_ticket(&ticket_a)
        .await
        .expect("import a into b");

    display_topic(&app_a, topic)
        .await
        .expect("subscribe a timeline");
    display_topic(&app_b, topic)
        .await
        .expect("subscribe b timeline");
    wait_for_topic_delivery(&app_a, topic, 1).await;
    wait_for_topic_delivery(&app_b, topic, 1).await;

    let source_id = app_a
        .create_post(topic, "relay source post", None)
        .await
        .expect("create source post");
    if let Err(error) = timeout(p2p_replication_timeout(), async {
        loop {
            let timeline = app_b
                .list_timeline(topic, None, 20)
                .await
                .expect("timeline b");
            if timeline
                .items
                .iter()
                .any(|post| post.object_id == source_id)
            {
                return;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    {
        panic!(
            "source propagation timeout: {error:?}; {}",
            iroh_sync_diagnostics(&app_a, &app_b, &stack_a, &stack_b, topic).await
        );
    }

    let repost_id = app_b
        .create_repost(topic, topic, source_id.as_str(), None)
        .await
        .expect("create repost");
    if let Err(error) = timeout(p2p_replication_timeout(), async {
        loop {
            let timeline = app_a
                .list_timeline(topic, None, 20)
                .await
                .expect("timeline a");
            let notifications = app_a.list_notifications().await.expect("notifications a");
            let has_repost = timeline
                .items
                .iter()
                .any(|post| post.object_id == repost_id);
            let has_notification = notifications.iter().any(|item| {
                item.kind == NotificationKind::Repost
                    && item.object_id.as_deref() == Some(repost_id.as_str())
            });
            if has_repost && has_notification {
                return;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    {
        panic!(
            "repost propagation timeout: {error:?}; {}",
            iroh_sync_diagnostics(&app_a, &app_b, &stack_a, &stack_b, topic).await
        );
    }

    assert_eq!(
        app_a
            .get_notification_status()
            .await
            .expect("notification status")
            .unread_count,
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn iroh_transport_syncs_reply_into_thread() {
    let _guard = iroh_integration_test_lock().lock_owned().await;
    let dir = tempdir().expect("tempdir");
    let stack_a = TestIrohStack::new(&dir.path().join("reply-a")).await;
    let stack_b = TestIrohStack::new(&dir.path().join("reply-b")).await;
    let store_a = Arc::new(MemoryStore::default());
    let store_b = Arc::new(MemoryStore::default());
    let app_a = app_with_iroh_services(store_a.clone(), &stack_a);
    let app_b = app_with_iroh_services(store_b, &stack_b);
    let topic = "kukuri:topic:reply-thread";

    let ticket_a = app_a
        .peer_ticket()
        .await
        .expect("ticket a")
        .expect("ticket a value");
    let ticket_b = app_b
        .peer_ticket()
        .await
        .expect("ticket b")
        .expect("ticket b value");
    app_a
        .import_peer_ticket(&ticket_b)
        .await
        .expect("import b into a");
    app_b
        .import_peer_ticket(&ticket_a)
        .await
        .expect("import a into b");
    display_topic(&app_a, topic)
        .await
        .expect("subscribe a timeline");
    display_topic(&app_b, topic)
        .await
        .expect("subscribe b timeline");
    wait_for_topic_delivery(&app_a, topic, 1).await;
    wait_for_topic_delivery(&app_b, topic, 1).await;

    let root_id = app_a
        .create_post(topic, "root over iroh", None)
        .await
        .expect("create root");

    timeout(Duration::from_secs(10), async {
        loop {
            let timeline = app_b
                .list_timeline(topic, None, 20)
                .await
                .expect("timeline b");
            if timeline.items.iter().any(|post| post.object_id == root_id) {
                return;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("root propagation timeout");

    let reply_id = app_b
        .create_post(topic, "reply over iroh", Some(root_id.as_str()))
        .await
        .expect("create reply");
    let thread = timeout(p2p_replication_timeout(), async {
        loop {
            let thread = app_b
                .list_thread(topic, root_id.as_str(), None, 20)
                .await
                .expect("thread b");
            if thread.items.iter().any(|post| post.object_id == reply_id) {
                return thread;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("local reply propagation timeout");

    let thread_ids = thread
        .items
        .iter()
        .map(|post| post.object_id.clone())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        thread_ids.len(),
        2,
        "thread items: {:?}",
        thread
            .items
            .iter()
            .map(|post| format!(
                "{}|reply={:?}|root={:?}",
                post.object_id, post.reply_to, post.root_id
            ))
            .collect::<Vec<_>>()
    );
    assert!(thread_ids.contains(root_id.as_str()));
    assert!(thread_ids.contains(reply_id.as_str()));
    let reply = thread
        .items
        .iter()
        .find(|post| post.object_id == reply_id)
        .expect("reply in thread");
    assert_eq!(reply.reply_to.as_deref(), Some(root_id.as_str()));
    assert_eq!(reply.root_id.as_deref(), Some(root_id.as_str()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn iroh_transport_syncs_multiple_topics_bidirectionally() {
    let _guard = iroh_integration_test_lock().lock_owned().await;
    let dir = tempdir().expect("tempdir");
    let stack_a = TestIrohStack::new(&dir.path().join("multi-a")).await;
    let stack_b = TestIrohStack::new(&dir.path().join("multi-b")).await;
    let store_a = Arc::new(MemoryStore::default());
    let store_b = Arc::new(MemoryStore::default());
    let app_a = app_with_iroh_services(store_a, &stack_a);
    let app_b = app_with_iroh_services(store_b, &stack_b);
    let topic_one = "kukuri:topic:one";
    let topic_two = "kukuri:topic:two";

    let ticket_a = app_a
        .peer_ticket()
        .await
        .expect("ticket a")
        .expect("ticket a value");
    let ticket_b = app_b
        .peer_ticket()
        .await
        .expect("ticket b")
        .expect("ticket b value");
    app_a
        .import_peer_ticket(&ticket_b)
        .await
        .expect("import b into a");
    app_b
        .import_peer_ticket(&ticket_a)
        .await
        .expect("import a into b");

    display_topic(&app_a, topic_one)
        .await
        .expect("subscribe a topic one");
    display_topic(&app_a, topic_two)
        .await
        .expect("subscribe a topic two");
    display_topic(&app_b, topic_one)
        .await
        .expect("subscribe b topic one");
    display_topic(&app_b, topic_two)
        .await
        .expect("subscribe b topic two");

    let id_one = app_a
        .create_post(topic_one, "topic one from a", None)
        .await
        .expect("post one");
    let id_two = app_b
        .create_post(topic_two, "topic two from b", None)
        .await
        .expect("post two");

    timeout(Duration::from_secs(10), async {
        loop {
            let timeline_b = app_b
                .list_timeline(topic_one, None, 20)
                .await
                .expect("timeline b");
            let timeline_a = app_a
                .list_timeline(topic_two, None, 20)
                .await
                .expect("timeline a");
            let has_one = timeline_b.items.iter().any(|post| post.object_id == id_one);
            let has_two = timeline_a.items.iter().any(|post| post.object_id == id_two);
            if has_one && has_two {
                return;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("multi topic propagation timeout");
}
