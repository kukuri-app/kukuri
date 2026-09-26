use super::super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn friend_only_channel_restore_keeps_archived_epoch_history() {
    let _resource = lock_test_resource(TestResource::IrohNetwork).await;
    let dir = tempdir().expect("tempdir");
    let db_a = dir.path().join("friend-only-runtime-a.db");
    let db_b = dir.path().join("friend-only-runtime-b.db");
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

    let a_pubkey = runtime_a
        .get_sync_status()
        .await
        .expect("status a")
        .local_author_pubkey;
    let b_pubkey = runtime_b
        .get_sync_status()
        .await
        .expect("status b")
        .local_author_pubkey;
    let topic = "kukuri:topic:desktop-friend-only-restart";
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
        "friend-only recipient topic delivery timeout",
    )
    .await;
    warm_author_social_view(
        &runtime_a,
        b_pubkey.as_str(),
        "friend-only owner author warm timeout",
    )
    .await;
    warm_author_social_view(
        &runtime_b,
        a_pubkey.as_str(),
        "friend-only recipient author warm timeout",
    )
    .await;
    runtime_a
        .follow_author(AuthorRequest {
            pubkey: b_pubkey.clone(),
        })
        .await
        .expect("a follows b");
    runtime_b
        .follow_author(AuthorRequest {
            pubkey: a_pubkey.clone(),
        })
        .await
        .expect("b follows a");

    timeout(social_graph_propagation_timeout(), async {
        loop {
            let a_view = runtime_a
                .get_author_social_view(AuthorRequest {
                    pubkey: b_pubkey.clone(),
                })
                .await
                .expect("a loads b");
            let b_view = runtime_b
                .get_author_social_view(AuthorRequest {
                    pubkey: a_pubkey.clone(),
                })
                .await
                .expect("b loads a");
            if a_view.mutual && b_view.mutual {
                return;
            }
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("mutual propagation timeout");

    let channel = runtime_a
        .create_private_channel(CreatePrivateChannelRequest {
            topic: topic.into(),
            label: "friends".into(),
            audience_kind: ChannelAudienceKind::FriendOnly,
        })
        .await
        .expect("create friend-only channel");
    let grant = runtime_a
        .export_friend_only_grant(ExportFriendOnlyGrantRequest {
            topic: topic.into(),
            channel_id: channel.channel_id.clone(),
            expires_at: None,
        })
        .await
        .expect("export friend-only grant");
    let preview = wait_for_friend_only_grant_import(
        &runtime_b,
        grant.as_str(),
        social_graph_propagation_timeout(),
        "import friend-only grant",
    )
    .await;
    let original_epoch_id = preview.epoch_id.clone();
    assert_eq!(preview.topic_id.as_str(), topic);
    assert_eq!(preview.channel_id.as_str(), channel.channel_id);

    let private_channel_id = kukuri_core::ChannelId::new(channel.channel_id.clone());
    let private_channel_ref = ChannelRef::PrivateChannel {
        channel_id: private_channel_id.clone(),
    };
    let private_scope = TimelineScope::Channel {
        channel_id: private_channel_id.clone(),
    };
    let private_post_id = runtime_b
        .create_post(CreatePostRequest {
            topic: topic.into(),
            content: "friends hello from b".into(),
            reply_to: None,
            channel_ref: private_channel_ref,
            attachments: vec![],
            content_labels: Vec::new(),
        })
        .await
        .expect("create friend-only post");

    timeout(runtime_replication_timeout(), async {
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
                "friend-only post leaked into public timeline"
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
            if private_timeline
                .items
                .iter()
                .any(|post| post.object_id == private_post_id)
            {
                return;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("friend-only post timeout");

    let rotated = runtime_a
        .rotate_private_channel(RotatePrivateChannelRequest {
            topic: topic.into(),
            channel_id: channel.channel_id.clone(),
        })
        .await
        .expect("rotate friend-only channel");
    assert_ne!(rotated.current_epoch_id, original_epoch_id);
    assert_eq!(rotated.archived_epoch_ids, vec![original_epoch_id.clone()]);

    let fresh_grant = runtime_a
        .export_friend_only_grant(ExportFriendOnlyGrantRequest {
            topic: topic.into(),
            channel_id: channel.channel_id.clone(),
            expires_at: None,
        })
        .await
        .expect("export fresh friend-only grant");
    let fresh_preview = wait_for_friend_only_grant_import(
        &runtime_b,
        fresh_grant.as_str(),
        social_graph_propagation_timeout(),
        "import fresh friend-only grant",
    )
    .await;
    assert_eq!(fresh_preview.epoch_id, rotated.current_epoch_id);

    let joined_before_restart = [wait_for_joined_private_channel_epoch(
        &runtime_b,
        topic,
        channel.channel_id.as_str(),
        rotated.current_epoch_id.as_str(),
        2,
        "joined channel update timeout",
    )
    .await];
    assert_eq!(joined_before_restart.len(), 1);
    assert_eq!(
        joined_before_restart[0].archived_epoch_ids,
        vec![original_epoch_id.clone()]
    );

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
    assert_eq!(
        joined_after_restart[0].audience_kind,
        ChannelAudienceKind::FriendOnly
    );
    assert_eq!(
        joined_after_restart[0].current_epoch_id,
        rotated.current_epoch_id
    );
    assert_eq!(
        joined_after_restart[0].archived_epoch_ids,
        vec![original_epoch_id.clone()]
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

    timeout(Duration::from_secs(30), restarted_b.shutdown())
        .await
        .expect("restarted runtime shutdown timeout");
}
