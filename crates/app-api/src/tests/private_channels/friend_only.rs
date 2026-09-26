use super::super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn friend_only_grant_requires_mutual_and_rotate_requires_fresh_grant() {
    let _guard = iroh_integration_test_lock().lock_owned().await;
    let dir = tempdir().expect("tempdir");
    let stack_a = TestIrohStack::new(&dir.path().join("friend-only-a")).await;
    let stack_b = TestIrohStack::new(&dir.path().join("friend-only-b")).await;
    let stack_c = TestIrohStack::new(&dir.path().join("friend-only-c")).await;
    let stack_d = TestIrohStack::new(&dir.path().join("friend-only-d")).await;
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
    app_a
        .import_peer_ticket(&ticket_b)
        .await
        .expect("a imports b");
    app_b
        .import_peer_ticket(&ticket_a)
        .await
        .expect("b imports a");
    app_a
        .import_peer_ticket(&ticket_d)
        .await
        .expect("a imports d");
    app_d
        .import_peer_ticket(&ticket_a)
        .await
        .expect("d imports a");

    let a_pubkey = keys_a.public_key_hex();
    let b_pubkey = keys_b.public_key_hex();
    let d_pubkey = keys_d.public_key_hex();
    let topic = "kukuri:topic:friend-only";

    for app in [&app_a, &app_b, &app_d] {
        display_topic(app, topic)
            .await
            .expect("subscribe public timeline");
    }
    wait_for_topic_delivery(&app_a, topic, 1).await;
    wait_for_topic_delivery(&app_b, topic, 1).await;
    wait_for_topic_delivery(&app_d, topic, 1).await;
    warm_author_social_view(&app_a, b_pubkey.as_str(), topic).await;
    warm_author_social_view(&app_b, a_pubkey.as_str(), topic).await;
    warm_author_social_view(&app_a, d_pubkey.as_str(), topic).await;
    warm_author_social_view(&app_d, a_pubkey.as_str(), topic).await;

    app_a
        .follow_author(b_pubkey.as_str())
        .await
        .expect("a follows b");
    app_b
        .follow_author(a_pubkey.as_str())
        .await
        .expect("b follows a");
    app_a
        .follow_author(d_pubkey.as_str())
        .await
        .expect("a follows d");
    app_d
        .follow_author(a_pubkey.as_str())
        .await
        .expect("d follows a");
    wait_for_mutual_author_view(&app_a, b_pubkey.as_str(), topic).await;
    wait_for_mutual_author_view(&app_b, a_pubkey.as_str(), topic).await;
    wait_for_mutual_author_view(&app_a, d_pubkey.as_str(), topic).await;
    wait_for_mutual_author_view(&app_d, a_pubkey.as_str(), topic).await;

    let channel = app_a
        .create_private_channel(CreatePrivateChannelInput {
            topic_id: TopicId::new(topic),
            label: "friends".into(),
            audience_kind: ChannelAudienceKind::FriendOnly,
        })
        .await
        .expect("create friend-only channel");
    let grant = app_a
        .export_friend_only_grant(topic, channel.channel_id.as_str(), None)
        .await
        .expect("export friend-only grant");
    let preview = wait_for_friend_only_grant_import(
        &app_b,
        grant.as_str(),
        social_graph_propagation_timeout(),
    )
    .await;
    assert_eq!(preview.channel_id.as_str(), channel.channel_id);

    app_a
        .import_peer_ticket(&ticket_c)
        .await
        .expect("a imports c");
    app_c
        .import_peer_ticket(&ticket_a)
        .await
        .expect("c imports a");
    display_topic(&app_c, topic)
        .await
        .expect("subscribe public timeline c");
    wait_for_topic_delivery(&app_c, topic, 1).await;
    warm_author_social_view(&app_c, a_pubkey.as_str(), topic).await;

    let non_mutual_error = app_c
        .import_friend_only_grant(grant.as_str())
        .await
        .expect_err("c should not join without mutual");
    assert_eq!(
        non_mutual_error.to_string(),
        "friend-only grant import requires a mutual relationship with the channel owner"
    );

    let private_channel_id = ChannelId::new(channel.channel_id.clone());
    let private_scope = TimelineScope::Channel {
        channel_id: private_channel_id.clone(),
    };
    let private_ref = ChannelRef::PrivateChannel {
        channel_id: private_channel_id.clone(),
    };
    let object_id = app_a
        .create_post_in_channel(topic, private_ref, "friends hello", None)
        .await
        .expect("create friend-only post");

    timeout(Duration::from_secs(10), async {
        loop {
            let public = app_b
                .list_timeline_scoped(topic, TimelineScope::Public, None, 20)
                .await
                .expect("public");
            assert!(public.items.iter().all(|post| post.object_id != object_id));
            let private = app_b
                .list_timeline_scoped(topic, private_scope.clone(), None, 20)
                .await
                .expect("private");
            if private.items.iter().any(|post| post.object_id == object_id) {
                break;
            }
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("friend-only post propagation timeout");

    app_a
        .unfollow_author(b_pubkey.as_str())
        .await
        .expect("a unfollows b");
    let joined_a = app_a
        .list_joined_private_channels(topic)
        .await
        .expect("list joined channels on a");
    let channel_a = joined_a
        .into_iter()
        .find(|entry| entry.channel_id == channel.channel_id)
        .expect("friend-only channel view");
    assert!(channel_a.rotation_required);
    assert_eq!(channel_a.stale_participant_count, 1);

    let rotated = app_a
        .rotate_private_channel(topic, channel.channel_id.as_str())
        .await
        .expect("rotate friend-only channel");
    assert_ne!(rotated.current_epoch_id, channel_a.current_epoch_id);
    assert_eq!(
        rotated.archived_epoch_ids,
        vec![channel_a.current_epoch_id.clone()]
    );

    app_d
        .import_friend_only_grant(grant.as_str())
        .await
        .expect_err("stale old grant should fail");

    let fresh_grant = app_a
        .export_friend_only_grant(topic, channel.channel_id.as_str(), None)
        .await
        .expect("export fresh friend-only grant");
    display_topic(&app_a, topic)
        .await
        .expect("resubscribe a before fresh grant");
    display_topic(&app_d, topic)
        .await
        .expect("resubscribe d before fresh grant");
    wait_for_topic_delivery(&app_a, topic, 1).await;
    wait_for_topic_delivery(&app_d, topic, 1).await;
    warm_author_social_view(&app_a, d_pubkey.as_str(), topic).await;
    warm_author_social_view(&app_d, a_pubkey.as_str(), topic).await;
    wait_for_mutual_author_view(&app_a, d_pubkey.as_str(), topic).await;
    wait_for_mutual_author_view(&app_d, a_pubkey.as_str(), topic).await;
    let fresh_preview = wait_for_friend_only_grant_import(
        &app_d,
        fresh_grant.as_str(),
        social_graph_propagation_timeout(),
    )
    .await;
    assert_eq!(fresh_preview.epoch_id, rotated.current_epoch_id);

    let d_private = app_d
        .list_timeline_scoped(topic, private_scope.clone(), None, 20)
        .await
        .expect("d private timeline after rotate");
    assert!(
        d_private
            .items
            .iter()
            .all(|post| post.object_id != object_id)
    );
}
