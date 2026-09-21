use super::super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn friend_plus_channel_restore_accepts_fresh_share_after_restart() {
    let _resource = lock_test_resource(TestResource::IrohNetwork).await;
    let dir = tempdir().expect("tempdir");
    let db_a = dir.path().join("friend-plus-runtime-a.db");
    let db_b = dir.path().join("friend-plus-runtime-b.db");
    let db_c = dir.path().join("friend-plus-runtime-c.db");
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
    let runtime_c = DesktopRuntime::new_with_config_and_identity(
        &db_c,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime c");

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
    let ticket_c = runtime_c
        .local_peer_ticket()
        .await
        .expect("ticket c")
        .expect("ticket c value");

    runtime_a
        .import_peer_ticket(ImportPeerTicketRequest {
            ticket: ticket_b.clone(),
        })
        .await
        .expect("a imports b");
    runtime_b
        .import_peer_ticket(ImportPeerTicketRequest {
            ticket: ticket_a.clone(),
        })
        .await
        .expect("b imports a");

    let status_a = runtime_a.get_sync_status().await.expect("status a");
    let a_pubkey = status_a.local_author_pubkey;
    let status_b = runtime_b.get_sync_status().await.expect("status b");
    let b_pubkey = status_b.local_author_pubkey;
    let status_c = runtime_c.get_sync_status().await.expect("status c");
    let c_pubkey = status_c.local_author_pubkey;
    let topic = "kukuri:topic:desktop-friend-plus-restart";
    for runtime in [&runtime_a, &runtime_b] {
        let _ = runtime
            .list_timeline(ListTimelineRequest {
                topic: topic.into(),
                scope: TimelineScope::Public,
                cursor: None,
                limit: Some(20),
            })
            .await
            .expect("subscribe runtime");
    }
    wait_for_topic_delivery(
        &runtime_a,
        topic,
        1,
        "friend-plus owner topic delivery timeout",
    )
    .await;
    wait_for_topic_delivery(
        &runtime_b,
        topic,
        1,
        "friend-plus sponsor topic delivery timeout",
    )
    .await;
    warm_author_social_view(
        &runtime_a,
        b_pubkey.as_str(),
        "friend-plus owner author warm timeout",
    )
    .await;
    warm_author_social_view(
        &runtime_b,
        a_pubkey.as_str(),
        "friend-plus sponsor owner author warm timeout",
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
    wait_for_mutual_author_view(&runtime_a, b_pubkey.as_str(), topic).await;
    wait_for_mutual_author_view(&runtime_b, a_pubkey.as_str(), topic).await;
    let channel = runtime_a
        .create_private_channel(CreatePrivateChannelRequest {
            topic: topic.into(),
            label: "friends+".into(),
            audience_kind: ChannelAudienceKind::FriendPlus,
        })
        .await
        .expect("create friend-plus channel");
    let share_ab = runtime_a
        .export_friend_plus_share(ExportFriendPlusShareRequest {
            topic: topic.into(),
            channel_id: channel.channel_id.clone(),
            expires_at: None,
        })
        .await
        .expect("export a->b share");
    let _preview_b = wait_for_friend_plus_share_import(
        &runtime_b,
        share_ab.as_str(),
        social_graph_propagation_timeout(),
        "b imports friend-plus share",
    )
    .await;
    runtime_a
        .import_peer_ticket(ImportPeerTicketRequest {
            ticket: ticket_c.clone(),
        })
        .await
        .expect("a imports c");
    runtime_c
        .import_peer_ticket(ImportPeerTicketRequest {
            ticket: ticket_a.clone(),
        })
        .await
        .expect("c imports a");
    runtime_b
        .import_peer_ticket(ImportPeerTicketRequest {
            ticket: ticket_c.clone(),
        })
        .await
        .expect("b imports c");
    runtime_c
        .import_peer_ticket(ImportPeerTicketRequest {
            ticket: ticket_b.clone(),
        })
        .await
        .expect("c imports b");
    let _ = runtime_c
        .list_timeline(ListTimelineRequest {
            topic: topic.into(),
            scope: TimelineScope::Public,
            cursor: None,
            limit: Some(20),
        })
        .await
        .expect("subscribe runtime c");
    // C joins after A and B have already subscribed, so re-import the peer tickets to
    // rebuild the existing topic subscriptions against C's endpoint instead of leaving
    // them assist-only.
    runtime_a
        .import_peer_ticket(ImportPeerTicketRequest {
            ticket: ticket_c.clone(),
        })
        .await
        .expect("a refreshes c after subscribe");
    runtime_b
        .import_peer_ticket(ImportPeerTicketRequest {
            ticket: ticket_c.clone(),
        })
        .await
        .expect("b refreshes c after subscribe");
    runtime_c
        .import_peer_ticket(ImportPeerTicketRequest {
            ticket: ticket_a.clone(),
        })
        .await
        .expect("c refreshes a after subscribe");
    runtime_c
        .import_peer_ticket(ImportPeerTicketRequest {
            ticket: ticket_b.clone(),
        })
        .await
        .expect("c refreshes b after subscribe");
    wait_for_topic_delivery(
        &runtime_a,
        topic,
        1,
        "friend-plus owner topic mesh delivery timeout",
    )
    .await;
    wait_for_topic_delivery(
        &runtime_b,
        topic,
        1,
        "friend-plus sponsor topic mesh delivery timeout",
    )
    .await;
    wait_for_topic_delivery(
        &runtime_c,
        topic,
        1,
        "friend-plus recipient topic mesh delivery timeout",
    )
    .await;
    // Relay-assisted sync is sufficient for the downstream share import and private-channel
    // replication assertions in this test. Slower CI hosts can remain assist-only here even
    // after ticket refresh, so keep the ticket re-imports above but rely on the actual
    // friend-plus restore/share assertions below instead of requiring direct topic peers.
    warm_author_social_view(
        &runtime_b,
        c_pubkey.as_str(),
        "friend-plus sponsor recipient author warm timeout",
    )
    .await;
    warm_author_social_view(
        &runtime_c,
        b_pubkey.as_str(),
        "friend-plus recipient sponsor author warm timeout",
    )
    .await;
    runtime_b
        .follow_author(AuthorRequest {
            pubkey: c_pubkey.clone(),
        })
        .await
        .expect("b follows c");
    runtime_c
        .follow_author(AuthorRequest {
            pubkey: b_pubkey.clone(),
        })
        .await
        .expect("c follows b");
    wait_for_mutual_author_view(&runtime_b, c_pubkey.as_str(), topic).await;
    wait_for_mutual_author_view(&runtime_c, b_pubkey.as_str(), topic).await;
    let share_bc = runtime_b
        .export_friend_plus_share(ExportFriendPlusShareRequest {
            topic: topic.into(),
            channel_id: channel.channel_id.clone(),
            expires_at: None,
        })
        .await
        .expect("export b->c share");
    let preview_c = wait_for_friend_plus_share_import(
        &runtime_c,
        share_bc.as_str(),
        social_graph_propagation_timeout(),
        "c imports friend-plus share",
    )
    .await;
    let original_epoch_id = preview_c.epoch_id.clone();
    assert_eq!(preview_c.sponsor_pubkey.as_str(), b_pubkey.as_str());
    // Importing the fresh share updates C's joined private-channel state after A and B have
    // already built their active topic/private subscriptions, so refresh the tickets once more
    // to rebuild those subscriptions against the new friend-plus epoch before the first write.
    runtime_a
        .import_peer_ticket(ImportPeerTicketRequest {
            ticket: ticket_c.clone(),
        })
        .await
        .expect("a refreshes c after friend-plus share");
    runtime_b
        .import_peer_ticket(ImportPeerTicketRequest {
            ticket: ticket_c.clone(),
        })
        .await
        .expect("b refreshes c after friend-plus share");
    runtime_c
        .import_peer_ticket(ImportPeerTicketRequest {
            ticket: ticket_a.clone(),
        })
        .await
        .expect("c refreshes a after friend-plus share");
    runtime_c
        .import_peer_ticket(ImportPeerTicketRequest {
            ticket: ticket_b.clone(),
        })
        .await
        .expect("c refreshes b after friend-plus share");

    let private_scope = TimelineScope::Channel {
        channel_id: kukuri_core::ChannelId::new(channel.channel_id.clone()),
    };
    let private_ref = ChannelRef::PrivateChannel {
        channel_id: kukuri_core::ChannelId::new(channel.channel_id.clone()),
    };
    let _ = runtime_b
        .list_timeline(ListTimelineRequest {
            topic: topic.into(),
            scope: private_scope.clone(),
            cursor: None,
            limit: Some(20),
        })
        .await
        .expect("subscribe friend-plus private b");
    let _ = runtime_c
        .list_timeline(ListTimelineRequest {
            topic: topic.into(),
            scope: private_scope.clone(),
            cursor: None,
            limit: Some(20),
        })
        .await
        .expect("subscribe friend-plus private c");
    let joined_a_before_history = wait_for_joined_private_channel_epoch(
        &runtime_a,
        topic,
        channel.channel_id.as_str(),
        original_epoch_id.as_str(),
        3,
        "friend-plus owner private readiness timeout",
    )
    .await;
    assert_eq!(joined_a_before_history.participant_count, 3);
    let joined_b_before_history = wait_for_joined_private_channel_epoch(
        &runtime_b,
        topic,
        channel.channel_id.as_str(),
        original_epoch_id.as_str(),
        3,
        "friend-plus sponsor private readiness timeout",
    )
    .await;
    assert_eq!(
        joined_b_before_history.joined_via_pubkey.as_deref(),
        Some(a_pubkey.as_str())
    );
    assert_eq!(joined_b_before_history.participant_count, 3);
    let joined_c_before_history = wait_for_joined_private_channel_epoch(
        &runtime_c,
        topic,
        channel.channel_id.as_str(),
        original_epoch_id.as_str(),
        3,
        "friend-plus recipient private readiness timeout",
    )
    .await;
    assert_eq!(
        joined_c_before_history.joined_via_pubkey.as_deref(),
        Some(b_pubkey.as_str())
    );
    assert_eq!(joined_c_before_history.participant_count, 3);
    let old_post_id = replicate_private_post_with_retry(
        &runtime_a,
        &[&runtime_b, &runtime_c],
        topic,
        &private_scope,
        &private_ref,
        "friend-plus history",
        "friend-plus history propagation timeout",
    )
    .await;

    let public_timeline_c = runtime_c
        .list_timeline(ListTimelineRequest {
            topic: topic.into(),
            scope: TimelineScope::Public,
            cursor: None,
            limit: Some(20),
        })
        .await
        .expect("public timeline c");
    assert!(
        public_timeline_c
            .items
            .iter()
            .all(|post| post.object_id != old_post_id),
        "friend-plus post leaked into public timeline"
    );

    let joined_before_restart = runtime_c
        .list_joined_private_channels(ListJoinedPrivateChannelsRequest {
            topic: topic.into(),
        })
        .await
        .expect("joined channels before restart");
    assert_eq!(joined_before_restart.len(), 1);
    assert_eq!(joined_before_restart[0].channel_id, channel.channel_id);
    let restored_epoch_id = joined_before_restart[0].current_epoch_id.clone();
    assert_eq!(restored_epoch_id, original_epoch_id);
    assert_eq!(
        joined_before_restart[0].joined_via_pubkey.as_deref(),
        Some(b_pubkey.as_str())
    );

    timeout(Duration::from_secs(30), runtime_c.shutdown())
        .await
        .expect("runtime c shutdown timeout");
    drop(runtime_c);
    delete_sqlite_artifacts(&db_c);

    let restarted_c = DesktopRuntime::new_with_config_and_identity(
        &db_c,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("restart runtime c");
    restarted_c
        .import_peer_ticket(ImportPeerTicketRequest {
            ticket: ticket_a.clone(),
        })
        .await
        .expect("restarted c imports a");
    restarted_c
        .import_peer_ticket(ImportPeerTicketRequest {
            ticket: ticket_b.clone(),
        })
        .await
        .expect("restarted c imports b");
    let restarted_ticket_c = restarted_c
        .local_peer_ticket()
        .await
        .expect("restarted ticket c")
        .expect("restarted ticket c value");
    runtime_a
        .import_peer_ticket(ImportPeerTicketRequest {
            ticket: restarted_ticket_c.clone(),
        })
        .await
        .expect("a imports restarted c");
    runtime_b
        .import_peer_ticket(ImportPeerTicketRequest {
            ticket: restarted_ticket_c.clone(),
        })
        .await
        .expect("b imports restarted c");
    let _ = restarted_c
        .list_timeline(ListTimelineRequest {
            topic: topic.into(),
            scope: TimelineScope::Public,
            cursor: None,
            limit: Some(20),
        })
        .await
        .expect("subscribe restarted c public");
    let _ = restarted_c
        .list_timeline(ListTimelineRequest {
            topic: topic.into(),
            scope: private_scope.clone(),
            cursor: None,
            limit: Some(20),
        })
        .await
        .expect("subscribe restarted c private");
    // Re-importing tickets forces existing topic subscriptions to rebuild against C's new endpoint.
    restarted_c
        .import_peer_ticket(ImportPeerTicketRequest {
            ticket: ticket_a.clone(),
        })
        .await
        .expect("restarted c refreshes a");
    restarted_c
        .import_peer_ticket(ImportPeerTicketRequest {
            ticket: ticket_b.clone(),
        })
        .await
        .expect("restarted c refreshes b");
    runtime_a
        .import_peer_ticket(ImportPeerTicketRequest {
            ticket: restarted_ticket_c.clone(),
        })
        .await
        .expect("a refreshes restarted c");
    runtime_b
        .import_peer_ticket(ImportPeerTicketRequest {
            ticket: restarted_ticket_c.clone(),
        })
        .await
        .expect("b refreshes restarted c");
    let joined_after_restart = restarted_c
        .list_joined_private_channels(ListJoinedPrivateChannelsRequest {
            topic: topic.into(),
        })
        .await
        .expect("joined channels after restart");
    assert_eq!(joined_after_restart.len(), 1);
    assert_eq!(joined_after_restart[0].channel_id, channel.channel_id);
    assert_eq!(joined_after_restart[0].current_epoch_id, restored_epoch_id);
    assert_eq!(
        joined_after_restart[0].joined_via_pubkey.as_deref(),
        Some(b_pubkey.as_str())
    );

    let private_timeline_after_restart = restarted_c
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
            .any(|post| post.object_id == old_post_id)
    );
    let joined_restarted_before_rotate = wait_for_joined_private_channel_epoch(
        &restarted_c,
        topic,
        channel.channel_id.as_str(),
        restored_epoch_id.as_str(),
        3,
        "friend-plus restarted private readiness timeout",
    )
    .await;
    assert_eq!(
        joined_restarted_before_rotate.joined_via_pubkey.as_deref(),
        Some(b_pubkey.as_str())
    );
    assert_eq!(joined_restarted_before_rotate.participant_count, 3);

    wait_for_topic_delivery(
        &runtime_a,
        topic,
        1,
        "friend-plus owner topic delivery timeout",
    )
    .await;
    wait_for_topic_delivery(
        &runtime_b,
        topic,
        1,
        "friend-plus sponsor topic delivery timeout",
    )
    .await;

    let rotated = runtime_a
        .rotate_private_channel(RotatePrivateChannelRequest {
            topic: topic.into(),
            channel_id: channel.channel_id.clone(),
        })
        .await
        .expect("rotate friend-plus channel");
    assert_ne!(rotated.current_epoch_id, restored_epoch_id);

    let refreshed_share_ab = runtime_a
        .export_friend_plus_share(ExportFriendPlusShareRequest {
            topic: topic.into(),
            channel_id: channel.channel_id.clone(),
            expires_at: None,
        })
        .await
        .expect("export refreshed a->b share after rotate");
    let preview_b_after_rotate = wait_for_friend_plus_share_import(
        &runtime_b,
        refreshed_share_ab.as_str(),
        social_graph_propagation_timeout(),
        "b imports refreshed friend-plus share",
    )
    .await;
    let shared_epoch_id = preview_b_after_rotate.epoch_id.clone();
    assert_ne!(shared_epoch_id, restored_epoch_id);
    assert_eq!(
        preview_b_after_rotate.sponsor_pubkey.as_str(),
        a_pubkey.as_str()
    );
    let joined_b_after_rotate = wait_for_joined_private_channel_epoch(
        &runtime_b,
        topic,
        channel.channel_id.as_str(),
        shared_epoch_id.as_str(),
        2,
        "friend-plus sponsor refresh share redeem timeout",
    )
    .await;
    assert_eq!(
        joined_b_after_rotate.joined_via_pubkey.as_deref(),
        Some(a_pubkey.as_str())
    );
    assert!(
        joined_b_after_rotate
            .archived_epoch_ids
            .iter()
            .any(|epoch_id| epoch_id == &restored_epoch_id)
    );

    let fresh_share = runtime_b
        .export_friend_plus_share(ExportFriendPlusShareRequest {
            topic: topic.into(),
            channel_id: channel.channel_id.clone(),
            expires_at: None,
        })
        .await
        .expect("export fresh friend-plus share after restart");
    let preview_after_restart = wait_for_friend_plus_share_import(
        &restarted_c,
        fresh_share.as_str(),
        social_graph_propagation_timeout(),
        "restarted c imports fresh friend-plus share",
    )
    .await;
    assert_eq!(preview_after_restart.epoch_id, shared_epoch_id);
    assert_eq!(
        preview_after_restart.sponsor_pubkey.as_str(),
        b_pubkey.as_str()
    );
    let joined_after_rotate = wait_for_joined_private_channel_epoch(
        &restarted_c,
        topic,
        channel.channel_id.as_str(),
        shared_epoch_id.as_str(),
        3,
        "friend-plus restarted share redeem timeout",
    )
    .await;
    assert_eq!(
        joined_after_rotate.joined_via_pubkey.as_deref(),
        Some(b_pubkey.as_str())
    );
    assert_eq!(joined_after_rotate.participant_count, 3);
    assert!(
        joined_after_rotate
            .archived_epoch_ids
            .iter()
            .any(|epoch_id| epoch_id == &restored_epoch_id)
    );
    wait_for_connected_topic_peer_count(
        &restarted_c,
        topic,
        1,
        "friend-plus restarted topic reconnect timeout",
    )
    .await;
    restarted_c
        .import_peer_ticket(ImportPeerTicketRequest {
            ticket: ticket_a.clone(),
        })
        .await
        .expect("restarted c refreshes a after rotate");
    restarted_c
        .import_peer_ticket(ImportPeerTicketRequest {
            ticket: ticket_b.clone(),
        })
        .await
        .expect("restarted c refreshes b after rotate");
    runtime_a
        .import_peer_ticket(ImportPeerTicketRequest {
            ticket: restarted_ticket_c.clone(),
        })
        .await
        .expect("a refreshes restarted c after rotate");
    runtime_b
        .import_peer_ticket(ImportPeerTicketRequest {
            ticket: restarted_ticket_c.clone(),
        })
        .await
        .expect("b refreshes restarted c after rotate");
    let _ = restarted_c
        .list_timeline(ListTimelineRequest {
            topic: topic.into(),
            scope: private_scope.clone(),
            cursor: None,
            limit: Some(20),
        })
        .await
        .expect("resubscribe restarted c private after fresh share");

    let restarted_post_id = restarted_c
        .create_post(CreatePostRequest {
            topic: topic.into(),
            content: "friend-plus restarted after rotate".into(),
            reply_to: None,
            channel_ref: private_ref.clone(),
            attachments: vec![],
            content_labels: Vec::new(),
        })
        .await
        .expect("restarted c creates friend-plus rotated post");
    match timeout(runtime_replication_timeout(), async {
        loop {
            let public_timeline = restarted_c
                .list_timeline(ListTimelineRequest {
                    topic: topic.into(),
                    scope: TimelineScope::Public,
                    cursor: None,
                    limit: Some(20),
                })
                .await
                .expect("public timeline after rotate");
            assert!(
                public_timeline
                    .items
                    .iter()
                    .all(|post| post.object_id != restarted_post_id),
                "friend-plus rotated post leaked into public timeline"
            );
            let private_timeline = restarted_c
                .list_timeline(ListTimelineRequest {
                    topic: topic.into(),
                    scope: private_scope.clone(),
                    cursor: None,
                    limit: Some(20),
                })
                .await
                .expect("private timeline after rotate");
            if private_timeline
                .items
                .iter()
                .any(|post| post.object_id == restarted_post_id)
            {
                return;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    {
        Ok(()) => {}
        Err(_) => {
            let restarted_status = restarted_c.get_sync_status().await.expect("status c");
            let joined = restarted_c
                .list_joined_private_channels(ListJoinedPrivateChannelsRequest {
                    topic: topic.into(),
                })
                .await
                .unwrap_or_default();
            let private_timeline = restarted_c
                .list_timeline(ListTimelineRequest {
                    topic: topic.into(),
                    scope: private_scope.clone(),
                    cursor: None,
                    limit: Some(20),
                })
                .await
                .unwrap_or_else(|_| TimelineView {
                    items: vec![],
                    next_cursor: None,
                    unavailable_count: 0,
                });
            panic!(
                "friend-plus restarted rotated post visibility timeout: restarted={} joined={joined:?} private_items={:?}",
                format_sync_snapshot(&restarted_status, topic),
                private_timeline
                    .items
                    .iter()
                    .map(|item| item.object_id.clone())
                    .collect::<Vec<_>>()
            );
        }
    }

    timeout(Duration::from_secs(30), runtime_a.shutdown())
        .await
        .expect("runtime a shutdown timeout");
    timeout(Duration::from_secs(30), runtime_b.shutdown())
        .await
        .expect("runtime b shutdown timeout");
    timeout(Duration::from_secs(30), restarted_c.shutdown())
        .await
        .expect("restarted runtime c shutdown timeout");
}
