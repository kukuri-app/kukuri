use super::*;
#[tokio::test]
async fn remote_reply_to_local_post_creates_single_unread_reply_notification() {
    let (app, store, docs_sync, blob_service) = local_app_with_memory_services();
    let topic = TopicId::new("notifications-reply");
    let local_object_id = app
        .create_post(topic.as_str(), "local root", None)
        .await
        .expect("create local post");
    let local_envelope = store
        .get_envelope(&EnvelopeId::from(local_object_id.as_str()))
        .await
        .expect("load local envelope")
        .expect("local envelope");
    let remote_keys = generate_keys();
    let remote_envelope = persist_test_post(
        docs_sync.as_ref(),
        None,
        &remote_keys,
        &topic,
        PayloadRef::InlineText {
            text: "remote reply".into(),
        },
        Vec::new(),
        Some(&local_envelope),
    )
    .await;
    let remote_object = remote_envelope
        .to_post_object()
        .expect("parse remote reply")
        .expect("remote reply object");
    let created = create_remote_object_notification(
        &app,
        store.as_ref(),
        docs_sync.as_ref(),
        blob_service.as_ref(),
        remote_doc_event(
            docs_sync.as_ref(),
            &topic_replica_id(topic.as_str()),
            stable_key(
                "objects",
                &format!("{}/state", remote_object.object_id.as_str()),
            ),
        )
        .await,
    )
    .await;

    assert!(created);
    ObjectProjectionStore::put_object_projection(
        store.as_ref(),
        verified_projection_row(&remote_envelope, &topic_replica_id(topic.as_str()), None),
    )
    .await
    .expect("put remote projection");
    let notifications = app.list_notifications().await.expect("list notifications");
    assert_eq!(notifications.len(), 1);
    assert_eq!(notifications[0].kind, NotificationKind::Reply);
    assert_eq!(
        notifications[0].object_id.as_deref(),
        Some(remote_object.object_id.as_str())
    );
    assert_eq!(
        notifications[0].thread_root_object_id.as_deref(),
        Some(local_object_id.as_str())
    );
    assert_eq!(
        app.get_notification_status()
            .await
            .expect("notification status")
            .unread_count,
        1
    );
}

#[tokio::test]
async fn object_notification_view_exposes_thread_root_object_id_for_click_through() {
    let (app, store, docs_sync, blob_service) = local_app_with_memory_services();
    let topic = TopicId::new("notifications-thread-root");
    let local_object_id = app
        .create_post(topic.as_str(), "local root", None)
        .await
        .expect("create local post");
    let local_envelope = store
        .get_envelope(&EnvelopeId::from(local_object_id.as_str()))
        .await
        .expect("load local envelope")
        .expect("local envelope");
    let remote_keys = generate_keys();
    let remote_envelope = persist_test_post(
        docs_sync.as_ref(),
        None,
        &remote_keys,
        &topic,
        PayloadRef::InlineText {
            text: "thread root follow-up".into(),
        },
        Vec::new(),
        Some(&local_envelope),
    )
    .await;
    let remote_object = remote_envelope
        .to_post_object()
        .expect("parse remote reply")
        .expect("remote reply object");

    assert!(
        create_remote_object_notification(
            &app,
            store.as_ref(),
            docs_sync.as_ref(),
            blob_service.as_ref(),
            remote_doc_event(
                docs_sync.as_ref(),
                &topic_replica_id(topic.as_str()),
                stable_key(
                    "objects",
                    &format!("{}/state", remote_object.object_id.as_str()),
                ),
            )
            .await,
        )
        .await
    );
    ObjectProjectionStore::put_object_projection(
        store.as_ref(),
        verified_projection_row(&remote_envelope, &topic_replica_id(topic.as_str()), None),
    )
    .await
    .expect("put remote projection");

    let notifications = app.list_notifications().await.expect("list notifications");
    assert_eq!(notifications.len(), 1);
    assert_eq!(
        notifications[0].thread_root_object_id.as_deref(),
        Some(local_object_id.as_str())
    );
}

#[tokio::test]
async fn public_or_private_post_with_pubkey_mention_creates_mention_notification() {
    let (app, store, docs_sync, blob_service) = local_app_with_memory_services();
    let topic = TopicId::new("notifications-mention");
    let remote_keys = generate_keys();
    let remote_envelope = persist_test_post(
        docs_sync.as_ref(),
        None,
        &remote_keys,
        &topic,
        PayloadRef::InlineText {
            text: format!("hello @{}", app.current_author_pubkey()),
        },
        Vec::new(),
        None,
    )
    .await;
    let remote_object = remote_envelope
        .to_post_object()
        .expect("parse remote mention")
        .expect("remote mention object");

    let created = create_remote_object_notification(
        &app,
        store.as_ref(),
        docs_sync.as_ref(),
        blob_service.as_ref(),
        remote_doc_event(
            docs_sync.as_ref(),
            &topic_replica_id(topic.as_str()),
            stable_key(
                "objects",
                &format!("{}/state", remote_object.object_id.as_str()),
            ),
        )
        .await,
    )
    .await;

    assert!(created);
    let notifications = app.list_notifications().await.expect("list notifications");
    assert_eq!(notifications.len(), 1);
    assert_eq!(notifications[0].kind, NotificationKind::Mention);
    let expected_preview = format!("hello @{}", app.current_author_pubkey());
    assert_eq!(
        notifications[0].preview_text.as_deref(),
        Some(expected_preview.as_str())
    );
}

#[tokio::test]
async fn simple_repost_of_local_post_creates_repost_notification() {
    let (app, store, docs_sync, blob_service) = local_app_with_memory_services();
    let topic = TopicId::new("notifications-repost");
    let source_object_id = app
        .create_post(topic.as_str(), "source post", None)
        .await
        .expect("create source post");
    let remote_keys = generate_keys();
    let repost_source = app
        .resolve_repost_source(topic.as_str(), source_object_id.as_str())
        .await
        .expect("resolve repost source");
    let remote_envelope =
        build_repost_envelope(&remote_keys, &topic, repost_source.repost_of, None)
            .expect("build simple repost");
    let remote_object = remote_envelope
        .to_post_object()
        .expect("parse simple repost")
        .expect("simple repost object");
    persist_post_object(
        docs_sync.as_ref(),
        &topic_replica_id(topic.as_str()),
        remote_object.clone(),
        remote_envelope,
    )
    .await
    .expect("persist simple repost");

    let created = create_remote_object_notification(
        &app,
        store.as_ref(),
        docs_sync.as_ref(),
        blob_service.as_ref(),
        remote_doc_event(
            docs_sync.as_ref(),
            &topic_replica_id(topic.as_str()),
            stable_key(
                "objects",
                &format!("{}/state", remote_object.object_id.as_str()),
            ),
        )
        .await,
    )
    .await;

    assert!(created);
    let notifications = app.list_notifications().await.expect("list notifications");
    assert_eq!(notifications.len(), 1);
    assert_eq!(notifications[0].kind, NotificationKind::Repost);
    assert_eq!(
        notifications[0].preview_text.as_deref(),
        Some("source post")
    );
}

#[tokio::test]
async fn quote_repost_of_local_post_creates_quote_notification() {
    let (app, store, docs_sync, blob_service) = local_app_with_memory_services();
    let topic = TopicId::new("notifications-quote");
    let source_object_id = app
        .create_post(topic.as_str(), "quoted source", None)
        .await
        .expect("create source post");
    let remote_keys = generate_keys();
    let repost_source = app
        .resolve_repost_source(topic.as_str(), source_object_id.as_str())
        .await
        .expect("resolve repost source");
    let remote_envelope = build_repost_envelope(
        &remote_keys,
        &topic,
        repost_source.repost_of,
        Some("quote commentary"),
    )
    .expect("build quote repost");
    let remote_object = remote_envelope
        .to_post_object()
        .expect("parse quote repost")
        .expect("quote repost object");
    persist_post_object(
        docs_sync.as_ref(),
        &topic_replica_id(topic.as_str()),
        remote_object.clone(),
        remote_envelope,
    )
    .await
    .expect("persist quote repost");

    let created = create_remote_object_notification(
        &app,
        store.as_ref(),
        docs_sync.as_ref(),
        blob_service.as_ref(),
        remote_doc_event(
            docs_sync.as_ref(),
            &topic_replica_id(topic.as_str()),
            stable_key(
                "objects",
                &format!("{}/state", remote_object.object_id.as_str()),
            ),
        )
        .await,
    )
    .await;

    assert!(created);
    let notifications = app.list_notifications().await.expect("list notifications");
    assert_eq!(notifications.len(), 1);
    assert_eq!(notifications[0].kind, NotificationKind::QuoteRepost);
    assert_eq!(
        notifications[0].preview_text.as_deref(),
        Some("quote commentary")
    );
}

#[tokio::test]
async fn repost_notification_survives_hydration_before_live_doc_event() {
    let (app, store, docs_sync, blob_service) = local_app_with_memory_services();
    let topic = TopicId::new("notifications-repost-race");
    let replica = topic_replica_id(topic.as_str());
    let source_object_id = app
        .create_post(topic.as_str(), "source post", None)
        .await
        .expect("create source post");
    let baseline = snapshot_window_notification_baseline(docs_sync.as_ref(), &replica)
        .await
        .expect("snapshot object baseline");
    let remote_keys = generate_keys();
    let repost_source = app
        .resolve_repost_source(topic.as_str(), source_object_id.as_str())
        .await
        .expect("resolve repost source");
    let remote_envelope =
        build_repost_envelope(&remote_keys, &topic, repost_source.repost_of, None)
            .expect("build simple repost");
    let remote_object = remote_envelope
        .to_post_object()
        .expect("parse simple repost")
        .expect("simple repost object");
    persist_post_object(
        docs_sync.as_ref(),
        &replica,
        remote_object.clone(),
        remote_envelope,
    )
    .await
    .expect("persist simple repost");
    hydrate_subscription_state(
        &app.services,
        topic.as_str(),
        &replica,
        DocFetchPolicy::LocalThenRemote,
    )
    .await
    .expect("hydrate topic state");
    assert!(
        app.list_notifications()
            .await
            .expect("list notifications")
            .is_empty()
    );

    let created = create_remote_object_notification_with_baseline(
        &app,
        store.as_ref(),
        docs_sync.as_ref(),
        blob_service.as_ref(),
        &baseline,
        remote_doc_event(
            docs_sync.as_ref(),
            &replica,
            stable_key(
                "objects",
                &format!("{}/state", remote_object.object_id.as_str()),
            ),
        )
        .await,
    )
    .await;

    assert!(created);
    let notifications = app.list_notifications().await.expect("list notifications");
    assert_eq!(notifications.len(), 1);
    assert_eq!(notifications[0].kind, NotificationKind::Repost);
    assert_eq!(
        notifications[0].object_id.as_deref(),
        Some(remote_object.object_id.as_str())
    );
}

#[tokio::test]
async fn quote_repost_notification_survives_hydration_before_live_doc_event() {
    let (app, store, docs_sync, blob_service) = local_app_with_memory_services();
    let topic = TopicId::new("notifications-quote-race");
    let replica = topic_replica_id(topic.as_str());
    let source_object_id = app
        .create_post(topic.as_str(), "quoted source", None)
        .await
        .expect("create source post");
    let baseline = snapshot_window_notification_baseline(docs_sync.as_ref(), &replica)
        .await
        .expect("snapshot object baseline");
    let remote_keys = generate_keys();
    let repost_source = app
        .resolve_repost_source(topic.as_str(), source_object_id.as_str())
        .await
        .expect("resolve repost source");
    let remote_envelope = build_repost_envelope(
        &remote_keys,
        &topic,
        repost_source.repost_of,
        Some("quote commentary"),
    )
    .expect("build quote repost");
    let remote_object = remote_envelope
        .to_post_object()
        .expect("parse quote repost")
        .expect("quote repost object");
    persist_post_object(
        docs_sync.as_ref(),
        &replica,
        remote_object.clone(),
        remote_envelope,
    )
    .await
    .expect("persist quote repost");
    hydrate_subscription_state(
        &app.services,
        topic.as_str(),
        &replica,
        DocFetchPolicy::LocalThenRemote,
    )
    .await
    .expect("hydrate topic state");
    assert!(
        app.list_notifications()
            .await
            .expect("list notifications")
            .is_empty()
    );

    let created = create_remote_object_notification_with_baseline(
        &app,
        store.as_ref(),
        docs_sync.as_ref(),
        blob_service.as_ref(),
        &baseline,
        remote_doc_event(
            docs_sync.as_ref(),
            &replica,
            stable_key(
                "objects",
                &format!("{}/state", remote_object.object_id.as_str()),
            ),
        )
        .await,
    )
    .await;

    assert!(created);
    let notifications = app.list_notifications().await.expect("list notifications");
    assert_eq!(notifications.len(), 1);
    assert_eq!(notifications[0].kind, NotificationKind::QuoteRepost);
    assert_eq!(
        notifications[0].preview_text.as_deref(),
        Some("quote commentary")
    );
}

#[tokio::test]
async fn incoming_dm_frame_creates_single_direct_message_notification_after_store() {
    let (app, _store, _, blob_service) = local_app_with_memory_services();
    let local_keys = app.services.keys.clone();
    let local_author_pubkey = app.current_author_pubkey();
    let remote_keys = generate_keys();
    let remote_pubkey = remote_keys.public_key_hex();
    let dm_id = direct_message_id_for_participants(
        &Pubkey::from(local_author_pubkey.as_str()),
        &Pubkey::from(remote_pubkey.as_str()),
    );
    let message_id = "dm-message-remote-1";
    let topic =
        derive_direct_message_topic(local_keys.as_ref(), &Pubkey::from(remote_pubkey.as_str()))
            .expect("derive dm topic");
    let frame = encrypt_direct_message_frame(
        &remote_keys,
        &Pubkey::from(local_author_pubkey.as_str()),
        dm_id.as_str(),
        message_id,
        1234,
        &DirectMessagePayloadV1 {
            text: Some("hello from remote".into()),
            reply_to: None,
            attachment_manifest: None,
        },
    )
    .expect("encrypt dm frame");
    let frame_blob = blob_service
        .put_blob(
            serde_json::to_vec(&frame).expect("encode dm frame"),
            DIRECT_MESSAGE_FRAME_MIME,
        )
        .await
        .expect("store frame blob");

    let created = AppService::ingest_direct_message_frame(
        &app.services,
        local_author_pubkey.as_str(),
        remote_pubkey.as_str(),
        &topic,
        dm_id.as_str(),
        message_id,
        &frame_blob.hash,
    )
    .await
    .expect("ingest direct message frame");

    assert!(created);
    let notifications = app.list_notifications().await.expect("list notifications");
    assert_eq!(notifications.len(), 1);
    assert_eq!(notifications[0].kind, NotificationKind::DirectMessage);
    assert_eq!(notifications[0].dm_id.as_deref(), Some(dm_id.as_str()));
    assert_eq!(notifications[0].message_id.as_deref(), Some(message_id));
    assert_eq!(
        notifications[0].preview_text.as_deref(),
        Some("hello from remote")
    );
}

#[tokio::test]
async fn incoming_follow_edge_to_local_author_creates_followed_notification_for_observed_author() {
    let (app, store, docs_sync, _) = local_app_with_memory_services();
    let local_author_pubkey = app.current_author_pubkey();
    let remote_keys = generate_keys();
    let remote_pubkey = remote_keys.public_key_hex();
    let envelope = build_follow_edge_envelope(
        &remote_keys,
        &Pubkey::from(local_author_pubkey.as_str()),
        FollowEdgeStatus::Active,
    )
    .expect("build follow edge");
    let edge = parse_follow_edge(&envelope)
        .expect("parse follow edge")
        .expect("follow edge");
    persist_follow_edge_doc(docs_sync.as_ref(), &edge, &envelope)
        .await
        .expect("persist follow edge doc");

    let created = create_remote_follow_notification(
        &app,
        store.as_ref(),
        store.as_ref(),
        docs_sync.as_ref(),
        remote_pubkey.as_str(),
        remote_doc_event(
            docs_sync.as_ref(),
            &author_replica_id(remote_pubkey.as_str()),
            stable_key("graph/follows", local_author_pubkey.as_str()),
        )
        .await,
    )
    .await;

    assert!(created);
    let notifications = app.list_notifications().await.expect("list notifications");
    assert_eq!(notifications.len(), 1);
    assert_eq!(notifications[0].kind, NotificationKind::Followed);
    assert_eq!(notifications[0].actor_pubkey, remote_pubkey);
}

#[tokio::test]
async fn follow_notification_survives_hydration_before_live_doc_event() {
    let (app, store, docs_sync, _) = local_app_with_memory_services();
    let local_author_pubkey = app.current_author_pubkey();
    let remote_keys = generate_keys();
    let remote_pubkey = remote_keys.public_key_hex();
    let replica = author_replica_id(remote_pubkey.as_str());
    let baseline = snapshot_follow_notification_baseline(
        docs_sync.as_ref(),
        &replica,
        DocFetchPolicy::LocalThenRemote,
    )
    .await
    .expect("snapshot follow baseline");
    let envelope = build_follow_edge_envelope(
        &remote_keys,
        &Pubkey::from(local_author_pubkey.as_str()),
        FollowEdgeStatus::Active,
    )
    .expect("build follow edge");
    let edge = parse_follow_edge(&envelope)
        .expect("parse follow edge")
        .expect("follow edge");
    persist_follow_edge_doc(docs_sync.as_ref(), &edge, &envelope)
        .await
        .expect("persist follow edge doc");
    hydrate_author_state(
        &app.services,
        local_author_pubkey.as_str(),
        remote_pubkey.as_str(),
        DocFetchPolicy::LocalThenRemote,
    )
    .await
    .expect("hydrate author state");
    assert!(
        app.list_notifications()
            .await
            .expect("list notifications")
            .is_empty()
    );

    let created = create_remote_follow_notification_with_baseline(
        &app,
        store.as_ref(),
        store.as_ref(),
        docs_sync.as_ref(),
        remote_pubkey.as_str(),
        &baseline,
        remote_doc_event(
            docs_sync.as_ref(),
            &replica,
            stable_key("graph/follows", local_author_pubkey.as_str()),
        )
        .await,
    )
    .await;

    assert!(created);
    let notifications = app.list_notifications().await.expect("list notifications");
    assert_eq!(notifications.len(), 1);
    assert_eq!(notifications[0].kind, NotificationKind::Followed);
    assert_eq!(notifications[0].actor_pubkey, remote_pubkey);
}

#[tokio::test]
async fn initial_follow_baseline_prevents_backfill_notification() {
    let (app, store, docs_sync, _) = local_app_with_memory_services();
    let local_author_pubkey = app.current_author_pubkey();
    let remote_keys = generate_keys();
    let remote_pubkey = remote_keys.public_key_hex();
    let replica = author_replica_id(remote_pubkey.as_str());
    let envelope = build_follow_edge_envelope(
        &remote_keys,
        &Pubkey::from(local_author_pubkey.as_str()),
        FollowEdgeStatus::Active,
    )
    .expect("build follow edge");
    let edge = parse_follow_edge(&envelope)
        .expect("parse follow edge")
        .expect("follow edge");
    persist_follow_edge_doc(docs_sync.as_ref(), &edge, &envelope)
        .await
        .expect("persist follow edge doc");
    let baseline = snapshot_follow_notification_baseline(
        docs_sync.as_ref(),
        &replica,
        DocFetchPolicy::LocalThenRemote,
    )
    .await
    .expect("snapshot follow baseline");

    hydrate_author_state(
        &app.services,
        local_author_pubkey.as_str(),
        remote_pubkey.as_str(),
        DocFetchPolicy::LocalThenRemote,
    )
    .await
    .expect("hydrate author state");
    assert!(
        app.list_notifications()
            .await
            .expect("list notifications")
            .is_empty()
    );

    let created = create_remote_follow_notification_with_baseline(
        &app,
        store.as_ref(),
        store.as_ref(),
        docs_sync.as_ref(),
        remote_pubkey.as_str(),
        &baseline,
        remote_doc_event(
            docs_sync.as_ref(),
            &replica,
            stable_key("graph/follows", local_author_pubkey.as_str()),
        )
        .await,
    )
    .await;

    assert!(!created);
    assert!(
        app.list_notifications()
            .await
            .expect("list notifications")
            .is_empty()
    );
}

#[tokio::test]
async fn notification_overlap_uses_precedence_and_does_not_double_insert() {
    let (app, store, docs_sync, blob_service) = local_app_with_memory_services();
    let topic = TopicId::new("notifications-overlap");
    let local_object_id = app
        .create_post(topic.as_str(), "local root", None)
        .await
        .expect("create local post");
    let local_envelope = store
        .get_envelope(&EnvelopeId::from(local_object_id))
        .await
        .expect("load local envelope")
        .expect("local envelope");
    let remote_keys = generate_keys();
    let remote_envelope = persist_test_post(
        docs_sync.as_ref(),
        None,
        &remote_keys,
        &topic,
        PayloadRef::InlineText {
            text: format!("reply to @{}", app.current_author_pubkey()),
        },
        Vec::new(),
        Some(&local_envelope),
    )
    .await;
    let remote_object = remote_envelope
        .to_post_object()
        .expect("parse overlap reply")
        .expect("overlap reply object");
    let event = remote_doc_event(
        docs_sync.as_ref(),
        &topic_replica_id(topic.as_str()),
        stable_key(
            "objects",
            &format!("{}/state", remote_object.object_id.as_str()),
        ),
    )
    .await;

    assert!(
        create_remote_object_notification(
            &app,
            store.as_ref(),
            docs_sync.as_ref(),
            blob_service.as_ref(),
            event.clone(),
        )
        .await
    );
    assert!(
        !create_remote_object_notification(
            &app,
            store.as_ref(),
            docs_sync.as_ref(),
            blob_service.as_ref(),
            event,
        )
        .await
    );

    let notifications = app.list_notifications().await.expect("list notifications");
    assert_eq!(notifications.len(), 1);
    assert_eq!(notifications[0].kind, NotificationKind::Reply);
}

#[tokio::test]
async fn restart_or_manual_hydration_does_not_backfill_or_duplicate_notifications() {
    let (app, store, docs_sync, blob_service) = local_app_with_memory_services();
    let topic = TopicId::new("notifications-hydration");
    let replica = topic_replica_id(topic.as_str());
    let local_object_id = app
        .create_post(topic.as_str(), "local root", None)
        .await
        .expect("create local post");
    let local_envelope = store
        .get_envelope(&EnvelopeId::from(local_object_id))
        .await
        .expect("load local envelope")
        .expect("local envelope");
    let remote_keys = generate_keys();

    let existing_reply = persist_test_post(
        docs_sync.as_ref(),
        None,
        &remote_keys,
        &topic,
        PayloadRef::InlineText {
            text: "existing remote reply".into(),
        },
        Vec::new(),
        Some(&local_envelope),
    )
    .await;
    let existing_object = existing_reply
        .to_post_object()
        .expect("parse existing reply")
        .expect("existing reply object");
    let baseline = snapshot_window_notification_baseline(docs_sync.as_ref(), &replica)
        .await
        .expect("snapshot object baseline");
    hydrate_subscription_state(
        &app.services,
        topic.as_str(),
        &replica,
        DocFetchPolicy::LocalThenRemote,
    )
    .await
    .expect("hydrate topic state");
    assert!(
        app.list_notifications()
            .await
            .expect("list notifications")
            .is_empty()
    );
    assert!(
        !create_remote_object_notification_with_baseline(
            &app,
            store.as_ref(),
            docs_sync.as_ref(),
            blob_service.as_ref(),
            &baseline,
            remote_doc_event(
                docs_sync.as_ref(),
                &replica,
                stable_key(
                    "objects",
                    &format!("{}/state", existing_object.object_id.as_str())
                ),
            )
            .await,
        )
        .await
    );

    let new_reply = persist_test_post(
        docs_sync.as_ref(),
        None,
        &remote_keys,
        &topic,
        PayloadRef::InlineText {
            text: "new remote reply".into(),
        },
        Vec::new(),
        Some(&local_envelope),
    )
    .await;
    let new_object = new_reply
        .to_post_object()
        .expect("parse new reply")
        .expect("new reply object");
    assert!(
        create_remote_object_notification_with_baseline(
            &app,
            store.as_ref(),
            docs_sync.as_ref(),
            blob_service.as_ref(),
            &baseline,
            remote_doc_event(
                docs_sync.as_ref(),
                &replica,
                stable_key(
                    "objects",
                    &format!("{}/state", new_object.object_id.as_str())
                ),
            )
            .await,
        )
        .await
    );
    hydrate_subscription_state(
        &app.services,
        topic.as_str(),
        &replica,
        DocFetchPolicy::LocalThenRemote,
    )
    .await
    .expect("rehydrate topic state");
    assert_eq!(
        app.list_notifications()
            .await
            .expect("list notifications")
            .len(),
        1
    );
}

#[tokio::test]
async fn mark_notification_read_and_mark_all_read_update_unread_count() {
    let (app, store, docs_sync, blob_service) = local_app_with_memory_services();
    let topic = TopicId::new("notifications-read");
    let remote_keys = generate_keys();
    let mention_envelope = persist_test_post(
        docs_sync.as_ref(),
        None,
        &remote_keys,
        &topic,
        PayloadRef::InlineText {
            text: format!("hello @{}", app.current_author_pubkey()),
        },
        Vec::new(),
        None,
    )
    .await;
    let mention_object = mention_envelope
        .to_post_object()
        .expect("parse mention")
        .expect("mention object");
    assert!(
        create_remote_object_notification(
            &app,
            store.as_ref(),
            docs_sync.as_ref(),
            blob_service.as_ref(),
            remote_doc_event(
                docs_sync.as_ref(),
                &topic_replica_id(topic.as_str()),
                stable_key(
                    "objects",
                    &format!("{}/state", mention_object.object_id.as_str())
                ),
            )
            .await,
        )
        .await
    );

    let local_author_pubkey = app.current_author_pubkey();
    let follower_keys = generate_keys();
    let follower_pubkey = follower_keys.public_key_hex();
    let follow_envelope = build_follow_edge_envelope(
        &follower_keys,
        &Pubkey::from(local_author_pubkey.as_str()),
        FollowEdgeStatus::Active,
    )
    .expect("build follow edge");
    let follow_edge = parse_follow_edge(&follow_envelope)
        .expect("parse follow edge")
        .expect("follow edge");
    persist_follow_edge_doc(docs_sync.as_ref(), &follow_edge, &follow_envelope)
        .await
        .expect("persist follow edge");
    assert!(
        create_remote_follow_notification(
            &app,
            store.as_ref(),
            store.as_ref(),
            docs_sync.as_ref(),
            follower_pubkey.as_str(),
            remote_doc_event(
                docs_sync.as_ref(),
                &author_replica_id(follower_pubkey.as_str()),
                stable_key("graph/follows", local_author_pubkey.as_str()),
            )
            .await,
        )
        .await
    );

    let notifications = app.list_notifications().await.expect("list notifications");
    assert_eq!(notifications.len(), 2);
    assert_eq!(
        app.get_notification_status()
            .await
            .expect("notification status")
            .unread_count,
        2
    );

    let status = app
        .mark_notification_read(notifications[0].notification_id.as_str())
        .await
        .expect("mark notification read");
    assert_eq!(status.unread_count, 1);

    let status = app
        .mark_all_notifications_read()
        .await
        .expect("mark all notifications read");
    assert_eq!(status.unread_count, 0);
}
