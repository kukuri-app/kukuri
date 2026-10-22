//! CLI daemon が保持する公開 topic と、取り込みの続きの位置（#1909、ADR 0055 §1.1）。native は `SqliteStore`、
//! 試験は `MemoryStore` が実装する（Web は保持しない）。

use anyhow::Result;
use async_trait::async_trait;

/// 保持する topic の 1 行。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TopicRetentionRow {
    pub topic_id: String,
    /// 保持を始めた時刻（ミリ秒）。
    pub retained_at: i64,
}

/// topic の bucket の時刻順の索引を、読み終えた最後の entry。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TopicRetentionCursor {
    /// 索引の prefix（`indexes/timeline/`・`channels/list/`・`channels/activity/`）。
    pub index_prefix: String,
    /// bucket の番号（UTC の日）。
    pub bucket: u64,
    /// entry の時刻（秒）。
    pub created_at: i64,
    pub object_id: String,
}

#[async_trait]
pub trait TopicRetentionStore: Send + Sync {
    /// 保持する topic を足す。既にあれば何もしない。
    async fn retain_topic(&self, topic_id: &str, retained_at: i64) -> Result<()>;
    /// 保持をやめ、その topic の続きの位置も消す。
    async fn release_topic(&self, topic_id: &str) -> Result<()>;
    /// 保持する topic を topic の順に `limit` 件まで返す。
    async fn retained_topics(&self, limit: usize) -> Result<Vec<TopicRetentionRow>>;
    /// topic の `from_bucket` 以後の bucket の続きの位置を、索引と bucket の順に返す。
    async fn topic_retention_cursors(
        &self,
        topic_id: &str,
        from_bucket: u64,
    ) -> Result<Vec<TopicRetentionCursor>>;
    /// 続きの位置を置く。保持していない topic なら置かない（保持をやめた後に終わった取り込みが位置を残さない）。
    async fn put_topic_retention_cursor(
        &self,
        topic_id: &str,
        cursor: &TopicRetentionCursor,
    ) -> Result<()>;
    /// topic の `before_bucket` より前の bucket の続きの位置を消す。
    async fn prune_topic_retention_cursors(&self, topic_id: &str, before_bucket: u64)
    -> Result<()>;
}
