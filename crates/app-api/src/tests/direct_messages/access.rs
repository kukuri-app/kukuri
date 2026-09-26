use super::super::*;

#[tokio::test]
async fn dm_send_requires_mutual_relationship() {
    let (app, _, _, _) = local_app_with_memory_services();
    let peer_keys = generate_keys();

    let error = app
        .send_direct_message(
            peer_keys.public_key_hex().as_str(),
            Some("hello"),
            None,
            Vec::new(),
        )
        .await
        .expect_err("direct message send should require mutual relationship");

    assert!(
        error
            .to_string()
            .contains("direct message requires a mutual relationship")
    );
}

#[tokio::test]
async fn dm_status_uses_a_bounded_peer_outbox_window() {
    use kukuri_core::BlobHash;
    use kukuri_store::DirectMessageOutboxRow;

    let (app, store, _, _) = local_app_with_memory_services();
    let peer = generate_keys().public_key_hex();
    let unrelated = generate_keys().public_key_hex();
    for (target, count) in [(&unrelated, 1_000), (&peer, 130)] {
        for index in 0..count {
            DirectMessageStore::put_direct_message_outbox(
                store.as_ref(),
                DirectMessageOutboxRow {
                    dm_id: format!("dm-{target}"),
                    message_id: format!("message-{index:04}"),
                    peer_pubkey: target.clone(),
                    frame_blob_hash: BlobHash::new("frame-hash"),
                    created_at: index,
                    last_attempt_at: None,
                },
            )
            .await
            .unwrap();
        }
    }
    let status = app.direct_message_status_view(&peer).await.unwrap();
    assert_eq!(status.pending_outbox_count, 64);
    assert!(status.pending_outbox_has_more);
}
