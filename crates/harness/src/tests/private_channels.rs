use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn private_channel_invite_connectivity() {
    disable_keyring_for_tests();
    let _serial = acquire_scenario_test_lock().await;
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root");
    let artifacts = root
        .join("test-results")
        .join("kukuri")
        .join("private-channel-invite-connectivity");
    let result = run_named_scenario(root, "private_channel_invite_connectivity", &artifacts)
        .await
        .expect("scenario");

    assert_eq!(result.status, HarnessStatus::Pass);
    assert!(artifacts.join("result.json").exists());
}
#[tokio::test]
async fn friend_only_rotate_requires_fresh_grant() {
    if std::env::var_os("GITHUB_ACTIONS").is_some() {
        // CI still covers fresh-grant rotation in app-api and desktop-runtime.
        return;
    }
    disable_keyring_for_tests();
    let _serial = acquire_scenario_test_lock().await;
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root");
    let artifacts = root
        .join("test-results")
        .join("kukuri")
        .join("friend-only-rotate-connectivity");
    std::fs::create_dir_all(&artifacts).expect("create artifacts dir");

    let db_a = artifacts.join("friend-only-a.db");
    let db_b = artifacts.join("friend-only-b.db");
    let db_c = artifacts.join("friend-only-c.db");
    cleanup_runtime_artifacts(&db_a).expect("cleanup a");
    cleanup_runtime_artifacts(&db_b).expect("cleanup b");
    cleanup_runtime_artifacts(&db_c).expect("cleanup c");

    let runtime_a = DesktopRuntime::new_with_config(&db_a, TransportNetworkConfig::loopback())
        .await
        .expect("runtime a");
    let runtime_b = DesktopRuntime::new_with_config(&db_b, TransportNetworkConfig::loopback())
        .await
        .expect("runtime b");
    let runtime_c = DesktopRuntime::new_with_config(&db_c, TransportNetworkConfig::loopback())
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
    let c_pubkey = runtime_c
        .get_sync_status()
        .await
        .expect("status c")
        .local_author_pubkey;

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
    runtime_a
        .follow_author(AuthorRequest {
            pubkey: c_pubkey.clone(),
        })
        .await
        .expect("a follows c");
    runtime_c
        .follow_author(AuthorRequest {
            pubkey: a_pubkey.clone(),
        })
        .await
        .expect("c follows a");

    let topic = "kukuri:topic:harness-friend-only";
    let public_scope = TimelineScope::Public;
    open_topic_column(&runtime_a, topic, &public_scope)
        .await
        .expect("subscribe a");
    open_topic_column(&runtime_b, topic, &public_scope)
        .await
        .expect("subscribe b");
    open_topic_column(&runtime_c, topic, &public_scope)
        .await
        .expect("subscribe c");
    let topic_timeout = social_graph_propagation_timeout();
    wait_for_topic_delivery(&runtime_a, topic, 1, topic_timeout)
        .await
        .expect("desktop a did not observe public topic connectivity");
    wait_for_topic_delivery(&runtime_b, topic, 1, topic_timeout)
        .await
        .expect("desktop b did not observe public topic connectivity");
    wait_for_topic_delivery(&runtime_c, topic, 1, topic_timeout)
        .await
        .expect("desktop c did not observe public topic connectivity");
    warm_author_social_view(&runtime_a, b_pubkey.as_str()).await;
    warm_author_social_view(&runtime_b, a_pubkey.as_str()).await;
    warm_author_social_view(&runtime_a, c_pubkey.as_str()).await;
    warm_author_social_view(&runtime_c, a_pubkey.as_str()).await;

    let channel = runtime_a
        .create_private_channel(CreatePrivateChannelRequest {
            topic: topic.to_string(),
            label: "friends".to_string(),
            audience_kind: ChannelAudienceKind::FriendOnly,
        })
        .await
        .expect("create friend-only channel");
    let old_grant = runtime_a
        .export_friend_only_grant(ExportFriendOnlyGrantRequest {
            topic: topic.to_string(),
            channel_id: channel.channel_id.clone(),
            expires_at: None,
        })
        .await
        .expect("export old grant");

    wait_for_friend_only_grant_import(
        &runtime_b,
        old_grant.clone(),
        social_graph_propagation_timeout(),
    )
    .await
    .expect("b imports old grant");
    wait_for_joined_private_channel(
        &runtime_b,
        topic,
        channel.channel_id.as_str(),
        topic_timeout,
    )
    .await
    .expect("desktop b did not join friend-only channel");

    let private_scope = TimelineScope::Channel {
        channel_id: kukuri_core::ChannelId::new(channel.channel_id.clone()),
    };
    let private_ref = ChannelRef::PrivateChannel {
        channel_id: kukuri_core::ChannelId::new(channel.channel_id.clone()),
    };
    let private_post_id = runtime_b
        .create_post(CreatePostRequest {
            topic: topic.to_string(),
            content: "friend-only history".to_string(),
            reply_to: None,
            channel_ref: private_ref,
            attachments: Vec::new(),
            content_labels: Vec::new(),
        })
        .await
        .expect("create friend-only post");
    wait_for_timeline_object_in_scope(
        &runtime_a,
        topic,
        private_scope.clone(),
        private_post_id.as_str(),
        Duration::from_secs(10),
    )
    .await
    .expect("desktop a did not receive friend-only post");
    assert_timeline_scope_excludes_object(
        &runtime_c,
        topic,
        public_scope.clone(),
        private_post_id.as_str(),
        Duration::from_millis(500),
    )
    .await
    .expect("desktop c public scope leaked friend-only post");

    runtime_a
        .unfollow_author(AuthorRequest {
            pubkey: b_pubkey.clone(),
        })
        .await
        .expect("a unfollows b");
    let joined_a = timeout(Duration::from_secs(10), async {
        loop {
            let joined = runtime_a
                .list_joined_private_channels(ListJoinedPrivateChannelsRequest {
                    topic: topic.to_string(),
                })
                .await
                .expect("list joined channels on a");
            if joined.iter().any(|entry| {
                entry.channel_id == channel.channel_id
                    && entry.rotation_required
                    && entry.stale_participant_count == 1
            }) {
                return joined;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("rotation required timeout");
    assert!(joined_a.iter().any(|entry| {
        entry.channel_id == channel.channel_id
            && entry.rotation_required
            && entry.stale_participant_count == 1
    }));

    let rotated = runtime_a
        .rotate_private_channel(RotatePrivateChannelRequest {
            topic: topic.to_string(),
            channel_id: channel.channel_id.clone(),
        })
        .await
        .expect("rotate friend-only channel");

    runtime_c
        .import_friend_only_grant(ImportFriendOnlyGrantRequest { token: old_grant })
        .await
        .expect_err("old grant should fail after rotate");

    let fresh_grant = runtime_a
        .export_friend_only_grant(ExportFriendOnlyGrantRequest {
            topic: topic.to_string(),
            channel_id: channel.channel_id.clone(),
            expires_at: None,
        })
        .await
        .expect("export fresh grant");
    open_topic_column(&runtime_a, topic, &public_scope)
        .await
        .expect("resubscribe a before fresh grant");
    open_topic_column(&runtime_c, topic, &public_scope)
        .await
        .expect("resubscribe c before fresh grant");
    wait_for_topic_delivery(&runtime_a, topic, 1, topic_timeout)
        .await
        .expect("desktop a did not observe public topic connectivity after rotate");
    wait_for_topic_delivery(&runtime_c, topic, 1, topic_timeout)
        .await
        .expect("desktop c did not observe public topic connectivity after rotate");
    runtime_a
        .follow_author(AuthorRequest {
            pubkey: c_pubkey.clone(),
        })
        .await
        .expect("a refreshes follow to c");
    runtime_c
        .follow_author(AuthorRequest {
            pubkey: a_pubkey.clone(),
        })
        .await
        .expect("c refreshes follow to a");
    warm_author_social_view(&runtime_a, c_pubkey.as_str()).await;
    warm_author_social_view(&runtime_c, a_pubkey.as_str()).await;
    wait_for_mutual_author_view(&runtime_a, c_pubkey.as_str(), topic).await;
    wait_for_mutual_author_view(&runtime_c, a_pubkey.as_str(), topic).await;
    let fresh_preview = wait_for_friend_only_grant_import(
        &runtime_c,
        fresh_grant,
        social_graph_propagation_timeout(),
    )
    .await
    .expect("c imports fresh grant");
    assert_eq!(fresh_preview.epoch_id, rotated.current_epoch_id);

    let joined_c = runtime_c
        .list_joined_private_channels(ListJoinedPrivateChannelsRequest {
            topic: topic.to_string(),
        })
        .await
        .expect("list joined channels on c");
    let channel_c = joined_c
        .iter()
        .find(|entry| entry.channel_id == channel.channel_id)
        .expect("friend-only channel on c");
    assert_eq!(channel_c.current_epoch_id, rotated.current_epoch_id);
    assert!(channel_c.archived_epoch_ids.is_empty());

    let c_private_timeline = runtime_c
        .list_timeline(ListTimelineRequest {
            topic: topic.to_string(),
            scope: private_scope.clone(),
            cursor: None,
            limit: Some(20),
        })
        .await
        .expect("list c private timeline");
    assert!(
        c_private_timeline
            .items
            .iter()
            .all(|item| item.object_id != private_post_id)
    );

    shutdown_runtime(runtime_a, "friend-only harness runtime a")
        .await
        .expect("shutdown runtime a");
    shutdown_runtime(runtime_b, "friend-only harness runtime b")
        .await
        .expect("shutdown runtime b");
    shutdown_runtime(runtime_c, "friend-only harness runtime c")
        .await
        .expect("shutdown runtime c");
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn friend_plus_share_freeze_rotate_connectivity() {
    if std::env::var_os("GITHUB_ACTIONS").is_some() {
        // CI still covers freeze/rotate/share recovery in app-api and desktop-runtime.
        return;
    }
    disable_keyring_for_tests();
    let _serial = acquire_scenario_test_lock().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let db_a = dir.path().join("friend-plus-harness-a.db");
    let db_b = dir.path().join("friend-plus-harness-b.db");
    let db_c = dir.path().join("friend-plus-harness-c.db");
    let db_d = dir.path().join("friend-plus-harness-d.db");
    let runtime_a = DesktopRuntime::new_with_config(&db_a, TransportNetworkConfig::loopback())
        .await
        .expect("runtime a");
    let runtime_b = DesktopRuntime::new_with_config(&db_b, TransportNetworkConfig::loopback())
        .await
        .expect("runtime b");
    let runtime_c = DesktopRuntime::new_with_config(&db_c, TransportNetworkConfig::loopback())
        .await
        .expect("runtime c");
    let runtime_d = DesktopRuntime::new_with_config(&db_d, TransportNetworkConfig::loopback())
        .await
        .expect("runtime d");

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
    let ticket_d = runtime_d
        .local_peer_ticket()
        .await
        .expect("ticket d")
        .expect("ticket d value");

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
    runtime_a
        .import_peer_ticket(ImportPeerTicketRequest {
            ticket: ticket_d.clone(),
        })
        .await
        .expect("a imports d");
    runtime_d
        .import_peer_ticket(ImportPeerTicketRequest {
            ticket: ticket_a.clone(),
        })
        .await
        .expect("d imports a");
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
    runtime_b
        .import_peer_ticket(ImportPeerTicketRequest {
            ticket: ticket_d.clone(),
        })
        .await
        .expect("b imports d");
    runtime_d
        .import_peer_ticket(ImportPeerTicketRequest {
            ticket: ticket_b.clone(),
        })
        .await
        .expect("d imports b");

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
    let c_pubkey = runtime_c
        .get_sync_status()
        .await
        .expect("status c")
        .local_author_pubkey;
    let d_pubkey = runtime_d
        .get_sync_status()
        .await
        .expect("status d")
        .local_author_pubkey;
    let topic = "kukuri:topic:harness-friend-plus";

    let public_scope = TimelineScope::Public;
    for runtime in [&runtime_a, &runtime_b, &runtime_c, &runtime_d] {
        open_topic_column(runtime, topic, &public_scope)
            .await
            .expect("subscribe runtime");
    }
    let topic_timeout = social_graph_propagation_timeout();
    wait_for_topic_peer_count(&runtime_a, topic, 1, topic_timeout)
        .await
        .expect("desktop a did not observe public topic connectivity");
    wait_for_topic_peer_count(&runtime_b, topic, 1, topic_timeout)
        .await
        .expect("desktop b did not observe public topic connectivity");
    wait_for_topic_peer_count(&runtime_c, topic, 1, topic_timeout)
        .await
        .expect("desktop c did not observe public topic connectivity");
    wait_for_topic_peer_count(&runtime_d, topic, 1, topic_timeout)
        .await
        .expect("desktop d did not observe public topic connectivity");

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
    runtime_b
        .follow_author(AuthorRequest {
            pubkey: d_pubkey.clone(),
        })
        .await
        .expect("b follows d");
    runtime_d
        .follow_author(AuthorRequest {
            pubkey: b_pubkey.clone(),
        })
        .await
        .expect("d follows b");

    wait_for_mutual_author_view(&runtime_b, a_pubkey.as_str(), topic).await;
    wait_for_mutual_author_view(&runtime_c, b_pubkey.as_str(), topic).await;
    wait_for_mutual_author_view(&runtime_d, b_pubkey.as_str(), topic).await;

    let channel = runtime_a
        .create_private_channel(CreatePrivateChannelRequest {
            topic: topic.to_string(),
            label: "friends+".to_string(),
            audience_kind: ChannelAudienceKind::FriendPlus,
        })
        .await
        .expect("create friend-plus channel");
    let share_ab = runtime_a
        .export_friend_plus_share(ExportFriendPlusShareRequest {
            topic: topic.to_string(),
            channel_id: channel.channel_id.clone(),
            expires_at: None,
        })
        .await
        .expect("export a->b share");
    wait_for_friend_plus_share_import(&runtime_b, share_ab, social_graph_propagation_timeout())
        .await
        .expect("b imports share");
    let share_bc = runtime_b
        .export_friend_plus_share(ExportFriendPlusShareRequest {
            topic: topic.to_string(),
            channel_id: channel.channel_id.clone(),
            expires_at: None,
        })
        .await
        .expect("export b->c share");
    wait_for_friend_plus_share_import(&runtime_c, share_bc, social_graph_propagation_timeout())
        .await
        .expect("c imports share");
    let stale_share_for_d = runtime_b
        .export_friend_plus_share(ExportFriendPlusShareRequest {
            topic: topic.to_string(),
            channel_id: channel.channel_id.clone(),
            expires_at: None,
        })
        .await
        .expect("export b->d share");

    let private_scope = TimelineScope::Channel {
        channel_id: kukuri_core::ChannelId::new(channel.channel_id.clone()),
    };
    let private_ref = ChannelRef::PrivateChannel {
        channel_id: kukuri_core::ChannelId::new(channel.channel_id.clone()),
    };
    let old_post_id = runtime_a
        .create_post(CreatePostRequest {
            topic: topic.to_string(),
            content: "friend-plus history".to_string(),
            reply_to: None,
            channel_ref: private_ref.clone(),
            attachments: Vec::new(),
            content_labels: Vec::new(),
        })
        .await
        .expect("create friend-plus post");
    wait_for_timeline_object_in_scope(
        &runtime_b,
        topic,
        private_scope.clone(),
        old_post_id.as_str(),
        Duration::from_secs(10),
    )
    .await
    .expect("b receives old post");
    wait_for_timeline_object_in_scope(
        &runtime_c,
        topic,
        private_scope.clone(),
        old_post_id.as_str(),
        Duration::from_secs(10),
    )
    .await
    .expect("c receives old post");
    assert_timeline_scope_excludes_object(
        &runtime_d,
        topic,
        public_scope.clone(),
        old_post_id.as_str(),
        Duration::from_millis(500),
    )
    .await
    .expect("public scope leaked friend-plus post");

    let frozen = runtime_a
        .freeze_private_channel(FreezePrivateChannelRequest {
            topic: topic.to_string(),
            channel_id: channel.channel_id.clone(),
        })
        .await
        .expect("freeze friend-plus channel");
    assert_eq!(
        frozen.sharing_state,
        kukuri_core::ChannelSharingState::Frozen
    );

    let freeze_post_id = runtime_b
        .create_post(CreatePostRequest {
            topic: topic.to_string(),
            content: "friend-plus frozen write".to_string(),
            reply_to: None,
            channel_ref: private_ref.clone(),
            attachments: Vec::new(),
            content_labels: Vec::new(),
        })
        .await
        .expect("write should continue after freeze");
    wait_for_timeline_object_in_scope(
        &runtime_c,
        topic,
        private_scope.clone(),
        freeze_post_id.as_str(),
        Duration::from_secs(10),
    )
    .await
    .expect("c receives frozen write");

    runtime_d
        .import_friend_plus_share(ImportFriendPlusShareRequest {
            token: stale_share_for_d.clone(),
        })
        .await
        .expect_err("frozen share should fail");

    let rotated = runtime_a
        .rotate_private_channel(RotatePrivateChannelRequest {
            topic: topic.to_string(),
            channel_id: channel.channel_id.clone(),
        })
        .await
        .expect("rotate friend-plus channel");

    timeout(Duration::from_secs(10), async {
        loop {
            let joined = runtime_c
                .list_joined_private_channels(ListJoinedPrivateChannelsRequest {
                    topic: topic.to_string(),
                })
                .await
                .expect("list joined channels on c");
            if joined.iter().any(|entry| {
                entry.channel_id == channel.channel_id
                    && entry.current_epoch_id == rotated.current_epoch_id
                    && entry.joined_via_pubkey.as_deref() == Some(b_pubkey.as_str())
            }) {
                return;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("c rotation redeem timeout");

    runtime_d
        .import_friend_plus_share(ImportFriendPlusShareRequest {
            token: stale_share_for_d,
        })
        .await
        .expect_err("old share should fail after rotate");

    let new_post_id = runtime_b
        .create_post(CreatePostRequest {
            topic: topic.to_string(),
            content: "friend-plus new".to_string(),
            reply_to: None,
            channel_ref: private_ref,
            attachments: Vec::new(),
            content_labels: Vec::new(),
        })
        .await
        .expect("create new epoch post");
    wait_for_timeline_object_in_scope(
        &runtime_c,
        topic,
        private_scope.clone(),
        new_post_id.as_str(),
        Duration::from_secs(10),
    )
    .await
    .expect("c receives new epoch post");

    let fresh_share = runtime_b
        .export_friend_plus_share(ExportFriendPlusShareRequest {
            topic: topic.to_string(),
            channel_id: channel.channel_id.clone(),
            expires_at: None,
        })
        .await
        .expect("export fresh share");
    wait_for_friend_plus_share_import(&runtime_d, fresh_share, social_graph_propagation_timeout())
        .await
        .expect("d imports fresh share");
    // 参加した channel の投稿は、参加で始まる購読の同期で届く(#1221 R2-C)。
    wait_for_timeline_object_in_scope(
        &runtime_d,
        topic,
        private_scope.clone(),
        new_post_id.as_str(),
        Duration::from_secs(10),
    )
    .await
    .expect("d receives new epoch post");
    let d_private_timeline = runtime_d
        .list_timeline(ListTimelineRequest {
            topic: topic.to_string(),
            scope: private_scope,
            cursor: None,
            limit: Some(20),
        })
        .await
        .expect("list d private timeline");
    assert!(
        d_private_timeline
            .items
            .iter()
            .all(|item| item.object_id != old_post_id)
    );
    assert!(
        d_private_timeline
            .items
            .iter()
            .any(|item| item.object_id == new_post_id)
    );

    shutdown_runtime(runtime_a, "friend-plus harness runtime a")
        .await
        .expect("shutdown runtime a");
    shutdown_runtime(runtime_b, "friend-plus harness runtime b")
        .await
        .expect("shutdown runtime b");
    shutdown_runtime(runtime_c, "friend-plus harness runtime c")
        .await
        .expect("shutdown runtime c");
    shutdown_runtime(runtime_d, "friend-plus harness runtime d")
        .await
        .expect("shutdown runtime d");
}
