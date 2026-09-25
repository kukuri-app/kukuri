use super::super::*;
use kukuri_core::{ReceiveOfferReferenceV1, ReceiveOfferScopeV1, seal_receive_offer};

#[cfg(feature = "iroh-integration-tests")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_public_offer_reaches_offscreen_account_from_bounded_app_cache() {
    let _guard = iroh_integration_test_lock().lock_owned().await;
    let dir = tempdir().unwrap();
    let sender_stack = TestIrohStack::new(&dir.path().join("public-offer-sender")).await;
    let recipient_stack = TestIrohStack::new(&dir.path().join("public-offer-recipient")).await;
    let sender = generate_keys();
    let recipient = generate_keys();
    sender_stack
        ._node
        .install_receive_binding(Arc::new(sender.clone()))
        .await
        .unwrap();
    recipient_stack
        ._node
        .install_receive_binding(Arc::new(recipient.clone()))
        .await
        .unwrap();
    let sender_store = Arc::new(
        SqliteStore::connect_file(dir.path().join("public-offer-sender.db"))
            .await
            .unwrap(),
    );
    sender_stack
        ._node
        .install_remote_cache(sender_store.clone())
        .unwrap();
    let sender_blob = Arc::new(IrohBlobService::with_account_store(
        sender_stack._node.clone(),
        sender_store.clone(),
    ));
    let recipient_store = Arc::new(MemoryStore::default());
    let sender_app = app_service_from_dependencies(
        sender_store.clone(),
        sender_store.clone(),
        sender_stack.transport.clone(),
        sender_stack.transport.clone(),
        sender_stack.docs_sync.clone(),
        sender_blob,
        sender,
    );
    let recipient_app = app_service_from_dependencies(
        recipient_store.clone(),
        recipient_store.clone(),
        recipient_stack.transport.clone(),
        recipient_stack.transport.clone(),
        recipient_stack.docs_sync.clone(),
        recipient_stack.blob_service.clone(),
        recipient.clone(),
    );
    let ticket = recipient_app.peer_ticket().await.unwrap().unwrap();
    sender_app.import_peer_ticket(&ticket).await.unwrap();
    recipient_app.start_account_receive_offers().await.unwrap();
    let topic = "kukuri:topic:offscreen-public-offer";
    sender_app
        .create_post(
            topic,
            &format!("hello @{}", recipient.public_key_hex()),
            None,
        )
        .await
        .unwrap();
    timeout(Duration::from_secs(20), async {
        loop {
            let notifications = recipient_app.list_notifications().await.unwrap();
            if notifications
                .iter()
                .any(|entry| entry.kind == NotificationKind::Mention)
            {
                break;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("public offer reached account without a topic subscription");
    assert!(
        recipient_app
            .get_sync_status()
            .await
            .unwrap()
            .subscribed_topics
            .iter()
            .all(|subscribed| subscribed != topic)
    );
    let cached_manifest: String = sqlx::query_scalar(
        "SELECT cache_key FROM remote_content_cache WHERE kind = 'blob' LIMIT 1",
    )
    .fetch_one(sender_store.pool())
    .await
    .unwrap();
    assert!(
        !sender_stack
            ._node
            .blobs()
            .blobs()
            .has(blake3::Hash::from_hex(&cached_manifest).unwrap())
            .await
            .unwrap(),
        "public offer manifest must be served from the bounded app cache"
    );
    sender_app
        .follow_author(recipient.public_key_hex().as_str())
        .await
        .unwrap();
    timeout(Duration::from_secs(20), async {
        loop {
            if recipient_app
                .list_notifications()
                .await
                .unwrap()
                .iter()
                .any(|entry| entry.kind == NotificationKind::Followed)
            {
                break;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("follow reached the same offscreen account route");
    let docs = MemoryDocsSync::default();
    let topic_id = TopicId::new(topic);
    let parent = persist_test_post(
        &docs,
        None,
        &recipient,
        &topic_id,
        PayloadRef::InlineText {
            text: "root".into(),
        },
        Vec::new(),
        None,
    )
    .await;
    let reply = persist_test_post(
        &docs,
        None,
        sender_app.services.keys.as_ref(),
        &topic_id,
        PayloadRef::InlineText {
            text: "reply".into(),
        },
        Vec::new(),
        Some(&parent),
    )
    .await;
    let target = recipient.public_key_hex();
    sender_app
        .queue_public_notification_offer(
            PublicNotificationSource::Post {
                replica: topic_replica_id(topic),
                envelope: reply,
                content: "reply".into(),
                reply_target: Some(parent.clone()),
            },
            BTreeSet::from([target.clone()]),
        )
        .await;
    let source = RepostSourceSnapshotV1 {
        source_object_id: parent.id,
        source_topic_id: topic_id.clone(),
        source_author_pubkey: recipient.public_key(),
        source_object_kind: "post".into(),
        content: "root".into(),
        attachments: Vec::new(),
        reply_to_object_id: None,
        root_id: None,
        content_labels: Vec::new(),
    };
    for commentary in [None, Some("quote")] {
        let envelope = build_repost_envelope_with_docs_author(
            sender_app.services.keys.as_ref(),
            &topic_id,
            source.clone(),
            commentary,
            None,
        )
        .unwrap();
        sender_app
            .queue_public_notification_offer(
                PublicNotificationSource::Post {
                    replica: topic_replica_id(topic),
                    envelope,
                    content: commentary.unwrap_or_default().into(),
                    reply_target: None,
                },
                BTreeSet::from([target.clone()]),
            )
            .await;
    }
    let all_kinds = timeout(Duration::from_secs(30), async {
        loop {
            let notifications = recipient_app.list_notifications().await.unwrap();
            if [
                NotificationKind::Mention,
                NotificationKind::Reply,
                NotificationKind::Repost,
                NotificationKind::QuoteRepost,
                NotificationKind::Followed,
            ]
            .iter()
            .all(|kind| notifications.iter().any(|entry| &entry.kind == kind))
            {
                assert_eq!(notifications.len(), 5);
                break;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    if all_kinds.is_err() {
        let kinds = recipient_app
            .list_notifications()
            .await
            .unwrap()
            .into_iter()
            .map(|entry| entry.kind)
            .collect::<Vec<_>>();
        panic!("all five public notifications reached the offscreen account route: {kinds:?}");
    }
    let cached_before: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM remote_content_cache WHERE kind = 'blob'")
            .fetch_one(sender_store.pool())
            .await
            .unwrap();
    sender_app
        .create_post(topic, "unrelated public post", None)
        .await
        .unwrap();
    let cached_after: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM remote_content_cache WHERE kind = 'blob'")
            .fetch_one(sender_store.pool())
            .await
            .unwrap();
    assert_eq!(
        cached_after, cached_before,
        "unrelated post created an offer manifest"
    );
    assert_eq!(recipient_app.list_notifications().await.unwrap().len(), 5);
    sender_app.shutdown().await;
    recipient_app.shutdown().await;
}

#[cfg(feature = "iroh-integration-tests")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_account_route_fetches_bound_provider_manifest_and_reflects_dm() {
    let _guard = iroh_integration_test_lock().lock_owned().await;
    let dir = tempdir().unwrap();
    let sender_stack = TestIrohStack::new(&dir.path().join("offer-sender")).await;
    let recipient_stack = TestIrohStack::new(&dir.path().join("offer-recipient")).await;
    let sender = generate_keys();
    let recipient = generate_keys();
    sender_stack
        ._node
        .install_receive_binding(Arc::new(sender.clone()))
        .await
        .unwrap();
    recipient_stack
        ._node
        .install_receive_binding(Arc::new(recipient.clone()))
        .await
        .unwrap();
    let sender_store = Arc::new(MemoryStore::default());
    let recipient_store = Arc::new(MemoryStore::default());
    let sender_app = app_service_from_dependencies(
        sender_store.clone(),
        sender_store.clone(),
        sender_stack.transport.clone(),
        sender_stack.transport.clone(),
        sender_stack.docs_sync.clone(),
        sender_stack.blob_service.clone(),
        sender.clone(),
    );
    let recipient_app = app_service_from_dependencies(
        recipient_store.clone(),
        recipient_store.clone(),
        recipient_stack.transport.clone(),
        recipient_stack.transport.clone(),
        recipient_stack.docs_sync.clone(),
        recipient_stack.blob_service.clone(),
        recipient.clone(),
    );
    let recipient_ticket = recipient_app.peer_ticket().await.unwrap().unwrap();
    sender_app
        .import_peer_ticket(&recipient_ticket)
        .await
        .unwrap();
    SocialProjectionStore::rebuild_author_relationships(
        recipient_store.as_ref(),
        &recipient.public_key_hex(),
        vec![AuthorRelationshipProjectionRow {
            local_author_pubkey: recipient.public_key_hex(),
            author_pubkey: sender.public_key_hex(),
            following: true,
            followed_by: true,
            mutual: true,
            friend_of_friend: false,
            friend_of_friend_via_pubkeys: Vec::new(),
            derived_at: 1,
        }],
    )
    .await
    .unwrap();
    SocialProjectionStore::rebuild_author_relationships(
        sender_store.as_ref(),
        &sender.public_key_hex(),
        vec![AuthorRelationshipProjectionRow {
            local_author_pubkey: sender.public_key_hex(),
            author_pubkey: recipient.public_key_hex(),
            following: true,
            followed_by: true,
            mutual: true,
            friend_of_friend: false,
            friend_of_friend_via_pubkeys: Vec::new(),
            derived_at: 1,
        }],
    )
    .await
    .unwrap();
    sender_app.start_account_receive_offers().await.unwrap();
    let dm_id = direct_message_id_for_participants(&sender.public_key(), &recipient.public_key());
    let message_id = "real-account-offer-1";
    let frame = encrypt_direct_message_frame(
        &sender,
        &recipient.public_key(),
        &dm_id,
        message_id,
        Utc::now().timestamp_millis(),
        &DirectMessagePayloadV1 {
            text: Some("bound provider route".into()),
            reply_to: None,
            attachment_manifest: None,
        },
    )
    .unwrap();
    let frame_blob = sender_stack
        .blob_service
        .put_blob(
            serde_json::to_vec(&frame).unwrap(),
            DIRECT_MESSAGE_FRAME_MIME,
        )
        .await
        .unwrap();
    let manifest = serde_json::to_vec(&GossipHint::DirectMessageFrame {
        topic_id: derive_direct_message_topic(&recipient, &sender.public_key()).unwrap(),
        dm_id: dm_id.clone(),
        message_id: message_id.into(),
        frame_hash: frame_blob.hash,
    })
    .unwrap();
    let manifest_blob = sender_stack
        .blob_service
        .put_blob(
            manifest,
            "application/vnd.kukuri.direct-message-receive-manifest+json",
        )
        .await
        .unwrap();
    let now = Utc::now().timestamp_millis();
    let offer = seal_receive_offer(
        &sender,
        &recipient.public_key(),
        ReceiveOfferReferenceV1 {
            provider_endpoint_id: sender_stack._node.endpoint().id().to_string(),
            payload_hash: manifest_blob.hash,
            payload_bytes: manifest_blob.bytes as u32,
            scope: ReceiveOfferScopeV1::DirectMessage,
        },
        now,
        now + 60_000,
    )
    .unwrap();
    recipient_app.start_account_receive_offers().await.unwrap();
    sender_stack
        .transport
        .publish_receive_offer(
            &recipient.public_key(),
            recipient_stack._node.endpoint().addr(),
            offer,
        )
        .await
        .unwrap();
    timeout(Duration::from_secs(20), async {
        loop {
            if DirectMessageStore::get_direct_message_message(
                recipient_store.as_ref(),
                &dm_id,
                message_id,
            )
            .await
            .unwrap()
            .is_some()
            {
                break;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("verified account offer DM reached recipient");

    let second_id = "real-account-outbox-2";
    let second_frame = encrypt_direct_message_frame(
        &sender,
        &recipient.public_key(),
        &dm_id,
        second_id,
        Utc::now().timestamp_millis(),
        &DirectMessagePayloadV1 {
            text: Some("sent by durable outbox".into()),
            reply_to: None,
            attachment_manifest: None,
        },
    )
    .unwrap();
    let second_blob = sender_stack
        .blob_service
        .put_blob(
            serde_json::to_vec(&second_frame).unwrap(),
            DIRECT_MESSAGE_FRAME_MIME,
        )
        .await
        .unwrap();
    sender_store
        .put_direct_message_outbox(kukuri_store::DirectMessageOutboxRow {
            dm_id: dm_id.clone(),
            message_id: second_id.into(),
            peer_pubkey: recipient.public_key_hex(),
            frame_blob_hash: second_blob.hash,
            created_at: Utc::now().timestamp_millis(),
            last_attempt_at: None,
        })
        .await
        .unwrap();
    AppService::flush_due_direct_message_outbox(
        &sender_app.services,
        Utc::now().timestamp_millis(),
    )
    .await
    .unwrap();
    timeout(Duration::from_secs(20), async {
        loop {
            let received = recipient_store
                .get_direct_message_message(&dm_id, second_id)
                .await
                .unwrap()
                .is_some();
            let acked = sender_store
                .get_direct_message_outbox(&dm_id, second_id)
                .await
                .unwrap()
                .is_none();
            if received && acked {
                break;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("bounded outbox offer and account-route ACK completed without pairwise subscription");
    recipient_app.shutdown().await;
    sender_app.shutdown().await;
}

// #1221 R4-C: private の投稿の mention は、channel の docs を同期していない参加者へも account の受信 route で届く。
// 参照は epoch の鍵で暗号化され、同じ channel に参加していない account へは通知にならない。
#[cfg(feature = "iroh-integration-tests")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_private_offer_reaches_a_member_without_channel_sync() {
    let _guard = iroh_integration_test_lock().lock_owned().await;
    let dir = tempdir().unwrap();
    let sender_stack = TestIrohStack::new(&dir.path().join("private-offer-sender")).await;
    let member_stack = TestIrohStack::new(&dir.path().join("private-offer-member")).await;
    let outsider_stack = TestIrohStack::new(&dir.path().join("private-offer-outsider")).await;
    let sender_store = Arc::new(
        SqliteStore::connect_file(dir.path().join("private-offer-sender.db"))
            .await
            .unwrap(),
    );
    sender_stack
        ._node
        .install_remote_cache(sender_store.clone())
        .unwrap();
    let sender_app = app_service_from_dependencies(
        sender_store.clone(),
        sender_store.clone(),
        sender_stack.transport.clone(),
        sender_stack.transport.clone(),
        sender_stack.docs_sync.clone(),
        Arc::new(IrohBlobService::with_account_store(
            sender_stack._node.clone(),
            sender_store.clone(),
        )),
        generate_keys(),
    );
    let member_app = app_with_iroh_services(Arc::new(MemoryStore::default()), &member_stack);
    let outsider_app = app_with_iroh_services(Arc::new(MemoryStore::default()), &outsider_stack);
    sender_stack.bind_account(&sender_app).await;
    member_stack.bind_account(&member_app).await;
    outsider_stack.bind_account(&outsider_app).await;
    for app in [&member_app, &outsider_app] {
        let ticket = app.peer_ticket().await.unwrap().unwrap();
        sender_app.import_peer_ticket(&ticket).await.unwrap();
        app.start_account_receive_offers().await.unwrap();
    }
    let topic = "kukuri:topic:offscreen-private-offer";
    let channel = sender_app
        .create_private_channel(CreatePrivateChannelInput {
            topic_id: TopicId::new(topic),
            label: "room".into(),
            audience_kind: ChannelAudienceKind::InviteOnly,
        })
        .await
        .unwrap();
    let state = sender_app
        .joined_private_channel_state(topic, channel.channel_id.as_str())
        .await
        .unwrap();
    // 参加者は channel を購読しない(docs を同期しない)。account の受信 route だけが届く経路になる。
    member_app.joined_private_channels.lock().await.insert(
        joined_private_channel_key(topic, channel.channel_id.as_str()),
        state,
    );
    let member = member_app.current_author_pubkey();
    let outsider = outsider_app.current_author_pubkey();
    let object_id = sender_app
        .create_post_in_channel(
            topic,
            ChannelRef::PrivateChannel {
                channel_id: ChannelId::new(channel.channel_id.clone()),
            },
            &format!("private hello @{member} @{outsider}"),
            None,
        )
        .await
        .unwrap();
    let notification = timeout(Duration::from_secs(20), async {
        loop {
            if let Some(entry) = member_app
                .list_notifications()
                .await
                .unwrap()
                .into_iter()
                .find(|entry| entry.kind == NotificationKind::Mention)
            {
                break entry;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("private offer reached the member without channel sync");
    assert_eq!(notification.object_id.as_deref(), Some(object_id.as_str()));
    assert_eq!(
        notification.channel_id.as_deref(),
        Some(channel.channel_id.as_str())
    );
    sleep(Duration::from_secs(2)).await;
    assert!(
        outsider_app.list_notifications().await.unwrap().is_empty(),
        "an account outside the channel cannot open the epoch payload"
    );
    sender_app.shutdown().await;
    member_app.shutdown().await;
    outsider_app.shutdown().await;
}
