use super::super::*;
use super::receive_offer_doubles::OfferBlobService;

#[tokio::test]
async fn dm_outbox_page_sends_sealed_account_offer_without_consuming_protected_row() {
    use kukuri_core::{BlobHash, ReceiveOfferScopeV1};
    use kukuri_store::{DirectMessageOutboxRow, DirectMessageStore};
    use kukuri_transport::EndpointAddr;

    let store = Arc::new(MemoryStore::default());
    let sender = generate_keys();
    let recipient = generate_keys();
    let local = sender.public_key_hex();
    let peer = recipient.public_key_hex();
    seed_follow_edges(
        store.as_ref(),
        &local,
        [peer.as_str()],
        FollowEdgeStatus::Active,
    )
    .await;
    let row = DirectMessageOutboxRow {
        dm_id: direct_message_id_for_participants(&sender.public_key(), &recipient.public_key()),
        message_id: "account-offer-message".into(),
        peer_pubkey: peer.clone(),
        frame_blob_hash: BlobHash::new("aa".repeat(32)),
        created_at: 42,
        last_attempt_at: None,
    };
    store.put_direct_message_outbox(row.clone()).await.unwrap();
    let destination = EndpointAddr::new(iroh::SecretKey::from_bytes(&[23; 32]).public());
    let provider = iroh::SecretKey::from_bytes(&[24; 32]).public();
    let hints = Arc::new(TrackingHintTransport::default());
    *hints.resolved_destination.lock().await = Some(destination.clone());
    let blobs = Arc::new(OfferBlobService::new(
        Arc::new(MemoryBlobService::default()),
    ));
    let services = ServiceHandles::new(
        store.clone(),
        store.clone(),
        Arc::new(
            StaticTransport::new(PeerSnapshot::default())
                .with_local_endpoint_id(provider.to_string()),
        ),
        hints.clone(),
        Arc::new(MemoryDocsSync::default()),
        blobs.clone(),
        sender.clone(),
    );
    let published = AppService::flush_due_direct_message_outbox(&services, 1_000)
        .await
        .unwrap();
    assert_eq!(published, 1);
    assert_eq!(hints.resolved_count.load(Ordering::SeqCst), 1);
    let offers = hints.offers.lock().await;
    assert_eq!(offers.len(), 1);
    assert_eq!(offers[0].0, recipient.public_key());
    assert_eq!(offers[0].1.id, destination.id);
    let opened = offers[0]
        .2
        .open(&recipient, Utc::now().timestamp_millis())
        .unwrap();
    assert_eq!(opened.sender(), &sender.public_key());
    assert_eq!(
        opened.reference().provider_endpoint_id,
        provider.to_string()
    );
    assert_eq!(opened.reference().payload_bytes, 0);
    assert!(opened.reference().payload_hash.as_str().is_empty());
    assert!(
        matches!(&opened.reference().scope, ReceiveOfferScopeV1::DirectMessageFrame { dm_id, message_id, frame_hash }
        if dm_id == &row.dm_id && message_id == &row.message_id && frame_hash == &row.frame_blob_hash)
    );
    assert_eq!(blobs.writes.load(Ordering::SeqCst), 0);
    assert!(
        store
            .get_direct_message_outbox(&row.dm_id, &row.message_id)
            .await
            .unwrap()
            .is_some()
    );
    drop(offers);
    let _busy = services
        .account_dm_offer_permits
        .acquire_many(4)
        .await
        .unwrap();
    timeout(
        Duration::from_secs(1),
        AppService::flush_due_direct_message_outbox(&services, 3_000),
    )
    .await
    .expect("busy account offer permits must defer without queuing")
    .unwrap();
    assert_eq!(hints.offers.lock().await.len(), 1);
    assert!(
        store
            .get_direct_message_outbox(&row.dm_id, &row.message_id)
            .await
            .unwrap()
            .is_some()
    );
    drop(_busy);
    hints.fail_offer_publish.store(true, Ordering::SeqCst);
    AppService::flush_due_direct_message_outbox(&services, 5_000)
        .await
        .unwrap();
    assert!(hints.resolved_destination.lock().await.is_none());
    assert_eq!(hints.offers.lock().await.len(), 1);
    assert!(
        store
            .get_direct_message_outbox(&row.dm_id, &row.message_id)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn revoked_mutual_after_destination_lookup_sends_no_account_dm_offer() {
    use kukuri_core::BlobHash;
    use kukuri_store::{DirectMessageOutboxRow, DirectMessageStore};
    use kukuri_transport::EndpointAddr;

    let store = Arc::new(MemoryStore::default());
    let sender = generate_keys();
    let recipient = generate_keys();
    let local = sender.public_key_hex();
    let peer = recipient.public_key_hex();
    seed_follow_edges(
        store.as_ref(),
        &local,
        [peer.as_str()],
        FollowEdgeStatus::Active,
    )
    .await;
    let row = DirectMessageOutboxRow {
        dm_id: direct_message_id_for_participants(&sender.public_key(), &recipient.public_key()),
        message_id: "revoked-while-resolving-destination".into(),
        peer_pubkey: peer.clone(),
        frame_blob_hash: BlobHash::new("bb".repeat(32)),
        created_at: 42,
        last_attempt_at: None,
    };
    store.put_direct_message_outbox(row.clone()).await.unwrap();
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let mut hint_double = TrackingHintTransport::default();
    hint_double.resolve_barrier = Some(barrier.clone());
    let hints = Arc::new(hint_double);
    *hints.resolved_destination.lock().await = Some(EndpointAddr::new(
        iroh::SecretKey::from_bytes(&[26; 32]).public(),
    ));
    let blob = Arc::new(OfferBlobService::new(
        Arc::new(MemoryBlobService::default()),
    ));
    let services = ServiceHandles::new(
        store.clone(),
        store.clone(),
        Arc::new(
            StaticTransport::new(PeerSnapshot::default()).with_local_endpoint_id(
                iroh::SecretKey::from_bytes(&[27; 32]).public().to_string(),
            ),
        ),
        hints.clone(),
        Arc::new(MemoryDocsSync::default()),
        blob.clone(),
        sender,
    );
    let local_for_revoke = local.clone();
    let flush =
        tokio::spawn(
            async move { AppService::flush_due_direct_message_outbox(&services, 1_000).await },
        );
    timeout(Duration::from_secs(5), barrier.wait())
        .await
        .expect("destination lookup must reach the pause");
    seed_follow_edges(
        store.as_ref(),
        &local_for_revoke,
        [peer.as_str()],
        FollowEdgeStatus::Revoked,
    )
    .await;
    timeout(Duration::from_secs(5), barrier.wait())
        .await
        .expect("destination lookup must resume");
    flush.await.unwrap().unwrap();
    assert!(hints.offers.lock().await.is_empty());
    assert_eq!(blob.writes.load(Ordering::SeqCst), 0);
    assert!(
        store
            .get_direct_message_outbox(&row.dm_id, &row.message_id)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn account_dm_retry_owner_is_single_and_shutdown_cancels_active_lookup() {
    use kukuri_core::BlobHash;
    use kukuri_store::{DirectMessageOutboxRow, DirectMessageStore};

    let store = Arc::new(MemoryStore::default());
    let sender = generate_keys();
    let recipient = generate_keys();
    let local = sender.public_key_hex();
    let peer = recipient.public_key_hex();
    seed_follow_edges(
        store.as_ref(),
        &local,
        [peer.as_str()],
        FollowEdgeStatus::Active,
    )
    .await;
    let row = DirectMessageOutboxRow {
        dm_id: direct_message_id_for_participants(&sender.public_key(), &recipient.public_key()),
        message_id: "cancel-owner-lookup".into(),
        peer_pubkey: peer,
        frame_blob_hash: BlobHash::new("cc".repeat(32)),
        created_at: 42,
        last_attempt_at: None,
    };
    store.put_direct_message_outbox(row.clone()).await.unwrap();
    for index in 0..100 {
        store
            .put_direct_message_outbox(DirectMessageOutboxRow {
                dm_id: format!("unrelated-dm-{index}"),
                message_id: format!("unrelated-message-{index}"),
                peer_pubkey: format!("unrelated-peer-{index}"),
                frame_blob_hash: BlobHash::new("dd".repeat(32)),
                created_at: 43,
                last_attempt_at: None,
            })
            .await
            .unwrap();
    }
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let mut hint_double = TrackingHintTransport::default();
    hint_double.resolve_barrier = Some(barrier.clone());
    let hints = Arc::new(hint_double);
    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        Arc::new(StaticTransport::new(PeerSnapshot::default())),
        hints.clone(),
        Arc::new(MemoryDocsSync::default()),
        Arc::new(MemoryBlobService::default()),
        sender,
    );
    app.start_direct_message_outbox_retry().await.unwrap();
    app.start_direct_message_outbox_retry().await.unwrap();
    assert_eq!(
        app.subscription_registry
            .dm_outbox_retry_starts
            .load(Ordering::SeqCst),
        1
    );
    timeout(Duration::from_secs(5), barrier.wait())
        .await
        .expect("owner must enter destination lookup");
    timeout(Duration::from_secs(2), app.shutdown())
        .await
        .expect("shutdown must cancel the active lookup");
    assert!(app.start_direct_message_outbox_retry().await.is_err());
    assert!(
        app.subscription_registry
            .dm_outbox_retry_task
            .lock()
            .await
            .is_none()
    );
    assert!(hints.offers.lock().await.is_empty());
    assert!(
        store
            .get_direct_message_outbox(&row.dm_id, &row.message_id)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn account_candidate_demand_uses_only_bounded_mutual_due_rows() {
    use kukuri_core::BlobHash;
    use kukuri_store::{DirectMessageOutboxRow, DirectMessageStore};

    let store = Arc::new(MemoryStore::default());
    let sender = generate_keys();
    let peers = (0..5).map(|_| generate_keys()).collect::<Vec<_>>();
    let local = sender.public_key_hex();
    seed_follow_edges(
        store.as_ref(),
        &local,
        [0usize, 1, 3].map(|index| peers[index].public_key_hex()),
        FollowEdgeStatus::Active,
    )
    .await;
    for (index, peer) in peers.iter().enumerate() {
        store
            .put_direct_message_outbox(DirectMessageOutboxRow {
                dm_id: format!("candidate-dm-{index}"),
                message_id: format!("candidate-message-{index}"),
                peer_pubkey: peer.public_key_hex(),
                frame_blob_hash: BlobHash::new("aa".repeat(32)),
                created_at: index as i64,
                last_attempt_at: (index == 3).then_some(0),
            })
            .await
            .unwrap();
    }
    let app = app_service_from_dependencies(
        store.clone(),
        store,
        Arc::new(StaticTransport::new(PeerSnapshot::default())),
        Arc::new(TrackingHintTransport::default()),
        Arc::new(MemoryDocsSync::default()),
        Arc::new(MemoryBlobService::default()),
        sender,
    );
    let actual = app
        .pending_receive_destination_recipients(None, None)
        .await
        .unwrap();
    let expected = [0usize, 1, 3]
        .into_iter()
        .map(|index| peers[index].public_key())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        actual.recipients.into_iter().collect::<BTreeSet<_>>(),
        expected
    );
    let next = app
        .pending_receive_destination_recipients(
            actual.next_cursor.as_ref(),
            actual.cycle_end.as_ref(),
        )
        .await
        .unwrap();
    assert!(next.recipients.is_empty());
    assert!(next.next_cursor.is_none());
}

#[tokio::test]
async fn dm_due_owner_processes_bounded_new_and_retry_lanes() {
    use kukuri_core::BlobHash;
    use kukuri_store::DirectMessageOutboxRow;

    let store = Arc::new(MemoryStore::default());
    let local_keys = generate_keys();
    let local = local_keys.public_key_hex();
    let peer = generate_keys().public_key_hex();
    let unrelated = generate_keys().public_key_hex();
    seed_follow_edges(
        store.as_ref(),
        &local,
        [peer.as_str()],
        FollowEdgeStatus::Active,
    )
    .await;
    for index in 0..1_000 {
        DirectMessageStore::put_direct_message_outbox(
            store.as_ref(),
            DirectMessageOutboxRow {
                dm_id: "dm-unrelated".into(),
                message_id: format!("unrelated-{index:04}"),
                peer_pubkey: unrelated.clone(),
                frame_blob_hash: BlobHash::new("unrelated-hash"),
                created_at: 42,
                last_attempt_at: None,
            },
        )
        .await
        .unwrap();
    }
    for index in 0..130 {
        DirectMessageStore::put_direct_message_outbox(
            store.as_ref(),
            DirectMessageOutboxRow {
                dm_id: "dm-target".into(),
                message_id: format!("target-{index:04}"),
                peer_pubkey: peer.clone(),
                frame_blob_hash: BlobHash::new("target-hash"),
                created_at: 42,
                last_attempt_at: Some(0),
            },
        )
        .await
        .unwrap();
    }
    let hint_transport = Arc::new(TrackingHintTransport::default());
    let projection_store = store.clone();
    let services = ServiceHandles::new(
        store.clone(),
        store,
        Arc::new(StaticTransport::new(PeerSnapshot::default())),
        hint_transport.clone(),
        Arc::new(MemoryDocsSync::default()),
        Arc::new(MemoryBlobService::default()),
        local_keys,
    );
    for tick in 0..3 {
        let processed = AppService::flush_due_direct_message_outbox(&services, 3_000)
            .await
            .unwrap();
        assert_eq!(processed, 4);
        assert_eq!(
            hint_transport.resolved_count.load(Ordering::SeqCst),
            tick + 1,
            "only one mutual recipient is resolved in this bounded tick"
        );
        for index in 0..3 {
            DirectMessageStore::put_direct_message_outbox(
                projection_store.as_ref(),
                DirectMessageOutboxRow {
                    dm_id: "dm-unrelated".into(),
                    message_id: format!("later-{tick}-{index}"),
                    peer_pubkey: unrelated.clone(),
                    frame_blob_hash: BlobHash::new("unrelated-hash"),
                    created_at: 43,
                    last_attempt_at: None,
                },
            )
            .await
            .unwrap();
        }
    }
    let app = AppService::from_handles(services);
    app.send_direct_message_internal(peer.as_str(), Some("fresh"), None, Vec::new())
        .await
        .unwrap();
    // #1221 R4-D: 送信も再送も pairwise の topic を購読・publish しない。
    assert_eq!(hint_transport.published_count.load(Ordering::SeqCst), 0);
    assert_eq!(*hint_transport.subscribe_count.lock().await, 0);
}
