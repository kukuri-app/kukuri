use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn late_joiner_backfills_timeline_from_docs() {
    let _resource = lock_test_resource(TestResource::IrohNetwork).await;
    let dir = tempdir().expect("tempdir");
    let db_a = dir.path().join("late-a.db");
    let db_b = dir.path().join("late-b.db");
    let runtime_a = DesktopRuntime::new_with_config_and_identity(
        &db_a,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime a");
    let topic = "kukuri:topic:late-join";
    let object_id = runtime_a
        .create_post(CreatePostRequest {
            topic: topic.into(),
            content: "hello from before join".into(),
            reply_to: None,
            channel_ref: ChannelRef::Public,
            attachments: vec![],
            content_labels: Vec::new(),
        })
        .await
        .expect("create post before join");
    let ticket_a = runtime_a
        .local_peer_ticket()
        .await
        .expect("ticket a")
        .expect("ticket a value");

    let runtime_b = DesktopRuntime::new_with_config_and_identity(
        &db_b,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime b");
    runtime_b
        .import_peer_ticket(ImportPeerTicketRequest { ticket: ticket_a })
        .await
        .expect("import a into b");

    let received = timeout(Duration::from_secs(10), async {
        loop {
            let timeline = runtime_b
                .list_timeline(ListTimelineRequest {
                    topic: topic.into(),
                    scope: TimelineScope::Public,
                    cursor: None,
                    limit: Some(20),
                })
                .await
                .expect("timeline b");
            // 行は docs の索引から反映され、本文(blob)は背景の取得で後から入ることがある(#1239)。
            // 本文が入るまで待つ。
            if let Some(post) = timeline.items.iter().find(|post| {
                post.object_id == object_id && post.content == "hello from before join"
            }) {
                return post.clone();
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("late join timeout");

    assert_eq!(received.content, "hello from before join");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn late_joiner_backfills_image_post_from_docs() {
    let _resource = lock_test_resource(TestResource::IrohNetwork).await;
    let dir = tempdir().expect("tempdir");
    let db_a = dir.path().join("late-image-a.db");
    let db_b = dir.path().join("late-image-b.db");
    let runtime_a = DesktopRuntime::new_with_config_and_identity(
        &db_a,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime a");
    let topic = "kukuri:topic:late-image-runtime";
    let object_id = runtime_a
        .create_post(CreatePostRequest {
            topic: topic.into(),
            content: "late image".into(),
            reply_to: None,
            channel_ref: ChannelRef::Public,
            attachments: vec![image_attachment_request(
                "late.png",
                "image/png",
                b"late-image-runtime",
            )],
            content_labels: Vec::new(),
        })
        .await
        .expect("create image post before join");
    let ticket_a = runtime_a
        .local_peer_ticket()
        .await
        .expect("ticket a")
        .expect("ticket a value");

    let runtime_b = DesktopRuntime::new_with_config_and_identity(
        &db_b,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime b");
    runtime_b
        .import_peer_ticket(ImportPeerTicketRequest { ticket: ticket_a })
        .await
        .expect("import a into b");

    let received = timeout(Duration::from_secs(10), async {
        loop {
            let timeline = runtime_b
                .list_timeline(ListTimelineRequest {
                    topic: topic.into(),
                    scope: TimelineScope::Public,
                    cursor: None,
                    limit: Some(20),
                })
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
    .expect("late image timeout");

    assert_eq!(received.attachments.len(), 1);
    let preview = runtime_b
        .get_blob_preview_url(GetBlobPreviewRequest {
            hash: received.attachments[0].hash.clone(),
            mime: received.attachments[0].mime.clone(),
            metaverse_kind: None,
        })
        .await
        .expect("blob preview");
    assert!(preview.is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn late_joiner_backfills_video_media_payload() {
    let _resource = lock_test_resource(TestResource::IrohNetwork).await;
    let dir = tempdir().expect("tempdir");
    let db_a = dir.path().join("late-video-a.db");
    let db_b = dir.path().join("late-video-b.db");
    let runtime_a = DesktopRuntime::new_with_config_and_identity(
        &db_a,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime a");
    let topic = "kukuri:topic:late-video-runtime";
    let object_id = runtime_a
        .create_post(CreatePostRequest {
            topic: topic.into(),
            content: "late video".into(),
            reply_to: None,
            channel_ref: ChannelRef::Public,
            attachments: vec![
                video_attachment_request(
                    "late-video.mp4",
                    "video/mp4",
                    b"late-video-runtime",
                    "video_manifest",
                ),
                video_attachment_request(
                    "late-poster.jpg",
                    "image/jpeg",
                    b"late-video-poster",
                    "video_poster",
                ),
            ],
            content_labels: Vec::new(),
        })
        .await
        .expect("create video post before join");
    let ticket_a = runtime_a
        .local_peer_ticket()
        .await
        .expect("ticket a")
        .expect("ticket a value");

    let runtime_b = DesktopRuntime::new_with_config_and_identity(
        &db_b,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime b");
    runtime_b
        .import_peer_ticket(ImportPeerTicketRequest { ticket: ticket_a })
        .await
        .expect("import a into b");

    let received = timeout(Duration::from_secs(10), async {
        loop {
            let timeline = runtime_b
                .list_timeline(ListTimelineRequest {
                    topic: topic.into(),
                    scope: TimelineScope::Public,
                    cursor: None,
                    limit: Some(20),
                })
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
    .expect("late video timeout");

    let poster = received
        .attachments
        .iter()
        .find(|attachment| attachment.role == "video_poster")
        .expect("video poster");
    let preview = runtime_b
        .get_blob_media_payload(GetBlobMediaRequest {
            hash: poster.hash.clone(),
            mime: poster.mime.clone(),
        })
        .await
        .expect("video poster payload");
    assert!(preview.is_some());
    let manifest = received
        .attachments
        .iter()
        .find(|attachment| attachment.role == "video_manifest")
        .expect("video manifest");
    let playback = runtime_b
        .get_blob_media_payload(GetBlobMediaRequest {
            hash: manifest.hash.clone(),
            mime: manifest.mime.clone(),
        })
        .await
        .expect("video playback payload");
    assert!(playback.is_some());
}

#[tokio::test]
async fn blob_media_payload_roundtrip() {
    let _resource = lock_test_resource(TestResource::IrohNetwork).await;
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("blob-media-roundtrip.db");
    let runtime = DesktopRuntime::new_with_config_and_identity(
        &db_path,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime");
    let topic = "kukuri:topic:blob-media-roundtrip";
    let object_id = runtime
        .create_post(CreatePostRequest {
            topic: topic.into(),
            content: "roundtrip".into(),
            reply_to: None,
            channel_ref: ChannelRef::Public,
            attachments: vec![image_attachment_request(
                "roundtrip.png",
                "image/png",
                b"blob-media-roundtrip",
            )],
            content_labels: Vec::new(),
        })
        .await
        .expect("create image post");
    let timeline = runtime
        .list_timeline(ListTimelineRequest {
            topic: topic.into(),
            scope: TimelineScope::Public,
            cursor: None,
            limit: Some(20),
        })
        .await
        .expect("timeline");
    let created = timeline
        .items
        .iter()
        .find(|post| post.object_id == object_id)
        .expect("created post");

    let payload = runtime
        .get_blob_media_payload(GetBlobMediaRequest {
            hash: created.attachments[0].hash.clone(),
            mime: created.attachments[0].mime.clone(),
        })
        .await
        .expect("blob media payload")
        .expect("blob media payload present");

    assert_eq!(payload.mime, "image/png");
    assert_eq!(
        payload.bytes_base64,
        BASE64_STANDARD.encode(b"blob-media-roundtrip")
    );
}

#[tokio::test]
async fn blank_blob_media_hash_returns_none_without_panicking() {
    let _resource = lock_test_resource(TestResource::IrohNetwork).await;
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("blank-blob-media-hash.db");
    let runtime = DesktopRuntime::new_with_config_and_identity(
        &db_path,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime");

    let payload = runtime
        .get_blob_media_payload(GetBlobMediaRequest {
            hash: "   ".into(),
            mime: "image/png".into(),
        })
        .await
        .expect("blank hash payload");

    assert!(payload.is_none());
}

#[tokio::test]
async fn sqlite_deletion_does_not_lose_shared_state() {
    let _resource = lock_test_resource(TestResource::IrohNetwork).await;
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("delete-sqlite.db");
    let runtime = DesktopRuntime::new_with_config_and_identity(
        &db_path,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime");
    let topic = "kukuri:topic:sqlite-delete";
    let root_id = runtime
        .create_post(CreatePostRequest {
            topic: topic.into(),
            content: "root".into(),
            reply_to: None,
            channel_ref: ChannelRef::Public,
            attachments: vec![],
            content_labels: Vec::new(),
        })
        .await
        .expect("root post");
    let reply_id = runtime
        .create_post(CreatePostRequest {
            topic: topic.into(),
            content: "reply".into(),
            reply_to: Some(root_id.clone()),
            channel_ref: ChannelRef::Public,
            attachments: vec![],
            content_labels: Vec::new(),
        })
        .await
        .expect("reply post");
    runtime.shutdown().await;
    drop(runtime);
    delete_sqlite_artifacts(&db_path);

    let restarted = DesktopRuntime::new_with_config_and_identity(
        &db_path,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("restart");
    let timeline = restarted
        .list_timeline(ListTimelineRequest {
            topic: topic.into(),
            scope: TimelineScope::Public,
            cursor: None,
            limit: Some(20),
        })
        .await
        .expect("timeline");
    let thread = restarted
        .list_thread(ListThreadRequest {
            topic: topic.into(),
            thread_id: root_id.clone(),
            cursor: None,
            limit: Some(20),
        })
        .await
        .expect("thread");

    assert!(timeline.items.iter().any(|post| post.object_id == root_id));
    assert!(timeline.items.iter().any(|post| post.object_id == reply_id));
    assert!(thread.items.iter().any(|post| post.object_id == root_id));
    assert!(thread.items.iter().any(|post| post.object_id == reply_id));
}

#[tokio::test]
async fn restart_restores_from_docs_blobs_without_sqlite_seed() {
    let _resource = lock_test_resource(TestResource::IrohNetwork).await;
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("restart-no-seed.db");
    let runtime = DesktopRuntime::new_with_config_and_identity(
        &db_path,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime");
    let topic = "kukuri:topic:restart-no-seed";
    let object_id = runtime
        .create_post(CreatePostRequest {
            topic: topic.into(),
            content: "restored from docs".into(),
            reply_to: None,
            channel_ref: ChannelRef::Public,
            attachments: vec![],
            content_labels: Vec::new(),
        })
        .await
        .expect("create post");
    runtime.shutdown().await;
    drop(runtime);
    delete_sqlite_artifacts(&db_path);

    let restarted = DesktopRuntime::new_with_config_and_identity(
        &db_path,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("restart");
    let timeline = restarted
        .list_timeline(ListTimelineRequest {
            topic: topic.into(),
            scope: TimelineScope::Public,
            cursor: None,
            limit: Some(20),
        })
        .await
        .expect("timeline");

    let restored = timeline
        .items
        .iter()
        .find(|post| post.object_id == object_id)
        .expect("restored post");
    assert_eq!(restored.content, "restored from docs");
}

#[tokio::test]
async fn restart_restores_image_post_preview() {
    let _resource = lock_test_resource(TestResource::IrohNetwork).await;
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("restart-image.db");
    let runtime = DesktopRuntime::new_with_config_and_identity(
        &db_path,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime");
    let topic = "kukuri:topic:restart-image";
    let object_id = runtime
        .create_post(CreatePostRequest {
            topic: topic.into(),
            content: "restored image".into(),
            reply_to: None,
            channel_ref: ChannelRef::Public,
            attachments: vec![image_attachment_request(
                "restored.png",
                "image/png",
                b"restart-image-preview",
            )],
            content_labels: Vec::new(),
        })
        .await
        .expect("create image post");
    runtime.shutdown().await;
    drop(runtime);
    delete_sqlite_artifacts(&db_path);

    let restarted = DesktopRuntime::new_with_config_and_identity(
        &db_path,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("restart");
    let timeline = restarted
        .list_timeline(ListTimelineRequest {
            topic: topic.into(),
            scope: TimelineScope::Public,
            cursor: None,
            limit: Some(20),
        })
        .await
        .expect("timeline");
    let restored = timeline
        .items
        .iter()
        .find(|post| post.object_id == object_id)
        .expect("restored image post");

    assert_eq!(restored.attachments.len(), 1);
    let preview = restarted
        .get_blob_preview_url(GetBlobPreviewRequest {
            hash: restored.attachments[0].hash.clone(),
            mime: restored.attachments[0].mime.clone(),
            metaverse_kind: None,
        })
        .await
        .expect("preview after restart");
    assert!(preview.is_some());
}

#[tokio::test]
async fn restart_restores_video_media_payload() {
    let _resource = lock_test_resource(TestResource::IrohNetwork).await;
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("restart-video.db");
    let runtime = DesktopRuntime::new_with_config_and_identity(
        &db_path,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime");
    let topic = "kukuri:topic:restart-video";
    let object_id = runtime
        .create_post(CreatePostRequest {
            topic: topic.into(),
            content: "restored video".into(),
            reply_to: None,
            channel_ref: ChannelRef::Public,
            attachments: vec![
                video_attachment_request(
                    "clip.mp4",
                    "video/mp4",
                    b"restart-video-manifest",
                    "video_manifest",
                ),
                video_attachment_request(
                    "clip-poster.jpg",
                    "image/jpeg",
                    b"restart-video-poster",
                    "video_poster",
                ),
            ],
            content_labels: Vec::new(),
        })
        .await
        .expect("create video post");
    runtime.shutdown().await;
    drop(runtime);
    delete_sqlite_artifacts(&db_path);

    let restarted = DesktopRuntime::new_with_config_and_identity(
        &db_path,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("restart");
    let timeline = restarted
        .list_timeline(ListTimelineRequest {
            topic: topic.into(),
            scope: TimelineScope::Public,
            cursor: None,
            limit: Some(20),
        })
        .await
        .expect("timeline");
    let restored = timeline
        .items
        .iter()
        .find(|post| post.object_id == object_id)
        .expect("restored video post");

    let poster = restored
        .attachments
        .iter()
        .find(|attachment| attachment.role == "video_poster")
        .expect("restored poster");
    let preview = restarted
        .get_blob_media_payload(GetBlobMediaRequest {
            hash: poster.hash.clone(),
            mime: poster.mime.clone(),
        })
        .await
        .expect("video payload after restart");
    assert!(preview.is_some());
    let manifest = restored
        .attachments
        .iter()
        .find(|attachment| attachment.role == "video_manifest")
        .expect("restored video manifest");
    let playback = restarted
        .get_blob_media_payload(GetBlobMediaRequest {
            hash: manifest.hash.clone(),
            mime: manifest.mime.clone(),
        })
        .await
        .expect("video playback payload after restart");
    assert!(playback.is_some());
}

#[tokio::test]
async fn restart_restores_live_session_manifest() {
    let _resource = lock_test_resource(TestResource::IrohNetwork).await;
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("restart-live.db");
    let runtime = DesktopRuntime::new_with_config_and_identity(
        &db_path,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime");
    let topic = "kukuri:topic:restart-live";
    let session_id = runtime
        .create_live_session(CreateLiveSessionRequest {
            topic: topic.into(),
            channel_ref: ChannelRef::Public,
            title: "restart live".into(),
            description: "session".into(),
        })
        .await
        .expect("create live session");
    runtime
        .join_live_session(LiveSessionCommandRequest {
            topic: topic.into(),
            session_id: session_id.clone(),
        })
        .await
        .expect("join live session");
    runtime
        .end_live_session(LiveSessionCommandRequest {
            topic: topic.into(),
            session_id: session_id.clone(),
        })
        .await
        .expect("end live session");
    runtime.shutdown().await;
    drop(runtime);
    delete_sqlite_artifacts(&db_path);

    let restarted = DesktopRuntime::new_with_config_and_identity(
        &db_path,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("restart");
    let sessions = restarted
        .list_live_sessions(ListLiveSessionsRequest {
            topic: topic.into(),
            scope: TimelineScope::Public,
        })
        .await
        .expect("list live sessions");
    let restored = sessions
        .iter()
        .find(|session| session.session_id == session_id)
        .expect("restored live session");
    assert_eq!(restored.status, kukuri_core::LiveSessionStatus::Ended);
    assert_eq!(restored.viewer_count, 0);
    assert!(!restored.joined_by_me);
}

#[tokio::test]
async fn restart_restores_game_room_manifest() {
    let _resource = lock_test_resource(TestResource::IrohNetwork).await;
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("restart-game.db");
    let runtime = DesktopRuntime::new_with_config_and_identity(
        &db_path,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime");
    let topic = "kukuri:topic:restart-game";
    let room_id = runtime
        .create_game_room(CreateGameRoomRequest {
            topic: topic.into(),
            channel_ref: ChannelRef::Public,
            title: "restart finals".into(),
            description: "set".into(),
            participants: vec!["Alice".into(), "Bob".into()],
        })
        .await
        .expect("create game room");
    runtime
        .update_game_room(UpdateGameRoomRequest {
            topic: topic.into(),
            room_id: room_id.clone(),
            status: GameRoomStatus::Running,
            phase_label: Some("Round 3".into()),
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
        })
        .await
        .expect("update game room");
    runtime.shutdown().await;
    drop(runtime);
    delete_sqlite_artifacts(&db_path);

    let restarted = DesktopRuntime::new_with_config_and_identity(
        &db_path,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("restart");
    let rooms = restarted
        .list_game_rooms(ListGameRoomsRequest {
            topic: topic.into(),
            scope: TimelineScope::Public,
        })
        .await
        .expect("list game rooms");
    let restored = rooms
        .iter()
        .find(|room| room.room_id == room_id)
        .expect("restored game room");
    assert_eq!(restored.status, GameRoomStatus::Running);
    assert_eq!(restored.phase_label.as_deref(), Some("Round 3"));
    assert_eq!(
        restored
            .scores
            .iter()
            .find(|score| score.label == "Alice")
            .map(|score| score.score),
        Some(2)
    );
}
