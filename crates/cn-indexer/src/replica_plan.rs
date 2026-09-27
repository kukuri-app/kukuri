//! 索引の論理 scope と、読む replica の対応の確認。

use anyhow::{Result, ensure};
use kukuri_cn_core::IndexScopeKind;
use kukuri_core::ReplicaId;
use kukuri_docs_sync::{PostReplicaKind, post_replica_kind};

/// logical scopeの確認はopenより前に行う。locatorは権限の証明ではない。
pub(crate) fn validate_scope_replica(
    kind: IndexScopeKind,
    id: &str,
    replica: &ReplicaId,
) -> Result<()> {
    let matches = match (kind, post_replica_kind(replica)) {
        (IndexScopeKind::PublicTopic, Some(PostReplicaKind::PublicTopic { topic_id })) => {
            topic_id == id
        }
        (IndexScopeKind::PrivateChannel, Some(PostReplicaKind::PrivateChannel { channel_id })) => {
            channel_id == id
        }
        _ => false,
    };
    ensure!(
        matches,
        "replica does not belong to the requested indexing scope"
    );
    Ok(())
}
