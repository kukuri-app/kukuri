//! rendezvous / topic id 派生の golden テスト(WP-S3 T4)。
//!
//! rendezvous key はクライアント間で一致して初めて機能する wire 契約
//! (community-node はキーを opaque に扱い再導出しない)。ドメイン文字列・
//! 区切りバイト・ハッシュ構成のいずれの変更もネットワーク分断になる。
//! fail した場合はテストでなく変更側(コード・依存更新)を疑うこと。

use crate::{
    TopicId, author_profile_topic_id, private_topic_rendezvous_key_hex_secret,
    public_topic_rendezvous_key,
};

#[test]
fn public_topic_rendezvous_key_matches_golden() {
    // blake3("kukuri:rendezvous:public-topic:v1" || 0x00 || topic)
    assert_eq!(
        public_topic_rendezvous_key(&TopicId::new("kukuri:topic:golden")),
        "7f7f9902af85354eeadf44b6b953f31d0ec8fe39def5235b1f9a9ff0e1209e7e"
    );
}

#[test]
fn private_topic_rendezvous_key_matches_golden() {
    // HMAC-SHA256(namespace_secret, "kukuri:rendezvous:private-topic:v1" || 0x00 || topic)
    let key = private_topic_rendezvous_key_hex_secret(
        "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
        // `subscribed_topics` に入る、`hint/` 付与後の通信形式の文字列を固定する。
        &TopicId::new("hint/private/channel-golden"),
    )
    .expect("非公開ランデブー鍵を派生できる");
    assert_eq!(
        key,
        "abd813b33010433f3ce2032f67c55e1ec94062ee422a123bf430143d5df5241a"
    );
}

#[test]
fn author_profile_topic_id_matches_golden() {
    assert_eq!(
        author_profile_topic_id("79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798")
            .as_str(),
        "kukuri:topic:profile:79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
    );
}

// ADR 0053 §1: docs author の秘密鍵の種。context か手順が変わると、全アカウントの docs author の id が変わり、
// それまでの投稿の `docs_author` の tag と合わなくなる。
#[test]
fn docs_author_seed_matches_golden() {
    let keys = crate::KukuriKeys::parse(
        "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
    )
    .expect("test key");
    assert_eq!(
        hex::encode(keys.derive_docs_author_seed().expose_secret_bytes()),
        "cc17b04dc636c67c8d9a3e6145b5b86191dfb080d29a30c14253ab6adf50f49d"
    );
    // 別のアカウント鍵からは別の値になり、Debug は中身を出さない。
    let other = crate::KukuriKeys::parse(
        "101112131415161718191a1b1c1d1e1f000102030405060708090a0b0c0d0e0f",
    )
    .expect("test key");
    assert_ne!(
        other.derive_docs_author_seed().expose_secret_bytes(),
        keys.derive_docs_author_seed().expose_secret_bytes()
    );
    assert_eq!(
        format!("{:?}", keys.derive_docs_author_seed()),
        "DocsAuthorSeed { .. }"
    );
}
