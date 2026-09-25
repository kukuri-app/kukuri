use super::super::*;
use super::receive_offer::{offer_app, offer_for};
use super::receive_offer_doubles::{OfferBlobService, ProbeOfferTransport};
use crate::service::public_notification_offer_support::encode_public_notification_manifest;
use kukuri_core::ReceiveOfferScopeV1;
use kukuri_transport::ReceiveOfferEnvelope;

async fn deliver_public_source(
    app: &AppService,
    sender: &KukuriKeys,
    recipient: &KukuriKeys,
    memory_blob: &MemoryBlobService,
    source: PublicNotificationSource,
) -> Result<bool> {
    let payload = encode_public_notification_manifest(source).unwrap();
    let stored = memory_blob
        .put_blob(payload, "application/vnd.kukuri.public-notification+json")
        .await
        .unwrap();
    let (_, offer) = offer_for(
        sender,
        recipient,
        ReceiveOfferScopeV1::PublicSource,
        stored.hash,
        stored.bytes as u32,
    );
    AppService::ingest_account_receive_offer(
        &app.services,
        ReceiveOfferEnvelope {
            offer,
            received_at: Utc::now().timestamp_millis(),
            source_peer: "offscreen".into(),
        },
    )
    .await
}

#[tokio::test]
async fn public_account_offer_creates_one_offscreen_mention_notification() {
    let sender = generate_keys();
    let recipient = generate_keys();
    let store = Arc::new(MemoryStore::default());
    let memory_blob = Arc::new(MemoryBlobService::default());
    let blob = Arc::new(OfferBlobService::new(memory_blob.clone()));
    let transport = Arc::new(FakeTransport::new("recipient", FakeNetwork::default()));
    let app = offer_app(recipient.clone(), store, transport, blob.clone());
    let topic = TopicId::new("offscreen-public-notification");
    let content = format!("hello @{}", recipient.public_key_hex());
    let docs = MemoryDocsSync::default();
    let envelope = persist_test_post(
        &docs,
        None,
        &sender,
        &topic,
        PayloadRef::InlineText {
            text: content.clone(),
        },
        Vec::new(),
        None,
    )
    .await;
    let source = PublicNotificationSource::Post {
        replica: topic_replica_id(topic.as_str()),
        envelope,
        content,
        reply_target: None,
    };
    for expected in [true, false] {
        assert_eq!(
            deliver_public_source(
                &app,
                &sender,
                &recipient,
                memory_blob.as_ref(),
                source.clone()
            )
            .await
            .unwrap(),
            expected
        );
    }
    let notifications = app.list_notifications().await.unwrap();
    assert_eq!(notifications.len(), 1);
    assert_eq!(notifications[0].kind, NotificationKind::Mention);
    assert_eq!(blob.fetches.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn public_account_offer_rejects_unsigned_content_before_notification_storage() {
    let sender = generate_keys();
    let recipient = generate_keys();
    let store = Arc::new(MemoryStore::default());
    let memory_blob = Arc::new(MemoryBlobService::default());
    let blob = Arc::new(OfferBlobService::new(memory_blob.clone()));
    let transport = Arc::new(FakeTransport::new("recipient", FakeNetwork::default()));
    let app = offer_app(recipient.clone(), store, transport, blob);
    let topic = TopicId::new("offscreen-forged-content");
    let docs = MemoryDocsSync::default();
    let envelope = persist_test_post(
        &docs,
        None,
        &sender,
        &topic,
        PayloadRef::InlineText {
            text: format!("hello @{}", recipient.public_key_hex()),
        },
        Vec::new(),
        None,
    )
    .await;
    assert!(
        deliver_public_source(
            &app,
            &sender,
            &recipient,
            memory_blob.as_ref(),
            PublicNotificationSource::Post {
                replica: topic_replica_id(topic.as_str()),
                envelope,
                content: "forged preview".into(),
                reply_target: None,
            },
        )
        .await
        .is_err()
    );
    assert!(app.list_notifications().await.unwrap().is_empty());
}

#[tokio::test]
async fn public_account_offer_rejects_a_signed_private_post() {
    let sender = generate_keys();
    let recipient = generate_keys();
    let store = Arc::new(MemoryStore::default());
    let memory_blob = Arc::new(MemoryBlobService::default());
    let blob = Arc::new(OfferBlobService::new(memory_blob.clone()));
    let transport = Arc::new(FakeTransport::new("recipient", FakeNetwork::default()));
    let app = offer_app(recipient.clone(), store, transport, blob);
    let topic = TopicId::new("private-source-notification");
    let channel = ChannelId::new("private-channel");
    let content = format!("private hello @{}", recipient.public_key_hex());
    let envelope = build_post_envelope_with_docs_author(
        &sender,
        &topic,
        PayloadRef::InlineText {
            text: content.clone(),
        },
        Vec::new(),
        Vec::new(),
        None,
        ObjectVisibility::Private,
        Some(&channel),
        Vec::new(),
        None,
    )
    .unwrap();
    assert!(
        deliver_public_source(
            &app,
            &sender,
            &recipient,
            memory_blob.as_ref(),
            PublicNotificationSource::Post {
                replica: private_channel_replica_id(channel.as_str()),
                envelope,
                content,
                reply_target: None,
            },
        )
        .await
        .is_err()
    );
    assert!(app.list_notifications().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_fifth_public_offer_waits_for_the_account_worker_instead_of_disappearing() {
    let sender = generate_keys();
    let recipient = generate_keys();
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(FakeTransport::new("sender", FakeNetwork::default()));
    let gate = Arc::new((
        std::sync::atomic::AtomicBool::new(false),
        tokio::sync::Notify::new(),
    ));
    let offers = Arc::new(ProbeOfferTransport {
        resolve_gate: Some(gate.clone()),
        ..Default::default()
    });
    let app = AppService::from_handles(ServiceHandles::new(
        store.clone(),
        store,
        transport,
        offers.clone(),
        Arc::new(MemoryDocsSync::default()),
        Arc::new(MemoryBlobService::default()),
        sender.clone(),
    ));
    for _ in 0..5 {
        let envelope = build_follow_edge_envelope_with_docs_author(
            &sender,
            &recipient.public_key(),
            FollowEdgeStatus::Active,
            None,
        )
        .unwrap();
        app.queue_public_notification_offer(
            PublicNotificationSource::Follow { envelope },
            BTreeSet::from([recipient.public_key_hex()]),
        )
        .await;
    }
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while offers.resolve_attempts.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    gate.0.store(true, Ordering::SeqCst);
    gate.1.notify_waiters();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while offers.resolve_attempts.load(Ordering::SeqCst) < 5 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("all five explicit recipients must reach destination lookup");
    gate.0.store(false, Ordering::SeqCst);
    let envelope = build_follow_edge_envelope_with_docs_author(
        &sender,
        &recipient.public_key(),
        FollowEdgeStatus::Active,
        None,
    )
    .unwrap();
    app.queue_public_notification_offer(
        PublicNotificationSource::Follow { envelope },
        BTreeSet::from([recipient.public_key_hex()]),
    )
    .await;
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while offers.resolve_attempts.load(Ordering::SeqCst) < 6 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    app.shutdown().await;
    assert!(
        app.subscription_registry
            .public_notification_offer_queue
            .lock()
            .await
            .is_none(),
        "account shutdown must remove the public offer worker"
    );
}

// #1221 R4-D: 相手の unfollow は follow の offer で届き、手元の edge を更新する。mutual の解除は次の送信の判定に
// 反映し、保護 outbox は ACK まで残す。自分の unfollow も相手へ follow の offer を送る。
#[tokio::test]
async fn unfollow_offer_revokes_mutual_and_keeps_protected_outbox() {
    use kukuri_core::BlobHash;
    use kukuri_store::DirectMessageOutboxRow;

    let peer = generate_keys();
    let local = generate_keys();
    let store = Arc::new(MemoryStore::default());
    let memory_blob = Arc::new(MemoryBlobService::default());
    let blob = Arc::new(OfferBlobService::new(memory_blob.clone()));
    let transport = Arc::new(FakeTransport::new("local", FakeNetwork::default()));
    let app = offer_app(local.clone(), store.clone(), transport, blob);
    let peer_hex = peer.public_key_hex();
    seed_follow_edges(
        store.as_ref(),
        &local.public_key_hex(),
        [peer_hex.as_str()],
        FollowEdgeStatus::Active,
    )
    .await;
    let row = DirectMessageOutboxRow {
        dm_id: direct_message_id_for_participants(&local.public_key(), &peer.public_key()),
        message_id: "protected".into(),
        peer_pubkey: peer_hex.clone(),
        frame_blob_hash: BlobHash::new("aa".repeat(32)),
        created_at: 1,
        last_attempt_at: None,
    };
    store.put_direct_message_outbox(row.clone()).await.unwrap();
    assert!(
        app.direct_message_status_view(&peer_hex)
            .await
            .unwrap()
            .send_enabled
    );

    let unfollow = build_follow_edge_envelope_with_docs_author(
        &peer,
        &local.public_key(),
        FollowEdgeStatus::Revoked,
        None,
    )
    .unwrap();
    deliver_public_source(
        &app,
        &peer,
        &local,
        memory_blob.as_ref(),
        PublicNotificationSource::Follow { envelope: unfollow },
    )
    .await
    .unwrap();
    let status = app.direct_message_status_view(&peer_hex).await.unwrap();
    assert!(!status.mutual && !status.send_enabled);
    assert!(
        app.send_direct_message(&peer_hex, Some("after unfollow"), None, Vec::new())
            .await
            .is_err()
    );
    assert!(
        store
            .get_direct_message_outbox(&row.dm_id, &row.message_id)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn unfollow_sends_a_follow_offer_to_the_target() {
    use kukuri_transport::EndpointAddr;

    let local = generate_keys();
    let peer = generate_keys();
    let hints = Arc::new(TrackingHintTransport::default());
    *hints.resolved_destination.lock().await = Some(EndpointAddr::new(
        iroh::SecretKey::from_bytes(&[61; 32]).public(),
    ));
    let store = Arc::new(MemoryStore::default());
    let app = app_service_from_dependencies(
        store.clone(),
        store,
        Arc::new(
            StaticTransport::new(PeerSnapshot::default()).with_local_endpoint_id(
                iroh::SecretKey::from_bytes(&[62; 32]).public().to_string(),
            ),
        ),
        hints.clone(),
        Arc::new(MemoryDocsSync::default()),
        Arc::new(MemoryBlobService::default()),
        local.clone(),
    );
    let peer_hex = peer.public_key_hex();
    app.follow_author(&peer_hex).await.unwrap();
    app.unfollow_author(&peer_hex).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while hints.offers.lock().await.len() < 2 {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("follow and unfollow both reach the target by account offer");
    let offers = hints.offers.lock().await;
    assert!(offers.iter().all(|(recipient, _, offer)| {
        recipient == &peer.public_key()
            && matches!(
                offer
                    .open(&peer, Utc::now().timestamp_millis())
                    .unwrap()
                    .reference()
                    .scope,
                ReceiveOfferScopeV1::PublicSource
            )
    }));
    drop(offers);
    app.shutdown().await;
}
