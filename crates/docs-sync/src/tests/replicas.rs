//! replica 命名・namespace 派生の golden テスト(WP-S3 T4)。
//!
//! replica id とその blake3 派生 namespace はネットワーク全体の rendezvous を
//! 決める凍結境界(1 文字の変更で既存ピアと同じ replica を発見できなくなる)。
//! fail した場合はテストでなく変更側を疑うこと。

use kukuri_core::ReplicaId;

use crate::replicas::{
    PostReplicaKind, author_replica_id, device_replica_id, post_replica_kind,
    private_channel_epoch_replica_id, private_channel_hint_topic, private_channel_replica_id,
    public_replica_secret, stable_key, topic_replica_id,
};

#[test]
fn replica_id_helpers_match_golden() {
    assert_eq!(topic_replica_id("demo").as_str(), "topic::demo");
    assert_eq!(
        private_channel_replica_id("chan-1").as_str(),
        "channel::chan-1"
    );
    assert_eq!(
        private_channel_epoch_replica_id("chan-1", "epoch-2").as_str(),
        "channel::chan-1::epoch::epoch-2"
    );
    assert_eq!(
        private_channel_hint_topic("chan-1").as_str(),
        "private/chan-1"
    );
    assert_eq!(author_replica_id("pubkey-a").as_str(), "author::pubkey-a");
    assert_eq!(
        device_replica_id("pubkey-a", "device-1").as_str(),
        "device::pubkey-a::device-1"
    );
    assert_eq!(stable_key("objects", "obj-1/state"), "objects/obj-1/state");
}

// #1248: 投稿の反映は、読んだ replica の種別から「その replica に置いてよい投稿」を決める。
#[test]
fn post_replica_kind_inverts_the_replica_id_helpers() {
    assert_eq!(
        post_replica_kind(&topic_replica_id("kukuri:topic:demo")),
        Some(PostReplicaKind::PublicTopic {
            topic_id: "kukuri:topic:demo".into()
        })
    );
    // topic id は区切りを含んでよい(接頭辞より後ろの全体が topic id)。
    assert_eq!(
        post_replica_kind(&topic_replica_id("a::epoch::b")),
        Some(PostReplicaKind::PublicTopic {
            topic_id: "a::epoch::b".into()
        })
    );
    let private = Some(PostReplicaKind::PrivateChannel {
        channel_id: "chan-1".into(),
    });
    assert_eq!(
        post_replica_kind(&private_channel_replica_id("chan-1")),
        private
    );
    assert_eq!(
        post_replica_kind(&private_channel_epoch_replica_id("chan-1", "epoch-2")),
        private
    );
}

#[test]
fn post_replica_kind_rejects_replicas_that_do_not_hold_posts_and_ambiguous_ids() {
    for replica in [
        author_replica_id("pubkey-a"),
        device_replica_id("pubkey-a", "device-1"),
        ReplicaId::new("topic::"),
        ReplicaId::new("channel::"),
        ReplicaId::new("channel::chan-1::epoch::"),
        ReplicaId::new("unknown::value"),
        // owner が決める id に区切りが入ると、複数の channel として読めてしまう。
        private_channel_epoch_replica_id("chan-1::epoch::epoch-2", "epoch-3"),
        private_channel_epoch_replica_id("chan-1", "epoch-2::epoch::epoch-3"),
        private_channel_replica_id("chan-1::other"),
    ] {
        assert_eq!(post_replica_kind(&replica), None, "{}", replica.as_str());
    }
}

#[test]
fn public_replica_namespace_secret_matches_golden_digest() {
    // blake3("kukuri-docs:" || replica_id) がそのまま NamespaceSecret になる。
    let secret =
        public_replica_secret(&ReplicaId::new("topic::demo")).expect("public replica secret");
    assert_eq!(
        hex::encode(secret.to_bytes()),
        "ebd5f493d3f598727ea4b7b14bb42e5cec7d7b19adef0a3629586be201cddee1"
    );
}

#[test]
fn private_channel_replicas_never_get_public_namespace_secret() {
    assert!(public_replica_secret(&ReplicaId::new("channel::chan-1")).is_none());
    assert!(public_replica_secret(&ReplicaId::new("channel::chan-1::epoch::epoch-2")).is_none());
}

#[test]
fn bucket_post_scope_is_decoded_before_integrity_validation() {
    assert_eq!(
        post_replica_kind(&ReplicaId::new("bucket::v1::topic::64656d6f::20718")),
        Some(PostReplicaKind::PublicTopic {
            topic_id: "demo".into(),
        })
    );
    assert_eq!(
        post_replica_kind(&ReplicaId::new(
            "bucket::v1::channel::6368616e::6531::20718"
        )),
        Some(PostReplicaKind::PrivateChannel {
            channel_id: "chan".into(),
        })
    );
}

#[test]
fn private_or_unknown_bucket_cannot_derive_a_public_secret() {
    for id in [
        "bucket::v1::channel::6368616e::6531::20718",
        "bucket::v2::topic::64656d6f::20718",
        "bucket::v1::channel::6368616e::20718",
        "bucket::v1::topic::64656d6f::020718",
        "bucket::v1::topic::ff::20718",
    ] {
        assert!(public_replica_secret(&ReplicaId::new(id)).is_none(), "{id}");
    }
}
