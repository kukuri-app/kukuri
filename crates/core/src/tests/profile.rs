use crate::*;

#[test]
fn profile_envelope_roundtrip() {
    let keys = generate_keys();
    let content = KukuriProfileEnvelopeContentV1 {
        author_pubkey: keys.public_key(),
        name: Some("alice".into()),
        display_name: Some("Alice".into()),
        about: Some("hello".into()),
        picture_asset: Some(AssetRef {
            hash: BlobHash::new("avatar-hash"),
            mime: "image/png".into(),
            bytes: 42,
            role: AssetRole::ProfileAvatar,
        }),
        nip05: Some("alice@example.com".into()),
    };
    for docs_author in [None, Some("a".repeat(64))] {
        let envelope =
            build_profile_envelope_with_docs_author(&keys, &content, docs_author.as_deref())
                .expect("profile envelope");

        envelope.verify().expect("signature verification");
        let profile = parse_profile(&envelope)
            .expect("parse profile")
            .expect("profile");
        assert_eq!(
            profile
                .envelope_id_hint(docs_author.as_deref())
                .expect("ID hint"),
            envelope.id
        );
        assert_eq!(profile.pubkey, keys.public_key());
        assert_eq!(profile.display_name.as_deref(), Some("Alice"));
        assert_eq!(profile.about.as_deref(), Some("hello"));
        assert_eq!(profile.nip05.as_deref(), Some("alice@example.com"));
        assert_eq!(
            profile
                .picture_asset
                .as_ref()
                .map(|asset| asset.role.clone()),
            Some(AssetRole::ProfileAvatar)
        );
    }
}

// #1670 INVAR-1: nip05 の無い profile の content は key を持たず、欄を足す前の client が作った envelope の ID を
// 表示用の行から同じく求められる。
#[test]
fn profile_without_nip05_keeps_the_previous_content_and_id() {
    let keys = generate_keys();
    // 欄を足す前の client が書いた content(struct の宣言の順)。
    let previous_content = format!(
        r#"{{"author_pubkey":"{}","name":"alice","display_name":"Alice","about":null,"picture_asset":null}}"#,
        keys.public_key_hex()
    );
    let envelope = crate::sign_envelope_at(
        &keys,
        "identity-profile",
        vec![
            vec!["author".into(), keys.public_key_hex()],
            vec!["object".into(), "identity-profile".into()],
        ],
        previous_content,
        42,
    )
    .expect("previous profile envelope");

    let profile = parse_profile(&envelope)
        .expect("parse profile")
        .expect("profile");
    assert_eq!(profile.nip05, None);
    assert_eq!(
        profile.envelope_id_hint(None).expect("ID hint"),
        envelope.id
    );
    let rebuilt = build_profile_envelope(
        &keys,
        &KukuriProfileEnvelopeContentV1 {
            author_pubkey: keys.public_key(),
            name: Some("alice".into()),
            ..Default::default()
        },
    )
    .expect("profile envelope");
    assert!(
        !rebuilt.content.contains("nip05"),
        "content: {}",
        rebuilt.content
    );
}

// #1670 AC-1.1・1.2: 識別子は前後の空白を除いて小文字にし、名前とドメインの形に合わないものを断る。
#[test]
fn profile_nip05_is_normalized_and_rejects_invalid_identifiers() {
    assert_eq!(
        normalize_profile_nip05("  Alice_1.x-y@Sub.Example.COM ").expect("valid"),
        "alice_1.x-y@sub.example.com"
    );
    assert_eq!(
        normalize_profile_nip05("_@xn--wgv71a119e.jp").expect("punycode domain"),
        "_@xn--wgv71a119e.jp"
    );
    let long_name = format!("{}@example.com", "a".repeat(65));
    let long_domain = format!("a@{}.com", vec!["a".repeat(63); 4].join("."));
    for invalid in [
        "",
        "alice",
        "@example.com",
        "alice@",
        "alice@localhost",
        "a b@example.com",
        "alice+tag@example.com",
        "alice@192.0.2.1",
        "alice@example.com.",
        "alice@-example.com",
        "alice@exa_mple.com",
        "alice@例え.jp",
        "alice@bob@example.com",
        long_name.as_str(),
        long_domain.as_str(),
    ] {
        assert!(normalize_profile_nip05(invalid).is_err(), "{invalid}");
    }
}

#[test]
fn profile_parser_ignores_removed_picture_url_field() {
    let keys = generate_keys();
    let author_pubkey = keys.public_key();
    let content = serde_json::json!({
        "author_pubkey": author_pubkey,
        "name": "legacy-alice",
        "display_name": "Legacy Alice",
        "about": "old preview profile",
        "picture": "https://tracker.example/avatar.png",
        "picture_asset": null
    });
    let envelope = crate::sign_envelope_at(
        &keys,
        "identity-profile",
        vec![
            vec!["author".into(), keys.public_key_hex()],
            vec!["object".into(), "identity-profile".into()],
        ],
        content.to_string(),
        42,
    )
    .expect("legacy profile envelope");

    let profile = parse_profile(&envelope)
        .expect("parse legacy profile")
        .expect("profile");
    let serialized = serde_json::to_value(&profile).expect("serialize current profile");
    assert_eq!(profile.display_name.as_deref(), Some("Legacy Alice"));
    assert_eq!(profile.picture_asset, None);
    assert!(serialized.get("picture").is_none());
}

#[test]
fn profile_post_envelope_roundtrip() {
    let keys = generate_keys();
    let author_pubkey = keys.public_key();
    let envelope = build_profile_post_envelope(
        &keys,
        &KukuriProfilePostEnvelopeContentV1 {
            author_pubkey: author_pubkey.clone(),
            profile_topic_id: author_profile_topic_id(author_pubkey.as_str()),
            published_topic_id: TopicId::new("kukuri:topic:demo"),
            object_id: EnvelopeId::from("post-1"),
            created_at: 42,
            object_kind: "comment".into(),
            content: "hello profile topic".into(),
            attachments: vec![AssetRef {
                hash: BlobHash::new("hash-1"),
                mime: "image/png".into(),
                bytes: 12,
                role: AssetRole::ImageOriginal,
            }],
            reply_to_object_id: Some(EnvelopeId::from("root-1")),
            root_id: Some(EnvelopeId::from("root-1")),
            content_labels: Vec::new(),
        },
    )
    .expect("profile post envelope");

    envelope.verify().expect("signature verification");
    let profile_post = parse_profile_post(&envelope)
        .expect("parse profile post")
        .expect("profile post");
    assert_eq!(profile_post.author_pubkey, author_pubkey);
    assert_eq!(
        profile_post.profile_topic_id,
        author_profile_topic_id(author_pubkey.as_str())
    );
    assert_eq!(
        profile_post.published_topic_id.as_str(),
        "kukuri:topic:demo"
    );
    assert_eq!(profile_post.object_id.as_str(), "post-1");
    assert_eq!(profile_post.created_at, 42);
    assert_eq!(profile_post.object_kind, "comment");
    assert_eq!(profile_post.content, "hello profile topic");
    assert_eq!(profile_post.attachments.len(), 1);
    assert_eq!(
        profile_post
            .reply_to_object_id
            .as_ref()
            .map(EnvelopeId::as_str),
        Some("root-1")
    );
    assert_eq!(
        profile_post.root_id.as_ref().map(EnvelopeId::as_str),
        Some("root-1")
    );
    assert_eq!(profile_post.envelope_id, envelope.id);
}

#[test]
fn profile_repost_envelope_roundtrip() {
    let keys = generate_keys();
    let author_pubkey = keys.public_key();
    let envelope = build_profile_repost_envelope(
        &keys,
        &KukuriProfileRepostEnvelopeContentV1 {
            author_pubkey: author_pubkey.clone(),
            profile_topic_id: author_profile_topic_id(author_pubkey.as_str()),
            published_topic_id: TopicId::new("kukuri:topic:target"),
            object_id: EnvelopeId::from("repost-1"),
            created_at: 55,
            commentary: Some("quote commentary".into()),
            repost_of: RepostSourceSnapshotV1 {
                source_object_id: EnvelopeId::from("source-1"),
                source_topic_id: TopicId::new("kukuri:topic:source"),
                source_author_pubkey: generate_keys().public_key(),
                source_object_kind: "post".into(),
                content: "source content".into(),
                attachments: Vec::new(),
                reply_to_object_id: None,
                root_id: Some(EnvelopeId::from("source-1")),
                content_labels: Vec::new(),
            },
        },
    )
    .expect("profile repost envelope");

    envelope.verify().expect("signature verification");
    let profile_repost = parse_profile_repost(&envelope)
        .expect("parse profile repost")
        .expect("profile repost");
    assert_eq!(profile_repost.author_pubkey, author_pubkey);
    assert_eq!(
        profile_repost.published_topic_id.as_str(),
        "kukuri:topic:target"
    );
    assert_eq!(profile_repost.object_id.as_str(), "repost-1");
    assert_eq!(
        profile_repost.commentary.as_deref(),
        Some("quote commentary")
    );
    assert_eq!(
        profile_repost.repost_of.source_topic_id.as_str(),
        "kukuri:topic:source"
    );
}

#[test]
fn follow_edge_roundtrip_and_self_follow_rejected() {
    let keys = generate_keys();
    let target = generate_keys().public_key();
    let envelope =
        build_follow_edge_envelope(&keys, &target, FollowEdgeStatus::Active).expect("envelope");

    envelope.verify().expect("signature verification");
    let edge = parse_follow_edge(&envelope)
        .expect("parse follow edge")
        .expect("follow edge");
    assert_eq!(edge.subject_pubkey, keys.public_key());
    assert_eq!(edge.target_pubkey, target);
    assert_eq!(edge.status, FollowEdgeStatus::Active);

    let self_follow_error =
        build_follow_edge_envelope(&keys, &keys.public_key(), FollowEdgeStatus::Active)
            .expect_err("self follow should be rejected");
    assert!(self_follow_error.to_string().contains("self follow"));
}

#[test]
fn follow_edge_parser_rejects_subject_mismatch() {
    let signer = generate_keys();
    let subject = generate_keys().public_key();
    let target = generate_keys().public_key();
    let envelope = sign_envelope_json(
        &signer,
        "follow-edge",
        vec![vec!["object".into(), "follow-edge".into()]],
        &KukuriFollowEdgeEnvelopeContentV1 {
            subject_pubkey: subject,
            target_pubkey: target,
            status: FollowEdgeStatus::Active,
        },
    )
    .expect("envelope");

    let error = parse_follow_edge(&envelope).expect_err("subject mismatch must fail");
    assert!(error.to_string().contains("subject pubkey must match"));
}

#[test]
fn block_edge_roundtrip_and_self_block_rejected() {
    let keys = generate_keys();
    let target = generate_keys().public_key();
    let envelope =
        build_block_edge_envelope(&keys, &target, BlockEdgeStatus::Active).expect("envelope");

    envelope.verify().expect("signature verification");
    let edge = parse_block_edge(&envelope)
        .expect("parse block edge")
        .expect("block edge");
    assert_eq!(edge.subject_pubkey, keys.public_key());
    assert_eq!(edge.target_pubkey, target);
    assert_eq!(edge.status, BlockEdgeStatus::Active);

    let self_block_error =
        build_block_edge_envelope(&keys, &keys.public_key(), BlockEdgeStatus::Active)
            .expect_err("self block should be rejected");
    assert!(self_block_error.to_string().contains("self block"));
}

#[test]
fn block_edge_parser_rejects_subject_mismatch() {
    let signer = generate_keys();
    let subject = generate_keys().public_key();
    let target = generate_keys().public_key();
    let envelope = sign_envelope_json(
        &signer,
        "block-edge",
        vec![vec!["object".into(), "block-edge".into()]],
        &KukuriBlockEdgeEnvelopeContentV1 {
            subject_pubkey: subject,
            target_pubkey: target,
            status: BlockEdgeStatus::Active,
        },
    )
    .expect("envelope");

    let error = parse_block_edge(&envelope).expect_err("subject mismatch must fail");
    assert!(error.to_string().contains("subject pubkey must match"));
}
