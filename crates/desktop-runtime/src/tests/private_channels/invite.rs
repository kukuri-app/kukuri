use super::super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn preview_channel_access_token_is_non_mutating() {
    let _resource = lock_test_resource(TestResource::IrohNetwork).await;
    let dir = tempdir().expect("tempdir");
    let db_a = dir.path().join("preview-runtime-a.db");
    let db_b = dir.path().join("preview-runtime-b.db");
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
    let topic = "kukuri:topic:desktop-preview-channel";

    open_topic_column(&runtime_a, topic, TimelineScope::Public)
        .await
        .expect("subscribe a");

    let channel = runtime_a
        .create_private_channel(CreatePrivateChannelRequest {
            topic: topic.into(),
            label: "preview".into(),
            audience_kind: ChannelAudienceKind::InviteOnly,
        })
        .await
        .expect("create private channel");
    let invite = runtime_a
        .export_private_channel_invite(ExportPrivateChannelInviteRequest {
            topic: topic.into(),
            channel_id: channel.channel_id.clone(),
            expires_at: None,
        })
        .await
        .expect("export invite");

    let joined_before = runtime_b
        .list_joined_private_channels(ListJoinedPrivateChannelsRequest {
            topic: topic.into(),
        })
        .await
        .expect("joined before preview");
    assert!(
        joined_before.is_empty(),
        "preview should not require pre-existing joined state"
    );

    let preview = runtime_b
        .preview_channel_access_token(PreviewChannelAccessTokenRequest { token: invite })
        .await
        .expect("preview invite");
    assert_eq!(preview.kind, kukuri_app_api::ChannelAccessTokenKind::Invite);
    assert_eq!(preview.topic_id, topic);
    assert_eq!(preview.channel_id, channel.channel_id);

    let joined_after = runtime_b
        .list_joined_private_channels(ListJoinedPrivateChannelsRequest {
            topic: topic.into(),
        })
        .await
        .expect("joined after preview");
    assert!(
        joined_after.is_empty(),
        "preview must not mutate runtime state"
    );

    let invalid = runtime_b
        .preview_channel_access_token(PreviewChannelAccessTokenRequest {
            token: "not-a-token".into(),
        })
        .await;
    assert!(invalid.is_err(), "invalid tokens should fail preview");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn private_channel_import_without_local_posts_restores_after_restart() {
    let _resource = lock_test_resource(TestResource::IrohNetwork).await;
    let dir = tempdir().expect("tempdir");
    let db_a = dir.path().join("private-import-owner.db");
    let db_b = dir.path().join("private-import-joiner.db");
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
        .import_peer_ticket(ImportPeerTicketRequest { ticket: ticket_b })
        .await
        .expect("import b");
    runtime_b
        .import_peer_ticket(ImportPeerTicketRequest { ticket: ticket_a })
        .await
        .expect("import a");

    let topic = "kukuri:topic:desktop-private-import-no-posts";
    open_topic_column(&runtime_a, topic, TimelineScope::Public)
        .await
        .expect("subscribe a");
    open_topic_column(&runtime_b, topic, TimelineScope::Public)
        .await
        .expect("subscribe b");
    wait_for_topic_delivery(&runtime_a, topic, 1, "import owner topic delivery timeout").await;
    wait_for_topic_delivery(&runtime_b, topic, 1, "import joiner topic delivery timeout").await;

    let channel = runtime_a
        .create_private_channel(CreatePrivateChannelRequest {
            topic: topic.into(),
            label: "no-post-import".into(),
            audience_kind: ChannelAudienceKind::InviteOnly,
        })
        .await
        .expect("create private channel");
    let invite = runtime_a
        .export_private_channel_invite(ExportPrivateChannelInviteRequest {
            topic: topic.into(),
            channel_id: channel.channel_id.clone(),
            expires_at: None,
        })
        .await
        .expect("export invite");

    let imported = runtime_b
        .import_private_channel_invite(ImportPrivateChannelInviteRequest { token: invite })
        .await
        .expect("import invite");
    assert_eq!(imported.topic_id.as_str(), topic);
    assert_eq!(imported.channel_id.as_str(), channel.channel_id);

    let joined_before_restart = runtime_b
        .list_joined_private_channels(ListJoinedPrivateChannelsRequest {
            topic: topic.into(),
        })
        .await
        .expect("list joined before restart");
    assert_eq!(joined_before_restart.len(), 1);
    assert_eq!(joined_before_restart[0].channel_id, channel.channel_id);

    timeout(runtime_shutdown_timeout(), runtime_a.shutdown())
        .await
        .expect("runtime a shutdown timeout");
    timeout(runtime_shutdown_timeout(), runtime_b.shutdown())
        .await
        .expect("runtime b shutdown timeout");
    drop(runtime_a);
    drop(runtime_b);

    let restarted_b = DesktopRuntime::new_with_config_and_identity(
        &db_b,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("restart runtime b");
    let joined_after_restart = restarted_b
        .list_joined_private_channels(ListJoinedPrivateChannelsRequest {
            topic: topic.into(),
        })
        .await
        .expect("list joined after restart");
    assert_eq!(joined_after_restart.len(), 1);
    assert_eq!(joined_after_restart[0].channel_id, channel.channel_id);
    assert_eq!(joined_after_restart[0].label, "no-post-import");

    timeout(runtime_shutdown_timeout(), restarted_b.shutdown())
        .await
        .expect("restarted runtime shutdown timeout");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn private_channel_invite_restores_after_restart_without_reimport() {
    let _resource = lock_test_resource(TestResource::IrohNetwork).await;
    let dir = tempdir().expect("tempdir");
    let db_a = dir.path().join("private-runtime-a.db");
    let db_b = dir.path().join("private-runtime-b.db");
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
        .import_peer_ticket(ImportPeerTicketRequest { ticket: ticket_b })
        .await
        .expect("import b");
    runtime_b
        .import_peer_ticket(ImportPeerTicketRequest { ticket: ticket_a })
        .await
        .expect("import a");

    let topic = "kukuri:topic:desktop-private-channel";
    open_topic_column(&runtime_a, topic, TimelineScope::Public)
        .await
        .expect("subscribe a");
    open_topic_column(&runtime_b, topic, TimelineScope::Public)
        .await
        .expect("subscribe b");
    wait_for_topic_delivery(
        &runtime_a,
        topic,
        1,
        "friend-only owner topic delivery timeout",
    )
    .await;
    wait_for_topic_delivery(
        &runtime_b,
        topic,
        1,
        "friend-only invitee topic delivery timeout",
    )
    .await;
    let channel = runtime_a
        .create_private_channel(CreatePrivateChannelRequest {
            topic: topic.into(),
            label: "core".into(),
            audience_kind: ChannelAudienceKind::InviteOnly,
        })
        .await
        .expect("create private channel");
    let invite = runtime_a
        .export_private_channel_invite(ExportPrivateChannelInviteRequest {
            topic: topic.into(),
            channel_id: channel.channel_id.clone(),
            expires_at: None,
        })
        .await
        .expect("export invite");
    let preview = runtime_b
        .import_private_channel_invite(ImportPrivateChannelInviteRequest { token: invite })
        .await
        .expect("import invite");
    assert_eq!(preview.topic_id.as_str(), topic);
    assert_eq!(preview.channel_id.as_str(), channel.channel_id);

    let private_channel_id = kukuri_core::ChannelId::new(channel.channel_id.clone());
    let private_channel_ref = ChannelRef::PrivateChannel {
        channel_id: private_channel_id.clone(),
    };
    let private_scope = TimelineScope::Channel {
        channel_id: private_channel_id.clone(),
    };
    open_topic_column(&runtime_a, topic, private_scope.clone())
        .await
        .expect("subscribe private a");
    open_topic_column(&runtime_b, topic, private_scope.clone())
        .await
        .expect("subscribe private b");

    let private_post_id = runtime_b
        .create_post(CreatePostRequest {
            topic: topic.into(),
            content: "private hello from b".into(),
            reply_to: None,
            channel_ref: private_channel_ref.clone(),
            attachments: vec![],
            content_labels: Vec::new(),
        })
        .await
        .expect("create private post");

    let private_post = timeout(Duration::from_secs(10), async {
        loop {
            let public_timeline = runtime_b
                .list_timeline(ListTimelineRequest {
                    topic: topic.into(),
                    scope: TimelineScope::Public,
                    cursor: None,
                    limit: Some(20),
                })
                .await
                .expect("public timeline");
            assert!(
                public_timeline
                    .items
                    .iter()
                    .all(|post| post.object_id != private_post_id),
                "private post leaked into public timeline"
            );
            let private_timeline = runtime_b
                .list_timeline(ListTimelineRequest {
                    topic: topic.into(),
                    scope: private_scope.clone(),
                    cursor: None,
                    limit: Some(20),
                })
                .await
                .expect("private timeline");
            if let Some(post) = private_timeline
                .items
                .iter()
                .find(|post| post.object_id == private_post_id)
            {
                return post.clone();
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("private post timeout");
    assert_eq!(
        private_post.channel_id.as_deref(),
        Some(channel.channel_id.as_str())
    );
    assert_eq!(private_post.audience_label, "core");
    let _ = runtime_b
        .list_thread(ListThreadRequest {
            topic: topic.into(),
            thread_id: private_post_id.clone(),
            cursor: None,
            limit: Some(20),
        })
        .await
        .expect("subscribe private thread");

    let private_reply_id = runtime_b
        .create_post(CreatePostRequest {
            topic: topic.into(),
            content: "private reply".into(),
            reply_to: Some(private_post_id.clone()),
            channel_ref: ChannelRef::Public,
            attachments: vec![],
            content_labels: Vec::new(),
        })
        .await
        .expect("create private reply");
    let private_thread = timeout(Duration::from_secs(10), async {
        loop {
            let thread = runtime_b
                .list_thread(ListThreadRequest {
                    topic: topic.into(),
                    thread_id: private_post_id.clone(),
                    cursor: None,
                    limit: Some(20),
                })
                .await
                .expect("thread");
            if thread
                .items
                .iter()
                .any(|post| post.object_id == private_reply_id)
            {
                return thread;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("private thread timeout");
    let reply = private_thread
        .items
        .iter()
        .find(|post| post.object_id == private_reply_id)
        .expect("reply");
    assert_eq!(
        reply.channel_id.as_deref(),
        Some(channel.channel_id.as_str())
    );

    let session_id = runtime_b
        .create_live_session(CreateLiveSessionRequest {
            topic: topic.into(),
            channel_ref: private_channel_ref.clone(),
            title: "core live".into(),
            description: "private stream".into(),
        })
        .await
        .expect("create private live session");
    let _private_session = timeout(Duration::from_secs(10), async {
        loop {
            let sessions = runtime_b
                .list_live_sessions(ListLiveSessionsRequest {
                    topic: topic.into(),
                    scope: private_scope.clone(),
                })
                .await
                .expect("list private live sessions");
            if let Some(session) = sessions
                .iter()
                .find(|session| session.session_id == session_id)
            {
                return session.clone();
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("private live timeout");
    runtime_b
        .end_live_session(LiveSessionCommandRequest {
            topic: topic.into(),
            session_id: session_id.clone(),
        })
        .await
        .expect("end live session");
    timeout(Duration::from_secs(10), async {
        loop {
            let sessions = runtime_b
                .list_live_sessions(ListLiveSessionsRequest {
                    topic: topic.into(),
                    scope: private_scope.clone(),
                })
                .await
                .expect("list live sessions b");
            if sessions.iter().any(|session| {
                session.session_id == session_id
                    && session.status == kukuri_core::LiveSessionStatus::Ended
            }) {
                return;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("live end timeout");

    let room_id = runtime_b
        .create_game_room(CreateGameRoomRequest {
            topic: topic.into(),
            channel_ref: private_channel_ref.clone(),
            title: "core room".into(),
            description: "private set".into(),
            participants: vec!["Alice".into(), "Bob".into()],
        })
        .await
        .expect("create private game room");
    let room_before_update = timeout(Duration::from_secs(10), async {
        loop {
            let rooms = runtime_b
                .list_game_rooms(ListGameRoomsRequest {
                    topic: topic.into(),
                    scope: private_scope.clone(),
                })
                .await
                .expect("list private game rooms");
            if let Some(room) = rooms.iter().find(|room| room.room_id == room_id) {
                return room.clone();
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("private game timeout");
    runtime_b
        .update_game_room(UpdateGameRoomRequest {
            topic: topic.into(),
            room_id: room_id.clone(),
            status: GameRoomStatus::Running,
            phase_label: Some("Round 2".into()),
            scores: room_before_update
                .scores
                .iter()
                .map(|score| GameScoreView {
                    participant_id: score.participant_id.clone(),
                    label: score.label.clone(),
                    score: if score.label == "Alice" { 2 } else { 1 },
                })
                .collect(),
        })
        .await
        .expect("update private game room");
    timeout(Duration::from_secs(10), async {
        loop {
            let rooms = runtime_b
                .list_game_rooms(ListGameRoomsRequest {
                    topic: topic.into(),
                    scope: private_scope.clone(),
                })
                .await
                .expect("list updated game rooms");
            if rooms.iter().any(|room| {
                room.room_id == room_id
                    && room.phase_label.as_deref() == Some("Round 2")
                    && room
                        .scores
                        .iter()
                        .any(|score| score.label == "Alice" && score.score == 2)
            }) {
                return;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("game update timeout");

    let joined_before_restart = runtime_b
        .list_joined_private_channels(ListJoinedPrivateChannelsRequest {
            topic: topic.into(),
        })
        .await
        .expect("list joined channels before restart");
    assert_eq!(joined_before_restart.len(), 1);
    assert_eq!(joined_before_restart[0].channel_id, channel.channel_id);

    timeout(Duration::from_secs(30), runtime_a.shutdown())
        .await
        .expect("runtime a shutdown timeout");
    timeout(Duration::from_secs(30), runtime_b.shutdown())
        .await
        .expect("runtime b shutdown timeout");
    drop(runtime_a);
    drop(runtime_b);
    delete_sqlite_artifacts(&db_b);

    let restarted_b = DesktopRuntime::new_with_config_and_identity(
        &db_b,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("restart runtime b");

    let joined_after_restart = restarted_b
        .list_joined_private_channels(ListJoinedPrivateChannelsRequest {
            topic: topic.into(),
        })
        .await
        .expect("list joined channels after restart");
    assert_eq!(joined_after_restart.len(), 1);
    assert_eq!(joined_after_restart[0].channel_id, channel.channel_id);
    assert_eq!(joined_after_restart[0].label, "core");

    let public_timeline_after_restart = restarted_b
        .list_timeline(ListTimelineRequest {
            topic: topic.into(),
            scope: TimelineScope::Public,
            cursor: None,
            limit: Some(20),
        })
        .await
        .expect("public timeline after restart");
    assert!(
        public_timeline_after_restart
            .items
            .iter()
            .all(|post| post.object_id != private_post_id)
    );
    let private_timeline_after_restart = restarted_b
        .list_timeline(ListTimelineRequest {
            topic: topic.into(),
            scope: private_scope.clone(),
            cursor: None,
            limit: Some(20),
        })
        .await
        .expect("private timeline after restart");
    assert!(
        private_timeline_after_restart
            .items
            .iter()
            .any(|post| post.object_id == private_post_id)
    );
    assert!(
        private_timeline_after_restart
            .items
            .iter()
            .any(|post| post.object_id == private_reply_id)
    );

    let private_thread_after_restart = restarted_b
        .list_thread(ListThreadRequest {
            topic: topic.into(),
            thread_id: private_post_id.clone(),
            cursor: None,
            limit: Some(20),
        })
        .await
        .expect("private thread after restart");
    assert!(
        private_thread_after_restart
            .items
            .iter()
            .any(|post| post.object_id == private_reply_id)
    );

    let sessions_after_restart = restarted_b
        .list_live_sessions(ListLiveSessionsRequest {
            topic: topic.into(),
            scope: private_scope.clone(),
        })
        .await
        .expect("live sessions after restart");
    assert!(sessions_after_restart.iter().any(|session| {
        session.session_id == session_id && session.status == kukuri_core::LiveSessionStatus::Ended
    }));

    let rooms_after_restart = restarted_b
        .list_game_rooms(ListGameRoomsRequest {
            topic: topic.into(),
            scope: private_scope.clone(),
        })
        .await
        .expect("game rooms after restart");
    assert!(rooms_after_restart.iter().any(|room| {
        room.room_id == room_id
            && room.phase_label.as_deref() == Some("Round 2")
            && room
                .scores
                .iter()
                .any(|score| score.label == "Alice" && score.score == 2)
    }));

    let fresh_invite = restarted_b
        .export_private_channel_invite(ExportPrivateChannelInviteRequest {
            topic: topic.into(),
            channel_id: channel.channel_id.clone(),
            expires_at: None,
        })
        .await
        .expect("export fresh invite");
    assert!(fresh_invite.contains(topic));
    assert!(fresh_invite.contains(channel.channel_id.as_str()));

    timeout(Duration::from_secs(30), restarted_b.shutdown())
        .await
        .expect("restarted runtime shutdown timeout");
}
