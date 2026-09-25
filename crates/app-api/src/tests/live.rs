use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[cfg(feature = "iroh-integration-tests")]
async fn late_joiner_backfills_live_session_manifest() {
    let _guard = iroh_integration_test_lock().lock_owned().await;
    let dir = tempdir().expect("tempdir");
    let stack_a = TestIrohStack::new(&dir.path().join("live-a")).await;
    let stack_b = TestIrohStack::new(&dir.path().join("live-b")).await;
    let store_a = Arc::new(MemoryStore::default());
    let store_b = Arc::new(MemoryStore::default());
    let app_a = app_with_iroh_services(store_a, &stack_a);
    let app_b = app_with_iroh_services(store_b, &stack_b);
    let topic = "kukuri:topic:live-late";

    let session_id = app_a
        .create_live_session(
            topic,
            CreateLiveSessionInput {
                title: "late live".into(),
                description: "watch along".into(),
            },
        )
        .await
        .expect("create live session");

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
    app_a.import_peer_ticket(&ticket_b).await.expect("import b");
    app_b.import_peer_ticket(&ticket_a).await.expect("import a");
    display_remote_session(&app_b, topic, &session_id, "live").await;

    let received = timeout(Duration::from_secs(10), async {
        loop {
            let sessions = app_b
                .list_live_sessions(topic)
                .await
                .expect("list live sessions");
            if let Some(session) = sessions
                .into_iter()
                .find(|session| session.session_id == session_id)
            {
                return session;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("live session backfill timeout");

    assert_eq!(received.title, "late live");
    assert_eq!(received.status, LiveSessionStatus::Live);
}

#[tokio::test]
async fn live_presence_expires_without_heartbeat() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(FakeTransport::new("self", FakeNetwork::default()));
    let app = AppService::new(store.clone(), transport.clone());
    let topic = "kukuri:topic:presence-expiry";
    let session_id = app
        .create_live_session(
            topic,
            CreateLiveSessionInput {
                title: "presence".into(),
                description: "ttl".into(),
            },
        )
        .await
        .expect("create live session");

    let sessions = app
        .list_live_sessions(topic)
        .await
        .expect("list live sessions before presence");
    assert!(
        sessions
            .iter()
            .any(|session| session.session_id == session_id),
        "live session should be visible before presence is published"
    );

    transport
        .publish_hint(
            &TopicId::new(topic),
            GossipHint::LivePresence {
                topic_id: TopicId::new(topic),
                session_id: session_id.clone(),
                author: Pubkey::from("a".repeat(64)),
                ttl_ms: 100,
            },
        )
        .await
        .expect("publish live presence");

    timeout(Duration::from_secs(2), async {
        loop {
            let sessions = store
                .list_channel_live_sessions(topic, "public", 100)
                .await
                .expect("list cached live sessions");
            if sessions
                .iter()
                .any(|session| session.session_id == session_id && session.viewer_count == 1)
            {
                break;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("viewer count update timeout");

    sleep(Duration::from_millis(150)).await;
    let sessions = app
        .list_live_sessions(topic)
        .await
        .expect("list after expiry");
    let session = sessions
        .iter()
        .find(|session| session.session_id == session_id)
        .expect("session present");
    assert_eq!(session.viewer_count, 0);
}

// #1292: 一覧の100件窓より古いsessionでも、heartbeat自身が終了projectionを単一行で確認して止まる。
#[tokio::test]
async fn live_presence_heartbeat_stops_when_its_projection_ends() {
    let (viewer, _viewer_keys, owner, _owner_keys, _store, _docs_sync, _blob_service) =
        shared_apps_with_memory_services();
    let topic = "kukuri:topic:ended-heartbeat";
    let session_id = owner
        .create_live_session(
            topic,
            CreateLiveSessionInput {
                title: "heartbeat".into(),
                description: "self stop".into(),
            },
        )
        .await
        .expect("create live session");
    viewer
        .join_live_session(topic, session_id.as_str())
        .await
        .expect("join live session");
    let task_key = live_presence_task_key(topic, PUBLIC_CHANNEL_ID, session_id.as_str());
    assert!(
        viewer
            .subscription_registry
            .live_presence_tasks
            .lock()
            .await
            .contains_key(task_key.as_str())
    );

    owner
        .end_live_session(topic, session_id.as_str())
        .await
        .expect("end live session");

    timeout(Duration::from_secs(12), async {
        loop {
            if !viewer
                .subscription_registry
                .live_presence_tasks
                .lock()
                .await
                .contains_key(task_key.as_str())
            {
                break;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("ended heartbeat task must stop");
}

#[tokio::test]
async fn ended_live_session_rejects_new_viewers() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(FakeTransport::new("self", FakeNetwork::default()));
    let app = AppService::new(store, transport);
    let topic = "kukuri:topic:ended-live";
    let session_id = app
        .create_live_session(
            topic,
            CreateLiveSessionInput {
                title: "ended".into(),
                description: "session".into(),
            },
        )
        .await
        .expect("create live session");
    app.end_live_session(topic, session_id.as_str())
        .await
        .expect("end live session");

    let error = app
        .join_live_session(topic, session_id.as_str())
        .await
        .expect_err("join should fail");
    assert!(error.to_string().contains("ended live session"));
}

#[tokio::test]
async fn muted_author_is_filtered_from_live_and_game_lists() {
    let (local_app, _local_keys, remote_app, remote_keys, _store, _docs_sync, _blob_service) =
        shared_apps_with_memory_services();
    let topic = "kukuri:topic:mute-live-game";
    let remote_pubkey = remote_keys.public_key_hex();

    let session_id = remote_app
        .create_live_session(
            topic,
            CreateLiveSessionInput {
                title: "muted live".into(),
                description: "hidden".into(),
            },
        )
        .await
        .expect("create live session");
    let room_id = remote_app
        .create_game_room(
            topic,
            CreateGameRoomInput {
                title: "muted room".into(),
                description: "hidden".into(),
                participants: vec!["Alice".into(), "Bob".into()],
            },
        )
        .await
        .expect("create game room");

    let live_before = local_app
        .list_live_sessions(topic)
        .await
        .expect("list live sessions before mute");
    let games_before = local_app
        .list_game_rooms(topic)
        .await
        .expect("list game rooms before mute");
    assert!(
        live_before
            .iter()
            .any(|session| session.session_id == session_id)
    );
    assert!(games_before.iter().any(|room| room.room_id == room_id));

    local_app
        .mute_author(remote_pubkey.as_str())
        .await
        .expect("mute live/game host");

    let live_after = local_app
        .list_live_sessions(topic)
        .await
        .expect("list live sessions after mute");
    let games_after = local_app
        .list_game_rooms(topic)
        .await
        .expect("list game rooms after mute");

    assert!(
        live_after
            .iter()
            .all(|session| session.session_id != session_id)
    );
    assert!(games_after.iter().all(|room| room.room_id != room_id));
}

// #961: ブロック関係はどちらの向きでも配信・ゲーム部屋の一覧から host を隠す。
#[tokio::test]
async fn blocked_author_is_filtered_from_live_and_game_lists_in_both_directions() {
    let (local_app, local_keys, remote_app, remote_keys, _store, _docs_sync, _blob_service) =
        shared_apps_with_memory_services();
    let topic = "kukuri:topic:block-live-game";
    let local_pubkey = local_keys.public_key_hex();
    let remote_pubkey = remote_keys.public_key_hex();

    let session_id = remote_app
        .create_live_session(
            topic,
            CreateLiveSessionInput {
                title: "blocked live".into(),
                description: "hidden".into(),
            },
        )
        .await
        .expect("create live session");
    let room_id = remote_app
        .create_game_room(
            topic,
            CreateGameRoomInput {
                title: "blocked room".into(),
                description: "hidden".into(),
                participants: vec!["Alice".into(), "Bob".into()],
            },
        )
        .await
        .expect("create game room");

    let assert_visible = |visible: bool, label: &'static str| {
        let local_app = &local_app;
        let session_id = session_id.clone();
        let room_id = room_id.clone();
        async move {
            let live = local_app
                .list_live_sessions(topic)
                .await
                .unwrap_or_else(|_| panic!("list live sessions {label}"));
            let games = local_app
                .list_game_rooms(topic)
                .await
                .unwrap_or_else(|_| panic!("list game rooms {label}"));
            assert_eq!(
                live.iter().any(|session| session.session_id == session_id),
                visible,
                "live {label}"
            );
            assert_eq!(
                games.iter().any(|room| room.room_id == room_id),
                visible,
                "game {label}"
            );
        }
    };

    assert_visible(true, "before block").await;

    local_app
        .block_author(remote_pubkey.as_str())
        .await
        .expect("local blocks host");
    assert_visible(false, "while blocking").await;
    local_app
        .unblock_author(remote_pubkey.as_str())
        .await
        .expect("local unblocks host");
    assert_visible(true, "after unblock").await;

    remote_app
        .block_author(local_pubkey.as_str())
        .await
        .expect("host blocks local");
    assert_visible(false, "while blocked by host").await;
    remote_app
        .unblock_author(local_pubkey.as_str())
        .await
        .expect("host revokes block");
    assert_visible(true, "after revoke").await;
}

// 視聴者 0 人の配信は正常な状態。一覧の取得で topic の購読 task を張り直さない
// (張り直す間に届いた LivePresence を落とし、5 秒ごとに繰り返すと host の視聴者数が 0 のまま戻らない)。
#[tokio::test]
async fn listing_live_session_without_viewers_keeps_topic_subscription() {
    let store = Arc::new(MemoryStore::default());
    let hint_transport = Arc::new(TrackingHintTransport::default());
    let app = app_service_from_dependencies(
        store.clone(),
        store,
        Arc::new(StaticTransport::new(PeerSnapshot::default())),
        hint_transport.clone(),
        Arc::new(MemoryDocsSync::default()),
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    );
    let topic = "kukuri:topic:live-no-viewers";
    let session_id = app
        .create_live_session(
            topic,
            CreateLiveSessionInput {
                title: "no viewers".into(),
                description: "host only".into(),
            },
        )
        .await
        .expect("create live session");
    let subscribed = *hint_transport.subscribe_count.lock().await;

    let sessions = app
        .list_live_sessions(topic)
        .await
        .expect("list live sessions");
    assert!(
        sessions
            .iter()
            .any(|session| session.session_id == session_id && session.viewer_count == 0)
    );
    assert_eq!(*hint_transport.subscribe_count.lock().await, subscribed);
}
