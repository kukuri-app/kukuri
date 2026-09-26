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
async fn incoming_dm_frame_creates_single_direct_message_notification_after_store() {
    let (app, store, _, blob_service) = local_app_with_memory_services();
    let local_author_pubkey = app.current_author_pubkey();
    let remote_keys = generate_keys();
    let remote_pubkey = remote_keys.public_key_hex();
    seed_follow_edges(
        store.as_ref(),
        local_author_pubkey.as_str(),
        [remote_pubkey.as_str()],
        FollowEdgeStatus::Active,
    )
    .await;
    let dm_id = direct_message_id_for_participants(
        &Pubkey::from(local_author_pubkey.as_str()),
        &Pubkey::from(remote_pubkey.as_str()),
    );
    let message_id = "dm-message-remote-1";
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
        dm_id.as_str(),
        message_id,
        &frame_blob.hash,
        None,
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
