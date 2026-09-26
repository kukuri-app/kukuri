//! #1221 R2-C: 購読は 64 件までの lease を持つ key だけに置く。

use super::*;

struct Tracked {
    app: AppService,
    hints: Arc<TrackingHintTransport>,
    docs: Arc<TrackingDocsSync>,
}

fn tracked_app(store: Arc<MemoryStore>, keys: KukuriKeys) -> Tracked {
    let hints = Arc::new(TrackingHintTransport::default());
    let docs = Arc::new(TrackingDocsSync::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        transport,
        hints.clone(),
        docs.clone(),
        Arc::new(MemoryBlobService::default()),
        keys,
    );
    Tracked { app, hints, docs }
}

/// (lease の数、動いている task の数、購読中の hint topic の数、開いている replica の数)
async fn counts(tracked: &Tracked) -> (usize, usize, usize, usize) {
    let leases = tracked.app.subscription_registry.scope_leases.lock().await;
    (
        leases.len(),
        leases.running_tasks(),
        tracked.hints.active_topics.lock().await.len(),
        tracked.docs.open_replicas.lock().await.len(),
    )
}

fn topic_key(topic: &str) -> ScopeKey {
    ScopeKey::Topic(topic.into())
}

fn is_limit(error: &anyhow::Error) -> bool {
    error.downcast_ref::<ScopeLimitReached>().is_some()
}

async fn display(
    app: &AppService,
    observer: &str,
    target: crate::ScopeDisplayTarget,
) -> Result<()> {
    app.set_scope_display(crate::ScopeDisplayRequest {
        observer: observer.into(),
        target,
        visible: true,
    })
    .await
}

fn timeline(topic: &str) -> crate::ScopeDisplayTarget {
    crate::ScopeDisplayTarget::Timeline {
        topic: topic.into(),
        scope: TimelineScope::Public,
    }
}

async fn close(app: &AppService, observer: &str) {
    app.set_scope_display(crate::ScopeDisplayRequest {
        observer: observer.into(),
        target: timeline("unused"),
        visible: false,
    })
    .await
    .expect("close column");
}

async fn fill_to_limit(app: &AppService) {
    for index in 0..MAX_ACTIVE_SCOPES {
        display(
            app,
            &format!("filler-{index}"),
            timeline(&format!("kukuri:topic:filler-{index}")),
        )
        .await
        .expect("fill a column");
    }
}

fn capability(topic: &str, channel: &str, archived_epochs: usize) -> PrivateChannelCapability {
    let owner = generate_keys().public_key_hex();
    PrivateChannelCapability {
        topic_id: topic.into(),
        channel_id: channel.into(),
        label: channel.into(),
        creator_pubkey: owner.clone(),
        owner_pubkey: owner,
        joined_via_pubkey: None,
        audience_kind: ChannelAudienceKind::InviteOnly,
        current_epoch_id: format!("{channel}-current"),
        current_epoch_secret_hex: generate_keys().export_secret_hex(),
        archived_epochs: (0..archived_epochs)
            .map(|index| PrivateChannelEpochCapability {
                epoch_id: format!("{channel}-past-{index}"),
                namespace_secret_hex: generate_keys().export_secret_hex(),
            })
            .collect(),
        rotation_required: false,
        participant_count: 0,
        stale_participant_count: 0,
        namespace_secret_hex: String::new(),
    }
}

#[test]
fn the_ledger_holds_at_most_64_keys_and_never_takes_part_of_a_request() {
    let mut leases = ScopeLeases::default();
    for index in 0..MAX_ACTIVE_SCOPES {
        leases
            .set_holder(
                &format!("holder-{index}"),
                BTreeSet::from([topic_key(&format!("topic-{index}"))]),
            )
            .expect("within the limit");
    }
    assert_eq!(leases.len(), MAX_ACTIVE_SCOPES);
    // 既にある key を別の holder が取っても枠を使わない。
    leases
        .set_holder("shared", BTreeSet::from([topic_key("topic-0")]))
        .expect("an existing key");
    assert_eq!(
        leases
            .set_holder("over", BTreeSet::from([ScopeKey::Author("author".into())]))
            .err(),
        Some(ScopeLimitReached)
    );
    // 1 枠だけ空いた状態で 2 key を求める要求は、どちらも取らない。
    leases
        .set_holder("holder-63", BTreeSet::new())
        .expect("release");
    let pair = BTreeSet::from([
        topic_key("topic-new"),
        ScopeKey::Channel("topic-new".into(), "channel".into()),
    ]);
    assert_eq!(
        leases.set_holder("pair", pair).err(),
        Some(ScopeLimitReached)
    );
    assert_eq!(leases.len(), MAX_ACTIVE_SCOPES - 1);
    assert!(!leases.holds("pair", &topic_key("topic-new")));
    // 拒否された holder は元の key を持ち続ける。
    let three_new = BTreeSet::from([
        topic_key("topic-a"),
        topic_key("topic-b"),
        topic_key("topic-c"),
    ]);
    assert_eq!(
        leases.set_holder("holder-1", three_new).err(),
        Some(ScopeLimitReached)
    );
    assert!(leases.holds("holder-1", &topic_key("topic-1")));
    // 置き換えは自分が外す枠を数える。
    leases
        .set_holder("holder-2", BTreeSet::from([topic_key("topic-a")]))
        .expect("replace");
    assert_eq!(leases.len(), MAX_ACTIVE_SCOPES - 1);
}

#[tokio::test]
async fn the_65th_column_is_rejected_without_subscribing() {
    let tracked = tracked_app(Arc::new(MemoryStore::default()), generate_keys());
    fill_to_limit(&tracked.app).await;
    let error = display(&tracked.app, "over", timeline("kukuri:topic:over"))
        .await
        .expect_err("the 65th scope");
    assert!(is_limit(&error), "{error:#}");
    // 既に開いている topic の列は開ける。
    display(&tracked.app, "same", timeline("kukuri:topic:filler-0"))
        .await
        .expect("an existing scope");
    assert_eq!(
        counts(&tracked).await,
        (
            MAX_ACTIVE_SCOPES,
            MAX_ACTIVE_SCOPES,
            MAX_ACTIVE_SCOPES,
            MAX_ACTIVE_SCOPES
        )
    );
}

#[tokio::test]
async fn reads_and_writes_do_not_start_subscriptions() {
    let tracked = tracked_app(Arc::new(MemoryStore::default()), generate_keys());
    let app = &tracked.app;
    let topic = "kukuri:topic:no-implicit";
    let other = generate_keys().public_key_hex();
    let post = app.create_post(topic, "post", None).await.expect("post");
    app.create_post(topic, "reply", Some(post.as_str()))
        .await
        .expect("reply");
    app.list_timeline(topic, None, 20).await.expect("timeline");
    app.list_thread(topic, post.as_str(), None, 20)
        .await
        .expect("thread");
    app.list_live_sessions(topic).await.expect("live sessions");
    app.list_game_rooms(topic).await.expect("game rooms");
    app.follow_author(&other).await.expect("follow");
    app.get_author_social_view(&other)
        .await
        .expect("social view");
    app.list_profile_timeline(&other, None, 20)
        .await
        .expect("profile timeline");
    app.get_my_profile().await.expect("my profile");
    app.list_joined_private_channels(topic)
        .await
        .expect("joined channels");

    // 書き込みは replica を開きうるが、購読(task・hint・replica の通知)は置かない。
    let (leases, running, hints, _) = counts(&tracked).await;
    assert_eq!((leases, running, hints), (0, 0, 0));
    assert_eq!(*tracked.hints.subscribe_count.lock().await, 0);
    assert!(tracked.docs.subscribe_replicas.lock().await.is_empty());
}

#[tokio::test]
async fn the_last_holder_stops_the_task_leaves_hints_and_closes_the_replica() {
    let tracked = tracked_app(Arc::new(MemoryStore::default()), generate_keys());
    let app = &tracked.app;
    let topic = "kukuri:topic:two-columns";
    display(app, "left", timeline(topic)).await.expect("left");
    display(app, "right", timeline(topic)).await.expect("right");
    assert_eq!(*tracked.hints.subscribe_count.lock().await, 1);

    close(app, "left").await;
    assert!(app.has_topic_subscription(topic).await);
    close(app, "right").await;
    assert!(!app.has_topic_subscription(topic).await);
    assert_eq!(counts(&tracked).await, (0, 0, 0, 0));
    assert_eq!(
        tracked.hints.unsubscribed_topics.lock().await.clone(),
        vec![topic.to_string()]
    );
}

#[tokio::test]
async fn leaving_a_screen_and_ending_participation_are_separate() {
    let store = Arc::new(MemoryStore::default());
    let hints = Arc::new(TrackingHintTransport::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let app = app_service_from_dependencies(
        store.clone(),
        store,
        transport,
        hints,
        Arc::new(MemoryDocsSync::default()),
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    );
    let topic = "kukuri:topic:live-and-column";
    let session = app
        .create_live_session(
            topic,
            CreateLiveSessionInput {
                title: "live".into(),
                description: String::new(),
            },
        )
        .await
        .expect("live session");

    // 列を閉じても、live に参加している間は topic を購読する。
    display(&app, "column", timeline(topic))
        .await
        .expect("open");
    app.join_live_session(topic, &session).await.expect("join");
    close(&app, "column").await;
    assert!(app.has_topic_subscription(topic).await);

    // unsubscribe_topic は列・desired の holder だけを外し、参加は止めない。
    display(&app, "column", timeline(topic))
        .await
        .expect("reopen");
    app.unsubscribe_topic(topic).await.expect("unsubscribe");
    assert!(app.has_topic_subscription(topic).await);
    assert_eq!(
        app.subscription_registry
            .live_presence_tasks
            .lock()
            .await
            .len(),
        1
    );

    // 退出は presence と参加の holder だけを外し、開いている列の購読は続く。
    display(&app, "column", timeline(topic))
        .await
        .expect("reopen");
    app.leave_live_session(topic, &session)
        .await
        .expect("leave");
    assert!(
        app.subscription_registry
            .live_presence_tasks
            .lock()
            .await
            .is_empty()
    );
    assert!(app.has_topic_subscription(topic).await);
    close(&app, "column").await;
    assert!(!app.has_topic_subscription(topic).await);
}

async fn start_with_dormant_history(scale: usize) -> (usize, usize, usize, usize) {
    let store = Arc::new(MemoryStore::default());
    let keys = generate_keys();
    let local = Pubkey::from(keys.public_key_hex());
    // 前の起動までの履歴: 投稿した topic、follow・block した author、過去 epoch。
    let previous = tracked_app(store.clone(), keys.clone());
    for index in 0..scale {
        previous
            .app
            .create_post(&format!("kukuri:topic:dormant-{index}"), "old", None)
            .await
            .expect("dormant post");
    }
    for index in 0..5 * scale {
        let author = Pubkey::from(generate_keys().public_key_hex());
        store
            .upsert_follow_edge(FollowEdge {
                subject_pubkey: local.clone(),
                target_pubkey: author.clone(),
                status: FollowEdgeStatus::Active,
                updated_at: 1,
                envelope_id: EnvelopeId::from(format!("follow-{index}")),
            })
            .await
            .expect("follow edge");
        store
            .upsert_block_edge(BlockEdge {
                subject_pubkey: Pubkey::from(generate_keys().public_key_hex()),
                target_pubkey: local.clone(),
                status: BlockEdgeStatus::Active,
                updated_at: 1,
                envelope_id: EnvelopeId::from(format!("block-{index}")),
            })
            .await
            .expect("block edge");
    }
    previous.app.shutdown().await;

    // 起動: 参加中の private channel(現 epoch だけ)と desired を復元する。
    let tracked = tracked_app(store, keys);
    for channel in ["channel-a", "channel-b"] {
        tracked
            .app
            .restore_private_channel_capability(capability(
                "kukuri:topic:joined",
                channel,
                5 * scale,
            ))
            .await
            .expect("restore capability");
    }
    tracked
        .app
        .reconcile_blocked_dome_connections_at_start()
        .await
        .expect("startup reconcile");
    tracked
        .app
        .set_desired_scope("kukuri:topic:desired", &TimelineScope::Public, true)
        .await
        .expect("desired");
    let counts = counts(&tracked).await;
    tracked.app.shutdown().await;
    counts
}

#[tokio::test]
async fn startup_with_ten_times_the_dormant_history_subscribes_only_the_leases() {
    let base = start_with_dormant_history(1).await;
    let ten_times = start_with_dormant_history(10).await;
    // 2 channel(現 epoch)と desired の 1 topic だけ。task・hint・replica は lease の数と一致する。
    assert_eq!(base, (3, 3, 3, 3));
    assert_eq!(ten_times, base);
}

#[tokio::test]
async fn endpoint_rebuild_recreates_only_leased_tasks_and_peer_changes_do_not() {
    let tracked = tracked_app(Arc::new(MemoryStore::default()), generate_keys());
    let app = &tracked.app;
    let author = generate_keys().public_key_hex();
    display(app, "timeline", timeline("kukuri:topic:leased"))
        .await
        .expect("timeline");
    display(
        app,
        "profile",
        crate::ScopeDisplayTarget::Author {
            pubkey: author.clone(),
        },
    )
    .await
    .expect("profile");
    app.restore_private_channel_capability(capability("kukuri:topic:leased", "channel", 3))
        .await
        .expect("joined channel");
    // lease の無い対象。
    app.create_post("kukuri:topic:unleased", "post", None)
        .await
        .expect("post");
    app.follow_author(&generate_keys().public_key_hex())
        .await
        .expect("follow");
    let before = tracked.docs.subscribe_replicas.lock().await.len();
    assert_eq!(before, 3);

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
    .expect("seeds");
    app.import_peer_ticket("peer-ticket").await.expect("ticket");
    assert_eq!(*tracked.hints.subscribe_count.lock().await, 2);
    assert_eq!(tracked.docs.subscribe_replicas.lock().await.len(), before);

    app.rebuild_scope_subscriptions().await.expect("rebuild");
    assert_eq!(*tracked.hints.subscribe_count.lock().await, 4);
    let mut resubscribed = tracked.docs.subscribe_replicas.lock().await[before..].to_vec();
    resubscribed.sort();
    let mut leased = tracked.docs.subscribe_replicas.lock().await[..before].to_vec();
    leased.sort();
    assert_eq!(resubscribed, leased);
    assert!(
        !leased.contains(
            &topic_replica_id("kukuri:topic:unleased")
                .as_str()
                .to_string()
        )
    );
    let (leases, running, hints, _) = counts(&tracked).await;
    assert_eq!((leases, running, hints), (3, 3, 2));
}

#[tokio::test(start_paused = true)]
async fn shutdown_leaves_no_presence_heartbeat_or_subscription_running() {
    let store = Arc::new(MemoryStore::default());
    let hints = Arc::new(TrackingHintTransport::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let app = app_service_from_dependencies(
        store.clone(),
        store,
        transport,
        hints.clone(),
        Arc::new(MemoryDocsSync::default()),
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    );
    let topic = "kukuri:topic:shutdown-participation";
    let room = app
        .create_metaverse_room(
            topic,
            CreateMetaverseRoomInput {
                title: "Dome".into(),
                description: String::new(),
                max_peers: Some(8),
            },
        )
        .await
        .expect("Dome");
    app.start_owner_dome_hosting(crate::StartOwnerDomeHostingInput {
        expected_generation: None,
        spatial_context: kukuri_core::SpatialContextV1::Topic {
            topic_id: TopicId::new(topic),
        },
        instance_id: room,
        endpoint_id: "owner-endpoint".into(),
        lease_duration_millis: 60_000,
    })
    .await
    .expect("hosting");
    let session = app
        .create_live_session(
            topic,
            CreateLiveSessionInput {
                title: "live".into(),
                description: String::new(),
            },
        )
        .await
        .expect("live");
    app.join_live_session(topic, &session).await.expect("join");
    display(&app, "column", timeline(topic))
        .await
        .expect("column");
    assert_eq!(
        app.subscription_registry.dome_heartbeats.lock().await.len(),
        1
    );

    app.shutdown().await;
    let published = hints.published_count.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_secs(60)).await;
    assert_eq!(hints.published_count.load(Ordering::SeqCst), published);
    assert!(
        app.subscription_registry
            .live_presence_tasks
            .lock()
            .await
            .is_empty()
    );
    assert!(
        app.subscription_registry
            .dome_heartbeats
            .lock()
            .await
            .is_empty()
    );
    assert_eq!(app.subscription_registry.scope_leases.lock().await.len(), 0);
}

#[tokio::test]
async fn participation_over_the_limit_is_refused_without_saving_state() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let hints = Arc::new(TrackingHintTransport::default());
    let app = app_service_from_dependencies(
        store.clone(),
        store,
        transport,
        hints.clone(),
        Arc::new(MemoryDocsSync::default()),
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    );
    let topic = "kukuri:topic:over-participation";
    let session = app
        .create_live_session(
            topic,
            CreateLiveSessionInput {
                title: "live".into(),
                description: String::new(),
            },
        )
        .await
        .expect("live");
    fill_to_limit(&app).await;

    let error = app
        .join_live_session(topic, &session)
        .await
        .expect_err("live over the limit");
    assert!(is_limit(&error), "{error:#}");
    assert!(
        app.subscription_registry
            .live_presence_tasks
            .lock()
            .await
            .is_empty()
    );

    let error = app
        .create_private_channel(CreatePrivateChannelInput {
            topic_id: TopicId::new(topic),
            label: "over".into(),
            audience_kind: ChannelAudienceKind::InviteOnly,
        })
        .await
        .expect_err("channel over the limit");
    assert!(is_limit(&error), "{error:#}");
    assert!(app.joined_private_channels.lock().await.is_empty());

    // 列を 1 つ閉じれば参加でき、退出で参加の holder を外す。
    close(&app, "filler-0").await;
    let channel = app
        .create_private_channel(CreatePrivateChannelInput {
            topic_id: TopicId::new(topic),
            label: "joined".into(),
            audience_kind: ChannelAudienceKind::InviteOnly,
        })
        .await
        .expect("channel");
    assert_eq!(
        app.subscription_registry.scope_leases.lock().await.len(),
        MAX_ACTIVE_SCOPES
    );
    let channel_hints = private_channel_hint_topic(&channel.channel_id);
    assert!(
        hints
            .active_topics
            .lock()
            .await
            .contains(channel_hints.as_str())
    );
    app.leave_private_channel(topic, &channel.channel_id)
        .await
        .expect("leave");
    assert!(
        !hints
            .active_topics
            .lock()
            .await
            .contains(channel_hints.as_str())
    );
    assert_eq!(
        app.subscription_registry.scope_leases.lock().await.len(),
        MAX_ACTIVE_SCOPES - 1
    );
}
