use iroh_docs::NamespaceSecret;
use kukuri_core::{ReplicaId, TopicId, blob_hash};

pub(crate) fn public_replica_secret(replica_id: &ReplicaId) -> Option<NamespaceSecret> {
    if replica_id.as_str().starts_with("channel::") {
        return None;
    }
    let digest = blake3::hash(format!("kukuri-docs:{}", replica_id.as_str()).as_bytes());
    Some(NamespaceSecret::from_bytes(digest.as_bytes()))
}

pub fn topic_replica_id(topic_id: &str) -> ReplicaId {
    ReplicaId::new(format!("topic::{topic_id}"))
}

pub fn private_channel_replica_id(channel_id: &str) -> ReplicaId {
    ReplicaId::new(format!("channel::{channel_id}"))
}

pub fn private_channel_epoch_replica_id(channel_id: &str, epoch_id: &str) -> ReplicaId {
    ReplicaId::new(format!("channel::{channel_id}::epoch::{epoch_id}"))
}

/// 投稿を置く replica の種別(#1248)。replica id の組み立て(`topic_replica_id`・`private_channel_replica_id`・
/// `private_channel_epoch_replica_id`)の逆。
///
/// 投稿の反映は、読んだ replica と投稿が申告する topic / channel の整合を確かめるために使う。
/// private channel の replica id は topic を含まないので、topic は呼び出し側が購読の文脈から渡す。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PostReplicaKind {
    /// `topic::<topic id>`
    PublicTopic { topic_id: String },
    /// `channel::<channel id>` と `channel::<channel id>::epoch::<epoch id>`
    PrivateChannel { channel_id: String },
}

/// 投稿を置く replica でなければ `None`(author・device の replica、形の違う id)。
///
/// channel id と epoch id は owner が決める文字列なので、区切り(`::`)を含む id は受け付けない。
/// 受け付けると、`channel::a::epoch::b::epoch::c` のような id が複数の channel として読めてしまう。
pub fn post_replica_kind(replica_id: &ReplicaId) -> Option<PostReplicaKind> {
    let raw = replica_id.as_str();
    if let Some(topic_id) = raw.strip_prefix("topic::") {
        return (!topic_id.is_empty()).then(|| PostReplicaKind::PublicTopic {
            topic_id: topic_id.to_string(),
        });
    }
    let rest = raw.strip_prefix("channel::")?;
    let (channel_id, epoch_id) = match rest.split_once("::epoch::") {
        Some((channel_id, epoch_id)) => (channel_id, Some(epoch_id)),
        None => (rest, None),
    };
    let well_formed = |part: &str| !part.is_empty() && !part.contains("::");
    (well_formed(channel_id) && epoch_id.is_none_or(well_formed)).then(|| {
        PostReplicaKind::PrivateChannel {
            channel_id: channel_id.to_string(),
        }
    })
}

pub fn private_channel_hint_topic(channel_id: &str) -> TopicId {
    TopicId::new(format!(
        "{}{channel_id}",
        kukuri_core::wire::PRIVATE_CHANNEL_TOPIC_PREFIX
    ))
}

pub fn author_replica_id(author_pubkey: &str) -> ReplicaId {
    ReplicaId::new(format!("author::{author_pubkey}"))
}

pub fn device_replica_id(author_pubkey: &str, device_id: &str) -> ReplicaId {
    ReplicaId::new(format!("device::{author_pubkey}::{device_id}"))
}

pub fn stable_key(prefix: &str, key: &str) -> String {
    format!("{prefix}/{key}")
}

pub fn value_hash(value: impl AsRef<[u8]>) -> String {
    blob_hash(value).0
}
