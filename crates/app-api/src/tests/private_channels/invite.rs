use super::super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn private_channel_invite_scopes_posts_and_replies() {
    let _guard = iroh_integration_test_lock().lock_owned().await;
    let dir = tempdir().expect("tempdir");
    let stack_a = TestIrohStack::new(&dir.path().join("private-a")).await;
    let stack_b = TestIrohStack::new(&dir.path().join("private-b")).await;
    let store_a = Arc::new(MemoryStore::default());
    let store_b = Arc::new(MemoryStore::default());
    let app_a = app_with_iroh_services(store_a.clone(), &stack_a);
    let app_b = app_with_iroh_services(store_b, &stack_b);
    stack_a.bind_account(&app_a).await;
    stack_b.bind_account(&app_b).await;
    let topic = "kukuri:topic:private-channel";

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
    let _ = app_a
        .list_timeline(topic, None, 20)
        .await
        .expect("warm owner public timeline");
    let _ = app_b
        .list_timeline(topic, None, 20)
        .await
        .expect("warm invitee public timeline");
    wait_for_topic_delivery(&app_a, topic, 1).await;
    wait_for_topic_delivery(&app_b, topic, 1).await;

    let channel = app_a
        .create_private_channel(CreatePrivateChannelInput {
            topic_id: TopicId::new(topic),
            label: "core".into(),
            audience_kind: ChannelAudienceKind::InviteOnly,
        })
        .await
        .expect("create private channel");
    let invite = app_a
        .export_private_channel_invite(topic, channel.channel_id.as_str(), None)
        .await
        .expect("export invite");
    let preview = app_b
        .import_private_channel_invite(invite.as_str())
        .await
        .expect("import invite");
    assert_eq!(preview.channel_id.as_str(), channel.channel_id);

    let private_channel_id = ChannelId::new(channel.channel_id.clone());
    let private_ref = ChannelRef::PrivateChannel {
        channel_id: private_channel_id.clone(),
    };
    let private_scope = TimelineScope::Channel {
        channel_id: private_channel_id.clone(),
    };
    let _ = app_a
        .list_timeline_scoped(topic, private_scope.clone(), None, 20)
        .await
        .expect("warm owner private timeline");

    let object_id = app_a
        .create_post_in_channel(topic, private_ref.clone(), "private hello", None)
        .await
        .expect("create private post");

    let received = match timeout(p2p_replication_timeout(), async {
        loop {
            let public = app_b
                .list_timeline_scoped(topic, TimelineScope::Public, None, 20)
                .await
                .expect("public timeline");
            assert!(
                public.items.iter().all(|post| post.object_id != object_id),
                "private post leaked into public scope"
            );
            let private = app_b
                .list_timeline_scoped(topic, private_scope.clone(), None, 20)
                .await
                .expect("private timeline");
            if let Some(post) = private
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
    {
        Ok(post) => post,
        Err(_) => {
            let public = app_b
                .list_timeline_scoped(topic, TimelineScope::Public, None, 20)
                .await
                .expect("public timeline diagnostics");
            let private = app_b
                .list_timeline_scoped(topic, private_scope.clone(), None, 20)
                .await
                .expect("private timeline diagnostics");
            let joined = app_b
                .list_joined_private_channels(topic)
                .await
                .expect("joined private channels diagnostics");
            let status = app_b
                .get_sync_status()
                .await
                .expect("sync status diagnostics");
            panic!(
                "private timeline timeout; public={public:?}; private={private:?}; joined={joined:?}; status={status:?}"
            );
        }
    };
    assert_eq!(
        received.channel_id.as_deref(),
        Some(channel.channel_id.as_str())
    );

    let reply_id = app_b
        .create_post_in_channel(
            topic,
            ChannelRef::Public,
            "private reply",
            Some(object_id.as_str()),
        )
        .await
        .expect("reply in private channel");

    let thread = timeout(p2p_replication_timeout(), async {
        loop {
            let thread = app_b
                .list_thread(topic, object_id.as_str(), None, 20)
                .await
                .expect("thread b");
            if thread.items.iter().any(|post| post.object_id == reply_id) {
                return thread;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("private local thread timeout");
    let reply = thread
        .items
        .iter()
        .find(|post| post.object_id == reply_id)
        .expect("reply");
    assert_eq!(
        reply.channel_id.as_deref(),
        Some(channel.channel_id.as_str())
    );
    assert_eq!(reply.reply_to.as_deref(), Some(object_id.as_str()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn channel_access_preview_is_non_mutating_and_rejects_invalid_tokens() {
    let _guard = iroh_integration_test_lock().lock_owned().await;
    let dir = tempdir().expect("tempdir");
    let stack_a = TestIrohStack::new(&dir.path().join("preview-a")).await;
    let stack_b = TestIrohStack::new(&dir.path().join("preview-b")).await;
    let store_a = Arc::new(MemoryStore::default());
    let store_b = Arc::new(MemoryStore::default());
    let app_a = app_with_iroh_services(store_a, &stack_a);
    let app_b = app_with_iroh_services(store_b, &stack_b);
    stack_a.bind_account(&app_a).await;
    stack_b.bind_account(&app_b).await;
    let topic = "kukuri:topic:preview-channel";

    let channel = app_a
        .create_private_channel(CreatePrivateChannelInput {
            topic_id: TopicId::new(topic),
            label: "preview".into(),
            audience_kind: ChannelAudienceKind::InviteOnly,
        })
        .await
        .expect("create private channel");
    let invite = app_a
        .export_private_channel_invite(topic, channel.channel_id.as_str(), None)
        .await
        .expect("export invite");

    let joined_before = app_b
        .list_joined_private_channels(topic)
        .await
        .expect("joined before preview");
    assert!(
        joined_before.is_empty(),
        "preview should start without joined channels"
    );

    let preview = app_b
        .preview_channel_access_token(invite.as_str())
        .await
        .expect("preview invite");
    assert_eq!(preview.kind, ChannelAccessTokenKind::Invite);
    assert_eq!(preview.topic_id.as_str(), topic);
    assert_eq!(preview.channel_id.as_str(), channel.channel_id);

    let joined_after = app_b
        .list_joined_private_channels(topic)
        .await
        .expect("joined after preview");
    assert!(
        joined_after.is_empty(),
        "preview must not mutate joined channel state"
    );

    let invalid = app_b.preview_channel_access_token("not-a-token").await;
    assert!(invalid.is_err(), "invalid tokens should fail preview");
}
