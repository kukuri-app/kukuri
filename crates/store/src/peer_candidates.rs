//! 端末に保存する peer の接続候補（transport の候補の台帳）。native は account の `SqliteStore`、Web は IndexedDB が
//! 実装する（ADR 0056 §5、W4 AC-2）。上限と回収は両方で同じ定数を使う。

use anyhow::Result;
use async_trait::async_trait;

/// 学習した候補の出所。時間と容量で回収する（明示の ticket と設定の seed は利用者の入力なので回収しない）。
pub const LEARNED_SOURCE: &str = "learned";
/// 学習した候補を残す期間。
pub const LEARNED_RETENTION_MS: i64 = 30 * 24 * 60 * 60 * 1_000;
/// 学習した候補の合計の上限（`peer_candidate_bytes` の合計）。
pub const LEARNED_BUDGET_BYTES: i64 = 64 * 1024 * 1024;
/// 1 つの候補の住所の上限。
pub const MAX_ADDR_BYTES: usize = 4 * 1024;
/// 期限の過ぎた学習した候補を 1 回に消す数。
pub const LEARNED_PRUNE_STEP: usize = 64;

/// 1 つの候補の行が台帳の容量に数える bytes。
pub fn peer_candidate_bytes(scope: &str, source: &str, endpoint_id: &str, addr: &[u8]) -> i64 {
    (addr.len() + scope.len() + source.len() + endpoint_id.len() + 64) as i64
}

#[async_trait]
pub trait PeerCandidateStore: Send + Sync {
    /// 候補を置く。住所が変わった（新しい）ときは true。学習した候補は、置いた後に期限と容量の上限で古いものから消す。
    async fn put_peer_candidate(
        &self,
        scope: &str,
        source: &str,
        endpoint_id: &str,
        addr: &[u8],
        now_ms: i64,
    ) -> Result<bool>;
    /// (見た時刻, id) の順に `after` の後から最大 `limit`（64 まで）件。末尾に届いたら先頭から折り返す。
    async fn peer_candidate_window(
        &self,
        scope: &str,
        source: &str,
        after: Option<(i64, String)>,
        limit: usize,
        now_ms: i64,
    ) -> Result<Vec<(String, Vec<u8>, i64)>>;
    async fn peer_candidate_by_id(
        &self,
        scope: &str,
        source: &str,
        endpoint_id: &str,
        now_ms: i64,
    ) -> Result<Option<Vec<u8>>>;
    /// 取り込んだ ticket の id を、id の順に `after` の後から最大 `limit`（65 まで）件（折り返さない）。
    async fn imported_peer_candidate_ids(
        &self,
        scope: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<String>>;
    /// 取り込んだ ticket を、id の順に `after` の後から最大 `limit`（4 まで）件。末尾に届いたら先頭から折り返す。
    async fn imported_peer_candidate_window(
        &self,
        scope: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<(String, Vec<u8>)>>;
    /// 設定の seed を置き換える。前回と同じ設定なら何も書かない。
    async fn replace_seed_candidates(
        &self,
        scope: &str,
        seeds: Vec<(String, Vec<u8>)>,
        now_ms: i64,
    ) -> Result<()>;
}
