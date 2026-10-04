//! account の保存（projection・remote の cache と本人の書込みの保護・private index の grant・信頼評価の観測の提供）。
//! native は `SqliteStore`、Web は IndexedDB が実装する（ADR 0058・0059）。desktop-runtime は
//! `Arc<dyn AccountStore>` を持ち、native だけの処理（旧 store の退役・移行・backup）だけが `SqliteStore` を使う。

use anyhow::Result;
use async_trait::async_trait;
use kukuri_core::KukuriEnvelope;

use crate::{ContentCacheStore, ProjectionStore, Store};

/// Community Node へ private channel の索引を許した記録（node・channel ごと）。
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PrivateIndexGrant {
    pub base_url: String,
    pub topic_id: String,
    pub channel_id: String,
    pub applied_epoch_id: String,
}

#[async_trait]
pub trait PrivateIndexGrantStore: Send + Sync {
    async fn save_private_index_grant(&self, grant: &PrivateIndexGrant) -> Result<()>;
    /// 1 回の CN の tick で 1 行。account の grant の全件を読まない。
    async fn next_private_index_grant(
        &self,
        base_url: &str,
        now_ms: i64,
    ) -> Result<Option<PrivateIndexGrant>>;
    async fn mark_private_index_grant_applied(
        &self,
        grant: &PrivateIndexGrant,
        epoch_id: &str,
    ) -> Result<()>;
    async fn stop_private_index_grant(
        &self,
        base_url: &str,
        topic_id: &str,
        channel_id: &str,
    ) -> Result<()>;
    async fn stop_private_index_grants_for_node(&self, base_url: &str) -> Result<()>;
    async fn stop_private_index_grants_for_channel(
        &self,
        topic_id: &str,
        channel_id: &str,
    ) -> Result<()>;
}

/// CN ごとの観測の提供の状態。提供していない node は送信待ちを持たない。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TrustObservationNode {
    pub enabled: bool,
    pub needs_reconsent: bool,
    pub revocation_pending: bool,
}

/// 信頼評価の観測の提供の、CN ごとの状態と送信待ち（#1061、#1510）。pass と操作は、node の 1 行と、key の行・
/// 上限つきの範囲だけを読み書きする。送信待ちの key は `<対象>|<種別>`。
#[async_trait]
pub trait TrustObservationStore: Send + Sync {
    async fn trust_observation_node(&self, base_url: &str) -> Result<Option<TrustObservationNode>>;
    /// node の状態を書く(`None` は node を忘れる)。提供していない node の送信待ちは消す。
    async fn save_trust_observation_node(
        &self,
        base_url: &str,
        node: Option<TrustObservationNode>,
    ) -> Result<()>;
    /// 提供中(有効で、削除要求が未完了でない)の node があるか。
    async fn trust_observation_sharing(&self) -> Result<bool>;
    /// `key` の送信待ちのうち、最も新しい署名の時刻。次の署名はこれより後にする。
    async fn latest_queued_trust_observation_at(&self, key: &str) -> Result<Option<i64>>;
    /// 提供中の node の送信待ちへ置き、同じ key の古いものを置き換える。`base_url` を渡すと、その node だけへ置く。
    async fn queue_trust_observation(
        &self,
        base_url: Option<&str>,
        key: &str,
        envelope: &KukuriEnvelope,
    ) -> Result<()>;
    /// node の送信待ちを、key の順に最大 `limit` 件。
    async fn queued_trust_observations(
        &self,
        base_url: &str,
        limit: usize,
    ) -> Result<Vec<(String, KukuriEnvelope)>>;
    /// 送信待ちから外す。その間に新しい観測へ置き換わった key は残す。
    async fn dequeue_trust_observation(
        &self,
        base_url: &str,
        key: &str,
        envelope_id: &str,
    ) -> Result<()>;
    async fn count_queued_trust_observations(&self, base_url: &str) -> Result<i64>;
}

pub trait AccountStore:
    Store + ProjectionStore + ContentCacheStore + PrivateIndexGrantStore + TrustObservationStore
{
}

impl<
    T: Store + ProjectionStore + ContentCacheStore + PrivateIndexGrantStore + TrustObservationStore,
> AccountStore for T
{
}
