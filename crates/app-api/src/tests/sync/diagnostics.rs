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
    assert_eq!(discovery.configured_seed_peer_count, 1);
    assert_eq!(discovery.bootstrap_seed_peer_count, 1);
    assert_eq!(discovery.docs_assist_peer_count, 0);
    assert_eq!(discovery.blob_assist_peer_count, 0);
    for (kind, expected) in [
        (
            ConnectivityPeerKind::ConfiguredSeed,
            vec!["configured-peer"],
        ),
        (ConnectivityPeerKind::BootstrapSeed, vec!["bootstrap-peer"]),
        (
            ConnectivityPeerKind::ManualTicket,
            vec!["manual-ticket-peer"],
        ),
        (ConnectivityPeerKind::DocsAssist, vec![]),
    ] {
        assert_eq!(
            peer_ids(&app, kind, None).await,
            expected,
            "{kind:?} is listed on its own page"
        );
    }
}

async fn peer_ids(
    app: &AppService,
    kind: ConnectivityPeerKind,
    topic: Option<&str>,
) -> Vec<String> {
    app.list_connectivity_peers(ConnectivityPeersRequest {
        kind,
        topic: topic.map(str::to_string),
        cursor: None,
        limit: None,
    })
    .await
    .expect("peer page")
    .peer_ids
}

fn relay_assisted_snapshot(configured: usize) -> PeerSnapshot {
    PeerSnapshot {
        configured_peer_count: configured,
        subscribed_topics: vec!["kukuri:topic:relay-assisted".into()],
        status_detail: "No peers configured".into(),
        topic_diagnostics: vec![TopicPeerSnapshot {
            topic: "kukuri:topic:relay-assisted".into(),
            configured_peer_count: configured,
            missing_peer_count: configured,
            status_detail: "No peers configured".into(),
            ..TopicPeerSnapshot::default()
        }],
        ..PeerSnapshot::default()
    }
}

/// 未確認(neighbor の無い topic、docs の補助はあるが取得の活動が無い)は、成功(Live・DurableReady)として
/// 表示しない(#1221 R2-D AC-1・AC-6)。
#[tokio::test]
async fn docs_assisted_peers_do_not_mark_live_sync_connected() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(relay_assisted_snapshot(2)));
    let docs_sync = Arc::new(AssistedDocsSync::new(vec!["peer-b", "peer-a"]));
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
    assert_eq!(status.discovery.docs_assist_peer_count, 2);
    assert_eq!(status.discovery.blob_assist_peer_count, 2);
    assert_eq!(
        peer_ids(&app, ConnectivityPeerKind::DocsAssist, None).await,
        vec!["peer-a".to_string(), "peer-b".to_string()]
    );
    assert_eq!(
        peer_ids(&app, ConnectivityPeerKind::BlobAssist, None).await,
        vec!["peer-b".to_string(), "peer-c".to_string()]
    );
    assert_eq!(status.topic_diagnostics.len(), 1);
    assert!(!status.topic_diagnostics[0].joined);
    assert_eq!(
        status.topic_diagnostics[0].delivery_state,
        DeliveryState::DurableRecovering
    );
    assert_eq!(status.topic_diagnostics[0].peer_count, 0);
    assert_eq!(status.topic_diagnostics[0].missing_peer_count, 2);
    assert_eq!(
        status.topic_diagnostics[0].status_detail,
        "docs-assisted recovery is in progress via 2 peer(s); live topic delivery is unavailable"
    );
}

#[tokio::test]
async fn blob_only_assist_peers_do_not_mark_sync_healthy() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(relay_assisted_snapshot(1)));
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
    assert_eq!(status.discovery.docs_assist_peer_count, 0);
    assert_eq!(status.discovery.blob_assist_peer_count, 1);
    assert_eq!(status.topic_diagnostics.len(), 1);
    assert!(!status.topic_diagnostics[0].joined);
    assert_eq!(
        status.topic_diagnostics[0].delivery_state,
        DeliveryState::Offline
    );
    assert_eq!(status.topic_diagnostics[0].peer_count, 0);
    assert_eq!(
        status.topic_diagnostics[0].status_detail,
        "No peers configured"
    );
}

/// #1221 R2-D AC-1: gossip を止めた設定は、稼働中の scope(lease)の分だけを返す。止めた設定の履歴を
/// 10 倍にしても、状態に載る件数は変わらない。
#[tokio::test]
async fn status_lists_gossip_disabled_scopes_only_for_active_leases() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(FakeTransport::new("app", FakeNetwork::default()));
    let app = AppService::new(store, transport);
    display_topic(&app, "kukuri:topic:leased")
        .await
        .expect("open the leased column");
    app.set_topic_gossip_enabled("kukuri:topic:leased", false)
        .await
        .expect("disable leased");
    for count in [1, 10] {
        let mut disabled_topics = vec!["kukuri:topic:leased".to_string()];
        let mut disabled_channels = Vec::new();
        for index in 0..count {
            disabled_topics.push(format!("kukuri:topic:dormant-{index}"));
            disabled_channels.push(format!("kukuri:topic:dormant-{index}::channel"));
        }
        app.restore_gossip_disabled_state(disabled_topics, disabled_channels)
            .await;
        let status = app.get_sync_status().await.expect("status");
        assert_eq!(
            status.gossip_disabled_topics,
            vec!["kukuri:topic:leased".to_string()],
            "history x{count}"
        );
        assert!(status.gossip_disabled_channels.is_empty());
    }
}

/// #1221 R2-D AC-3: 詳細のページは、重複も欠落も無く全件を返し、1 回は上限(64)以内。
#[tokio::test]
async fn connectivity_peer_pages_cover_every_peer_once() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(FakeTransport::new("app", FakeNetwork::default()));
    let seeds = (0..150)
        .map(|index| SeedPeer {
            endpoint_id: format!("seed-{index:03}"),
            addr_hint: None,
        })
        .collect::<Vec<_>>();
    transport
        .configure_discovery(DiscoveryMode::StaticPeer, false, seeds, Vec::new())
        .await
        .expect("seeds");
    let app = AppService::new(store, transport);
    for limit in [None, Some(1_000), Some(7)] {
        let mut cursor = None;
        let mut seen = Vec::new();
        loop {
            let page = app
                .list_connectivity_peers(ConnectivityPeersRequest {
                    kind: ConnectivityPeerKind::ConfiguredSeed,
                    topic: None,
                    cursor: cursor.clone(),
                    limit,
                })
                .await
                .expect("page");
            assert!(page.peer_ids.len() <= limit.unwrap_or(64).min(CONNECTIVITY_PEER_PAGE_LIMIT));
            seen.extend(page.peer_ids);
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        let expected = (0..150)
            .map(|index| format!("seed-{index:03}"))
            .collect::<Vec<_>>();
        assert_eq!(seen, expected, "limit {limit:?}");
    }
}

/// #1221 R2-D AC-4: 差分は印の付いた topic だけを持ち、抜けた topic を別に返す。
#[tokio::test]
async fn sync_status_delta_keeps_only_marked_topics() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(FakeTransport::new("app", FakeNetwork::default()));
    let app = AppService::new(store, transport);
    for topic in ["kukuri:topic:a", "kukuri:topic:b"] {
        display_topic(&app, topic).await.expect("open");
    }
    let keys = BTreeSet::from([
        StatusKey::Summary,
        StatusKey::Topic("hint/kukuri:topic:b".into()),
        StatusKey::Topic("hint/kukuri:topic:gone".into()),
    ]);
    let (status, removed) = app.sync_status_delta(Some(&keys)).await.expect("delta");
    assert_eq!(
        status
            .topic_diagnostics
            .iter()
            .map(|topic| topic.topic.as_str())
            .collect::<Vec<_>>(),
        vec!["kukuri:topic:b"]
    );
    assert_eq!(removed, vec!["kukuri:topic:gone".to_string()]);
    assert_eq!(
        status.subscribed_topics.len(),
        2,
        "the summary keeps the counts"
    );
}

/// #1221 R2-D AC-1・AC-6: 状態の読取りは SQLite を読まない。休止した topic・gossip を止めた設定・seed を
/// 10 倍にしても、返す topic の数は稼働中の分のまま。
#[tokio::test]
async fn status_read_does_not_touch_sqlite_or_grow_with_history() {
    let steps = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let store = Arc::new(
        kukuri_store::SqliteStore::connect_memory_counting_vm_steps(steps.clone())
            .await
            .expect("store"),
    );
    let transport = Arc::new(FakeTransport::new("app", FakeNetwork::default()));
    let app = AppService::new(store, transport.clone());
    display_topic(&app, "kukuri:topic:leased")
        .await
        .expect("leased");
    let mut dormant = 0;
    let mut add_history = async |count: usize| {
        for _ in 0..count {
            let topic = format!("kukuri:topic:dormant-{dormant}");
            display_topic(&app, &topic).await.expect("dormant");
            app.set_scope_display(crate::ScopeDisplayRequest {
                observer: format!("test-topic:{topic}"),
                target: crate::ScopeDisplayTarget::Timeline {
                    topic: topic.clone(),
                    scope: TimelineScope::Public,
                },
                visible: false,
            })
            .await
            .expect("close dormant");
            app.set_topic_gossip_enabled(&topic, false)
                .await
                .expect("disable dormant");
            dormant += 1;
        }
        let seeds = (0..dormant * 10)
            .map(|index| SeedPeer {
                endpoint_id: format!("seed-{index}"),
                addr_hint: None,
            })
            .collect::<Vec<_>>();
        transport
            .configure_discovery(DiscoveryMode::StaticPeer, false, seeds, Vec::new())
            .await
            .expect("seeds");
    };
    let measure = async || {
        let before = steps.load(std::sync::atomic::Ordering::Relaxed);
        let status = app.get_sync_status().await.expect("status");
        assert_eq!(
            steps.load(std::sync::atomic::Ordering::Relaxed),
            before,
            "no SQLite read"
        );
        (
            status.topic_diagnostics.len(),
            status.subscribed_topics.len(),
            status.gossip_disabled_topics.len(),
        )
    };
    add_history(1).await;
    let once = measure().await;
    add_history(9).await;
    assert_eq!(once, (1, 1, 0));
    assert_eq!(measure().await, once);
}
