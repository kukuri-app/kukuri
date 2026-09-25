use super::super::*;
use crate::service::direct_messages_delivery_support::DirectMessageHintServices;
use kukuri_core::{build_direct_message_ack, direct_message_id_for_participants};
use kukuri_store::{DirectMessageMessageRow, DirectMessageOutboxRow};

#[tokio::test]
async fn signed_ack_for_another_conversation_cannot_remove_protected_outbox() {
    let store = Arc::new(MemoryStore::default());
    let local_keys = generate_keys();
    let peer_keys = generate_keys();
    let other_keys = generate_keys();
    let local = local_keys.public_key_hex();
    let peer = peer_keys.public_key_hex();
    let other = other_keys.public_key_hex();
    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        Arc::new(StaticTransport::new(PeerSnapshot::default())),
        Arc::new(TrackingHintTransport::default()),
        Arc::new(MemoryDocsSync::default()),
        Arc::new(MemoryBlobService::default()),
        local_keys.clone(),
    );
    let dm_id =
        direct_message_id_for_participants(&local_keys.public_key(), &other_keys.public_key());
    let message = DirectMessageMessageRow {
        dm_id: dm_id.clone(),
        message_id: "protected-message".into(),
        sender_pubkey: local.clone(),
        recipient_pubkey: other.clone(),
        created_at: 10,
        text: Some("still waiting for the intended recipient".into()),
        reply_to_message_id: None,
        attachment_manifest: None,
        outgoing: true,
        acked_at: None,
    };
    let outbox = DirectMessageOutboxRow {
        dm_id: dm_id.clone(),
        message_id: message.message_id.clone(),
        peer_pubkey: other.clone(),
        frame_blob_hash: kukuri_core::BlobHash::new("unchanged-encrypted-frame"),
        created_at: 10,
        last_attempt_at: None,
    };
    store
        .put_direct_message_message(message.clone())
        .await
        .unwrap();
    store
        .put_direct_message_outbox(outbox.clone())
        .await
        .unwrap();

    // The ACK is honestly signed by the connected peer and addressed to us,
    // but names the conversation with a different recipient.
    let topic = derive_direct_message_topic(&local_keys, &peer_keys.public_key()).unwrap();
    let forged_scope_ack = build_direct_message_ack(
        &peer_keys,
        &dm_id,
        &message.message_id,
        &local_keys.public_key(),
        20,
    )
    .unwrap();
    forged_scope_ack.verify().unwrap();
    let changed = AppService::handle_direct_message_hint(
        DirectMessageHintServices {
            services: &app.services,
            local_author_pubkey: &local,
            peer_pubkey: &peer,
            ack_destination: None,
        },
        &GossipHint::DirectMessageAck {
            topic_id: topic.clone(),
            ack: forged_scope_ack,
        },
    )
    .await
    .unwrap();
    let retained = store
        .get_direct_message_outbox(&dm_id, &message.message_id)
        .await
        .unwrap();
    assert_eq!(
        retained,
        Some(outbox),
        "another peer's ACK must not delete queued ciphertext"
    );
    assert_eq!(
        store
            .get_direct_message_message(&dm_id, &message.message_id)
            .await
            .unwrap(),
        Some(message.clone()),
        "another conversation must not be marked delivered"
    );
    assert!(!changed);

    let valid_topic = derive_direct_message_topic(&local_keys, &other_keys.public_key()).unwrap();
    let valid_ack = build_direct_message_ack(
        &other_keys,
        &dm_id,
        &message.message_id,
        &local_keys.public_key(),
        30,
    )
    .unwrap();
    assert!(
        AppService::handle_direct_message_hint(
            DirectMessageHintServices {
                services: &app.services,
                local_author_pubkey: &local,
                peer_pubkey: &other,
                ack_destination: None,
            },
            &GossipHint::DirectMessageAck {
                topic_id: valid_topic.clone(),
                ack: valid_ack
            },
        )
        .await
        .unwrap()
    );
    assert!(
        store
            .get_direct_message_outbox(&dm_id, &message.message_id)
            .await
            .unwrap()
            .is_none()
    );
    let delivered = store
        .get_direct_message_message(&dm_id, &message.message_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(delivered.acked_at, Some(30));
    assert_eq!(delivered.text, message.text);
    let repeated_ack = build_direct_message_ack(
        &other_keys,
        &dm_id,
        &message.message_id,
        &local_keys.public_key(),
        40,
    )
    .unwrap();
    AppService::handle_direct_message_hint(
        DirectMessageHintServices {
            services: &app.services,
            local_author_pubkey: &local,
            peer_pubkey: &other,
            ack_destination: None,
        },
        &GossipHint::DirectMessageAck {
            topic_id: valid_topic.clone(),
            ack: repeated_ack,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        store
            .get_direct_message_message(&dm_id, &message.message_id)
            .await
            .unwrap()
            .unwrap()
            .acked_at,
        Some(30),
        "duplicate ACKs across pairwise and account routes keep the first delivery time"
    );
}
