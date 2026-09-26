use crate::*;

#[test]
fn dm_pairwise_secret_derives_same_topic_for_pair() {
    let alice = generate_keys();
    let bob = generate_keys();
    let mallory = generate_keys();

    let topic_ab = derive_direct_message_topic(&alice, &bob.public_key()).expect("topic a->b");
    let topic_ba = derive_direct_message_topic(&bob, &alice.public_key()).expect("topic b->a");
    let topic_am = derive_direct_message_topic(&alice, &mallory.public_key()).expect("topic a->m");

    assert_eq!(topic_ab, topic_ba);
    assert_ne!(topic_ab, topic_am);
    assert_eq!(
        direct_message_id_for_participants(&alice.public_key(), &bob.public_key()),
        direct_message_id_for_participants(&bob.public_key(), &alice.public_key())
    );
}

#[test]
fn dm_frame_encrypt_decrypt_roundtrip_and_tamper_reject() {
    let alice = generate_keys();
    let bob = generate_keys();
    let dm_id = direct_message_id_for_participants(&alice.public_key(), &bob.public_key());
    let payload = DirectMessagePayloadV1 {
        text: Some("hello bob".into()),
        reply_to: Some("message-0".into()),
        attachment_manifest: None,
    };

    let frame = encrypt_direct_message_frame(
        &alice,
        &bob.public_key(),
        dm_id.as_str(),
        "message-1",
        42,
        &payload,
    )
    .expect("encrypt frame");
    let decrypted = decrypt_direct_message_frame(&bob, &frame).expect("decrypt frame");
    assert_eq!(decrypted, payload);
    // #1221 R5-G: 送信者は自分の frame を開けるが、受信者は送信者として開けない。
    assert_eq!(
        open_sent_direct_message_frame(&alice, &frame).expect("sender opens frame"),
        payload
    );
    assert!(open_sent_direct_message_frame(&bob, &frame).is_err());

    let mut tampered = frame.clone();
    tampered.ciphertext_hex = "00".repeat(32);
    let error =
        decrypt_direct_message_frame(&bob, &tampered).expect_err("tampered frame must fail");
    assert!(
        error.to_string().contains("signature")
            || error.to_string().contains("decrypt")
            || error.to_string().contains("ciphertext")
    );
}

#[test]
fn dm_attachment_encrypt_decrypt_roundtrip_and_wrong_recipient_fails() {
    let alice = generate_keys();
    let bob = generate_keys();
    let mallory = generate_keys();
    let encrypted = encrypt_direct_message_attachment(
        &alice,
        &bob.public_key(),
        "message-1",
        "attachment-1",
        b"attachment-bytes",
    )
    .expect("encrypt attachment");

    let decrypted =
        decrypt_direct_message_attachment(&bob, &alice.public_key(), "message-1", &encrypted)
            .expect("decrypt attachment");
    assert_eq!(decrypted, b"attachment-bytes");

    let error =
        decrypt_direct_message_attachment(&mallory, &alice.public_key(), "message-1", &encrypted)
            .expect_err("wrong recipient must fail");
    assert!(error.to_string().contains("decrypt"));
}

#[test]
fn dm_ack_signature_verification_rejects_wrong_signer() {
    let alice = generate_keys();
    let bob = generate_keys();
    let ack = build_direct_message_ack(&bob, "dm-1", "message-1", &alice.public_key(), 99)
        .expect("build ack");
    ack.verify().expect("verify ack");

    let mut tampered = ack.clone();
    tampered.sender = alice.public_key();
    let error = tampered.verify().expect_err("tampered ack must fail");
    assert!(error.to_string().contains("signature"));
}
