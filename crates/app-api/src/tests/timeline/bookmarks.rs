use super::super::*;

#[tokio::test]
async fn bookmark_pages_can_seek_back_without_loading_every_saved_post() {
    let (app, _, _, _) = local_app_with_memory_services();
    let topic = "kukuri:topic:bookmark-pages";
    let mut ids = Vec::new();
    for index in 0..30 {
        let id = app
            .create_post(topic, &format!("saved post {index}"), None)
            .await
            .expect("create post");
        app.bookmark_post(topic, id.as_str())
            .await
            .expect("bookmark post");
        ids.push(id);
    }
    let first = app
        .list_bookmarked_posts_page(None, false)
        .await
        .expect("first page");
    assert_eq!(first.items.len(), 20);
    assert!(first.newer_cursor.is_none());
    let older = app
        .list_bookmarked_posts_page(first.older_cursor.as_ref(), false)
        .await
        .expect("older page");
    assert_eq!(older.items.len(), 10);
    assert!(older.older_cursor.is_none());
    let newer = app
        .list_bookmarked_posts_page(older.newer_cursor.as_ref(), true)
        .await
        .expect("newer page");
    assert_eq!(newer.items.len(), 20);
    assert_eq!(newer.items, first.items);
    assert_eq!(
        app.bookmarked_post_ids(&[
            EnvelopeId::from(ids[0].clone()),
            EnvelopeId::from("missing")
        ])
        .await
        .expect("membership"),
        vec![ids[0].clone()],
    );
}

#[tokio::test]
async fn removing_the_last_bookmark_does_not_reopen_an_empty_page_on_refresh() {
    let (app, _, _, _) = local_app_with_memory_services();
    let topic = "kukuri:topic:bookmark-last-page";
    for index in 0..21 {
        let id = app
            .create_post(topic, &format!("saved post {index}"), None)
            .await
            .expect("create post");
        app.bookmark_post(topic, id.as_str())
            .await
            .expect("bookmark post");
    }
    let first = app
        .list_bookmarked_posts_page(None, false)
        .await
        .expect("first page");
    let last = app
        .list_bookmarked_posts_page(first.older_cursor.as_ref(), false)
        .await
        .expect("last page");
    assert_eq!(last.items.len(), 1);
    app.remove_bookmarked_post(last.items[0].post.object_id.as_str())
        .await
        .expect("remove last bookmark");
    for _ in 0..2 {
        let previous = app
            .list_bookmarked_posts_page(last.newer_cursor.as_ref(), true)
            .await
            .expect("previous page after removal");
        assert_eq!(previous.items.len(), 20);
        assert!(previous.older_cursor.is_none());
    }
}

#[tokio::test]
async fn bookmark_hydrates_a_missing_target_projection() {
    let (app, store, _, _) = local_app_with_memory_services();
    let topic = "kukuri:topic:bookmark-hydration";
    let object_id = app
        .create_post(topic, "hydrate before bookmarking", None)
        .await
        .expect("create post");
    ObjectProjectionStore::rebuild_object_projections(store.as_ref(), Vec::new())
        .await
        .expect("clear projections");

    let bookmarked = app
        .bookmark_post_in_channel(topic, object_id.as_str(), ChannelRef::Public)
        .await
        .expect("bookmark after hydration");

    assert_eq!(bookmarked.post.object_id, object_id);
    assert_eq!(bookmarked.post.content, "hydrate before bookmarking");
}

#[tokio::test]
async fn local_bookmarked_posts_restore_after_restart() {
    let dir = tempdir().expect("tempdir");
    let database_path = dir.path().join("bookmark-post-store.sqlite");
    let store = Arc::new(
        SqliteStore::connect_file(&database_path)
            .await
            .expect("sqlite store"),
    );
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let docs_sync = Arc::new(MemoryDocsSync::default());
    let blob_service = Arc::new(MemoryBlobService::default());
    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        transport.clone(),
        Arc::new(NoopHintTransport),
        docs_sync.clone(),
        blob_service.clone(),
        generate_keys(),
    );
    let topic = "kukuri:topic:bookmark-restore";
    let object_id = app
        .create_post(topic, "saved for restart", None)
        .await
        .expect("create post");

    let bookmarked = app
        .bookmark_post(topic, object_id.as_str())
        .await
        .expect("bookmark post");
    assert_eq!(bookmarked.post.object_id, object_id);

    drop(app);
    store.close().await;

    let reopened = Arc::new(
        SqliteStore::connect_file(&database_path)
            .await
            .expect("reopen sqlite store"),
    );
    let reopened_app = app_service_from_dependencies(
        reopened.clone(),
        reopened.clone(),
        transport,
        Arc::new(NoopHintTransport),
        docs_sync,
        blob_service,
        generate_keys(),
    );
    let bookmarks = reopened_app
        .list_bookmarked_posts_page(None, false)
        .await
        .expect("list bookmarked posts after restart")
        .items;

    assert_eq!(bookmarks.len(), 1);
    assert_eq!(bookmarks[0].post.object_id, object_id);
    assert_eq!(bookmarks[0].post.content, "saved for restart");
    assert!(bookmarks[0].bookmarked_at > 0);
}

#[tokio::test]
async fn bookmark_private_post_remains_local_only_and_readable_after_access_loss() {
    let (app, store, _, _) = local_app_with_memory_services();
    let topic = "kukuri:topic:bookmark-private";
    let channel = app
        .create_private_channel(CreatePrivateChannelInput {
            topic_id: TopicId::new(topic),
            label: "quiet room".into(),
            audience_kind: ChannelAudienceKind::InviteOnly,
        })
        .await
        .expect("create private channel");
    let channel_ref = ChannelRef::PrivateChannel {
        channel_id: ChannelId::new(channel.channel_id.clone()),
    };
    let object_id = app
        .create_post_in_channel(topic, channel_ref, "private bookmark body", None)
        .await
        .expect("create private post");

    app.bookmark_post(topic, object_id.as_str())
        .await
        .expect("bookmark private post");
    app.joined_private_channels.lock().await.clear();
    ObjectProjectionStore::rebuild_object_projections(store.as_ref(), Vec::new())
        .await
        .expect("clear object projections");

    let bookmarks = app
        .list_bookmarked_posts_page(None, false)
        .await
        .expect("list bookmarked private posts")
        .items;

    assert_eq!(bookmarks.len(), 1);
    assert_eq!(bookmarks[0].post.object_id, object_id);
    assert_eq!(bookmarks[0].post.content, "private bookmark body");
    assert_eq!(
        bookmarks[0].post.channel_id.as_deref(),
        Some(channel.channel_id.as_str())
    );
    assert_eq!(bookmarks[0].post.audience_label, "Private channel");
}

#[tokio::test]
async fn unbookmark_removes_only_local_bookmark_record() {
    let (app, store, _, _) = local_app_with_memory_services();
    let topic = "kukuri:topic:bookmark-remove";
    let object_id = app
        .create_post(topic, "still on timeline", None)
        .await
        .expect("create post");

    app.bookmark_post(topic, object_id.as_str())
        .await
        .expect("bookmark post");
    app.remove_bookmarked_post(object_id.as_str())
        .await
        .expect("remove bookmark");

    let bookmarks = app
        .list_bookmarked_posts_page(None, false)
        .await
        .expect("list bookmarks")
        .items;
    let timeline = app
        .list_timeline(topic, None, 20)
        .await
        .expect("list timeline");
    let projection = ObjectProjectionStore::get_object_projection(
        store.as_ref(),
        &EnvelopeId::from(object_id.clone()),
    )
    .await
    .expect("get projection");

    assert!(bookmarks.is_empty());
    assert!(
        timeline
            .items
            .iter()
            .any(|post| post.object_id == object_id)
    );
    assert!(projection.is_some());
}

#[tokio::test]
async fn bookmarked_posts_are_sorted_by_bookmarked_at_desc() {
    let (app, _, _, _) = local_app_with_memory_services();
    let topic = "kukuri:topic:bookmark-order";
    let first_id = app
        .create_post(topic, "first saved", None)
        .await
        .expect("create first post");
    app.bookmark_post(topic, first_id.as_str())
        .await
        .expect("bookmark first post");
    sleep(Duration::from_millis(5)).await;
    let second_id = app
        .create_post(topic, "second saved", None)
        .await
        .expect("create second post");
    app.bookmark_post(topic, second_id.as_str())
        .await
        .expect("bookmark second post");

    let bookmarks = app
        .list_bookmarked_posts_page(None, false)
        .await
        .expect("list bookmarks")
        .items;

    assert_eq!(bookmarks.len(), 2);
    assert_eq!(bookmarks[0].post.object_id, second_id);
    assert_eq!(bookmarks[1].post.object_id, first_id);
    assert!(bookmarks[0].bookmarked_at >= bookmarks[1].bookmarked_at);
}

#[tokio::test]
async fn bookmarked_repost_renders_from_saved_snapshot_without_source_timeline_hydration() {
    let (app, store, _, _) = local_app_with_memory_services();
    let source_topic = "kukuri:topic:bookmark-source";
    let target_topic = "kukuri:topic:bookmark-target";
    let source_object_id = app
        .create_post(source_topic, "source body", None)
        .await
        .expect("create source post");
    let repost_id = app
        .create_repost(
            target_topic,
            source_topic,
            source_object_id.as_str(),
            Some("keep this"),
        )
        .await
        .expect("create repost");

    app.bookmark_post(target_topic, repost_id.as_str())
        .await
        .expect("bookmark repost");
    ObjectProjectionStore::rebuild_object_projections(store.as_ref(), Vec::new())
        .await
        .expect("clear projections");

    let bookmarks = app
        .list_bookmarked_posts_page(None, false)
        .await
        .expect("list bookmarked reposts")
        .items;

    assert_eq!(bookmarks.len(), 1);
    assert_eq!(bookmarks[0].post.object_id, repost_id);
    assert_eq!(bookmarks[0].post.content, "keep this");
    let repost_of = bookmarks[0]
        .post
        .repost_of
        .as_ref()
        .expect("repost snapshot");
    assert_eq!(repost_of.source_object_id, source_object_id);
    assert_eq!(repost_of.source_topic_id, source_topic);
    assert_eq!(repost_of.content, "source body");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[cfg(feature = "iroh-integration-tests")]
async fn bookmarks_do_not_sync_between_apps() {
    let _guard = iroh_integration_test_lock().lock_owned().await;
    let dir = tempdir().expect("tempdir");
    let stack_a = TestIrohStack::new(&dir.path().join("bookmark-a")).await;
    let stack_b = TestIrohStack::new(&dir.path().join("bookmark-b")).await;
    let store_a = Arc::new(MemoryStore::default());
    let store_b = Arc::new(MemoryStore::default());
    let app_a = app_with_iroh_services(store_a, &stack_a);
    let app_b = app_with_iroh_services(store_b.clone(), &stack_b);

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
    app_a
        .import_peer_ticket(&ticket_b)
        .await
        .expect("import b into a");
    app_b
        .import_peer_ticket(&ticket_a)
        .await
        .expect("import a into b");

    let topic = "kukuri:topic:bookmark-local-only";
    let _ = app_b
        .list_timeline(topic, None, 20)
        .await
        .expect("app b should subscribe to topic");

    let object_id = app_a
        .create_post(topic, "bookmark should stay local", None)
        .await
        .expect("app a should create post");

    timeout(Duration::from_secs(30), async {
        loop {
            let timeline = app_b
                .list_timeline(topic, None, 20)
                .await
                .expect("timeline should load");
            if timeline
                .items
                .iter()
                .any(|post| post.object_id == object_id)
            {
                return;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("timeline sync timeout");

    app_a
        .bookmark_post(topic, object_id.as_str())
        .await
        .expect("bookmark post on app a");

    let bookmarks_a = app_a
        .list_bookmarked_posts_page(None, false)
        .await
        .expect("list bookmarks on app a")
        .items;
    let bookmarks_b = app_b
        .list_bookmarked_posts_page(None, false)
        .await
        .expect("list bookmarks on app b")
        .items;

    assert_eq!(bookmarks_a.len(), 1);
    assert_eq!(bookmarks_a[0].post.object_id, object_id);
    assert!(bookmarks_b.is_empty());
}
