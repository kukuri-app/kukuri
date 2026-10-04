//! 本人の端末間の account 同期の導出と item の封（ADR 0061 §1・§2）。

use crate::{
    AccountSyncItem, AccountSyncItemKey, ChannelId, ChannelParticipantV1, EnvelopeId, FollowEdge,
    FollowEdgeStatus, KukuriKeys, MAX_ACCOUNT_SYNC_ITEM_BYTES, Pubkey, SealedAccountSyncItem,
    receive_route_for_account,
};

const SECRET: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
const OTHER_SECRET: &str = "101112131415161718191a1b1c1d1e1f000102030405060708090a0b0c0d0e0f";

fn keys(secret: &str) -> KukuriKeys {
    KukuriKeys::parse(secret).expect("test key")
}

fn item(key: AccountSyncItemKey) -> AccountSyncItem {
    AccountSyncItem {
        key,
        op_id: "0123456789abcdef0123456789abcdef".to_string(),
        updated_at: 1_790_000_000_000,
        value: Some(serde_json::json!({ "secret_hex": "aa" })),
    }
}

fn capability() -> AccountSyncItemKey {
    AccountSyncItemKey::ChannelCapability {
        channel_id: ChannelId("channel-a".to_string()),
        epoch_id: "epoch-2".to_string(),
    }
}

// context・手順が変わると、全アカウントの同期先が変わり、端末間で同期できなくなる。
#[test]
fn account_sync_derivation_matches_golden() {
    let derived = keys(SECRET).derive_account_sync();
    assert_eq!(
        derived.replica_id().as_str(),
        "account::v1::82a779ca51bde2a5a4f3a5eef0338bc493087d106d7e6533af2877d5d2f923cb"
    );
    assert_eq!(
        derived.hint_topic().as_str(),
        "kukuri:account:7e1a4b5344b5c1c94c1833d64bb8a18585dbaf2d3b9b6c5d474475a5a7bb83eb"
    );
    assert_eq!(
        derived.expose_namespace_secret_hex(),
        "354df94405ac34641c66f575e6325739e2280a0c6d39c85eb221a17957c6820a"
    );
}

#[test]
fn the_same_key_derives_the_same_values_and_another_key_does_not() {
    let first = keys(SECRET).derive_account_sync();
    let again = keys(SECRET).derive_account_sync();
    let other = keys(OTHER_SECRET).derive_account_sync();
    assert_eq!(first.replica_id(), again.replica_id());
    assert_eq!(first.hint_topic(), again.hint_topic());
    assert_ne!(first.replica_id(), other.replica_id());
    assert_ne!(first.hint_topic(), other.hint_topic());
    assert_ne!(
        first.expose_namespace_secret_hex(),
        other.expose_namespace_secret_hex()
    );
    assert!(!format!("{first:?}").contains(&first.expose_namespace_secret_hex()));
}

#[test]
fn each_purpose_uses_a_separate_value() {
    let derived = keys(SECRET).derive_account_sync();
    let replica = derived
        .replica_id()
        .as_str()
        .strip_prefix("account::v1::")
        .expect("replica prefix")
        .to_string();
    let topic = derived
        .hint_topic()
        .as_str()
        .strip_prefix("kukuri:account:")
        .expect("topic prefix")
        .to_string();
    let namespace = derived.expose_namespace_secret_hex();
    assert_ne!(replica, topic);
    assert_ne!(replica, namespace);
    assert_ne!(topic, namespace);
    // docs author の導出（ADR 0053）とも別の値になる。
    let author = hex::encode(keys(SECRET).derive_docs_author_seed().expose_secret_bytes());
    assert!(![replica, topic, namespace].contains(&author));
}

// 診断の一覧・Community Node の公開 topic の索引と対応 topic は、この判定で account 同期の hint を外す（ADR 0061 §6）。
#[test]
fn the_account_hint_topic_is_not_a_public_topic() {
    let hint = keys(SECRET).derive_account_sync().hint_topic().clone();
    assert!(crate::wire::is_non_public_topic(hint.as_str()));
    assert!(crate::wire::is_non_public_topic(
        crate::wire::hint_topic_id(&hint).as_str()
    ));
    assert!(!crate::wire::is_non_public_topic("kukuri:topic:rust"));
    assert!(!crate::wire::is_non_public_topic("hint/kukuri:topic:rust"));
}

#[test]
fn the_account_route_is_separate_from_the_public_receive_route() {
    let account_keys = keys(SECRET);
    let route = receive_route_for_account(&account_keys.public_key()).expect("receive route");
    let derived = account_keys.derive_account_sync();
    assert!(route.as_str().starts_with("receive::v1::"));
    assert!(!derived.hint_topic().as_str().starts_with("receive::"));
    assert!(
        !derived
            .replica_id()
            .as_str()
            .contains(&account_keys.public_key_hex())
    );
    assert!(
        !derived
            .hint_topic()
            .as_str()
            .contains(&account_keys.public_key_hex())
    );
}

#[test]
fn a_sealed_item_opens_only_for_the_same_account_and_key() {
    let account_keys = keys(SECRET);
    let derived = account_keys.derive_account_sync();
    let account = account_keys.public_key();
    let original = item(capability());
    let docs_key = original.key.docs_key();
    let sealed = derived.seal(&account, &original).expect("seal");
    // 鍵を含む value を Debug へ出さない。
    assert!(!format!("{original:?}").contains("secret_hex"));
    assert!(!sealed.ciphertext_hex.contains(&hex::encode("secret_hex")));
    assert_eq!(
        derived.open(&account, &docs_key, &sealed).expect("open"),
        original
    );

    // 別のアカウント鍵では開けない。
    let other = keys(OTHER_SECRET);
    assert!(
        other
            .derive_account_sync()
            .open(&account, &docs_key, &sealed)
            .is_err()
    );
    // 別の account・別の key に置き換えても開けない。
    assert!(
        derived
            .open(&other.public_key(), &docs_key, &sealed)
            .is_err()
    );
    assert!(derived.open(&account, "profile", &sealed).is_err());
    // 改ざんは開けない。
    let mut tampered = sealed.clone();
    let last = tampered.ciphertext_hex.pop().expect("hex");
    tampered
        .ciphertext_hex
        .push(if last == '0' { '1' } else { '0' });
    assert!(derived.open(&account, &docs_key, &tampered).is_err());
}

#[test]
fn items_over_the_limits_or_outside_the_allowlist_are_rejected() {
    let account_keys = keys(SECRET);
    let derived = account_keys.derive_account_sync();
    let account = account_keys.public_key();

    let mut large = item(AccountSyncItemKey::Profile);
    large.value = Some(serde_json::Value::String(
        "x".repeat(MAX_ACCOUNT_SYNC_ITEM_BYTES),
    ));
    assert!(derived.seal(&account, &large).is_err());

    let oversized = SealedAccountSyncItem {
        v: 1,
        nonce_hex: "00".repeat(24),
        ciphertext_hex: "00".repeat(2 * MAX_ACCOUNT_SYNC_ITEM_BYTES),
    };
    assert!(derived.open(&account, "profile", &oversized).is_err());

    let mut bad_op = item(AccountSyncItemKey::Profile);
    bad_op.op_id = "not-hex".to_string();
    assert!(derived.seal(&account, &bad_op).is_err());

    let bad_author = item(AccountSyncItemKey::TrustAlwaysVisible {
        author: Pubkey("not-a-pubkey".to_string()),
    });
    assert!(derived.seal(&account, &bad_author).is_err());

    // allowlist の外の種類（端末の同意など）は読めない。
    assert!(serde_json::from_str::<AccountSyncItemKey>(r#"{"kind":"app_consent"}"#).is_err());
    assert!(serde_json::from_str::<AccountSyncItemKey>(r#"{"kind":"endpoint_secret"}"#).is_err());
}

#[test]
fn docs_keys_do_not_contain_separators_from_ids() {
    let key = AccountSyncItemKey::ChannelCapability {
        channel_id: ChannelId("a/b".to_string()),
        epoch_id: "c/d".to_string(),
    };
    assert_eq!(key.docs_key(), "channel/612f62/epoch/632f64");
}

/// 世代の鍵の item の op_id は (channel, epoch) から決まり、書き直しや受け取った時刻で変わらない(ADR 0061 §9)。
#[test]
fn channel_epoch_items_keep_their_op_id_across_rewrites() {
    let channel = ChannelId::new("room");
    let value = serde_json::json!({"epoch_id": "epoch-1", "namespace_secret_hex": "11".repeat(32)});
    let first = AccountSyncItem::channel_epoch(&channel, "epoch-1", 10, value.clone()).unwrap();
    let rewritten = AccountSyncItem::channel_epoch(&channel, "epoch-1", 20, value.clone()).unwrap();
    let other = AccountSyncItem::channel_epoch(&channel, "epoch-2", 10, value).unwrap();
    assert_eq!(first.op_id, rewritten.op_id);
    assert_ne!(first.op_id, other.op_id);
    assert_eq!(
        first.key.docs_key(),
        "channel/726f6f6d/epoch/65706f63682d31"
    );
    assert_eq!(
        AccountSyncItemKey::ChannelMembership {
            channel_id: channel
        }
        .docs_key(),
        "channel/726f6f6d/membership"
    );
}

/// 参加者の item と、参加者との follow の item は docs の key を往復でき、同じ record・edge から作り直しても版が
/// 変わらない（#1219 AC-5）。
#[test]
fn participant_items_round_trip_and_keep_their_version() {
    let channel = ChannelId::new("room");
    let participant = keys(SECRET).public_key();
    let joined = ChannelParticipantV1 {
        epoch_id: "epoch-1".into(),
        joined_at: 10,
        left_at: None,
    };
    let first = AccountSyncItem::channel_participant(&channel, &participant, &joined).unwrap();
    assert_eq!(
        first,
        AccountSyncItem::channel_participant(&channel, &participant, &joined).unwrap()
    );
    let left = AccountSyncItem::channel_participant(
        &channel,
        &participant,
        &ChannelParticipantV1 {
            left_at: Some(20),
            ..joined.clone()
        },
    )
    .unwrap();
    assert_eq!((first.updated_at, left.updated_at), (10, 20));
    assert_ne!(first.op_id, left.op_id);
    let follow = AccountSyncItem::participant_follow(&FollowEdge {
        subject_pubkey: participant.clone(),
        target_pubkey: keys(OTHER_SECRET).public_key(),
        status: FollowEdgeStatus::Active,
        updated_at: 30,
        envelope_id: EnvelopeId::from("ab".repeat(32)),
    })
    .unwrap();
    assert_eq!(
        (follow.op_id.as_str(), follow.updated_at),
        ("ab".repeat(16).as_str(), 30)
    );
    assert_eq!(
        first.key.docs_key(),
        format!("channel/726f6f6d/participant/{}", participant.as_str())
    );
    for item in [first, follow] {
        assert_eq!(
            AccountSyncItemKey::from_docs_key(&item.key.docs_key()).unwrap(),
            item.key
        );
    }
}
