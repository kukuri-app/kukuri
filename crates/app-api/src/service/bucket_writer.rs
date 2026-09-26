//! #1221 R5-H: 書込み先の選択。切替の前は旧 replica、後は ADR 0054 §1・§2 の時間 bucket へ書く。
//!
//! bucket は署名する `created_at`(秒)から 1 回だけ決める。再試行や日付の変化で宛先を変えない。

use super::*;
use kukuri_docs_sync::{BucketReplica, BucketScope, TimeBucket};

impl ServiceHandles {
    /// 新形式の writer へ切り替えたか(端末に保存した切替状態を desktop-runtime が渡す)。
    pub(crate) fn writes_buckets(&self) -> bool {
        self.writer_switched_at.get().is_some()
    }

    /// 投稿・media manifest・reaction・継続状態の locator を置く replica。private は現 epoch。
    pub(crate) fn scope_write_replica(
        &self,
        topic_id: &str,
        private: Option<&JoinedPrivateChannelState>,
        created_at: i64,
    ) -> Result<ReplicaId> {
        if !self.writes_buckets() {
            return Ok(private
                .map(current_private_channel_replica_id)
                .unwrap_or_else(|| topic_replica_id(topic_id)));
        }
        let scope = match private {
            Some(state) => BucketScope::PrivateChannel {
                channel_id: state.channel_id.as_str().to_owned(),
                epoch_id: state.current_epoch_id.clone(),
            },
            None => BucketScope::Topic {
                topic_id: topic_id.to_owned(),
            },
        };
        Ok(BucketReplica::new(scope, TimeBucket::from_unix_seconds(created_at)?)?.replica_id())
    }

    /// author の現在値(profile・follow・block・asset・Dome の preset/移動)の更新を、更新時の author bucket へ
    /// event(署名済み envelope)として置く。現在値そのものは従来どおり `author::<pubkey>` の key。切替前は何もしない。
    pub(crate) async fn persist_author_event(
        &self,
        author: &str,
        envelope: &KukuriEnvelope,
    ) -> Result<()> {
        if !self.writes_buckets() {
            return Ok(());
        }
        let replica = self.author_index_replica(author, Utc::now().timestamp())?;
        persist_session_envelope(self.docs_sync.as_ref(), &replica, envelope).await
    }

    /// author の索引(プロフィールの行・現在値の event)を置く replica。現在値そのものは `author::<pubkey>` に残す。
    pub(crate) fn author_index_replica(&self, author: &str, created_at: i64) -> Result<ReplicaId> {
        if !self.writes_buckets() {
            return Ok(author_replica_id(author));
        }
        Ok(BucketReplica::new(
            BucketScope::Author {
                author_pubkey: author.to_owned(),
            },
            TimeBucket::from_unix_seconds(created_at)?,
        )?
        .replica_id())
    }
}

/// 継続状態(live・game)の更新が、entity の state を置いた bucket と違う日に起きたときの、更新時の bucket。
/// 署名済み envelope をそこへ locator として置く(ADR 0054 §2)。旧 replica と同じ日なら `None`。
pub(crate) fn update_locator_replica(
    replica: &ReplicaId,
    now_secs: i64,
) -> Result<Option<ReplicaId>> {
    if !replica.as_str().starts_with("bucket::") {
        return Ok(None);
    }
    let source = BucketReplica::parse(replica)?;
    let now = TimeBucket::from_unix_seconds(now_secs)?;
    Ok((source.bucket() != now)
        .then(|| BucketReplica::new(source.scope().clone(), now).map(|bucket| bucket.replica_id()))
        .transpose()?)
}

impl AppService {
    /// 保存済みの切替状態を渡す(R5-H AC-1)。以後の新しい操作は新形式だけへ書く。2 回目以降は無視する。
    pub fn switch_writer(&self, switched_at: i64) {
        let _ = self.services.writer_switched_at.set(switched_at);
    }

    pub fn writer_switched_at(&self) -> Option<i64> {
        self.services.writer_switched_at.get().copied()
    }
}
