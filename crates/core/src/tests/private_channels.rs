use crate::*;

#[test]
fn friend_only_grant_roundtrip_and_expiry_reject() {
    let keys = generate_keys();
    let token = build_friend_only_grant_token(
        &keys,
        &TopicId::new("kukuri:topic:friends"),
        &ChannelId::new("channel-1"),
        "friends",
        "epoch-1",
        &generate_keys().export_secret_hex(),
        None,
    )
    .expect("friend-only grant");

    let preview = parse_friend_only_grant_token(token.as_str()).expect("parse friend-only grant");
    assert_eq!(preview.owner_pubkey, keys.public_key());
    assert_eq!(preview.epoch_id, "epoch-1");

    let expired = build_friend_only_grant_token(
        &keys,
        &TopicId::new("kukuri:topic:friends"),
        &ChannelId::new("channel-1"),
        "friends",
        "epoch-1",
        &generate_keys().export_secret_hex(),
        Some(1),
    )
    .expect("expired grant");
    let error = parse_friend_only_grant_token(expired.as_str()).expect_err("expired grant");
    assert!(error.to_string().contains("expired"));
}

#[test]
fn friend_only_grant_parser_rejects_signer_mismatch() {
    let signer = generate_keys();
    let other = generate_keys();
    let token = FriendOnlyGrantTokenV1 {
        envelope: sign_envelope_json(
            &signer,
            "channel-friend-grant",
            vec![vec!["object".into(), "channel-friend-grant".into()]],
            &KukuriFriendOnlyGrantEnvelopeContentV1 {
                channel_id: ChannelId::new("channel-1"),
                topic_id: TopicId::new("kukuri:topic:friends"),
                channel_label: "friends".into(),
                owner_pubkey: other.public_key(),
                epoch_id: "epoch-1".into(),
                namespace_secret_hex: generate_keys().export_secret_hex(),
                expires_at: None,
            },
        )
        .expect("grant envelope"),
    };
    let encoded = serde_json::to_string(&token).expect("encode token");
    let error =
        parse_friend_only_grant_token(encoded.as_str()).expect_err("owner mismatch must fail");
    assert!(error.to_string().contains("owner pubkey must match"));
}

#[test]
fn channel_policy_and_participant_roundtrip() {
    let owner = generate_keys();
    let participant = generate_keys();
    let policy = PrivateChannelPolicyDocV1 {
        channel_id: ChannelId::new("channel-1"),
        topic_id: TopicId::new("kukuri:topic:friends"),
        audience_kind: ChannelAudienceKind::FriendOnly,
        owner_pubkey: owner.public_key(),
        epoch_id: "epoch-1".into(),
        sharing_state: ChannelSharingState::Open,
        rotated_at: None,
        previous_epoch_id: None,
        entry_dome_instance_id: None,
    };
    let policy_envelope =
        build_private_channel_policy_envelope(&owner, &policy).expect("policy envelope");
    let parsed_policy = parse_private_channel_policy(&policy_envelope)
        .expect("parse policy")
        .expect("policy");
    assert_eq!(parsed_policy.audience_kind, ChannelAudienceKind::FriendOnly);

    let participant_doc = PrivateChannelParticipantDocV1 {
        channel_id: ChannelId::new("channel-1"),
        topic_id: TopicId::new("kukuri:topic:friends"),
        epoch_id: "epoch-1".into(),
        participant_pubkey: participant.public_key(),
        joined_at: 10,
        is_owner: false,
        join_mode: Some(PrivateChannelJoinMode::FriendOnlyGrant),
        sponsor_pubkey: Some(owner.public_key()),
        share_token_id: None,
        left_at: None,
    };
    let participant_envelope =
        build_private_channel_participant_envelope(&participant, &participant_doc)
            .expect("participant envelope");
    let parsed_participant = parse_private_channel_participant(&participant_envelope)
        .expect("parse participant")
        .expect("participant");
    assert_eq!(
        parsed_participant.participant_pubkey,
        participant.public_key()
    );
}

#[test]
fn friend_plus_share_roundtrip_and_expiry_reject() {
    let owner = generate_keys();
    let sponsor = generate_keys();
    let token = build_friend_plus_share_token(
        &sponsor,
        &TopicId::new("kukuri:topic:friends-plus"),
        &ChannelId::new("channel-1"),
        "friends+",
        &owner.public_key(),
        "epoch-1",
        &generate_keys().export_secret_hex(),
        None,
    )
    .expect("friend-plus share");

    let preview = parse_friend_plus_share_token(token.as_str()).expect("parse friend-plus share");
    assert_eq!(preview.owner_pubkey, owner.public_key());
    assert_eq!(preview.sponsor_pubkey, sponsor.public_key());
    assert_eq!(preview.epoch_id, "epoch-1");
    assert_eq!(preview.share_token_id.len(), 64);

    let expired = build_friend_plus_share_token(
        &sponsor,
        &TopicId::new("kukuri:topic:friends-plus"),
        &ChannelId::new("channel-1"),
        "friends+",
        &owner.public_key(),
        "epoch-1",
        &generate_keys().export_secret_hex(),
        Some(1),
    )
    .expect("expired friend-plus share");
    let error = parse_friend_plus_share_token(expired.as_str()).expect_err("expired share");
    assert!(error.to_string().contains("expired"));
}

#[test]
fn friend_plus_share_parser_rejects_signer_mismatch() {
    let owner = generate_keys();
    let signer = generate_keys();
    let sponsor = generate_keys();
    let token = FriendPlusShareTokenV1 {
        envelope: sign_envelope_json(
            &signer,
            "channel-share",
            vec![vec!["object".into(), "channel-share".into()]],
            &KukuriFriendPlusShareEnvelopeContentV1 {
                channel_id: ChannelId::new("channel-1"),
                topic_id: TopicId::new("kukuri:topic:friends-plus"),
                channel_label: "friends+".into(),
                owner_pubkey: owner.public_key(),
                sponsor_pubkey: sponsor.public_key(),
                epoch_id: "epoch-1".into(),
                namespace_secret_hex: generate_keys().export_secret_hex(),
                expires_at: None,
            },
        )
        .expect("share envelope"),
    };
    let encoded = serde_json::to_string(&token).expect("encode share");
    let error =
        parse_friend_plus_share_token(encoded.as_str()).expect_err("sponsor mismatch must fail");
    assert!(error.to_string().contains("sponsor pubkey must match"));
}

#[test]
fn channel_epoch_handoff_grant_encrypt_decrypt_roundtrip_and_wrong_recipient_fails() {
    let owner = generate_keys();
    let recipient = generate_keys();
    let wrong_recipient = generate_keys();
    let payload = PrivateChannelEpochHandoffGrantPayloadV1 {
        channel_id: ChannelId::new("channel-1"),
        topic_id: TopicId::new("kukuri:topic:friends-plus"),
        owner_pubkey: owner.public_key(),
        recipient_pubkey: recipient.public_key(),
        old_epoch_id: "epoch-1".into(),
        new_epoch_id: "epoch-2".into(),
        new_namespace_secret_hex: generate_keys().export_secret_hex(),
    };
    let doc = encrypt_private_channel_epoch_handoff_grant(&owner, &payload)
        .expect("encrypt epoch handoff grant");
    let envelope =
        build_private_channel_epoch_handoff_grant_envelope(&owner, &doc).expect("envelope");
    let parsed_doc = parse_private_channel_epoch_handoff_grant(&envelope)
        .expect("parse epoch handoff grant")
        .expect("epoch handoff grant");
    let decrypted = decrypt_private_channel_epoch_handoff_grant(&recipient, &parsed_doc)
        .expect("decrypt epoch handoff grant");
    assert_eq!(decrypted.new_epoch_id, "epoch-2");
    assert_eq!(decrypted.recipient_pubkey, recipient.public_key());

    let error = decrypt_private_channel_epoch_handoff_grant(&wrong_recipient, &parsed_doc)
        .expect_err("wrong recipient must fail");
    assert!(error.to_string().contains("recipient pubkey"));
}

#[test]
fn epoch_handoff_grant_reads_legacy_wire_fixture_and_preserves_wire_shape() {
    let owner =
        KukuriKeys::parse("0000000000000000000000000000000000000000000000000000000000000001")
            .expect("owner key");
    let recipient =
        KukuriKeys::parse("0000000000000000000000000000000000000000000000000000000000000002")
            .expect("recipient key");
    let expected_payload = PrivateChannelEpochHandoffGrantPayloadV1 {
        channel_id: ChannelId::new("channel-fixture"),
        topic_id: TopicId::new("kukuri:topic:fixture"),
        owner_pubkey: owner.public_key(),
        recipient_pubkey: recipient.public_key(),
        old_epoch_id: "epoch-7".into(),
        new_epoch_id: "epoch-8".into(),
        new_namespace_secret_hex:
            "0000000000000000000000000000000000000000000000000000000000000003".into(),
    };
    let legacy_doc = PrivateChannelEpochHandoffGrantDocV1 {
        channel_id: expected_payload.channel_id.clone(),
        topic_id: expected_payload.topic_id.clone(),
        owner_pubkey: expected_payload.owner_pubkey.clone(),
        recipient_pubkey: expected_payload.recipient_pubkey.clone(),
        old_epoch_id: expected_payload.old_epoch_id.clone(),
        new_epoch_id: expected_payload.new_epoch_id.clone(),
        nonce_hex: "030106977157b65e271235713e1c2557e15b37fa50368f70".into(),
        ciphertext_hex: concat!(
            "3c868683bf8283ec5050cb619321f6327ec2c2171e66fb705c649b014debdc7b",
            "a95d249f29ac6649c42229f6aaf42ac417152de3f9b5988670d34859100ea2c60",
            "36cbbeeb6508f22f574c625954150888ba0388ab7324d743db21efe12616cf71a1",
            "287d91e08c8adf4cae64ff9539a94dc1c6d3b976f7c7218dbded6bd1359d5910",
            "63b585665acfa027ca789b0eedf1dbeff4756211269e3ff943c3ce047a876dd57",
            "ff9abb4251cf84db0bf632fd2ae138ac18e202f4857b492af5c6accfd7b013cbc",
            "1331bbf28fc79ae3b7c191d7a0c44d5f15bfd9c8def83778426427e25d08e85d",
            "8431707db9c4189e98a7a281ecfa8ea4d29718e149ce72cead7263f7412a77ae1",
            "683b6e75c9d6d8b0504b0b0340209b80c65eaf7062f4b06b76e741f6e86db27",
            "ba5aa277bbf8778c624206c31eb471be084a0655adadda2214bdca72ca47e9021",
            "eeac0e239476c344cc9920c6fbe91f7eb834061ebe83c7338e595844e1b339a0",
            "b7a875d7ce92c1116d66a67421f06b48ca8e8ccceff4cac00008d2a1e23adbd",
            "d01687de8f49e56"
        )
        .into(),
    };

    let decrypted =
        decrypt_private_channel_epoch_handoff_grant(&recipient, &legacy_doc).expect("legacy grant");
    assert_eq!(decrypted, expected_payload);

    let doc_json = serde_json::to_value(&legacy_doc).expect("doc json");
    let doc_fields = doc_json
        .as_object()
        .expect("doc object")
        .keys()
        .map(String::as_str)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        doc_fields,
        [
            "channel_id",
            "ciphertext_hex",
            "new_epoch_id",
            "nonce_hex",
            "old_epoch_id",
            "owner_pubkey",
            "recipient_pubkey",
            "topic_id",
        ]
        .into_iter()
        .collect()
    );

    let payload_json = serde_json::to_value(&decrypted).expect("payload json");
    let payload_fields = payload_json
        .as_object()
        .expect("payload object")
        .keys()
        .map(String::as_str)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        payload_fields,
        [
            "channel_id",
            "new_epoch_id",
            "new_namespace_secret_hex",
            "old_epoch_id",
            "owner_pubkey",
            "recipient_pubkey",
            "topic_id",
        ]
        .into_iter()
        .collect()
    );

    let envelope = build_private_channel_epoch_handoff_grant_envelope(&owner, &legacy_doc)
        .expect("legacy envelope");
    assert_eq!(envelope.kind, "channel-rotation-grant");
    let expected_tags: Vec<Vec<String>> = vec![
        vec!["topic".into(), "kukuri:topic:fixture".into()],
        vec!["channel".into(), "channel-fixture".into()],
        vec!["epoch".into(), "epoch-7".into()],
        vec![
            "recipient".into(),
            "c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5".into(),
        ],
        vec!["object".into(), "channel-rotation-grant".into()],
    ];
    assert_eq!(envelope.tags, expected_tags);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&envelope.content).expect("content json"),
        doc_json
    );
}

#[test]
fn key_row_seal_opens_only_for_the_same_account_and_row() {
    let keys = generate_keys();
    let seal = keys.derive_private_channel_key_row_seal();
    let secret = "11".repeat(32);
    let sealed = seal.seal("channel-a", "epoch-1", &secret).expect("seal");
    assert_eq!(
        seal.open("channel-a", "epoch-1", &sealed).expect("open"),
        secret
    );
    // 同じ鍵からは同じ封の鍵になる(再起動・復元の後も開ける)。
    assert_eq!(
        keys.derive_private_channel_key_row_seal()
            .open("channel-a", "epoch-1", &sealed)
            .expect("reopen"),
        secret
    );
    // 別の行・別の account・改ざんは開けない。
    assert!(seal.open("channel-a", "epoch-2", &sealed).is_err());
    assert!(seal.open("channel-b", "epoch-1", &sealed).is_err());
    assert!(
        generate_keys()
            .derive_private_channel_key_row_seal()
            .open("channel-a", "epoch-1", &sealed)
            .is_err()
    );
    let mut tampered = sealed.clone();
    *tampered.last_mut().expect("ciphertext") ^= 1;
    assert!(seal.open("channel-a", "epoch-1", &tampered).is_err());
    assert!(seal.seal("channel-a", "epoch-1", "11").is_err());
}
