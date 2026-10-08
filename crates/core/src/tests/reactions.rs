use crate::*;

#[test]
fn reaction_envelope_roundtrip_for_emoji() {
    let keys = generate_keys();
    let topic = TopicId::new("kukuri:topic:demo");
    let target_object_id = EnvelopeId::from("post-1");
    let reaction_id = deterministic_reaction_id(
        &ReplicaId::new("replica-1"),
        &target_object_id,
        &keys.public_key(),
        "emoji:👍",
    );
    let envelope = build_reaction_envelope(
        &keys,
        &topic,
        None,
        &target_object_id,
        ReactionKeyV1::Emoji {
            emoji: " 👍 ".into(),
        },
        &reaction_id,
        ObjectStatus::Active,
    )
    .expect("reaction envelope");

    envelope.verify().expect("signature verification");
    let reaction = parse_reaction(&envelope)
        .expect("parse reaction")
        .expect("reaction");
    assert_eq!(reaction.reaction_id, reaction_id);
    assert_eq!(reaction.target_topic_id, topic);
    assert_eq!(reaction.target_object_id, target_object_id);
    assert_eq!(reaction.reaction_key_kind, ReactionKeyKind::Emoji);
    assert_eq!(reaction.emoji.as_deref(), Some("👍"));
    assert_eq!(reaction.normalized_reaction_key, "emoji:👍");
    assert_eq!(reaction.status, ObjectStatus::Active);
}

#[test]
fn custom_reaction_asset_roundtrip_and_reaction_id_stability() {
    let keys = generate_keys();
    let envelope = build_custom_reaction_asset_envelope(
        &keys,
        BlobHash::new("blob-asset-1"),
        "party".into(),
        "image/png".into(),
        128,
        128,
        128,
    )
    .expect("asset envelope");

    envelope.verify().expect("signature verification");
    let asset = parse_custom_reaction_asset(&envelope)
        .expect("parse asset")
        .expect("asset");
    assert_eq!(asset.asset_id, envelope.id.0);
    assert_eq!(asset.mime, "image/png");

    let reaction_key = ReactionKeyV1::CustomAsset {
        asset_id: asset.asset_id.clone(),
        snapshot: CustomReactionAssetSnapshotV1 {
            asset_id: asset.asset_id.clone(),
            owner_pubkey: asset.author_pubkey.clone(),
            blob_hash: asset.blob_hash.clone(),
            search_key: asset.search_key.clone(),
            mime: asset.mime.clone(),
            bytes: asset.bytes,
            width: asset.width,
            height: asset.height,
        },
    };
    let normalized = reaction_key.normalized_key().expect("normalized key");
    let first = deterministic_reaction_id(
        &ReplicaId::new("replica-a"),
        &EnvelopeId::from("post-1"),
        &keys.public_key(),
        normalized.as_str(),
    );
    let second = deterministic_reaction_id(
        &ReplicaId::new("replica-a"),
        &EnvelopeId::from("post-1"),
        &keys.public_key(),
        normalized.as_str(),
    );
    let different = deterministic_reaction_id(
        &ReplicaId::new("replica-a"),
        &EnvelopeId::from("post-1"),
        &keys.public_key(),
        "emoji:🔥",
    );
    assert_eq!(first, second);
    assert_ne!(first, different);
}

#[test]
fn custom_reaction_id_depends_only_on_the_image_and_the_search_key() {
    let party = custom_reaction_id("blob-1", "party");
    // 他の client も同じ値を求められるよう、形式を固定する（#1232 D1）。
    assert_eq!(
        party,
        "5fa064b060d8ea505dccd4ed0dc5596b63fcda6afccf1a7455bb25dc04a1e3ae"
    );
    assert_eq!(custom_reaction_id("blob-1", " party "), party);
    assert_ne!(custom_reaction_id("blob-1", "Party"), party);
    assert_ne!(custom_reaction_id("blob-2", "party"), party);

    // 別の作者が別の時刻に作った asset は envelope の ID が違っても、同じ ID になる。
    let ids = [" party ", "party"].map(|search_key| {
        let envelope = build_custom_reaction_asset_envelope(
            &generate_keys(),
            BlobHash::new("blob-1"),
            search_key.into(),
            "image/png".into(),
            128,
            128,
            128,
        )
        .expect("asset envelope");
        let asset = parse_custom_reaction_asset(&envelope)
            .expect("parse asset")
            .expect("asset");
        (
            asset.asset_id,
            custom_reaction_id(asset.blob_hash.as_str(), &asset.search_key),
        )
    });
    assert_ne!(ids[0].0, ids[1].0);
    assert_eq!(ids[0].1, party);
    assert_eq!(ids[1].1, party);
}

fn set_item(seed: u8, search_key: &str) -> CustomReactionSetItemV1 {
    CustomReactionSetItemV1 {
        owner_pubkey: generate_keys().public_key(),
        blob_hash: BlobHash::new(format!("{seed:02x}").repeat(32)),
        search_key: search_key.into(),
        mime: "image/png".into(),
        bytes: 128,
        width: 128,
        height: 128,
    }
}

#[test]
fn custom_reaction_set_roundtrips_and_rejects_invalid_contents() {
    let set = CustomReactionSetV1 {
        name: "ねこ".into(),
        items: vec![set_item(1, "cat"), set_item(2, "dog")],
    };
    let bytes = set.to_bytes().expect("set bytes");
    assert_eq!(CustomReactionSetV1::from_bytes(&bytes).expect("parse"), set);

    // 上限の 100 件は 64 KiB に収まり、101 件は作れない。
    let full = CustomReactionSetV1 {
        name: "full".into(),
        items: (0..100).map(|seed| set_item(seed, "same")).collect(),
    };
    assert!(full.to_bytes().is_ok());
    let over = CustomReactionSetV1 {
        items: (0..101).map(|seed| set_item(seed, "same")).collect(),
        ..full
    };
    assert!(over.to_bytes().is_err());

    for invalid in [
        CustomReactionSetV1 {
            name: " ".into(),
            ..set.clone()
        },
        CustomReactionSetV1 {
            name: "a".repeat(65),
            ..set.clone()
        },
        CustomReactionSetV1 {
            items: Vec::new(),
            ..set.clone()
        },
        // 前後の空白だけが違う検索名は同じリアクション。
        CustomReactionSetV1 {
            items: vec![set_item(1, "cat"), set_item(1, " cat ")],
            ..set.clone()
        },
        CustomReactionSetV1 {
            items: vec![CustomReactionSetItemV1 {
                blob_hash: BlobHash::new("A".repeat(64)),
                ..set_item(1, "cat")
            }],
            ..set.clone()
        },
        CustomReactionSetV1 {
            items: vec![CustomReactionSetItemV1 {
                mime: "text/plain".into(),
                ..set_item(1, "cat")
            }],
            ..set.clone()
        },
    ] {
        assert!(invalid.to_bytes().is_err(), "{invalid:?}");
    }
    assert!(CustomReactionSetV1::from_bytes(b"{}").is_err());
}

#[test]
fn custom_reaction_set_links_are_found_in_text() {
    let first = "a".repeat(64);
    let second = "b".repeat(64);
    let text = format!(
        "セット kukuri:reaction-set:{first} と\nkukuri:reaction-set:{second}。\
         kukuri:reaction-set:{first} kukuri:reaction-set:{}x kukuri:reaction-set:short",
        "c".repeat(64)
    );
    assert_eq!(custom_reaction_set_hashes_in_text(&text), [first, second]);
}
