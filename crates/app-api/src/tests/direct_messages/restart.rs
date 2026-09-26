use super::super::*;

// #1221 R4-D: offline で保存した DM は、再起動後に account の再送 owner が account route だけで送る。
// 受信側は DM を開かずに会話一覧へ出し、手元で消した message は同じ frame が再び届いても戻さない。
#[tokio::test]
async fn dm_restart_resends_outbox_by_account_route_and_recipient_keeps_local_delete() {
    use kukuri_core::ReceiveOfferScopeV1;
    use kukuri_transport::EndpointAddr;

    let transport = Arc::new(
        StaticTransport::new(PeerSnapshot::default())
            .with_local_endpoint_id(iroh::SecretKey::from_bytes(&[51; 32]).public().to_string()),
    );
    let hint_transport = Arc::new(TrackingHintTransport::default());
    *hint_transport.resolved_destination.lock().await = Some(EndpointAddr::new(
        iroh::SecretKey::from_bytes(&[52; 32]).public(),
    ));
    let docs_sync = Arc::new(MemoryDocsSync::default());
    let blob_service = Arc::new(MemoryBlobService::default());
    let store_a = Arc::new(MemoryStore::default());
    let store_b = Arc::new(MemoryStore::default());
    let keys_a = generate_keys();
    let keys_b = generate_keys();
    let a_pubkey = keys_a.public_key_hex();
    let b_pubkey = keys_b.public_key_hex();
    seed_follow_edges(
        store_a.as_ref(),
        &a_pubkey,
        [b_pubkey.as_str()],
        FollowEdgeStatus::Active,
    )
    .await;
    seed_follow_edges(
        store_b.as_ref(),
        &b_pubkey,
        [a_pubkey.as_str()],
        FollowEdgeStatus::Active,
    )
    .await;
    let app = |store: &Arc<MemoryStore>, keys: &KukuriKeys| {
        app_service_from_dependencies(
            store.clone(),
            store.clone(),
            transport.clone(),
            hint_transport.clone(),
            docs_sync.clone(),
            blob_service.clone(),
            keys.clone(),
        )
    };

    let app_a = app(&store_a, &keys_a);
    let message_id = app_a
        .send_direct_message(
            b_pubkey.as_str(),
            Some("offline image"),
            None,
            vec![pending_image_attachment(
                "image/png",
                tiny_png_bytes().as_slice(),
            )],
        )
        .await
        .expect("queue direct message while offline");
    let queued = store_a.list_direct_message_outbox().await.unwrap();
    assert_eq!(queued.len(), 1);
    drop(app_a);

    let reopened_app_a = app(&store_a, &keys_a);
    reopened_app_a
        .resume_direct_message_state()
        .await
        .expect("resume direct message state");
    assert_eq!(
        reopened_app_a
            .subscription_registry
            .dm_outbox_retry_starts
            .load(Ordering::SeqCst),
        1,
        "restore starts one account retry owner"
    );
    AppService::flush_due_direct_message_outbox(
        &reopened_app_a.services,
        Utc::now().timestamp_millis() + DIRECT_MESSAGE_RETRY_INTERVAL_MS as i64,
    )
    .await
    .expect("flush queued direct message after restart");
    {
        let offers = hint_transport.offers.lock().await;
        assert!(!offers.is_empty());
        let opened = offers[0]
            .2
            .open(&keys_b, Utc::now().timestamp_millis())
            .unwrap();
        assert!(matches!(
            &opened.reference().scope,
            ReceiveOfferScopeV1::DirectMessageFrame { message_id: offered, .. } if offered == &message_id
        ));
    }
    assert_eq!(
        store_a.list_direct_message_outbox().await.unwrap().len(),
        1,
        "the protected outbox stays until the signed ACK"
    );
    assert_eq!(hint_transport.published_count.load(Ordering::SeqCst), 0);

    // 受信側は DM を開かない。account route の検証後の反映(ingest)で会話一覧に出る。
    let app_b = app(&store_b, &keys_b);
    let ingest = || {
        AppService::ingest_direct_message_frame(
            &app_b.services,
            &b_pubkey,
            &a_pubkey,
            &queued[0].dm_id,
            &queued[0].message_id,
            &queued[0].frame_blob_hash,
            None,
        )
    };
    assert!(ingest().await.unwrap());
    let conversation = app_b
        .list_direct_messages()
        .await
        .unwrap()
        .into_iter()
        .find(|item| item.peer_pubkey == a_pubkey)
        .expect("conversation appears without opening the DM");
    assert_eq!(
        conversation.last_message_id.as_deref(),
        Some(message_id.as_str())
    );
    assert_eq!(
        conversation.last_message_preview.as_deref(),
        Some("offline image")
    );
    let delivered = app_b
        .list_direct_message_messages(a_pubkey.as_str(), None, 20)
        .await
        .unwrap();
    assert_eq!(delivered.items.len(), 1);
    assert_eq!(delivered.items[0].attachments[0].role, "image_original");

    app_b
        .delete_direct_message_message(a_pubkey.as_str(), message_id.as_str())
        .await
        .expect("delete direct message locally");
    assert!(!ingest().await.unwrap());
    let after_duplicate = app_b
        .list_direct_message_messages(a_pubkey.as_str(), None, 20)
        .await
        .unwrap();
    assert!(after_duplicate.items.is_empty());
}
