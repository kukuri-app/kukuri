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

/// 保持の期間の既定と上限(日)。期間は内容の作成日(bucket の日)から数える(#1909 判断 1)。
pub const DEFAULT_RETENTION_DAYS: u32 = 30;
pub const MAX_RETENTION_DAYS: u32 = 365;
/// 保持の容量の既定と下限(bytes。判断 2)。
pub const DEFAULT_RETENTION_CAPACITY_BYTES: u64 = 10 << 30;
pub const MIN_RETENTION_CAPACITY_BYTES: u64 = 1 << 30;

/// topic の日の保持の保護参照(remote cache の `ref_id`)。topic の名前は `:` を含みうるので、日は最後の `:` の後ろ。
pub fn retain_reference(topic_id: &str, bucket: u64) -> String {
    format!("retain:{topic_id}:{bucket}")
}

/// 保持の期間と容量と、保護参照を付けた record と blob の大きさの合計。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TopicRetentionLimits {
    pub retention_days: u32,
    pub capacity_bytes: u64,
    pub used_bytes: u64,
}

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
    /// 保持の期間と容量と、保護の合計。
    async fn topic_retention_limits(&self) -> Result<TopicRetentionLimits>;
    /// 保持の期間と容量を変える。
    async fn set_topic_retention_limits(
        &self,
        retention_days: u32,
        capacity_bytes: u64,
    ) -> Result<()>;
    /// topic の `from_bucket`〜`to_bucket` の日のうち、行の無い日の行を作る。日の行は古い側からしか消えないので、既にある
    /// 日より古い側と新しい側だけを作る(容量で保護を外した日とそれより前の日は作らない)。保持していない topic なら作らない。
    /// 停止していた間の日と、期間を延ばして新しく期間に入った日も、ここで最後まで読む日になる。
    async fn open_topic_retention_days(
        &self,
        topic_id: &str,
        from_bucket: u64,
        to_bucket: u64,
    ) -> Result<()>;
    /// 日を読む前に呼ぶ。容量で保護を外した日なら偽を返す。そうでなければ、その日の topic の bucket(`scope_key`)の
    /// record に保護参照を付ける登録をして真を返す。
    async fn protect_topic_retention_day(
        &self,
        topic_id: &str,
        bucket: u64,
        scope_key: &str,
    ) -> Result<bool>;
    /// `from_bucket`〜`to_bucket` のうち、まだ最後まで読んでいない最も新しい日(容量で保護を外した日を除く)。
    async fn next_topic_retention_backfill(
        &self,
        topic_id: &str,
        from_bucket: u64,
        to_bucket: u64,
    ) -> Result<Option<u64>>;
    /// 日を最後まで読んだ。その日の続きの位置を消す(読み終えた日は読み直さない)。
    async fn finish_topic_retention_day(&self, topic_id: &str, bucket: u64) -> Result<()>;
    /// 保持した blob に日の保護参照を付ける(容量で保護を外した日なら付けない)。内容がまだ無くても先に付け、届いた内容を
    /// cache の容量の計数と回収の外にする。
    async fn claim_topic_retention_blob(
        &self,
        topic_id: &str,
        bucket: u64,
        hash: &str,
    ) -> Result<()>;
    /// 保持した author replica の record(`scope_key` の `record_key` の、すべての名義)に日の保護参照を付ける。
    async fn claim_topic_retention_records(
        &self,
        topic_id: &str,
        bucket: u64,
        scope_key: &str,
        record_key: &str,
    ) -> Result<()>;
    /// 登録より前に置かれた、保護する bucket の record に、保護参照を `limit` 件まで付ける。付けた件数を返す。
    async fn protect_topic_retention_scopes_step(&self, limit: usize) -> Result<usize>;
    /// 保護参照を `limit` 件まで外す(外し終えた日も 1 件と数える)。外している途中の日(`cutoff_bucket` より前の日と、
    /// 保持をやめた topic の日)を古い日から外し、無ければ合計が容量を超えている間の最も古い日を外し始める。進めた件数を返す。
    async fn reclaim_topic_retention_step(&self, cutoff_bucket: u64, limit: usize)
    -> Result<usize>;
    /// topic の保護の合計。
    async fn topic_retention_used(&self, topic_id: &str) -> Result<u64>;
}
