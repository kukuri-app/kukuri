//! account の保存（projection・remote の cache と本人の書込みの保護・private index の grant）。
//! native は `SqliteStore`、Web は IndexedDB が実装する（ADR 0058・0059）。desktop-runtime は
//! `Arc<dyn AccountStore>` を持ち、native だけの処理（旧 store の退役・移行・backup）だけが `SqliteStore` を使う。

use anyhow::Result;
use async_trait::async_trait;

use crate::{ContentCacheStore, ProjectionStore, Store};

/// Community Node へ private channel の索引を許した記録（node・channel ごと）。
#[derive(Clone, Debug, PartialEq, Eq)]
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

pub trait AccountStore:
    Store + ProjectionStore + ContentCacheStore + PrivateIndexGrantStore
{
}

impl<T: Store + ProjectionStore + ContentCacheStore + PrivateIndexGrantStore> AccountStore for T {}
