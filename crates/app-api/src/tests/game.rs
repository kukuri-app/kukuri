use super::*;
use kukuri_core::{
    DomeCustomizationV1, FIXED_DOME_SPEC_ID, METAVERSE_AUDIO_SAMPLE_RATE_HZ,
    METAVERSE_WORLD_VERSION, MetaverseAssetKind, MetaverseRoomChatMessageV1, MetaverseRoomEventV1,
    MetaverseRoomPresenceV1, MetaverseSpatialAudioFrameV1,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[cfg(feature = "iroh-integration-tests")]
async fn game_room_score_update_replicates() {
    let _guard = iroh_integration_test_lock().lock_owned().await;
    let dir = tempdir().expect("tempdir");
    let stack_a = TestIrohStack::new(&dir.path().join("game-a")).await;
    let stack_b = TestIrohStack::new(&dir.path().join("game-b")).await;
    let store_a = Arc::new(MemoryStore::default());
    let store_b = Arc::new(MemoryStore::default());
    let app_a = app_with_iroh_services(store_a, &stack_a);
    let app_b = app_with_iroh_services(store_b, &stack_b);
    let topic = "kukuri:topic:game-sync";
    display_topic_in(&[&app_a, &app_b], topic).await;

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

    let room_id = app_a
        .create_game_room(
            topic,
            CreateGameRoomInput {
                title: "sync room".into(),
                description: "set".into(),
                participants: vec!["Alice".into(), "Bob".into()],
            },
        )
        .await
        .expect("create game room");
    app_a
        .update_game_room(
            topic,
            room_id.as_str(),
            UpdateGameRoomInput {
                status: GameRoomStatus::Running,
                phase_label: Some("Round 2".into()),
                scores: vec![
                    GameScoreView {
                        participant_id: "participant-1".into(),
                        label: "Alice".into(),
                        score: 2,
                    },
                    GameScoreView {
                        participant_id: "participant-2".into(),
                        label: "Bob".into(),
                        score: 1,
                    },
                ],
            },
        )
        .await
        .expect("update game room");

    display_remote_session(&app_b, topic, &room_id, "game").await;

    let received = timeout(Duration::from_secs(60), async {
        loop {
            let rooms = app_b.list_game_rooms(topic).await.expect("list game rooms");
            if let Some(room) = rooms.into_iter().find(|room| room.room_id == room_id) {
                let alice_score = room
                    .scores
                    .iter()
                    .find(|score| score.label == "Alice")
                    .map(|score| score.score);
                if room.status == GameRoomStatus::Running
                    && room.phase_label.as_deref() == Some("Round 2")
                    && alice_score == Some(2)
                {
                    return room;
                }
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("game room replication timeout");

    assert_eq!(received.status, GameRoomStatus::Running);
    assert_eq!(received.phase_label.as_deref(), Some("Round 2"));
    assert_eq!(
        received
            .scores
            .iter()
            .find(|score| score.label == "Alice")
            .map(|score| score.score),
        Some(2)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[cfg(feature = "iroh-integration-tests")]
async fn metaverse_room_events_replicate_between_iroh_peers() {
    let _guard = iroh_integration_test_lock().lock_owned().await;
    let dir = tempdir().expect("tempdir");
    let stack_a = TestIrohStack::new(&dir.path().join("meta-event-a")).await;
    let stack_b = TestIrohStack::new(&dir.path().join("meta-event-b")).await;
    let app_a = app_with_iroh_services(Arc::new(MemoryStore::default()), &stack_a);
    let app_b = app_with_iroh_services(Arc::new(MemoryStore::default()), &stack_b);
    let topic = "kukuri:topic:metaverse-iroh-events";
    display_topic_in(&[&app_a, &app_b], topic).await;

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

    let room_id = app_a
        .create_metaverse_room(
            topic,
            CreateMetaverseRoomInput {
                title: "iroh room".into(),
                description: "event transport".into(),
                max_peers: Some(4),
            },
        )
        .await
        .expect("create metaverse room");

    display_remote_session(&app_b, topic, &room_id, "game").await;

    timeout(Duration::from_secs(60), async {
        loop {
            let rooms = app_b.list_game_rooms(topic).await.expect("list rooms");
            if rooms.iter().any(|room| room.room_id == room_id) {
                return;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("metaverse room discovery timeout");

    app_a
        .publish_metaverse_room_event(
            topic,
            PublishMetaverseRoomEventInput {
                room_id: room_id.clone(),
                peer_id: "peer-a".into(),
                seq: 11,
                event: MetaverseRoomEventV1::ChatMessage {
                    message: MetaverseRoomChatMessageV1 {
                        room_id: room_id.clone(),
                        message_id: "chat-iroh-1".into(),
                        author_peer_id: "peer-a".into(),
                        display_name: Some("Peer A".into()),
                        body: "hello over iroh".into(),
                        created_at: Utc::now().timestamp_millis(),
                    },
                },
            },
        )
        .await
        .expect("publish chat event");

    let received = timeout(Duration::from_secs(60), async {
        loop {
            let events = app_b
                .list_metaverse_room_events(topic, room_id.as_str(), None, Some(32))
                .await
                .expect("list events");
            if let Some(event) = events.into_iter().find(|event| event.content.seq == 11) {
                return event;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("metaverse event replication timeout");

    received.envelope.verify().expect("signed received event");
    assert!(matches!(
        received.content.event,
        MetaverseRoomEventV1::ChatMessage { .. }
    ));
}

#[tokio::test]
async fn finished_game_room_rejects_updates() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(FakeTransport::new("self", FakeNetwork::default()));
    let app = AppService::new(store, transport);
    let topic = "kukuri:topic:game-finished";
    let room_id = app
        .create_game_room(
            topic,
            CreateGameRoomInput {
                title: "finished room".into(),
                description: "set".into(),
                participants: vec!["Alice".into(), "Bob".into()],
            },
        )
        .await
        .expect("create game room");

    app.update_game_room(
        topic,
        room_id.as_str(),
        UpdateGameRoomInput {
            status: GameRoomStatus::Ended,
            phase_label: Some("Final".into()),
            scores: vec![
                GameScoreView {
                    participant_id: "participant-1".into(),
                    label: "Alice".into(),
                    score: 2,
                },
                GameScoreView {
                    participant_id: "participant-2".into(),
                    label: "Bob".into(),
                    score: 0,
                },
            ],
        },
    )
    .await
    .expect("finish room");

    let error = app
        .update_game_room(
            topic,
            room_id.as_str(),
            UpdateGameRoomInput {
                status: GameRoomStatus::Ended,
                phase_label: Some("After".into()),
                scores: vec![
                    GameScoreView {
                        participant_id: "participant-1".into(),
                        label: "Alice".into(),
                        score: 3,
                    },
                    GameScoreView {
                        participant_id: "participant-2".into(),
                        label: "Bob".into(),
                        score: 1,
                    },
                ],
            },
        )
        .await
        .expect_err("ended room update should fail");
    assert!(error.to_string().contains("ended game room"));
}

#[tokio::test]
async fn metaverse_room_uses_game_room_projection_without_scores() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(FakeTransport::new("self", FakeNetwork::default()));
    let app = AppService::new(store, transport);
    let topic = "kukuri:topic:metaverse";

    let room_id = app
        .create_metaverse_room(
            topic,
            CreateMetaverseRoomInput {
                title: "atrium".into(),
                description: "small social space".into(),
                max_peers: Some(8),
            },
        )
        .await
        .expect("create metaverse room");

    let rooms = app.list_game_rooms(topic).await.expect("list rooms");
    let room = rooms
        .into_iter()
        .find(|room| room.room_id == room_id)
        .expect("metaverse room in game projection");
    assert_eq!(room.room_kind, GameRoomKind::MetaverseRoom);
    assert!(room.scores.is_empty());
    assert_eq!(
        room.metaverse.as_ref().and_then(|state| state.max_peers),
        Some(8)
    );
    let mut customization = room
        .metaverse
        .as_ref()
        .expect("metaverse state")
        .dome
        .customization
        .clone();
    assert_eq!(
        room.metaverse.as_ref().map(|state| state.world_version),
        Some(METAVERSE_WORLD_VERSION)
    );
    assert_eq!(
        room.metaverse
            .as_ref()
            .map(|state| state.dome.spec_id.as_str()),
        Some(FIXED_DOME_SPEC_ID)
    );
    assert!(!room.manifest_blob_hash.trim().is_empty());

    customization.environment.gravity_milli = 7_500;
    customization.persistent_props[0].position = [50, 50, -240];

    app.update_metaverse_room(
        topic,
        room_id.as_str(),
        UpdateMetaverseRoomInput {
            status: GameRoomStatus::Running,
            customization,
        },
    )
    .await
    .expect("update metaverse object");

    let updated = app
        .list_game_rooms(topic)
        .await
        .expect("list updated rooms")
        .into_iter()
        .find(|room| room.room_id == room_id)
        .expect("updated metaverse room");
    assert_eq!(updated.status, GameRoomStatus::Running);
    assert_eq!(
        updated
            .metaverse
            .as_ref()
            .map(|state| state.dome.customization.persistent_props[0].position),
        Some([50, 50, -240])
    );
    assert_eq!(
        updated
            .metaverse
            .as_ref()
            .map(|state| state.dome.customization.environment.gravity_milli),
        Some(7_500)
    );
}

#[tokio::test]
async fn metaverse_room_events_are_signed_and_delivered_over_hint_transport() {
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let docs_sync = Arc::new(MemoryDocsSync::default());
    let blob_service = Arc::new(MemoryBlobService::default());
    let store_a = Arc::new(MemoryStore::default());
    let store_b = Arc::new(MemoryStore::default());
    let app_a = app_service_from_dependencies(
        store_a.clone(),
        store_a,
        transport.clone(),
        transport.clone(),
        docs_sync.clone(),
        blob_service.clone(),
        generate_keys(),
    );
    let app_b = app_service_from_dependencies(
        store_b.clone(),
        store_b,
        transport.clone(),
        transport,
        docs_sync,
        blob_service,
        generate_keys(),
    );
    let topic = "kukuri:topic:metaverse-events";

    let room_id = app_a
        .create_metaverse_room(
            topic,
            CreateMetaverseRoomInput {
                title: "events".into(),
                description: "transport".into(),
                max_peers: Some(4),
            },
        )
        .await
        .expect("create metaverse room");
    display_topic(&app_b, topic).await.unwrap();
    app_b.list_game_rooms(topic).await.expect("list rooms");

    let local_event = app_a
        .publish_metaverse_room_event(
            topic,
            PublishMetaverseRoomEventInput {
                room_id: room_id.clone(),
                peer_id: "peer-a".into(),
                seq: 7,
                event: MetaverseRoomEventV1::PresenceJoin {
                    presence: MetaverseRoomPresenceV1 {
                        room_id: room_id.clone(),
                        peer_id: "peer-a".into(),
                        display_name: None,
                        avatar_asset_ref: None,
                        joined_at: Utc::now().timestamp_millis(),
                        last_seen_at: Utc::now().timestamp_millis(),
                    },
                },
            },
        )
        .await
        .expect("publish metaverse event");
    local_event.envelope.verify().expect("signed local event");

    let received = timeout(Duration::from_secs(5), async {
        loop {
            let events = app_b
                .list_metaverse_room_events(topic, room_id.as_str(), None, Some(16))
                .await
                .expect("list metaverse events");
            if let Some(event) = events.into_iter().find(|event| event.content.seq == 7) {
                return event;
            }
            sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("metaverse event delivery timeout");

    received.envelope.verify().expect("signed received event");
    assert_eq!(received.content.room_id, room_id);
    assert_eq!(received.content.peer_id, "peer-a");
    assert!(matches!(
        received.content.event,
        MetaverseRoomEventV1::PresenceJoin { .. }
    ));

    let visitor_pubkey = app_b.current_author_pubkey();
    let owner_pubkey = app_a.current_author_pubkey();
    // owner の block は、owner の profile を開いている間に届く(#1221 R2-C)。
    display_author(&app_b, &owner_pubkey).await.unwrap();
    app_a
        .block_author(visitor_pubkey.as_str())
        .await
        .expect("owner blocks visitor");
    timeout(Duration::from_secs(5), async {
        loop {
            let view = app_b
                .get_author_social_view(owner_pubkey.as_str())
                .await
                .expect("hydrate owner block");
            if view.blocked_by {
                break;
            }
            sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("owner block delivery timeout");
    assert!(
        app_b
            .list_metaverse_room_events(topic, room_id.as_str(), None, Some(16))
            .await
            .expect("blocked room event list")
            .is_empty()
    );
    let now = Utc::now().timestamp_millis();
    assert!(
        app_b
            .publish_metaverse_room_event(
                topic,
                PublishMetaverseRoomEventInput {
                    room_id: room_id.clone(),
                    peer_id: "peer-b".into(),
                    seq: 8,
                    event: MetaverseRoomEventV1::PresenceLeave {
                        room_id,
                        peer_id: "peer-b".into(),
                        left_at: now,
                    },
                },
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn metaverse_chat_messages_persist_to_recent_room_history() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(FakeTransport::new("self", FakeNetwork::default()));
    let app = AppService::new(store, transport);
    let topic = "kukuri:topic:metaverse-chat-history";
    let room_id = app
        .create_metaverse_room(
            topic,
            CreateMetaverseRoomInput {
                title: "chat history".into(),
                description: "recent messages".into(),
                max_peers: Some(4),
            },
        )
        .await
        .expect("create metaverse room");

    app.publish_metaverse_room_event(
        topic,
        PublishMetaverseRoomEventInput {
            room_id: room_id.clone(),
            peer_id: "peer-a".into(),
            seq: 1,
            event: MetaverseRoomEventV1::ChatMessage {
                message: MetaverseRoomChatMessageV1 {
                    room_id: room_id.clone(),
                    message_id: "chat-1".into(),
                    author_peer_id: "peer-a".into(),
                    display_name: Some("Peer A".into()),
                    body: "persistent hello".into(),
                    created_at: Utc::now().timestamp_millis(),
                },
            },
        },
    )
    .await
    .expect("publish chat message");

    let room = app
        .list_game_rooms(topic)
        .await
        .expect("list rooms")
        .into_iter()
        .find(|room| room.room_id == room_id)
        .expect("metaverse room");
    let history = &room.metaverse.as_ref().expect("metaverse").chat_history;
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].message_id, "chat-1");
    assert_eq!(history[0].body, "persistent hello");
}

#[tokio::test]
async fn metaverse_chat_history_keeps_latest_one_hundred_messages() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(FakeTransport::new("self", FakeNetwork::default()));
    let app = AppService::new(store, transport);
    let topic = "kukuri:topic:metaverse-chat-cap";
    let room_id = app
        .create_metaverse_room(
            topic,
            CreateMetaverseRoomInput {
                title: "chat cap".into(),
                description: "recent only".into(),
                max_peers: Some(4),
            },
        )
        .await
        .expect("create metaverse room");

    for seq in 0..105_u64 {
        app.publish_metaverse_room_event(
            topic,
            PublishMetaverseRoomEventInput {
                room_id: room_id.clone(),
                peer_id: "peer-a".into(),
                seq,
                event: MetaverseRoomEventV1::ChatMessage {
                    message: MetaverseRoomChatMessageV1 {
                        room_id: room_id.clone(),
                        message_id: format!("chat-{seq}"),
                        author_peer_id: "peer-a".into(),
                        display_name: None,
                        body: format!("message {seq}"),
                        created_at: Utc::now().timestamp_millis(),
                    },
                },
            },
        )
        .await
        .expect("publish chat message");
    }

    let room = app
        .list_game_rooms(topic)
        .await
        .expect("list rooms")
        .into_iter()
        .find(|room| room.room_id == room_id)
        .expect("metaverse room");
    let history = &room.metaverse.as_ref().expect("metaverse").chat_history;
    assert_eq!(history.len(), 100);
    assert_eq!(
        history.first().map(|message| message.message_id.as_str()),
        Some("chat-5")
    );
    assert_eq!(
        history.last().map(|message| message.message_id.as_str()),
        Some("chat-104")
    );
}

#[tokio::test]
async fn metaverse_room_event_rejects_mismatched_payload_identity() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(FakeTransport::new("self", FakeNetwork::default()));
    let app = AppService::new(store, transport);
    let topic = "kukuri:topic:metaverse-event-identity";

    let room_id = app
        .create_metaverse_room(
            topic,
            CreateMetaverseRoomInput {
                title: "identity".into(),
                description: "reject mismatch".into(),
                max_peers: Some(4),
            },
        )
        .await
        .expect("create metaverse room");

    let error = app
        .publish_metaverse_room_event(
            topic,
            PublishMetaverseRoomEventInput {
                room_id: room_id.clone(),
                peer_id: "peer-a".into(),
                seq: 1,
                event: MetaverseRoomEventV1::PresenceJoin {
                    presence: MetaverseRoomPresenceV1 {
                        room_id,
                        peer_id: "peer-b".into(),
                        display_name: None,
                        avatar_asset_ref: None,
                        joined_at: Utc::now().timestamp_millis(),
                        last_seen_at: Utc::now().timestamp_millis(),
                    },
                },
            },
        )
        .await
        .expect_err("mismatched peer identity should be rejected");

    assert!(
        error
            .to_string()
            .contains("metaverse presence event identity")
    );
}

#[tokio::test]
async fn metaverse_audio_is_ephemeral_and_enforces_the_player_frame_budget() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(FakeTransport::new("self", FakeNetwork::default()));
    let app = AppService::new(store, transport);
    let topic = "kukuri:topic:metaverse-audio-budget";
    let room_id = app
        .create_metaverse_room(
            topic,
            CreateMetaverseRoomInput {
                title: "audio".into(),
                description: "ephemeral frames".into(),
                max_peers: Some(4),
            },
        )
        .await
        .expect("create metaverse room");

    for seq in 0..50_u64 {
        let now = Utc::now().timestamp_millis();
        app.publish_metaverse_room_event(
            topic,
            PublishMetaverseRoomEventInput {
                room_id: room_id.clone(),
                peer_id: "peer-a".into(),
                seq,
                event: MetaverseRoomEventV1::SpatialAudioFrame {
                    frame: MetaverseSpatialAudioFrameV1 {
                        room_id: room_id.clone(),
                        peer_id: "peer-a".into(),
                        position: [0, 100, 0],
                        sample_rate_hz: METAVERSE_AUDIO_SAMPLE_RATE_HZ,
                        samples: vec![0; 320],
                        captured_at: now,
                    },
                },
            },
        )
        .await
        .expect("audio frame within budget");
    }
    let now = Utc::now().timestamp_millis();
    let error = app
        .publish_metaverse_room_event(
            topic,
            PublishMetaverseRoomEventInput {
                room_id: room_id.clone(),
                peer_id: "peer-a".into(),
                seq: 50,
                event: MetaverseRoomEventV1::SpatialAudioFrame {
                    frame: MetaverseSpatialAudioFrameV1 {
                        room_id: room_id.clone(),
                        peer_id: "peer-a".into(),
                        position: [0, 100, 0],
                        sample_rate_hz: METAVERSE_AUDIO_SAMPLE_RATE_HZ,
                        samples: vec![0; 320],
                        captured_at: now,
                    },
                },
            },
        )
        .await
        .expect_err("audio frame rate must be bounded");
    assert!(error.to_string().contains("AUDIO_FRAME_RATE_RATE_EXCEEDED"));

    let room = app
        .list_game_rooms(topic)
        .await
        .expect("list room")
        .into_iter()
        .find(|room| room.room_id == room_id)
        .expect("audio room");
    assert!(room.metaverse.expect("metaverse").chat_history.is_empty());
}

#[tokio::test]
async fn metaverse_room_customization_rejects_non_owner() {
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let docs_sync = Arc::new(MemoryDocsSync::default());
    let blob_service = Arc::new(MemoryBlobService::default());
    let owner_store = Arc::new(MemoryStore::default());
    let peer_store = Arc::new(MemoryStore::default());
    let app_owner = app_service_from_dependencies(
        owner_store.clone(),
        owner_store,
        transport.clone(),
        transport.clone(),
        docs_sync.clone(),
        blob_service.clone(),
        generate_keys(),
    );
    let app_peer = app_service_from_dependencies(
        peer_store.clone(),
        peer_store,
        transport.clone(),
        transport,
        docs_sync,
        blob_service,
        generate_keys(),
    );
    let topic = "kukuri:topic:metaverse-object-non-owner";

    let room_id = app_owner
        .create_metaverse_room(
            topic,
            CreateMetaverseRoomInput {
                title: "shared object".into(),
                description: "participant updates".into(),
                max_peers: Some(4),
            },
        )
        .await
        .expect("create metaverse room");

    let error = app_peer
        .update_metaverse_room(
            topic,
            room_id.as_str(),
            UpdateMetaverseRoomInput {
                status: GameRoomStatus::Waiting,
                customization: DomeCustomizationV1::default(),
            },
        )
        .await
        .expect_err("non-owner customization should fail");
    assert!(error.to_string().contains("owner can update Dome"));
}

#[tokio::test]
async fn metaverse_room_invalid_customization_keeps_current_manifest() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(FakeTransport::new("self", FakeNetwork::default()));
    let app = AppService::new(store, transport);
    let topic = "kukuri:topic:metaverse-invalid-customization";
    let room_id = app
        .create_metaverse_room(
            topic,
            CreateMetaverseRoomInput {
                title: "fixed Dome".into(),
                description: "invalid update".into(),
                max_peers: Some(4),
            },
        )
        .await
        .expect("create room");
    let before = app
        .list_game_rooms(topic)
        .await
        .expect("list before")
        .into_iter()
        .find(|room| room.room_id == room_id)
        .expect("room before");
    let mut invalid = before
        .metaverse
        .as_ref()
        .expect("Dome state")
        .dome
        .customization
        .clone();
    invalid.environment.gravity_milli = 999;

    app.update_metaverse_room(
        topic,
        room_id.as_str(),
        UpdateMetaverseRoomInput {
            status: GameRoomStatus::Running,
            customization: invalid,
        },
    )
    .await
    .expect_err("invalid customization must fail");

    let after = app
        .list_game_rooms(topic)
        .await
        .expect("list after")
        .into_iter()
        .find(|room| room.room_id == room_id)
        .expect("room after");
    assert_eq!(after.manifest_blob_hash, before.manifest_blob_hash);
    assert_eq!(after.status, before.status);
}

#[tokio::test]
async fn metaverse_avatar_asset_imports_to_blob_ref_without_event_bytes() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(FakeTransport::new("self", FakeNetwork::default()));
    let app = AppService::new(store, transport);
    let topic = "kukuri:topic:metaverse-asset";
    let room_id = app
        .create_metaverse_room(
            topic,
            CreateMetaverseRoomInput {
                title: "asset room".into(),
                description: "vrm".into(),
                max_peers: Some(4),
            },
        )
        .await
        .expect("create room");

    let asset = app
        .import_metaverse_room_asset(
            topic,
            ImportMetaverseRoomAssetInput {
                room_id: room_id.clone(),
                kind: MetaverseAssetKind::Vrm,
                mime_type: "model/vrm".into(),
                name: Some("avatar.vrm".into()),
                bytes: minimal_metaverse_glb_bytes(),
            },
        )
        .await
        .expect("import avatar asset");

    assert_eq!(asset.kind, MetaverseAssetKind::Vrm);
    assert_eq!(asset.mime_type.as_deref(), Some("model/vrm"));
    assert_eq!(
        asset.size_bytes,
        asset
            .budget_metadata
            .as_ref()
            .map(|metadata| metadata.stored_bytes)
    );
    assert!(!asset.blob_hash.trim().is_empty());

    let payload = app
        .blob_media_payload(asset.blob_hash.as_str(), "model/vrm")
        .await
        .expect("blob payload")
        .expect("blob payload exists");
    assert_eq!(payload.mime, "model/vrm");
    assert!(!payload.bytes_base64.trim().is_empty());
}

#[tokio::test]
async fn metaverse_room_manifest_restores_after_restart_from_docs_and_blobs() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let docs_sync = Arc::new(MemoryDocsSync::default());
    let blob_service = Arc::new(MemoryBlobService::default());
    let keys = generate_keys();
    let app = app_service_from_dependencies(
        store.clone(),
        store,
        transport.clone(),
        transport.clone(),
        docs_sync.clone(),
        blob_service.clone(),
        keys.clone(),
    );
    let topic = "kukuri:topic:metaverse-restart";
    let room_id = app
        .create_metaverse_room(
            topic,
            CreateMetaverseRoomInput {
                title: "restart room".into(),
                description: "restore".into(),
                max_peers: Some(6),
            },
        )
        .await
        .expect("create room");
    let mut customization = DomeCustomizationV1::default();
    customization.environment.ambient_light_milli = 900;
    customization.persistent_props[0].position = [150, 50, -90];
    customization.persistent_props[0].rotation = [0, 30, 0];
    customization.persistent_props[0].scale = [120, 100, 80];
    app.update_metaverse_room(
        topic,
        room_id.as_str(),
        UpdateMetaverseRoomInput {
            status: GameRoomStatus::Running,
            customization,
        },
    )
    .await
    .expect("update shared object");
    app.publish_metaverse_room_event(
        topic,
        PublishMetaverseRoomEventInput {
            room_id: room_id.clone(),
            peer_id: "peer-a".into(),
            seq: 1,
            event: MetaverseRoomEventV1::ChatMessage {
                message: MetaverseRoomChatMessageV1 {
                    room_id: room_id.clone(),
                    message_id: "restart-chat-1".into(),
                    author_peer_id: "peer-a".into(),
                    display_name: Some("Peer A".into()),
                    body: "restored room chat".into(),
                    created_at: Utc::now().timestamp_millis(),
                },
            },
        },
    )
    .await
    .expect("publish restart chat");

    let restarted_store = Arc::new(MemoryStore::default());
    let restarted = app_service_from_dependencies(
        restarted_store.clone(),
        restarted_store,
        transport,
        Arc::new(NoopHintTransport),
        docs_sync,
        blob_service,
        keys,
    );
    let restored = restarted
        .list_game_rooms(topic)
        .await
        .expect("list rooms after restart")
        .into_iter()
        .find(|room| room.room_id == room_id)
        .expect("restored metaverse room");

    assert_eq!(restored.room_kind, GameRoomKind::MetaverseRoom);
    assert_eq!(restored.status, GameRoomStatus::Running);
    assert_eq!(
        restored
            .metaverse
            .as_ref()
            .map(|state| state.dome.customization.persistent_props[0].position),
        Some([150, 50, -90])
    );
    assert_eq!(
        restored.metaverse.as_ref().map(|state| state
            .dome
            .customization
            .environment
            .ambient_light_milli),
        Some(900)
    );
    assert_eq!(
        restored
            .metaverse
            .as_ref()
            .and_then(|state| state.chat_history.first())
            .map(|message| message.body.as_str()),
        Some("restored room chat")
    );
    assert!(!restored.manifest_blob_hash.trim().is_empty());
}
