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

/// live・game・Dome の session の署名済み envelope を state の replica へ置き、更新が別の日なら更新時の bucket にも
/// envelope と、state の replica を指す locator(`sessions/<kind>/<id>/locator`)を置く(ADR 0054 §2)。state を別の
/// replica へ移した(`moved`)ときも、その日の bucket に locator を置く。
pub(crate) async fn persist_session_envelope_and_locator(
    services: &ServiceHandles,
    replica: &ReplicaId,
    envelope: &KukuriEnvelope,
    locator_key: &str,
    moved: bool,
    now_ms: i64,
) -> Result<()> {
    persist_session_envelope(services.docs_sync.as_ref(), replica, envelope).await?;
    let locator = match update_locator_replica(replica, now_ms / 1_000)? {
        Some(locator) => {
            persist_session_envelope(services.docs_sync.as_ref(), &locator, envelope).await?;
            Some(locator)
        }
        None => (moved && replica.as_str().starts_with("bucket::")).then(|| replica.clone()),
    };
    if let Some(locator) = locator {
        services
            .docs_sync
            .apply_doc_op(
                &locator,
                DocOp::SetJson {
                    key: locator_key.to_owned(),
                    value: serde_json::to_value(replica.as_str())?,
                },
            )
            .await?;
    }
    Ok(())
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
    (source.bucket() != now)
        .then(|| BucketReplica::new(source.scope().clone(), now).map(|bucket| bucket.replica_id()))
        .transpose()
}

impl AppService {
    /// session(live・game・Dome)の state を書く replica(#1221 R5-H)。切替前は `source` のまま。切替後は、公開なら
    /// `source` の bucket、private なら現 epoch の `source` の bucket に書く。切替前に旧 replica へ置いた session は
    /// 公開なら作成時刻の bucket へ、回転の前の epoch の bucket・旧 replica に置いた private の session は現 epoch の
    /// その日の bucket へ移す(旧 replica と、回転で外れた参加者が読める旧 epoch の bucket へは書かない)。移したときは
    /// 書く側が locator を置く(`persist_session_envelope_and_locator`)。
    pub(crate) async fn session_write_replica(
        &self,
        topic_id: &str,
        channel_id: Option<&ChannelId>,
        source: &ReplicaId,
        created_at_ms: i64,
    ) -> Result<ReplicaId> {
        if !self.services.writes_buckets() {
            return Ok(source.clone());
        }
        let bucket = source
            .as_str()
            .starts_with("bucket::")
            .then(|| BucketReplica::parse(source))
            .transpose()?;
        let Some(channel_id) = channel_id else {
            return match bucket {
                Some(_) => Ok(source.clone()),
                None => self
                    .services
                    .scope_write_replica(topic_id, None, created_at_ms / 1_000),
            };
        };
        let private = self
            .joined_private_channel_state(topic_id, channel_id.as_str())
            .await
            .context("private channel is not joined")?;
        if bucket.is_some_and(|bucket| {
            matches!(bucket.scope(), BucketScope::PrivateChannel { epoch_id, .. } if *epoch_id == private.current_epoch_id)
        }) {
            return Ok(source.clone());
        }
        self.services
            .scope_write_replica(topic_id, Some(&private), Utc::now().timestamp())
    }

    /// 保存済みの切替状態を渡す(R5-H AC-1)。以後の新しい操作は新形式だけへ書く。2 回目以降は無視する。
    pub fn switch_writer(&self, switched_at: i64) {
        let _ = self.services.writer_switched_at.set(switched_at);
    }

    pub fn writer_switched_at(&self) -> Option<i64> {
        self.services.writer_switched_at.get().copied()
    }
}
