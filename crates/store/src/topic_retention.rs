//! CLI daemon が保持する公開 topic と、取り込みの続きの位置（#1909、ADR 0055 §1.1）。native は `SqliteStore`、
//! 試験は `MemoryStore` が実装する（Web は保持しない）。

use anyhow::Result;
use async_trait::async_trait;

/// 取り直しを待つ対象の上限(保持するすべての topic の合計)。超える分は諦めた数に足す。
pub const RETENTION_RETRY_LIMIT: usize = 4_096;
/// n 回目の取り直しまでの間(ミリ秒)。5 回目の取り直しも取れなければ諦める。
pub const RETENTION_RETRY_DELAYS_MS: [i64; 5] = [
    60_000,
    10 * 60_000,
    60 * 60_000,
    6 * 60 * 60_000,
    24 * 60 * 60_000,
];

/// 参照する blob を取れなかった対象の、取り直しの予定。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TopicRetentionRetry {
    /// 対象の種類(投稿・session)。
    pub object_kind: String,
    pub object_id: String,
    /// 対象を置いた bucket の番号。
    pub bucket: u64,
    /// 予定した取り直しの回数(1 から)。
    pub attempts: u32,
}

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
    /// 保持をやめ、その topic の続きの位置と取り直しの予定も消す。
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
    /// 参照する blob を取れなかった対象の、次の取り直しを予定する。予定が無ければ 1 回目を、あれば次の回を予定する。
    /// 5 回目の取り直しも取れなかったときと、予定が合計 `RETENTION_RETRY_LIMIT` 件に達しているときは、予定せずに諦めた数を
    /// 足す。保持していない topic なら何もしない。
    async fn schedule_topic_retention_retry(
        &self,
        topic_id: &str,
        object_kind: &str,
        object_id: &str,
        bucket: u64,
        now_ms: i64,
    ) -> Result<()>;
    /// 期限の来た取り直しを、期限の古い順に `limit` 件まで返す。
    async fn due_topic_retention_retries(
        &self,
        topic_id: &str,
        now_ms: i64,
        limit: usize,
    ) -> Result<Vec<TopicRetentionRetry>>;
    /// 取り直しの予定を消す(取れたとき)。
    async fn finish_topic_retention_retry(
        &self,
        topic_id: &str,
        object_kind: &str,
        object_id: &str,
    ) -> Result<()>;
    /// topic の取り直しを待つ数と、諦めた数。
    async fn topic_retention_retry_counts(&self, topic_id: &str) -> Result<(u64, u64)>;
}
