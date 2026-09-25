use super::super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn friend_plus_share_freeze_rotate_and_new_epoch_visibility() {
    let _guard = iroh_integration_test_lock().lock_owned().await;
    if std::env::var_os("GITHUB_ACTIONS").is_some() {
        // CI covers the network path in the harness friend-plus connectivity scenario.
        return;
    }
    let dir = tempdir().expect("tempdir");
    let stack_a = TestIrohStack::new(&dir.path().join("friend-plus-a")).await;
    let stack_b = TestIrohStack::new(&dir.path().join("friend-plus-b")).await;
    let stack_c = TestIrohStack::new(&dir.path().join("friend-plus-c")).await;
    let stack_d = TestIrohStack::new(&dir.path().join("friend-plus-d")).await;
    let store_a = Arc::new(MemoryStore::default());
    let store_b = Arc::new(MemoryStore::default());
    let store_c = Arc::new(MemoryStore::default());
    let store_d = Arc::new(MemoryStore::default());
    let keys_a = generate_keys();
    let keys_b = generate_keys();
    let keys_c = generate_keys();
    let keys_d = generate_keys();
    let app_a = app_service_from_dependencies(
        store_a.clone(),
        store_a.clone(),
        stack_a.transport.clone(),
        stack_a.transport.clone(),
        stack_a.docs_sync.clone(),
        stack_a.blob_service.clone(),
        keys_a.clone(),
    );
    let app_b = app_service_from_dependencies(
        store_b.clone(),
        store_b.clone(),
        stack_b.transport.clone(),
        stack_b.transport.clone(),
        stack_b.docs_sync.clone(),
        stack_b.blob_service.clone(),
        keys_b.clone(),
    );
    let app_c = app_service_from_dependencies(
        store_c.clone(),
        store_c.clone(),
        stack_c.transport.clone(),
        stack_c.transport.clone(),
        stack_c.docs_sync.clone(),
        stack_c.blob_service.clone(),
        keys_c.clone(),
    );
    let app_d = app_service_from_dependencies(
        store_d.clone(),
        store_d.clone(),
        stack_d.transport.clone(),
        stack_d.transport.clone(),
        stack_d.docs_sync.clone(),
        stack_d.blob_service.clone(),
        keys_d.clone(),
    );
    for (stack, app) in [
        (&stack_a, &app_a),
        (&stack_b, &app_b),
        (&stack_c, &app_c),
        (&stack_d, &app_d),
    ] {
        stack.bind_account(app).await;
    }
    app_a.warm_social_graph().await.expect("warm a");
    app_b.warm_social_graph().await.expect("warm b");
    app_c.warm_social_graph().await.expect("warm c");
    app_d.warm_social_graph().await.expect("warm d");

    let ticket_a = stack_a
        .transport
        .export_ticket()
        .await
        .expect("ticket a")
        .expect("ticket a value");
    let ticket_b = stack_b
        .transport
        .export_ticket()
        .await
        .expect("ticket b")
        .expect("ticket b value");
    let ticket_c = stack_c
        .transport
        .export_ticket()
        .await
        .expect("ticket c")
        .expect("ticket c value");
    let ticket_d = stack_d
        .transport
        .export_ticket()
        .await
        .expect("ticket d")
        .expect("ticket d value");

    let a_pubkey = keys_a.public_key_hex();
    let b_pubkey = keys_b.public_key_hex();
    let c_pubkey = keys_c.public_key_hex();
    let d_pubkey = keys_d.public_key_hex();
    let topic = "kukuri:topic:friend-plus";
    let social_timeout = social_graph_propagation_timeout();
    let replication_timeout = p2p_replication_timeout();
    let rotation_timeout = p2p_replication_timeout().max(Duration::from_secs(120));

    app_a
        .import_peer_ticket(&ticket_b)
        .await
        .expect("a imports b");
    app_b
        .import_peer_ticket(&ticket_a)
        .await
        .expect("b imports a");

    for app in [&app_a, &app_b] {
        let _ = app
            .list_timeline(topic, None, 20)
            .await
            .expect("subscribe public timeline");
    }
    wait_for_topic_delivery(&app_a, topic, 1).await;
    wait_for_topic_delivery(&app_b, topic, 1).await;
    warm_author_social_view(&app_a, b_pubkey.as_str(), topic).await;
    warm_author_social_view(&app_b, a_pubkey.as_str(), topic).await;

    app_a
        .follow_author(b_pubkey.as_str())
        .await
        .expect("a follows b");
    app_b
        .follow_author(a_pubkey.as_str())
        .await
        .expect("b follows a");
    wait_for_mutual_author_view(&app_a, b_pubkey.as_str(), topic).await;
    wait_for_mutual_author_view(&app_b, a_pubkey.as_str(), topic).await;

    let channel = app_a
        .create_private_channel(CreatePrivateChannelInput {
            topic_id: TopicId::new(topic),
            label: "friends+".into(),
            audience_kind: ChannelAudienceKind::FriendPlus,
        })
        .await
        .expect("create friend-plus channel");
    let share_ab = app_a
        .export_friend_plus_share(topic, channel.channel_id.as_str(), None)
        .await
        .expect("export a->b share");
    let preview_b =
        wait_for_friend_plus_share_import(&app_b, share_ab.as_str(), social_timeout).await;
    assert_eq!(preview_b.channel_id.as_str(), channel.channel_id);

    let private_channel_id = ChannelId::new(channel.channel_id.clone());
    let private_scope = TimelineScope::Channel {
        channel_id: private_channel_id.clone(),
    };
    let private_ref = ChannelRef::PrivateChannel {
        channel_id: private_channel_id.clone(),
    };
    let _ = app_a
        .list_timeline_scoped(topic, private_scope.clone(), None, 20)
        .await
        .expect("warm private timeline a");
    let _ = app_b
        .list_timeline_scoped(topic, private_scope.clone(), None, 20)
        .await
        .expect("warm private timeline b");
    let old_post_id = app_a
        .create_post_in_channel(topic, private_ref.clone(), "friends+ old", None)
        .await
        .expect("create old friend-plus post");

    timeout(replication_timeout, async {
        loop {
            let private_b = app_b
                .list_timeline_scoped(topic, private_scope.clone(), None, 20)
                .await
                .expect("private timeline b");
            if private_b
                .items
                .iter()
                .any(|post| post.object_id == old_post_id)
            {
                break;
            }
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("friend-plus old post propagation to b timeout");

    app_a
        .import_peer_ticket(&ticket_c)
        .await
        .expect("a imports c");
    app_c
        .import_peer_ticket(&ticket_a)
        .await
        .expect("c imports a");
    app_b
        .import_peer_ticket(&ticket_c)
        .await
        .expect("b imports c");
    app_c
        .import_peer_ticket(&ticket_b)
        .await
        .expect("c imports b");

    let _ = app_c
        .list_timeline(topic, None, 20)
        .await
        .expect("subscribe public timeline c");
    wait_for_topic_delivery(&app_a, topic, 1).await;
    wait_for_topic_delivery(&app_b, topic, 1).await;
    wait_for_topic_delivery(&app_c, topic, 1).await;
    warm_author_social_view(&app_b, c_pubkey.as_str(), topic).await;
    warm_author_social_view(&app_c, b_pubkey.as_str(), topic).await;

    app_b
        .follow_author(c_pubkey.as_str())
        .await
        .expect("b follows c");
    app_c
        .follow_author(b_pubkey.as_str())
        .await
        .expect("c follows b");
    wait_for_mutual_author_view(&app_b, c_pubkey.as_str(), topic).await;
    wait_for_mutual_author_view(&app_c, b_pubkey.as_str(), topic).await;

    let share_bc = app_b
        .export_friend_plus_share(topic, channel.channel_id.as_str(), None)
        .await
        .expect("export b->c share");
    let preview_c =
        wait_for_friend_plus_share_import(&app_c, share_bc.as_str(), social_timeout).await;
    assert_eq!(preview_c.sponsor_pubkey.as_str(), b_pubkey);

    let public_c = app_c
        .list_timeline_scoped(topic, TimelineScope::Public, None, 20)
        .await
        .expect("public c");
    assert!(
        public_c
            .items
            .iter()
            .all(|post| post.object_id != old_post_id)
    );

    timeout(replication_timeout, async {
        loop {
            let private_c = app_c
                .list_timeline_scoped(topic, private_scope.clone(), None, 20)
                .await
                .expect("private timeline c");
            if private_c
                .items
                .iter()
                .any(|post| post.object_id == old_post_id)
            {
                break;
            }
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("friend-plus old post propagation to c timeout");

    let stale_share_for_d = app_b
        .export_friend_plus_share(topic, channel.channel_id.as_str(), None)
        .await
        .expect("export b->d share");
    let stale_preview_d = kukuri_core::parse_friend_plus_share_token(stale_share_for_d.as_str())
        .expect("parse stale friend-plus share");

    let frozen = app_a
        .freeze_private_channel(topic, channel.channel_id.as_str())
        .await
        .expect("freeze friend-plus channel");
    assert_eq!(frozen.sharing_state, ChannelSharingState::Frozen);

    let freeze_post_id = app_b
        .create_post_in_channel(topic, private_ref.clone(), "friends+ frozen write", None)
        .await
        .expect("write should continue after freeze");

    timeout(replication_timeout, async {
        loop {
            let private_a = app_a
                .list_timeline_scoped(topic, private_scope.clone(), None, 20)
                .await
                .expect("private timeline a");
            let private_c = app_c
                .list_timeline_scoped(topic, private_scope.clone(), None, 20)
                .await
                .expect("private timeline c after freeze");
            if private_a
                .items
                .iter()
                .any(|post| post.object_id == freeze_post_id)
                && private_c
                    .items
                    .iter()
                    .any(|post| post.object_id == freeze_post_id)
            {
                break;
            }
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("friend-plus frozen write propagation timeout");

    app_b
        .import_peer_ticket(&ticket_d)
        .await
        .expect("b imports d");
    app_d
        .import_peer_ticket(&ticket_b)
        .await
        .expect("d imports b");
    let _ = app_d
        .list_timeline(topic, None, 20)
        .await
        .expect("subscribe public timeline d");
    wait_for_topic_delivery(&app_b, topic, 1).await;
    wait_for_topic_delivery(&app_d, topic, 1).await;
    warm_author_social_view(&app_b, d_pubkey.as_str(), topic).await;
    warm_author_social_view(&app_d, b_pubkey.as_str(), topic).await;

    app_b
        .follow_author(d_pubkey.as_str())
        .await
        .expect("b follows d");
    app_d
        .follow_author(b_pubkey.as_str())
        .await
        .expect("d follows b");
    wait_for_mutual_author_view(&app_b, d_pubkey.as_str(), topic).await;
    wait_for_mutual_author_view(&app_d, b_pubkey.as_str(), topic).await;

    let freeze_error_message =
        wait_for_friend_plus_share_rejection(&app_d, stale_share_for_d.as_str(), rotation_timeout)
            .await;
    assert_eq!(
        freeze_error_message,
        "friend-plus share is no longer open for import"
    );

    let rotated = app_a
        .rotate_private_channel(topic, channel.channel_id.as_str())
        .await
        .expect("rotate friend-plus channel");
    let rotated_source_replica = private_channel_epoch_replica_id(
        channel.channel_id.as_str(),
        stale_preview_d.epoch_id.as_str(),
    );
    assert!(
        fetch_private_channel_epoch_handoff_grant_from_replica(
            app_a.services.docs_sync.as_ref(),
            &rotated_source_replica,
            b_pubkey.as_str(),
            DocFetchPolicy::LocalThenRemote,
        )
        .await
        .expect("fetch published handoff grant")
        .is_some()
    );
    assert_ne!(rotated.current_epoch_id, stale_preview_d.epoch_id);
    assert!(!rotated.archived_epoch_ids.is_empty());
    assert!(
        rotated
            .archived_epoch_ids
            .iter()
            .any(|epoch_id| epoch_id == &stale_preview_d.epoch_id)
    );

    let joined_b = match timeout(rotation_timeout, async {
        loop {
            let joined = app_b
                .list_joined_private_channels(topic)
                .await
                .expect("list joined on b");
            let Some(item) = joined
                .iter()
                .find(|entry| entry.channel_id == channel.channel_id)
            else {
                sleep(Duration::from_millis(50)).await;
                continue;
            };
            if item.current_epoch_id == rotated.current_epoch_id {
                break item.clone();
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    {
        Ok(item) => item,
        Err(_) => {
            let joined = app_b
                .list_joined_private_channels(topic)
                .await
                .expect("list joined on b after timeout");
            let current = joined
                .iter()
                .find(|entry| entry.channel_id == channel.channel_id)
                .cloned();
            let grant_visible = fetch_private_channel_epoch_handoff_grant_from_replica(
                app_b.services.docs_sync.as_ref(),
                &rotated_source_replica,
                b_pubkey.as_str(),
                DocFetchPolicy::LocalThenRemote,
            )
            .await
            .expect("fetch handoff grant on b")
            .is_some();
            let snapshot = app_b
                .get_sync_status()
                .await
                .map(|status| format_sync_snapshot(&status, topic))
                .unwrap_or_else(|_| "failed to read sync status".to_string());
            panic!(
                "b rotation redeem timeout; current={current:?}, grant_visible={grant_visible}, {snapshot}"
            );
        }
    };
    assert_eq!(
        joined_b.joined_via_pubkey.as_deref(),
        Some(a_pubkey.as_str())
    );
    assert!(
        joined_b
            .archived_epoch_ids
            .iter()
            .any(|epoch_id| epoch_id == &preview_b.epoch_id)
    );

    let joined_c = timeout(rotation_timeout, async {
        loop {
            let joined = app_c
                .list_joined_private_channels(topic)
                .await
                .expect("list joined on c");
            let Some(item) = joined
                .iter()
                .find(|entry| entry.channel_id == channel.channel_id)
            else {
                sleep(Duration::from_millis(50)).await;
                continue;
            };
            if item.current_epoch_id == rotated.current_epoch_id {
                break item.clone();
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("c rotation redeem timeout");
    assert_eq!(
        joined_c.joined_via_pubkey.as_deref(),
        Some(b_pubkey.as_str())
    );
    assert!(
        joined_c
            .archived_epoch_ids
            .iter()
            .any(|epoch_id| epoch_id == &preview_c.epoch_id)
    );

    let old_share_error_message =
        wait_for_friend_plus_share_rejection(&app_d, stale_share_for_d.as_str(), rotation_timeout)
            .await;
    assert_eq!(
        old_share_error_message,
        "friend-plus share is no longer open for import"
    );

    let new_post_id = app_b
        .create_post_in_channel(topic, private_ref.clone(), "friends+ new", None)
        .await
        .expect("create new epoch post");

    timeout(replication_timeout, async {
        loop {
            let private_a = app_a
                .list_timeline_scoped(topic, private_scope.clone(), None, 20)
                .await
                .expect("private timeline a after rotate");
            let private_c = app_c
                .list_timeline_scoped(topic, private_scope.clone(), None, 20)
                .await
                .expect("private timeline c after rotate");
            if private_a
                .items
                .iter()
                .any(|post| post.object_id == new_post_id)
                && private_c
                    .items
                    .iter()
                    .any(|post| post.object_id == new_post_id)
            {
                break;
            }
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("friend-plus new epoch propagation timeout");

    let fresh_share = app_b
        .export_friend_plus_share(topic, channel.channel_id.as_str(), None)
        .await
        .expect("export fresh friend-plus share");
    let preview_d =
        wait_for_friend_plus_share_import(&app_d, fresh_share.as_str(), rotation_timeout).await;
    assert_eq!(preview_d.epoch_id, rotated.current_epoch_id);

    // #1221 R5-C: 取込みは参加に要る制御 record だけを読み、epoch の全体 sync を待たない。新しい epoch の投稿は
    // 通常のページの読取りと購読で表示へ届く。
    let d_private = timeout(replication_timeout, async {
        loop {
            let page = app_d
                .list_timeline_scoped(topic, private_scope.clone(), None, 20)
                .await
                .expect("d private timeline");
            if page.items.iter().any(|post| post.object_id == new_post_id) {
                break page;
            }
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("d sees the new epoch post");
    assert!(
        d_private
            .items
            .iter()
            .all(|post| post.object_id != old_post_id)
    );
}
