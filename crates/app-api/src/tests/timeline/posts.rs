use super::super::*;

#[tokio::test]
async fn create_post_and_list_timeline() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(FakeTransport::new("app", FakeNetwork::default()));
    let app = AppService::new(store, transport);

    let object_id = app
        .create_post("kukuri:topic:api", "hello app", None)
        .await
        .expect("create post");
    let timeline = app
        .list_timeline("kukuri:topic:api", None, 10)
        .await
        .expect("timeline");

    assert_eq!(timeline.items.len(), 1);
    assert_eq!(timeline.items[0].object_id, object_id);
    assert_eq!(timeline.items[0].content, "hello app");
}

#[tokio::test]
async fn community_index_resolution_is_canonical_and_fail_closed() {
    let (app, store, _, _) = local_app_with_memory_services();
    let topic = "kukuri:topic:index-resolution";
    let object_id = app
        .create_post(topic, "canonical body", None)
        .await
        .expect("create post");
    let post = app
        .list_timeline(topic, None, 20)
        .await
        .expect("timeline")
        .items
        .into_iter()
        .find(|post| post.object_id == object_id)
        .expect("created post");
    ObjectProjectionStore::rebuild_object_projections(store.as_ref(), Vec::new())
        .await
        .expect("clear projections before resolution");

    let response = app
        .resolve_community_index_posts(vec![
            CommunityIndexPostResolveInput {
                source_replica_id: None,
                key: "resolved".into(),
                topic: topic.into(),
                object_id: object_id.clone(),
                author_pubkey: post.author_pubkey.clone(),
                channel_ref: ChannelRef::Public,
            },
            CommunityIndexPostResolveInput {
                source_replica_id: None,
                key: "wrong-author".into(),
                topic: topic.into(),
                object_id: object_id.clone(),
                author_pubkey: "another-author".into(),
                channel_ref: ChannelRef::Public,
            },
            CommunityIndexPostResolveInput {
                source_replica_id: None,
                key: "missing".into(),
                topic: topic.into(),
                object_id: "missing-object".into(),
                author_pubkey: post.author_pubkey.clone(),
                channel_ref: ChannelRef::Public,
            },
        ])
        .await
        .expect("resolve index posts");

    assert_eq!(response.entries.len(), 3);
    let resolved = &response.entries[0];
    assert_eq!(
        resolved.post.as_ref().map(|post| post.content.as_str()),
        Some("canonical body")
    );
    assert!(resolved.capabilities.open_thread);
    assert!(resolved.capabilities.reply);
    assert!(resolved.capabilities.repost);
    assert!(resolved.capabilities.quote_repost);
    assert!(resolved.capabilities.react);
    assert!(resolved.capabilities.copy_link);
    assert!(resolved.capabilities.bookmark);
    assert!(resolved.capabilities.withdraw);

    for unresolved in &response.entries[1..] {
        assert!(unresolved.post.is_none());
        assert_eq!(
            unresolved.capabilities,
            CommunityIndexPostActionCapabilitiesView::default()
        );
    }
}

#[tokio::test]
async fn community_index_resolution_preserves_private_scope_capabilities() {
    let (app, store, _, _) = local_app_with_memory_services();
    let topic = "kukuri:topic:index-private-resolution";
    let channel = app
        .create_private_channel(CreatePrivateChannelInput {
            topic_id: TopicId::new(topic),
            label: "private index".into(),
            audience_kind: ChannelAudienceKind::InviteOnly,
        })
        .await
        .expect("create private channel");
    let channel_id = ChannelId::new(channel.channel_id.clone());
    let object_id = app
        .create_post_in_channel(
            topic,
            ChannelRef::PrivateChannel {
                channel_id: channel_id.clone(),
            },
            "private canonical body",
            None,
        )
        .await
        .expect("create private post");
    let author_pubkey = app.current_author_pubkey();
    ObjectProjectionStore::rebuild_object_projections(store.as_ref(), Vec::new())
        .await
        .expect("clear projections before resolution");

    let response = app
        .resolve_community_index_posts(vec![CommunityIndexPostResolveInput {
            source_replica_id: None,
            key: "private".into(),
            topic: topic.into(),
            object_id,
            author_pubkey,
            channel_ref: ChannelRef::PrivateChannel { channel_id },
        }])
        .await
        .expect("resolve private index post");
    let resolved = &response.entries[0];

    assert_eq!(
        resolved
            .post
            .as_ref()
            .and_then(|post| post.channel_id.as_deref()),
        Some(channel.channel_id.as_str())
    );
    assert!(resolved.capabilities.open_thread);
    assert!(resolved.capabilities.reply);
    assert!(!resolved.capabilities.repost);
    assert!(!resolved.capabilities.quote_repost);
    assert!(resolved.capabilities.react);
    assert!(resolved.capabilities.copy_link);
    assert!(resolved.capabilities.bookmark);
    assert!(resolved.capabilities.withdraw);
}

#[tokio::test]
async fn author_withdrawal_scrubs_timeline_bookmark_and_durable_docs_state() {
    let (app, _store, docs, _blobs) = local_app_with_memory_services();
    let topic = "kukuri:topic:withdrawal";
    let object_id = app
        .create_post_with_attachments(
            topic,
            "sensitive body",
            None,
            vec![pending_image_attachment("image/png", &tiny_png_bytes())],
        )
        .await
        .expect("create post");
    app.bookmark_post(topic, &object_id)
        .await
        .expect("bookmark before withdrawal");

    app.withdraw_post(
        topic,
        &object_id,
        ChannelRef::Public,
        None,
        WithdrawalReasonVisibility::Public,
        Some(PostWithdrawalReason::AuthorRequest),
    )
    .await
    .expect("withdraw post");

    let timeline = app.list_timeline(topic, None, 10).await.expect("timeline");
    let withdrawn = &timeline.items[0];
    assert_eq!(withdrawn.object_id, object_id);
    assert_eq!(withdrawn.content, "");
    assert!(withdrawn.attachments.is_empty());
    assert!(withdrawn.withdrawal.is_some());

    let bookmarks = app.list_bookmarked_posts().await.expect("bookmarks");
    assert_eq!(bookmarks[0].post.content, "");
    assert!(bookmarks[0].post.attachments.is_empty());
    assert!(bookmarks[0].post.withdrawal.is_some());

    let records = docs
        .query_replica(
            &topic_replica_id(topic),
            DocQuery::Exact(stable_key("withdrawals", &format!("{object_id}/state"))),
        )
        .await
        .expect("withdrawal docs query");
    assert_eq!(records.len(), 1);
}

#[tokio::test]
async fn non_author_cannot_withdraw_a_post() {
    let (author, _author_keys, other, _other_keys, _store, _docs, _blobs) =
        shared_apps_with_memory_services();
    let topic = "kukuri:topic:withdrawal-auth";
    let object_id = author
        .create_post(topic, "author content", None)
        .await
        .expect("create post");
    let error = other
        .withdraw_post(
            topic,
            &object_id,
            ChannelRef::Public,
            None,
            WithdrawalReasonVisibility::Public,
            Some(PostWithdrawalReason::AuthorRequest),
        )
        .await
        .expect_err("non-author withdrawal must fail");
    assert!(error.to_string().contains("original author"));
}

#[tokio::test]
async fn same_identity_can_withdraw_after_restoring_only_durable_docs() {
    let docs_sync = Arc::new(MemoryDocsSync::default());
    let blob_service = Arc::new(MemoryBlobService::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let keys = generate_keys();
    let source_store = Arc::new(MemoryStore::default());
    let source = app_service_from_dependencies(
        source_store.clone(),
        source_store,
        transport.clone(),
        Arc::new(NoopHintTransport),
        docs_sync.clone(),
        blob_service.clone(),
        keys.clone(),
    );
    let restored_store = Arc::new(MemoryStore::default());
    let restored = app_service_from_dependencies(
        restored_store.clone(),
        restored_store,
        transport,
        Arc::new(NoopHintTransport),
        docs_sync,
        blob_service,
        keys,
    );
    let topic = "kukuri:topic:withdraw-restored-docs";
    let object_id = source
        .create_post(topic, "restore my signed target", None)
        .await
        .expect("create source post");

    restored
        .withdraw_post(
            topic,
            object_id.as_str(),
            ChannelRef::Public,
            None,
            WithdrawalReasonVisibility::Public,
            Some(PostWithdrawalReason::AuthorRequest),
        )
        .await
        .expect("withdraw after docs-only restore");
    let timeline = restored
        .list_timeline(topic, None, 20)
        .await
        .expect("restored timeline");
    let withdrawn = timeline
        .items
        .iter()
        .find(|post| post.object_id == object_id)
        .expect("withdrawn post");

    assert!(withdrawn.withdrawal.is_some());
    assert!(withdrawn.content.is_empty());
}
