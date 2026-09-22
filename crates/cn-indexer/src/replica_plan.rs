//! 同じ論理scopeが複数の物理replicaを持つための読取り計画。
//! runtimeでの切替は移行・取り下げcontractが揃ってから行う。既定は旧形式。

use anyhow::{Result, ensure};
use kukuri_cn_core::IndexScopeKind;
use kukuri_core::ReplicaId;
use kukuri_docs_sync::{
    BucketReplica, BucketScope, PostReplicaKind, TimeBucket, post_replica_kind,
};

use crate::participant::ScopeReplica;

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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PublicReplicaReadMode {
    #[default]
    Legacy,
    Transition,
    TimeBucketV1,
}

impl PublicReplicaReadMode {
    pub(crate) fn scopes(
        self,
        kind: IndexScopeKind,
        id: &str,
        now: i64,
    ) -> Result<Vec<ScopeReplica>> {
        // privateのepoch/同意の切替は別contract。public設定でprivateを切り替えない。
        if kind == IndexScopeKind::PrivateChannel || self == Self::Legacy {
            return Ok(vec![ScopeReplica::from_scope(kind, id)]);
        }
        let mut scopes = Vec::with_capacity(3);
        for bucket in TimeBucket::from_unix_seconds(now)?.live_window() {
            let replica = BucketReplica::new(
                BucketScope::Topic {
                    topic_id: id.into(),
                },
                bucket,
            )?;
            scopes.push(ScopeReplica {
                kind,
                id: id.into(),
                replica_id: replica.replica_id(),
            });
        }
        if self == Self::Transition {
            scopes.push(ScopeReplica::from_scope(kind, id));
        }
        Ok(scopes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_public_replicas_do_not_include_the_cumulative_legacy_namespace() -> Result<()> {
        for day in [1, 10, 100, 1_000, 10_000] {
            let scopes = PublicReplicaReadMode::TimeBucketV1.scopes(
                IndexScopeKind::PublicTopic,
                "topic",
                day * 86_400,
            )?;
            assert_eq!(scopes.len(), 2);
            let buckets = scopes
                .iter()
                .map(|scope| {
                    BucketReplica::parse(&scope.replica_id)
                        .unwrap()
                        .bucket()
                        .index()
                })
                .collect::<Vec<_>>();
            assert_eq!(buckets, vec![day as u64, day as u64 - 1]);
        }
        Ok(())
    }

    #[test]
    fn transition_is_explicit_and_does_not_change_private_scope() -> Result<()> {
        let legacy = ScopeReplica::from_scope(IndexScopeKind::PublicTopic, "topic");
        let scopes = PublicReplicaReadMode::Transition.scopes(
            IndexScopeKind::PublicTopic,
            "topic",
            86_400,
        )?;
        assert_eq!(scopes.len(), 3);
        assert_eq!(scopes.last(), Some(&legacy));
        for mode in [
            PublicReplicaReadMode::Legacy,
            PublicReplicaReadMode::Transition,
            PublicReplicaReadMode::TimeBucketV1,
        ] {
            assert_eq!(
                mode.scopes(IndexScopeKind::PrivateChannel, "channel", 86_400)?,
                vec![ScopeReplica::from_scope(
                    IndexScopeKind::PrivateChannel,
                    "channel"
                )]
            );
        }
        assert!(
            PublicReplicaReadMode::TimeBucketV1
                .scopes(IndexScopeKind::PublicTopic, "topic", -1)
                .is_err()
        );
        Ok(())
    }
}
