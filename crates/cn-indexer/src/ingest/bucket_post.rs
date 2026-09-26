//! bucketの候補が別の保存先で検証済みの索引を消さないよう、署名から正しい配置を確かめる。

use kukuri_core::{KukuriEnvelope, ObjectVisibility, ReplicaId};
use kukuri_docs_sync::{BucketReplica, BucketScope, DocRecord};

use super::source::PostObjectView;

pub(super) fn canonical_post(
    replica: &ReplicaId,
    object_id: &str,
    record: &DocRecord,
) -> Option<PostObjectView> {
    let bucket = BucketReplica::parse(replica).ok()?;
    let envelope: KukuriEnvelope = serde_json::from_slice(&record.value).ok()?;
    envelope.verify().ok()?;
    if envelope.id.as_str() != object_id {
        return None;
    }
    let header = envelope.to_post_object().ok()??;
    if !bucket.bucket().contains(header.created_at)
        || header.created_at > chrono::Utc::now().timestamp().saturating_add(600)
    {
        return None;
    }
    let matches = match bucket.scope() {
        BucketScope::Topic { topic_id } => {
            header.topic_id.as_str() == topic_id
                && header.channel_id.is_none()
                && header.visibility == ObjectVisibility::Public
        }
        BucketScope::PrivateChannel { channel_id, .. } => header
            .channel_id
            .as_ref()
            .is_some_and(|id| id.as_str() == channel_id),
        BucketScope::Author { .. } => false,
    };
    matches.then_some(PostObjectView {
        object_id: header.object_id.0,
        author: header.author.0,
        created_at: header.created_at,
        payload_ref: header.payload_ref,
        attachments: header.attachments,
        media_manifest_refs: header.media_manifest_refs,
        status: header.status,
        reply_to: header.reply_to,
        repost_of: header.repost_of,
    })
}
