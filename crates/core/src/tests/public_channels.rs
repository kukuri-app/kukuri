//! 公開 channel(#1840)の ID の形と、作成者が署名した metadata の確認。
use crate::*;

const CREATED_AT_MS: i64 = 1_760_000_000_123;

fn content(keys: &KukuriKeys) -> KukuriPublicChannelContentV1 {
    KukuriPublicChannelContentV1 {
        channel_id: ChannelId::public_channel(CREATED_AT_MS, keys.public_key_hex().as_str()),
        topic_id: TopicId::new("kukuri:topic:threads"),
        title: "雑談".into(),
        first_post_id: Some(EnvelopeId::from("first-post")),
        revision: 0,
        deleted: false,
    }
}

// ID は作成時刻と作成者の公開鍵の先頭 16 桁から決まり、private channel の ID(`channel-…`)とは重ならない。
#[test]
fn public_channel_ids_are_bound_to_the_creator_and_their_creation_time() {
    let creator = generate_keys().public_key_hex();
    let id = ChannelId::public_channel(CREATED_AT_MS, &creator);
    assert_eq!(
        id.as_str(),
        format!("pc-{CREATED_AT_MS}-{}", &creator[..16])
    );
    assert!(id.is_public_channel());
    assert_eq!(id.public_channel_created_at_ms(), Some(CREATED_AT_MS));
    assert!(id.is_public_channel_of(&creator));
    assert!(!id.is_public_channel_of(&generate_keys().public_key_hex()));
    for other in [
        "public",
        "channel-1760000000123-abcdef01",
        "pc-",
        "pc-0123-0123456789abcdef",
        "pc-0-0123456789abcdef",
        "pc-12-0123456789ABCDEF",
        "pc-12-0123456789abcde",
        "pc-12-0123456789abcdef0",
        "pc-x-0123456789abcdef",
        "pc-+12-0123456789abcdef",
    ] {
        assert!(!ChannelId::new(other).is_public_channel(), "{other}");
    }
    assert_eq!(
        ChannelRef::for_channel(Some(&id)),
        ChannelRef::PublicChannel {
            channel_id: id.clone()
        }
    );
    assert_eq!(
        ChannelRef::for_channel(Some(&id)).visibility(),
        ObjectVisibility::Public
    );
    let private = ChannelId::new("channel-1760000000123-abcdef01");
    assert_eq!(
        ChannelRef::for_channel(Some(&private)),
        ChannelRef::PrivateChannel {
            channel_id: private.clone()
        }
    );
    assert_eq!(ChannelRef::for_channel(None), ChannelRef::Public);
}

// 作成者の署名つき metadata は ID の時刻で署名され、読む側は署名・形・作成者と時刻の結び付きを確かめる。
#[test]
fn public_channel_metadata_roundtrips_and_rejects_unbound_or_invalid_records() {
    let keys = generate_keys();
    let envelope = build_public_channel_envelope(&keys, &content(&keys)).expect("metadata");
    assert_eq!(envelope.kind, PUBLIC_CHANNEL_KIND);
    assert_eq!(envelope.created_at, CREATED_AT_MS / 1_000);
    assert_eq!(
        verify_public_channel(&envelope).expect("verified"),
        content(&keys)
    );

    // 別の鍵は、他人の作成者の ID で metadata を作れない。
    let other = generate_keys();
    assert!(build_public_channel_envelope(&other, &content(&keys)).is_err());
    let mut forged = envelope.clone();
    forged.pubkey = other.public_key();
    assert!(verify_public_channel(&forged).is_err());

    // 作成時刻を変えた envelope と、上限を超えるタイトルは受け付けない。
    let moved = sign_envelope_json_at(
        &keys,
        PUBLIC_CHANNEL_KIND,
        envelope.tags.clone(),
        &content(&keys),
        CREATED_AT_MS / 1_000 + 86_400,
    )
    .expect("moved");
    assert!(verify_public_channel(&moved).is_err());
    for title in [
        "",
        "  ",
        " 前後に空白 ",
        &"字".repeat(PUBLIC_CHANNEL_MAX_TITLE_CHARS + 1),
    ] {
        let mut invalid = content(&keys);
        invalid.title = title.to_string();
        assert!(
            build_public_channel_envelope(&keys, &invalid).is_err(),
            "{title:?}"
        );
    }
    let mut longest = content(&keys);
    longest.title = "字".repeat(PUBLIC_CHANNEL_MAX_TITLE_CHARS);
    assert!(build_public_channel_envelope(&keys, &longest).is_ok());
}

// #1888: 作成者は版を進めてタイトルを変え、削除する。署名の時刻は作成時刻のまま。版 0 の記録は版と削除の欄を持たない
// (既存の記録と同じ形)。削除した版はタイトルを持たない(旧版の確認はタイトルの無い記録を読まない)。
#[test]
fn public_channel_revisions_keep_the_creation_time_and_deletion_drops_the_title() {
    let keys = generate_keys();
    let created = build_public_channel_envelope(&keys, &content(&keys)).expect("created");
    assert!(!created.content.contains("revision") && !created.content.contains("deleted"));

    let mut renamed = content(&keys);
    renamed.title = "新しい雑談".into();
    renamed.revision = 1;
    let envelope = build_public_channel_envelope(&keys, &renamed).expect("renamed");
    assert_eq!(envelope.created_at, CREATED_AT_MS / 1_000);
    assert_eq!(verify_public_channel(&envelope).expect("verified"), renamed);

    let mut deleted = content(&keys);
    deleted.title = String::new();
    deleted.revision = 2;
    deleted.deleted = true;
    let envelope = build_public_channel_envelope(&keys, &deleted).expect("deleted");
    assert_eq!(verify_public_channel(&envelope).expect("verified"), deleted);

    for (title, revision) in [("雑談", 2), ("", 0)] {
        let mut invalid = deleted.clone();
        invalid.title = title.into();
        invalid.revision = revision;
        assert!(
            build_public_channel_envelope(&keys, &invalid).is_err(),
            "{title:?} {revision}"
        );
    }
}
