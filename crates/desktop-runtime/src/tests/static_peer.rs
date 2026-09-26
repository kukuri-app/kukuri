use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn desktop_runtime_imports_peer_ticket_and_tracks_local_posts() {
    let _resource = lock_test_resource(TestResource::IrohNetwork).await;
    let dir = tempdir().expect("tempdir");
    let db_a = dir.path().join("a.db");
    let db_b = dir.path().join("b.db");
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
    let endpoint_a = runtime_a
        .get_sync_status()
        .await
        .expect("status a before import")
        .discovery
        .local_endpoint_id;
    let endpoint_b = runtime_b
        .get_sync_status()
        .await
        .expect("status b before import")
        .discovery
        .local_endpoint_id;

    runtime_a
        .import_peer_ticket(ImportPeerTicketRequest {
            ticket: ticket_b.clone(),
        })
        .await
        .expect("import b");
    runtime_b
        .import_peer_ticket(ImportPeerTicketRequest {
            ticket: ticket_a.clone(),
        })
        .await
        .expect("import a");

    let status_a = runtime_a
        .get_sync_status()
        .await
        .expect("status a after import");
    let status_b = runtime_b
        .get_sync_status()
        .await
        .expect("status b after import");
    assert_eq!(status_a.discovery.manual_ticket_peer_ids, vec![endpoint_b]);
    assert_eq!(status_b.discovery.manual_ticket_peer_ids, vec![endpoint_a]);

    let topic = "kukuri:topic:desktop-runtime";
    let object_id = runtime_a
        .create_post(CreatePostRequest {
            topic: topic.into(),
            content: "hello desktop runtime".into(),
            reply_to: None,
            channel_ref: ChannelRef::Public,
            attachments: vec![],
            content_labels: Vec::new(),
        })
        .await
        .expect("create post");

    let timeline = runtime_a
        .list_timeline(ListTimelineRequest {
            topic: topic.into(),
            scope: TimelineScope::Public,
            cursor: None,
            limit: Some(20),
        })
        .await
        .expect("timeline a");
    let post = timeline
        .items
        .iter()
        .find(|post| post.object_id == object_id)
        .expect("local post");
    assert_eq!(post.content, "hello desktop runtime");
    let status = runtime_a.get_sync_status().await.expect("sync status");
    assert!(status.last_sync_ts.is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn profile_timeline_reads_author_public_posts_across_untracked_topics() {
    let _resource = lock_test_resource(TestResource::IrohNetwork).await;
    let dir = tempdir().expect("tempdir");
    let db_a = dir.path().join("profile-runtime-a.db");
    let db_b = dir.path().join("profile-runtime-b.db");
    let shared_keys = KukuriKeys::generate();
    let shared_secret = shared_keys.export_secret_hex();
    fs::write(
        db_a.with_extension("identity-key"),
        shared_secret.as_bytes(),
    )
    .expect("persist shared identity key a");
    fs::write(db_a.with_extension("identity-store"), b"file")
        .expect("persist shared identity backend a");
    fs::write(
        db_b.with_extension("identity-key"),
        shared_secret.as_bytes(),
    )
    .expect("persist shared identity key b");
    fs::write(db_b.with_extension("identity-store"), b"file")
        .expect("persist shared identity backend b");
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
        .import_peer_ticket(ImportPeerTicketRequest {
            ticket: ticket_b.clone(),
        })
        .await
        .expect("import b");
    runtime_b
        .import_peer_ticket(ImportPeerTicketRequest {
            ticket: ticket_a.clone(),
        })
        .await
        .expect("import a");

    let author_pubkey = runtime_a
        .get_sync_status()
        .await
        .expect("status a")
        .local_author_pubkey;
    assert_eq!(
        author_pubkey,
        runtime_b
            .get_sync_status()
            .await
            .expect("status b")
            .local_author_pubkey
    );
    let tracked_topic = "kukuri:topic:desktop-profile-demo";
    let untracked_topic = "kukuri:topic:desktop-profile-relay";
    let public_scope = TimelineScope::Public;

    open_topic_column(&runtime_a, tracked_topic, public_scope.clone())
        .await
        .expect("subscribe a tracked topic");
    open_topic_column(&runtime_b, tracked_topic, public_scope.clone())
        .await
        .expect("subscribe b tracked topic");
    wait_for_topic_delivery(
        &runtime_a,
        tracked_topic,
        1,
        "profile tracked topic delivery timeout a",
    )
    .await;
    wait_for_topic_delivery(
        &runtime_b,
        tracked_topic,
        1,
        "profile tracked topic delivery timeout b",
    )
    .await;

    let tracked_object_id = replicate_public_post_with_retry(
        &runtime_a,
        &runtime_b,
        tracked_topic,
        "tracked profile post",
        "tracked topic visibility timeout",
    )
    .await;
    let untracked_object_id = runtime_a
        .create_post(CreatePostRequest {
            topic: untracked_topic.into(),
            content: "untracked profile post".into(),
            reply_to: None,
            channel_ref: ChannelRef::Public,
            attachments: vec![],
            content_labels: Vec::new(),
        })
        .await
        .expect("untracked public post");
    let before_profile = runtime_b
        .get_sync_status()
        .await
        .expect("status before profile");
    assert!(
        before_profile
            .subscribed_topics
            .iter()
            .any(|topic| topic == tracked_topic)
    );
    assert!(
        before_profile
            .subscribed_topics
            .iter()
            .all(|topic| topic != untracked_topic)
    );

    let profile_object_ids = vec![tracked_object_id.clone(), untracked_object_id.clone()];
    let (runtime_b, profile_timeline) = match wait_for_profile_timeline_posts_result(
        &runtime_b,
        author_pubkey.as_str(),
        &profile_object_ids,
        "profile timeline visibility timeout",
    )
    .await
    {
        Ok(timeline) => (runtime_b, timeline),
        Err(first_error) => {
            timeout(runtime_shutdown_timeout(), runtime_b.shutdown())
                .await
                .expect("profile viewer restart shutdown timeout");
            drop(runtime_b);

            let restarted_b = DesktopRuntime::new_with_config_and_identity(
                &db_b,
                TransportNetworkConfig::loopback(),
                IdentityStorageMode::FileOnly,
            )
            .await
            .expect("restart runtime b");
            let restarted_ticket_b = restarted_b
                .local_peer_ticket()
                .await
                .expect("restarted ticket b")
                .expect("restarted ticket b value");
            runtime_a
                .import_peer_ticket(ImportPeerTicketRequest {
                    ticket: restarted_ticket_b,
                })
                .await
                .expect("import restarted b");
            restarted_b
                .import_peer_ticket(ImportPeerTicketRequest {
                    ticket: ticket_a.clone(),
                })
                .await
                .expect("import a after restart");
            open_topic_column(&restarted_b, tracked_topic, public_scope.clone())
                .await
                .expect("resubscribe restarted b tracked topic");
            wait_for_topic_delivery(
                &restarted_b,
                tracked_topic,
                1,
                "profile tracked topic restart delivery timeout b",
            )
            .await;
            let timeline = wait_for_profile_timeline_posts_result(
                    &restarted_b,
                    author_pubkey.as_str(),
                    &profile_object_ids,
                    "profile timeline visibility timeout after restart",
                )
                .await
                .unwrap_or_else(|second_error| {
                    panic!(
                        "profile timeline visibility timeout after viewer restart: first_error={first_error:#}; second_error={second_error:#}"
                    )
                });
            (restarted_b, timeline)
        }
    };
    assert!(
        profile_timeline
            .items
            .iter()
            .any(|post| post.object_id == tracked_object_id
                && post.origin_topic_id.as_deref() == Some(tracked_topic))
    );
    assert!(
        profile_timeline
            .items
            .iter()
            .any(|post| post.object_id == untracked_object_id
                && post.origin_topic_id.as_deref() == Some(untracked_topic))
    );

    let after_profile = runtime_b
        .get_sync_status()
        .await
        .expect("status after profile");
    assert!(
        after_profile
            .subscribed_topics
            .iter()
            .all(|topic| topic != untracked_topic)
    );

    open_topic_column(&runtime_b, untracked_topic, public_scope.clone())
        .await
        .expect("open original topic");
    wait_for_timeline_post(
        &runtime_b,
        untracked_topic,
        &public_scope,
        untracked_object_id.as_str(),
        "origin topic visibility timeout",
    )
    .await;

    let after_origin = runtime_b
        .get_sync_status()
        .await
        .expect("status after origin");
    assert!(
        after_origin
            .subscribed_topics
            .iter()
            .any(|topic| topic == untracked_topic)
    );
}
