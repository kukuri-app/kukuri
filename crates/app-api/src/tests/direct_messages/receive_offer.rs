use super::super::*;
use super::receive_offer_doubles::{OfferBlobService, ProbeOfferTransport};
use kukuri_core::{ReceiveOfferReferenceV1, ReceiveOfferScopeV1, seal_receive_offer};
use kukuri_transport::{EndpointAddr, ReceiveOfferEnvelope};

pub(super) fn offer_app(
    keys: KukuriKeys,
    store: Arc<MemoryStore>,
    transport: Arc<FakeTransport>,
    blob: Arc<OfferBlobService>,
) -> AppService {
    AppService::from_handles(ServiceHandles::new(
        store.clone(),
        store,
        transport.clone(),
        transport,
        Arc::new(MemoryDocsSync::default()),
        blob,
        keys,
    ))
}

pub(super) fn offer_for(
    sender: &KukuriKeys,
    recipient: &KukuriKeys,
    scope: ReceiveOfferScopeV1,
    payload_hash: kukuri_core::BlobHash,
    payload_bytes: u32,
) -> (EndpointAddr, kukuri_core::SealedReceiveOfferV1) {
    let provider = EndpointAddr::new(iroh::SecretKey::from_bytes(&[43; 32]).public());
    let now = Utc::now().timestamp_millis();
    let offer = seal_receive_offer(
        sender,
        &recipient.public_key(),
        ReceiveOfferReferenceV1 {
            provider_endpoint_id: provider.id.to_string(),
            payload_hash,
            payload_bytes,
            scope,
        },
        now,
        now + 60_000,
    )
    .unwrap();
    (provider, offer)
}

#[tokio::test]
async fn signed_account_route_ack_clears_only_matching_dm_outbox() {
    use kukuri_core::build_direct_message_ack;
    use kukuri_store::DirectMessageOutboxRow;

    let sender = generate_keys();
    let recipient = generate_keys();
    let store = Arc::new(MemoryStore::default());
    let local = sender.public_key_hex();
    let peer = recipient.public_key_hex();
    seed_follow_edges(
        store.as_ref(),
        &local,
        [peer.as_str()],
        FollowEdgeStatus::Active,
    )
    .await;
    let dm_id = direct_message_id_for_participants(&sender.public_key(), &recipient.public_key());
    let row = DirectMessageOutboxRow {
        dm_id: dm_id.clone(),
        message_id: "ack-on-account-route".into(),
        peer_pubkey: peer.clone(),
        frame_blob_hash: kukuri_core::BlobHash::new("aa".repeat(32)),
        created_at: 10,
        last_attempt_at: None,
    };
    store.put_direct_message_outbox(row.clone()).await.unwrap();
    let blob = Arc::new(OfferBlobService::new(
        Arc::new(MemoryBlobService::default()),
    ));
    let transport = Arc::new(FakeTransport::new("sender", FakeNetwork::default()));
    let app = offer_app(sender.clone(), store.clone(), transport, blob.clone());
    let ack = build_direct_message_ack(
        &recipient,
        &dm_id,
        &row.message_id,
        &sender.public_key(),
        Utc::now().timestamp_millis(),
    )
    .unwrap();
    let now = Utc::now().timestamp_millis();
    let forged = seal_receive_offer(
        &recipient,
        &sender.public_key(),
        ReceiveOfferReferenceV1::inline(
            String::new(),
            ReceiveOfferScopeV1::DirectMessageAck {
                dm_id: ack.dm_id.clone(),
                message_id: ack.message_id.clone(),
                acked_at: ack.acked_at,
                signature: "11".repeat(64),
            },
        )
        .unwrap(),
        now,
        now + 60_000,
    )
    .unwrap();
    assert!(
        AppService::ingest_account_receive_offer(
            &app.services,
            ReceiveOfferEnvelope {
                offer: forged,
                received_at: now,
                source_peer: "untrusted relay".into(),
            },
        )
        .await
        .is_err()
    );
    assert!(
        store
            .get_direct_message_outbox(&dm_id, &row.message_id)
            .await
            .unwrap()
            .is_some()
    );
    let offer = seal_receive_offer(
        &recipient,
        &sender.public_key(),
        ReceiveOfferReferenceV1::inline(
            String::new(),
            ReceiveOfferScopeV1::DirectMessageAck {
                dm_id: ack.dm_id,
                message_id: ack.message_id,
                acked_at: ack.acked_at,
                signature: ack.signature,
            },
        )
        .unwrap(),
        now,
        now + 60_000,
    )
    .unwrap();
    assert!(
        AppService::ingest_account_receive_offer(
            &app.services,
            ReceiveOfferEnvelope {
                offer,
                received_at: Utc::now().timestamp_millis(),
                source_peer: "untrusted relay".into(),
            },
        )
        .await
        .unwrap()
    );
    assert!(
        store
            .get_direct_message_outbox(&dm_id, &row.message_id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(blob.fetches.load(Ordering::SeqCst), 0);
    assert_eq!(blob.writes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn inline_dm_frame_requires_sender_bound_provider_before_blob_io() {
    let sender = generate_keys();
    let recipient = generate_keys();
    let store = Arc::new(MemoryStore::default());
    seed_follow_edges(
        store.as_ref(),
        &recipient.public_key_hex(),
        [sender.public_key_hex()],
        FollowEdgeStatus::Active,
    )
    .await;
    let network = FakeNetwork::default();
    let transport = Arc::new(FakeTransport::new("recipient", network.clone()));
    let blob = Arc::new(OfferBlobService::new(
        Arc::new(MemoryBlobService::default()),
    ));
    let app = offer_app(recipient.clone(), store, transport, blob.clone());
    let provider = iroh::SecretKey::from_bytes(&[44; 32]).public();
    let now = Utc::now().timestamp_millis();
    let offer = seal_receive_offer(
        &sender,
        &recipient.public_key(),
        ReceiveOfferReferenceV1::inline(
            provider.to_string(),
            ReceiveOfferScopeV1::DirectMessageFrame {
                dm_id: direct_message_id_for_participants(
                    &sender.public_key(),
                    &recipient.public_key(),
                ),
                message_id: "wrong-provider".into(),
                frame_hash: kukuri_core::BlobHash::new("aa".repeat(32)),
            },
        )
        .unwrap(),
        now,
        now + 60_000,
    )
    .unwrap();
    assert!(
        AppService::ingest_account_receive_offer(
            &app.services,
            ReceiveOfferEnvelope {
                offer: offer.clone(),
                received_at: now,
                source_peer: provider.to_string(),
            },
        )
        .await
        .is_err()
    );
    assert_eq!(blob.regular_fetches.load(Ordering::SeqCst), 0);
    network
        .trust_receive_provider(&generate_keys().public_key(), &provider.to_string())
        .await;
    assert!(
        AppService::ingest_account_receive_offer(
            &app.services,
            ReceiveOfferEnvelope {
                offer,
                received_at: now,
                source_peer: provider.to_string(),
            },
        )
        .await
        .is_err()
    );
    assert_eq!(blob.regular_fetches.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn account_receive_offer_rejects_unmutual_dm_and_private_scope_before_provider_io() {
    let sender = generate_keys();
    let recipient = generate_keys();
    let store = Arc::new(MemoryStore::default());
    let blob = Arc::new(OfferBlobService::new(
        Arc::new(MemoryBlobService::default()),
    ));
    let transport = Arc::new(FakeTransport::new("recipient", FakeNetwork::default()));
    let app = offer_app(recipient.clone(), store.clone(), transport, blob.clone());
    let hash = kukuri_core::BlobHash::new("11".repeat(32));

    let (_, unmutual) = offer_for(
        &sender,
        &recipient,
        ReceiveOfferScopeV1::DirectMessage,
        hash.clone(),
        1,
    );
    assert!(
        !AppService::ingest_account_receive_offer(
            &app.services,
            ReceiveOfferEnvelope {
                offer: unmutual,
                received_at: Utc::now().timestamp_millis(),
                source_peer: "untrusted relay".into(),
            },
        )
        .await
        .unwrap()
    );

    seed_follow_edges(
        store.as_ref(),
        &recipient.public_key_hex(),
        [sender.public_key_hex()],
        FollowEdgeStatus::Active,
    )
    .await;
    let (_, private) = offer_for(
        &sender,
        &recipient,
        ReceiveOfferScopeV1::PrivateSource {
            epoch_key_id: "22".repeat(32),
        },
        hash,
        1,
    );
    assert!(
        !AppService::ingest_account_receive_offer(
            &app.services,
            ReceiveOfferEnvelope {
                offer: private,
                received_at: Utc::now().timestamp_millis(),
                source_peer: "untrusted relay".into(),
            },
        )
        .await
        .unwrap()
    );
    assert_eq!(blob.fetches.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn account_route_ingests_verified_mutual_dm_and_stops_on_shutdown() {
    let sender = generate_keys();
    let recipient = generate_keys();
    let store = Arc::new(MemoryStore::default());
    let memory_blob = Arc::new(MemoryBlobService::default());
    let blob = Arc::new(OfferBlobService::new(memory_blob.clone()));
    let network = FakeNetwork::default();
    let receiver_endpoint_id = iroh::SecretKey::from_bytes(&[25; 32]).public();
    let receiver_transport = Arc::new(FakeTransport::new(
        receiver_endpoint_id.to_string(),
        network.clone(),
    ));
    let sender_transport = FakeTransport::new("sender", network);
    let (_ack_lease, mut account_acks, _) = sender_transport
        .subscribe_receive_offers(&sender.public_key())
        .await
        .unwrap();
    let app = offer_app(
        recipient.clone(),
        store.clone(),
        receiver_transport,
        blob.clone(),
    );
    seed_follow_edges(
        store.as_ref(),
        &recipient.public_key_hex(),
        [sender.public_key_hex()],
        FollowEdgeStatus::Active,
    )
    .await;
    let dm_id = direct_message_id_for_participants(&sender.public_key(), &recipient.public_key());
    let message_id = "account-route-dm-1";
    let frame = encrypt_direct_message_frame(
        &sender,
        &recipient.public_key(),
        &dm_id,
        message_id,
        Utc::now().timestamp_millis(),
        &DirectMessagePayloadV1 {
            text: Some("over account route".into()),
            reply_to: None,
            attachment_manifest: None,
        },
    )
    .unwrap();
    let frame_blob = memory_blob
        .put_blob(
            serde_json::to_vec(&frame).unwrap(),
            DIRECT_MESSAGE_FRAME_MIME,
        )
        .await
        .unwrap();
    let topic = derive_direct_message_topic(&recipient, &sender.public_key()).unwrap();
    let manifest = serde_json::to_vec(&GossipHint::DirectMessageFrame {
        topic_id: topic,
        dm_id: dm_id.clone(),
        message_id: message_id.into(),
        frame_hash: frame_blob.hash,
    })
    .unwrap();
    let manifest_blob = memory_blob
        .put_blob(
            manifest,
            "application/vnd.kukuri.direct-message-receive-manifest+json",
        )
        .await
        .unwrap();
    let (provider, offer) = offer_for(
        &sender,
        &recipient,
        ReceiveOfferScopeV1::DirectMessage,
        manifest_blob.hash,
        manifest_blob.bytes as u32,
    );

    app.start_account_receive_offers().await.unwrap();
    app.start_account_receive_offers().await.unwrap();
    let inserted_notify = app.notification_inserted_notify();
    let mut inserted = Box::pin(inserted_notify.notified());
    inserted.as_mut().enable();
    sender_transport
        .publish_receive_offer(&recipient.public_key(), provider.clone(), offer.clone())
        .await
        .unwrap();
    timeout(Duration::from_secs(2), async {
        loop {
            if DirectMessageStore::get_direct_message_message(store.as_ref(), &dm_id, message_id)
                .await
                .unwrap()
                .is_some()
            {
                break;
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    timeout(Duration::from_secs(2), inserted)
        .await
        .expect("DM notification event forwarded");
    assert_eq!(blob.fetches.load(Ordering::SeqCst), 1);
    let account_ack = timeout(Duration::from_secs(2), account_acks.next())
        .await
        .expect("DM ACK must return through sender account route")
        .expect("sender account route ended");
    let opened_ack = account_ack
        .offer
        .open(&sender, Utc::now().timestamp_millis())
        .unwrap();
    assert_eq!(opened_ack.sender(), &recipient.public_key());
    assert!(opened_ack.reference().provider_endpoint_id.is_empty());
    assert!(opened_ack.reference().payload_hash.as_str().is_empty());
    assert_eq!(opened_ack.reference().payload_bytes, 0);
    assert!(
        matches!(&opened_ack.reference().scope, ReceiveOfferScopeV1::DirectMessageAck {
        dm_id: ack_dm_id, message_id: ack_message_id, ..
    } if ack_dm_id == &dm_id && ack_message_id == message_id)
    );
    assert_eq!(blob.writes.load(Ordering::SeqCst), 0);

    app.shutdown().await;
    assert!(app.start_account_receive_offers().await.is_err());
    sender_transport
        .publish_receive_offer(&recipient.public_key(), provider, offer)
        .await
        .unwrap();
    sleep(Duration::from_millis(30)).await;
    assert_eq!(blob.fetches.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn account_offer_rechecks_mutual_after_provider_io_before_reflection() {
    let sender = generate_keys();
    let recipient = generate_keys();
    let store = Arc::new(MemoryStore::default());
    let memory_blob = Arc::new(MemoryBlobService::default());
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let mut offer_blob = OfferBlobService::new(memory_blob.clone());
    offer_blob.barrier = Some(barrier.clone());
    let blob = Arc::new(offer_blob);
    let transport = Arc::new(FakeTransport::new("recipient", FakeNetwork::default()));
    let app = offer_app(recipient.clone(), store.clone(), transport, blob.clone());
    seed_follow_edges(
        store.as_ref(),
        &recipient.public_key_hex(),
        [sender.public_key_hex()],
        FollowEdgeStatus::Active,
    )
    .await;
    let dm_id = direct_message_id_for_participants(&sender.public_key(), &recipient.public_key());
    let message_id = "revoked-before-reflection";
    let frame = encrypt_direct_message_frame(
        &sender,
        &recipient.public_key(),
        &dm_id,
        message_id,
        Utc::now().timestamp_millis(),
        &DirectMessagePayloadV1 {
            text: Some("must not appear".into()),
            reply_to: None,
            attachment_manifest: None,
        },
    )
    .unwrap();
    let frame_blob = memory_blob
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
    let manifest_blob = memory_blob
        .put_blob(
            manifest,
            "application/vnd.kukuri.direct-message-receive-manifest+json",
        )
        .await
        .unwrap();
    let (_, offer) = offer_for(
        &sender,
        &recipient,
        ReceiveOfferScopeV1::DirectMessage,
        manifest_blob.hash,
        manifest_blob.bytes as u32,
    );
    let services = app.services.clone();
    let ingest = tokio::spawn(async move {
        AppService::ingest_account_receive_offer(
            &services,
            ReceiveOfferEnvelope {
                offer,
                received_at: Utc::now().timestamp_millis(),
                source_peer: "untrusted relay".into(),
            },
        )
        .await
    });
    timeout(Duration::from_secs(2), barrier.wait())
        .await
        .expect("provider fetch started");
    seed_follow_edges(
        store.as_ref(),
        &recipient.public_key_hex(),
        [sender.public_key_hex()],
        FollowEdgeStatus::Revoked,
    )
    .await;
    barrier.wait().await;
    assert!(!ingest.await.unwrap().unwrap());
    assert_eq!(blob.fetches.load(Ordering::SeqCst), 1);
    assert!(
        DirectMessageStore::get_direct_message_message(store.as_ref(), &dm_id, message_id)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn revoked_mutual_during_attachment_fetch_never_persists_plaintext() {
    let sender = generate_keys();
    let recipient = generate_keys();
    let store = Arc::new(MemoryStore::default());
    let memory_blob = Arc::new(MemoryBlobService::default());
    let message_id = "revoke-during-attachment";
    let encrypted = encrypt_direct_message_attachment(
        &sender,
        &recipient.public_key(),
        message_id,
        "attachment-1",
        b"private attachment",
    )
    .unwrap();
    let encrypted_blob = memory_blob
        .put_blob(
            serde_json::to_vec(&encrypted).unwrap(),
            "application/vnd.kukuri.direct-message-attachment+json",
        )
        .await
        .unwrap();
    let dm_id = direct_message_id_for_participants(&sender.public_key(), &recipient.public_key());
    let frame = encrypt_direct_message_frame(
        &sender,
        &recipient.public_key(),
        &dm_id,
        message_id,
        Utc::now().timestamp_millis(),
        &DirectMessagePayloadV1 {
            text: None,
            reply_to: None,
            attachment_manifest: Some(DirectMessageAttachmentManifestV1 {
                attachment_id: "attachment-1".into(),
                kind: DirectMessageAttachmentKind::Image,
                original: DirectMessageEncryptedBlobRefV1 {
                    blob_id: "attachment-1".into(),
                    hash: encrypted_blob.hash.clone(),
                    mime: "image/png".into(),
                    bytes: 18,
                    nonce_hex: encrypted.nonce_hex,
                },
                poster: None,
            }),
        },
    )
    .unwrap();
    let frame_blob = memory_blob
        .put_blob(
            serde_json::to_vec(&frame).unwrap(),
            DIRECT_MESSAGE_FRAME_MIME,
        )
        .await
        .unwrap();
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let mut wrapped = OfferBlobService::new(memory_blob);
    wrapped.pause_blob_hash = Some(encrypted_blob.hash);
    wrapped.attachment_barrier = Some(barrier.clone());
    let blob = Arc::new(wrapped);
    let app = offer_app(
        recipient.clone(),
        store.clone(),
        Arc::new(FakeTransport::new("recipient", FakeNetwork::default())),
        blob.clone(),
    );
    seed_follow_edges(
        store.as_ref(),
        &recipient.public_key_hex(),
        [sender.public_key_hex()],
        FollowEdgeStatus::Active,
    )
    .await;
    let recipient_pubkey = recipient.public_key_hex();
    let sender_pubkey = sender.public_key_hex();
    let dm_id_for_task = dm_id.clone();
    let frame_hash = frame_blob.hash;
    let services = app.services.clone();
    let ingest = tokio::spawn(async move {
        AppService::ingest_direct_message_frame(
            &services,
            &recipient_pubkey,
            &sender_pubkey,
            &dm_id_for_task,
            message_id,
            &frame_hash,
            None,
        )
        .await
    });
    timeout(Duration::from_secs(2), barrier.wait())
        .await
        .expect("attachment fetch started");
    seed_follow_edges(
        store.as_ref(),
        &recipient.public_key_hex(),
        [sender.public_key_hex()],
        FollowEdgeStatus::Revoked,
    )
    .await;
    barrier.wait().await;
    assert!(!ingest.await.unwrap().unwrap());
    assert_eq!(blob.writes.load(Ordering::SeqCst), 0);
    assert!(
        DirectMessageStore::get_direct_message_message(store.as_ref(), &dm_id, message_id)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn dropping_account_owner_aborts_offer_stream() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(ProbeOfferTransport::default());
    let app = AppService::from_handles(ServiceHandles::new(
        store.clone(),
        store,
        Arc::new(StaticTransport::new(PeerSnapshot::default())),
        transport.clone(),
        Arc::new(MemoryDocsSync::default()),
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    ));
    app.start_account_receive_offers().await.unwrap();
    drop(app);
    timeout(Duration::from_secs(1), async {
        while transport.stream_drops.load(Ordering::SeqCst) != 1 {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("drop should cancel account offer processing");
}

#[tokio::test]
async fn shutdown_cancels_an_account_offer_subscription_still_registering() {
    let store = Arc::new(MemoryStore::default());
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let transport = Arc::new(ProbeOfferTransport {
        subscribe_barrier: Some(barrier.clone()),
        ..Default::default()
    });
    let app = Arc::new(AppService::from_handles(ServiceHandles::new(
        store.clone(),
        store,
        Arc::new(StaticTransport::new(PeerSnapshot::default())),
        transport,
        Arc::new(MemoryDocsSync::default()),
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    )));
    let starter = {
        let app = app.clone();
        tokio::spawn(async move { app.start_account_receive_offers().await })
    };
    timeout(Duration::from_secs(1), barrier.wait())
        .await
        .expect("subscribe reached registration wait");
    timeout(Duration::from_secs(1), app.shutdown())
        .await
        .expect("shutdown must cancel pending subscribe");
    assert!(starter.await.unwrap().is_err());
    assert!(
        app.subscription_registry
            .account_receive_offer_task
            .lock()
            .await
            .is_none()
    );
}

#[tokio::test]
async fn cancelled_shutdown_retries_the_account_route_lease_cleanup() {
    let store = Arc::new(MemoryStore::default());
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let transport = Arc::new(ProbeOfferTransport {
        unsubscribe_barrier: Some(barrier.clone()),
        ..Default::default()
    });
    let app = Arc::new(AppService::from_handles(ServiceHandles::new(
        store.clone(),
        store,
        Arc::new(StaticTransport::new(PeerSnapshot::default())),
        transport.clone(),
        Arc::new(MemoryDocsSync::default()),
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    )));
    app.start_account_receive_offers().await.unwrap();
    let first_shutdown = {
        let app = app.clone();
        tokio::spawn(async move { app.shutdown().await })
    };
    timeout(Duration::from_secs(1), barrier.wait())
        .await
        .expect("first unsubscribe started");
    first_shutdown.abort();
    let _ = first_shutdown.await;
    assert!(
        app.subscription_registry
            .account_receive_offer_lease
            .lock()
            .unwrap()
            .is_some(),
        "cancelled cleanup must retain its lease"
    );
    timeout(Duration::from_secs(1), app.shutdown())
        .await
        .expect("next shutdown should retry unsubscribe");
    assert_eq!(transport.unsubscribes.load(Ordering::SeqCst), 2);
    assert!(
        app.subscription_registry
            .account_receive_offer_lease
            .lock()
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn shutdown_cancels_an_in_flight_account_offer_provider_fetch() {
    let sender = generate_keys();
    let recipient = generate_keys();
    let store = Arc::new(MemoryStore::default());
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let mut offer_blob = OfferBlobService::new(Arc::new(MemoryBlobService::default()));
    offer_blob.barrier = Some(barrier.clone());
    let blob = Arc::new(offer_blob);
    let network = FakeNetwork::default();
    let receiver = Arc::new(FakeTransport::new("recipient", network.clone()));
    let publisher = FakeTransport::new("sender", network);
    let app = offer_app(recipient.clone(), store.clone(), receiver, blob.clone());
    seed_follow_edges(
        store.as_ref(),
        &recipient.public_key_hex(),
        [sender.public_key_hex()],
        FollowEdgeStatus::Active,
    )
    .await;
    let (provider, offer) = offer_for(
        &sender,
        &recipient,
        ReceiveOfferScopeV1::DirectMessage,
        kukuri_core::BlobHash::new("11".repeat(32)),
        1,
    );
    app.start_account_receive_offers().await.unwrap();
    publisher
        .publish_receive_offer(&recipient.public_key(), provider, offer)
        .await
        .unwrap();
    timeout(Duration::from_secs(1), barrier.wait())
        .await
        .expect("provider fetch started");
    timeout(Duration::from_secs(1), app.shutdown())
        .await
        .expect("shutdown must cancel in-flight provider fetch");
    assert_eq!(blob.fetches.load(Ordering::SeqCst), 1);
    assert!(
        app.subscription_registry
            .account_receive_offer_task
            .lock()
            .await
            .is_none()
    );
}

#[tokio::test]
async fn old_account_owner_shutdown_cannot_stop_new_same_account_receiver() {
    let sender = generate_keys();
    let recipient = generate_keys();
    let network = FakeNetwork::default();
    let receiver = Arc::new(FakeTransport::new("recipient", network.clone()));
    let publisher = FakeTransport::new("sender", network);
    let old_store = Arc::new(MemoryStore::default());
    let new_store = Arc::new(MemoryStore::default());
    for store in [&old_store, &new_store] {
        seed_follow_edges(
            store.as_ref(),
            &recipient.public_key_hex(),
            [sender.public_key_hex()],
            FollowEdgeStatus::Active,
        )
        .await;
    }
    let old_app = offer_app(
        recipient.clone(),
        old_store,
        receiver.clone(),
        Arc::new(OfferBlobService::new(
            Arc::new(MemoryBlobService::default()),
        )),
    );
    let new_blob = Arc::new(OfferBlobService::new(
        Arc::new(MemoryBlobService::default()),
    ));
    let new_app = offer_app(recipient.clone(), new_store, receiver, new_blob.clone());
    old_app.start_account_receive_offers().await.unwrap();
    new_app.start_account_receive_offers().await.unwrap();
    // Keep the old owner alive past its retry delay. It must not reclaim the
    // lease after the new owner supersedes its stream.
    sleep(Duration::from_millis(3_200)).await;
    old_app.shutdown().await;
    let (provider, offer) = offer_for(
        &sender,
        &recipient,
        ReceiveOfferScopeV1::DirectMessage,
        kukuri_core::BlobHash::new("11".repeat(32)),
        1,
    );
    publisher
        .publish_receive_offer(&recipient.public_key(), provider, offer)
        .await
        .unwrap();
    timeout(Duration::from_secs(1), async {
        while new_blob.fetches.load(Ordering::SeqCst) == 0 {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("new account route must survive old owner shutdown");
    new_app.shutdown().await;
}

#[tokio::test]
async fn superseding_account_owner_cancels_old_in_flight_provider_fetch() {
    let sender = generate_keys();
    let recipient = generate_keys();
    let store = Arc::new(MemoryStore::default());
    seed_follow_edges(
        store.as_ref(),
        &recipient.public_key_hex(),
        [sender.public_key_hex()],
        FollowEdgeStatus::Active,
    )
    .await;
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let mut old_blob = OfferBlobService::new(Arc::new(MemoryBlobService::default()));
    old_blob.barrier = Some(barrier.clone());
    let old_blob = Arc::new(old_blob);
    let network = FakeNetwork::default();
    let receiver = Arc::new(FakeTransport::new("recipient", network.clone()));
    let publisher = FakeTransport::new("sender", network);
    let old_app = offer_app(recipient.clone(), store, receiver.clone(), old_blob.clone());
    old_app.start_account_receive_offers().await.unwrap();
    let (provider, offer) = offer_for(
        &sender,
        &recipient,
        ReceiveOfferScopeV1::DirectMessage,
        kukuri_core::BlobHash::new("11".repeat(32)),
        1,
    );
    publisher
        .publish_receive_offer(&recipient.public_key(), provider, offer)
        .await
        .unwrap();
    timeout(Duration::from_secs(1), barrier.wait())
        .await
        .expect("old provider fetch started");
    assert_eq!(old_blob.in_flight.load(Ordering::SeqCst), 1);
    let new_app = offer_app(
        recipient,
        Arc::new(MemoryStore::default()),
        receiver,
        Arc::new(OfferBlobService::new(
            Arc::new(MemoryBlobService::default()),
        )),
    );
    new_app.start_account_receive_offers().await.unwrap();
    timeout(Duration::from_secs(1), async {
        while old_blob.in_flight.load(Ordering::SeqCst) != 0 {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("superseded owner's pending fetch must be canceled");
    assert_eq!(old_blob.writes.load(Ordering::SeqCst), 0);
    old_app.shutdown().await;
    new_app.shutdown().await;
}
