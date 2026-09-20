use crate::*;

#[test]
fn repost_envelope_roundtrip() {
    let keys = generate_keys();
    let envelope = build_repost_envelope(
        &keys,
        &TopicId::new("kukuri:topic:target"),
        RepostSourceSnapshotV1 {
            source_object_id: EnvelopeId::from("source-1"),
            source_topic_id: TopicId::new("kukuri:topic:source"),
            source_author_pubkey: generate_keys().public_key(),
            source_object_kind: "comment".into(),
            content: "quoted source".into(),
            attachments: vec![AssetRef {
                hash: BlobHash::new("hash-1"),
                mime: "image/png".into(),
                bytes: 24,
                role: AssetRole::ImageOriginal,
            }],
            reply_to_object_id: Some(EnvelopeId::from("root-1")),
            root_id: Some(EnvelopeId::from("root-1")),
            content_labels: Vec::new(),
        },
        Some("quote commentary"),
    )
    .expect("repost envelope");

    envelope.verify().expect("signature verification");
    let repost = envelope
        .to_post_object()
        .expect("parse repost")
        .expect("repost object");
    assert_eq!(repost.object_kind, "repost");
    assert_eq!(repost.topic_id.as_str(), "kukuri:topic:target");
    assert_eq!(
        repost
            .repost_of
            .as_ref()
            .map(|value| value.source_topic_id.as_str()),
        Some("kukuri:topic:source")
    );
    assert_eq!(
        match repost.payload_ref {
            PayloadRef::InlineText { text } => text,
            PayloadRef::BlobText { .. } => String::new(),
        },
        "quote commentary"
    );
}

#[test]
fn post_withdrawal_roundtrip_is_bound_to_the_original_author() {
    let author = generate_keys();
    let topic = TopicId::new("kukuri:topic:withdrawal");
    let original =
        build_post_envelope(&author, &topic, "withdraw me", None).expect("original envelope");
    let withdrawal = build_post_withdrawal_envelope(
        &author,
        &original,
        1,
        None,
        WithdrawalReasonVisibility::Public,
        Some(PostWithdrawalReason::AuthorRequest),
    )
    .expect("withdrawal envelope");

    withdrawal.verify().expect("withdrawal signature");
    let parsed = verify_post_withdrawal(&withdrawal, &original).expect("verified withdrawal");
    assert_eq!(parsed.target_object_id, original.id);
    assert_eq!(parsed.target_author, original.pubkey);
    assert_eq!(parsed.topic_id, topic);
    assert_eq!(parsed.withdrawn_at, withdrawal.created_at);
    assert_eq!(parsed.generation, 1);
    assert_eq!(parsed.reason, Some(PostWithdrawalReason::AuthorRequest));
}

#[test]
fn post_withdrawal_rejects_a_non_author_and_private_reason_disclosure() {
    let author = generate_keys();
    let attacker = generate_keys();
    let original = build_post_envelope(
        &author,
        &TopicId::new("kukuri:topic:withdrawal"),
        "withdraw me",
        None,
    )
    .expect("original envelope");

    let error = build_post_withdrawal_envelope(
        &attacker,
        &original,
        1,
        None,
        WithdrawalReasonVisibility::Public,
        Some(PostWithdrawalReason::AuthorRequest),
    )
    .expect_err("a non-author must not issue a withdrawal");
    assert!(error.to_string().contains("original author"));

    let error = build_post_withdrawal_envelope(
        &author,
        &original,
        1,
        None,
        WithdrawalReasonVisibility::Private,
        Some(PostWithdrawalReason::Other),
    )
    .expect_err("a private reason must not be replicated");
    assert!(error.to_string().contains("private withdrawal reason"));
}

// ADR 0053 §2: 投稿の envelope は、著者の docs author の id を署名の対象の tag で申告する。
#[test]
fn post_envelope_declares_the_docs_author_in_a_signed_tag() {
    let keys = generate_keys();
    let topic = TopicId::new("kukuri:topic:docs-author-tag");
    let docs_author = "ab".repeat(32);
    let build = |docs_author: Option<&str>| {
        build_post_envelope_with_docs_author(
            &keys,
            &topic,
            PayloadRef::InlineText {
                text: "hello".into(),
            },
            Vec::new(),
            Vec::new(),
            None,
            ObjectVisibility::Public,
            None,
            Vec::new(),
            docs_author,
        )
    };
    let tagged = build(Some(docs_author.as_str())).expect("tagged envelope");
    tagged.verify().expect("signature verification");
    assert_eq!(tagged.docs_author(), Some(docs_author.as_str()));
    // tag を知らない読み手と同じ読み方(投稿としての解釈)は変わらない。
    let object = tagged
        .to_post_object()
        .expect("post object")
        .expect("post object");
    assert_eq!(object.object_id, tagged.id);

    // tag は署名の対象。書き換えると検証に通らない。
    let mut tampered = tagged.clone();
    for tag in &mut tampered.tags {
        if tag.first().map(String::as_str) == Some(DOCS_AUTHOR_TAG) {
            tag[1] = "cd".repeat(32);
        }
    }
    assert!(tampered.verify().is_err());

    // tag の無い envelope(旧 record)は `None`。docs author の id の形でない値は付けられず、読むときも無視する。
    let legacy = build(None).expect("legacy envelope");
    assert_eq!(legacy.docs_author(), None);
    assert!(build(Some("not a docs author id")).is_err());
    assert!(build(Some("AB".repeat(32).as_str())).is_err());
    let malformed = sign_envelope_json(
        &keys,
        "post",
        vec![vec![DOCS_AUTHOR_TAG.into(), "zz".into()]],
        &serde_json::json!({}),
    )
    .expect("envelope");
    assert_eq!(malformed.docs_author(), None);
}

#[test]
fn repost_envelope_declares_the_docs_author() {
    let keys = generate_keys();
    let docs_author = "0f".repeat(32);
    let envelope = build_repost_envelope_with_docs_author(
        &keys,
        &TopicId::new("kukuri:topic:target"),
        RepostSourceSnapshotV1 {
            source_object_id: EnvelopeId::from("source-1"),
            source_topic_id: TopicId::new("kukuri:topic:source"),
            source_author_pubkey: generate_keys().public_key(),
            source_object_kind: "post".into(),
            content: "quoted source".into(),
            attachments: Vec::new(),
            reply_to_object_id: None,
            root_id: None,
            content_labels: Vec::new(),
        },
        None,
        Some(docs_author.as_str()),
    )
    .expect("repost envelope");
    envelope.verify().expect("signature verification");
    assert_eq!(envelope.docs_author(), Some(docs_author.as_str()));
}
