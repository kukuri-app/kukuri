//! #1225: replica の全件走査と、欠損した本文 blob の取り直しを有限にするための状態。
//!
//! - `ReplicaScanCache`: 走査した record 群の指紋を prefix ごとに覚え、前回と同じなら projection の
//!   書き直しを省く。走査の戻り値は「replica にある行数」ではなく「今回反映した行数」になる。
//! - `MissingBodyLedger`: 取得できない本文 blob の試行を hash 単位で数え、間隔と回数に上限を置く。

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};

use kukuri_core::BlobHash;
use kukuri_docs_sync::DocRecord;
use tokio::sync::Semaphore;

use super::*;

/// n 回目の失敗から次の試行までの待ち時間。最後の値を以後の間隔として使う。
pub(crate) const MISSING_BODY_RETRY_DELAYS_MS: [i64; 4] = [5_000, 30_000, 120_000, 600_000];
/// 本文 blob 1 つあたりの最大試行数。docs の event・hint の個別反映、利用者の操作の対象の反映、表示した行の
/// 取り直しは、どれもこの台帳を通る。超えた後は、再起動(台帳は memory 上にある)まで取り直さない。
pub(crate) const MISSING_BODY_MAX_ATTEMPTS: u32 = 8;
/// 背景で同時に取り直す本文 blob の上限。
pub(crate) const MISSING_BODY_MAX_CONCURRENT_FETCHES: usize = 4;
/// 台帳の上限。超えた分は、取得中でない項目から捨てる。
pub(crate) const MISSING_BODY_LEDGER_LIMIT: usize = 4_096;

pub(crate) fn scan_fingerprint(records: &[DocRecord]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    records.len().hash(&mut hasher);
    for record in records {
        record.key.hash(&mut hasher);
        record.content_hash.hash(&mut hasher);
        // LocalOnly で本体が未取得の entry と、取得後の entry を区別する。
        record.value.len().hash(&mut hasher);
    }
    hasher.finish()
}

#[derive(Default)]
pub(crate) struct ReplicaScanCache {
    fingerprints: Mutex<HashMap<(String, &'static str), u64>>,
}

impl ReplicaScanCache {
    pub(crate) fn is_unchanged(
        &self,
        replica: &str,
        prefix: &'static str,
        fingerprint: u64,
    ) -> bool {
        self.fingerprints
            .lock()
            .expect("replica scan cache lock")
            .get(&(replica.to_string(), prefix))
            .is_some_and(|known| *known == fingerprint)
    }

    /// 反映が最後まで終わった走査だけを記録する。途中で取りこぼしがあれば記録せず、次回もやり直す。
    pub(crate) fn record(&self, replica: &str, prefix: &'static str, fingerprint: u64) {
        self.fingerprints
            .lock()
            .expect("replica scan cache lock")
            .insert((replica.to_string(), prefix), fingerprint);
    }

    /// 指紋を記録し、前回から変わったかを返す(省略はせず、変化の有無だけを知りたい走査向け)。
    pub(crate) fn observe(&self, replica: &str, prefix: &'static str, fingerprint: u64) -> bool {
        self.fingerprints
            .lock()
            .expect("replica scan cache lock")
            .insert((replica.to_string(), prefix), fingerprint)
            != Some(fingerprint)
    }

    pub(crate) fn forget(&self, replica: &str, prefix: &'static str) {
        self.fingerprints
            .lock()
            .expect("replica scan cache lock")
            .remove(&(replica.to_string(), prefix));
    }
}

#[derive(Clone, Copy, Debug)]
struct MissingBodyEntry {
    attempts: u32,
    next_attempt_at_ms: i64,
    in_flight: bool,
}

pub(crate) struct MissingBodyLedger {
    entries: Arc<Mutex<HashMap<String, MissingBodyEntry>>>,
    fetch_permits: Arc<Semaphore>,
}

impl Default for MissingBodyLedger {
    fn default() -> Self {
        Self {
            entries: Arc::new(Mutex::new(HashMap::new())),
            fetch_permits: Arc::new(Semaphore::new(MISSING_BODY_MAX_CONCURRENT_FETCHES)),
        }
    }
}

/// 取得中の 1 試行。`succeed` を呼ばずに手放すと失敗として記録する。
///
/// 取得を待つ future は、購読タスクの abort や呼び出し側の cancel で途中で破棄されうる。その場合も
/// 「取得中」のまま残さず、次の間隔で取り直せるようにする。
pub(crate) struct MissingBodyAttempt {
    entries: Arc<Mutex<HashMap<String, MissingBodyEntry>>>,
    hash: String,
    settled: bool,
}

impl MissingBodyAttempt {
    pub(crate) fn succeed(mut self) {
        self.settled = true;
        self.entries
            .lock()
            .expect("missing body ledger lock")
            .remove(&self.hash);
    }

    /// 失敗を記録する。何もせずに手放しても同じ結果になる。
    pub(crate) fn fail(self) {}
}

impl Drop for MissingBodyAttempt {
    fn drop(&mut self) {
        if self.settled {
            return;
        }
        let now_ms = chrono::Utc::now().timestamp_millis();
        let mut entries = self.entries.lock().expect("missing body ledger lock");
        if let Some(entry) = entries.get_mut(&self.hash) {
            entry.in_flight = false;
            let step = (entry.attempts.saturating_sub(1) as usize)
                .min(MISSING_BODY_RETRY_DELAYS_MS.len() - 1);
            entry.next_attempt_at_ms = now_ms.saturating_add(MISSING_BODY_RETRY_DELAYS_MS[step]);
        }
    }
}

impl MissingBodyLedger {
    /// この hash を今 remote へ取りに行ってよいか。`Some` を返したときは試行を 1 回消費し、取得中にする。
    pub(crate) fn try_begin(&self, hash: &BlobHash, now_ms: i64) -> Option<MissingBodyAttempt> {
        let mut entries = self.entries.lock().expect("missing body ledger lock");
        if !entries.contains_key(hash.as_str()) && entries.len() >= MISSING_BODY_LEDGER_LIMIT {
            let evictable = entries
                .iter()
                .find(|(_, entry)| !entry.in_flight)
                .map(|(key, _)| key.clone());
            match evictable {
                Some(key) => {
                    entries.remove(&key);
                }
                None => return None,
            }
        }
        let entry = entries
            .entry(hash.as_str().to_string())
            .or_insert(MissingBodyEntry {
                attempts: 0,
                next_attempt_at_ms: 0,
                in_flight: false,
            });
        if entry.in_flight
            || entry.attempts >= MISSING_BODY_MAX_ATTEMPTS
            || entry.next_attempt_at_ms > now_ms
        {
            return None;
        }
        entry.attempts += 1;
        entry.in_flight = true;
        Some(MissingBodyAttempt {
            entries: Arc::clone(&self.entries),
            hash: hash.as_str().to_string(),
            settled: false,
        })
    }

    /// local から本文を読めた。台帳に残っている記録を消す。
    pub(crate) fn forget(&self, hash: &BlobHash) {
        let mut entries = self.entries.lock().expect("missing body ledger lock");
        if entries
            .get(hash.as_str())
            .is_some_and(|entry| !entry.in_flight)
        {
            entries.remove(hash.as_str());
        }
    }

    pub(crate) fn fetch_permits(&self) -> Arc<Semaphore> {
        Arc::clone(&self.fetch_permits)
    }

    #[cfg(test)]
    pub(crate) fn attempts(&self, hash: &BlobHash) -> u32 {
        self.entries
            .lock()
            .expect("missing body ledger lock")
            .get(hash.as_str())
            .map_or(0, |entry| entry.attempts)
    }

    #[cfg(test)]
    pub(crate) fn next_attempt_at(&self, hash: &BlobHash) -> Option<i64> {
        self.entries
            .lock()
            .expect("missing body ledger lock")
            .get(hash.as_str())
            .map(|entry| entry.next_attempt_at_ms)
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.lock().expect("missing body ledger lock").len()
    }
}

/// 走査中の本文取得。local にあれば読み、無ければ台帳の間隔と回数の内でだけ remote を試す。
pub(crate) async fn fetch_projection_blob_text_bounded(
    blob_service: &dyn BlobService,
    missing_bodies: &MissingBodyLedger,
    hash: &kukuri_core::BlobHash,
) -> Option<String> {
    let local = matches!(
        best_effort_blob_cache_status(blob_service, hash).await,
        BlobCacheStatus::Available | BlobCacheStatus::Pinned
    );
    // 取得を待つ間に走査が abort されても、`attempt` の drop で失敗として記録される。
    let attempt = if local {
        None
    } else {
        Some(missing_bodies.try_begin(hash, Utc::now().timestamp_millis())?)
    };
    let payload = fetch_projection_blob_text(blob_service, hash).await;
    match (payload.is_some(), attempt) {
        (true, Some(attempt)) => attempt.succeed(),
        (true, None) => missing_bodies.forget(hash),
        (false, Some(attempt)) => attempt.fail(),
        (false, None) => {}
    }
    payload
}

/// 手元にある本文だけを読む。remote からは取得しない。
pub(crate) async fn fetch_local_projection_blob_text(
    blob_service: &dyn BlobService,
    hash: &kukuri_core::BlobHash,
) -> Option<String> {
    let local = matches!(
        best_effort_blob_cache_status(blob_service, hash).await,
        BlobCacheStatus::Available | BlobCacheStatus::Pinned
    );
    if !local {
        return None;
    }
    fetch_projection_blob_text(blob_service, hash).await
}

/// 購読していない topic の投稿(repost 元、profile の投稿)の取り下げを確認する間隔(#1239)。
pub(crate) const WITHDRAWAL_CHECK_INTERVAL_MS: i64 = 60_000;
/// 取り下げの確認の台帳の上限。超えたら、期限の切れた項目を捨て、それでも超えるなら確認を見送る。
pub(crate) const WITHDRAWAL_CHECK_LEDGER_LIMIT: usize = 4_096;
/// 背景で同時に行う取り下げの確認の上限。
pub(crate) const WITHDRAWAL_CHECK_MAX_CONCURRENT: usize = 4;

/// 表示した投稿の取り下げを、確認先(replica と object id の組)ごとに間隔を空けて確認するための台帳(#1239)。
///
/// view の生成中に docs を読まないため、取り下げの確認は背景へ出す。同じ object を表示し続けても、
/// 確認は間隔ごとに 1 回で、key を指定した読み出しだけを行う(replica は走査しない)。
pub(crate) struct WithdrawalCheckLedger {
    next_check_at_ms: Mutex<HashMap<String, i64>>,
    permits: Arc<Semaphore>,
}

impl Default for WithdrawalCheckLedger {
    fn default() -> Self {
        Self {
            next_check_at_ms: Mutex::new(HashMap::new()),
            permits: Arc::new(Semaphore::new(WITHDRAWAL_CHECK_MAX_CONCURRENT)),
        }
    }
}

impl WithdrawalCheckLedger {
    /// この確認先の取り下げを今確認してよいか。`true` を返したときは、次の確認の時刻を先へ進める。
    ///
    /// `check_key` は replica と object id の組から作る。object id だけにすると、topic を偽った repost の
    /// snapshot が、正しい topic での確認を見送らせてしまう。
    pub(crate) fn try_begin(&self, check_key: &str, now_ms: i64) -> bool {
        let mut entries = self
            .next_check_at_ms
            .lock()
            .expect("withdrawal check ledger lock");
        if entries
            .get(check_key)
            .is_some_and(|next_check_at| *next_check_at > now_ms)
        {
            return false;
        }
        if !entries.contains_key(check_key) && entries.len() >= WITHDRAWAL_CHECK_LEDGER_LIMIT {
            entries.retain(|_, next_check_at| *next_check_at > now_ms);
            if entries.len() >= WITHDRAWAL_CHECK_LEDGER_LIMIT {
                return false;
            }
        }
        entries.insert(
            check_key.to_string(),
            now_ms.saturating_add(WITHDRAWAL_CHECK_INTERVAL_MS),
        );
        true
    }

    pub(crate) fn permits(&self) -> Arc<Semaphore> {
        Arc::clone(&self.permits)
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.next_check_at_ms
            .lock()
            .expect("withdrawal check ledger lock")
            .len()
    }
}

/// hint を契機にした全件走査の門(#1225)。
///
/// replica の内容を指さない hint は契機にしない。内容を指す hint でも、最初の 1 回はすぐ走査し、
/// 以後は最小間隔を空ける(取りこぼしは docs event と recovery tick が拾う)。
#[derive(Default)]
pub(crate) struct HintRecoveryGate {
    next_scan_at_ms: i64,
}

impl HintRecoveryGate {
    pub(crate) fn allow(&mut self, hint: &GossipHint, now_ms: i64) -> bool {
        if !hint_refers_to_replica_content(hint) || self.next_scan_at_ms > now_ms {
            return false;
        }
        self.next_scan_at_ms = now_ms.saturating_add(PUBLIC_TOPIC_RECOVERY_GRACE_MS);
        true
    }
}

/// recovery tick が走査の要否を決めるための peer の状態。
/// 戻り値は (topic に live な peer がいる, topic に設定済みの peer がいる, docs の支援 peer 数)。
pub(crate) async fn recovery_probe_peer_state(
    transport: &dyn Transport,
    docs_sync: &dyn DocsSync,
    topic: &str,
) -> (bool, bool, usize) {
    let (has_live_topic_peer, has_configured_topic_peer) = match transport.peers().await {
        Ok(snapshot) => snapshot
            .topic_diagnostics
            .iter()
            .find(|diagnostic| {
                normalize_topic_name(diagnostic.topic.clone()).as_deref() == Some(topic)
            })
            .map(|diagnostic| {
                (
                    diagnostic.joined && !diagnostic.connected_peers.is_empty(),
                    !diagnostic.configured_peer_ids.is_empty(),
                )
            })
            .unwrap_or((false, false)),
        Err(error) => {
            warn!(
                topic = %topic,
                error = %error,
                "failed to inspect live topic peer state during recovery tick"
            );
            (false, false)
        }
    };
    let docs_assist_peer_count = match docs_sync.assist_peer_ids().await {
        Ok(peer_ids) => peer_ids.len(),
        Err(error) => {
            warn!(
                topic = %topic,
                error = %error,
                "failed to inspect docs-assisted peers during recovery tick"
            );
            0
        }
    };
    (
        has_live_topic_peer,
        has_configured_topic_peer,
        docs_assist_peer_count,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(value: &str) -> BlobHash {
        BlobHash::new(value.repeat(64))
    }

    #[test]
    fn attempts_follow_the_delays_and_stop_at_the_limit() {
        let ledger = MissingBodyLedger::default();
        let target = hash("a");
        let mut now = chrono::Utc::now().timestamp_millis();
        let mut waits = Vec::new();
        for _ in 0..MISSING_BODY_MAX_ATTEMPTS {
            let attempt = ledger.try_begin(&target, now).expect("due");
            assert!(ledger.try_begin(&target, now).is_none(), "in flight");
            let failed_at = chrono::Utc::now().timestamp_millis();
            attempt.fail();
            let next = ledger.next_attempt_at(&target).expect("scheduled");
            assert!(
                ledger.try_begin(&target, next - 1).is_none(),
                "still waiting"
            );
            // 失敗時刻は実時計で記録されるので、待ち時間は 1 秒単位に丸めて比べる。
            waits.push((next - failed_at + 500) / 1_000 * 1_000);
            now = next;
        }
        assert_eq!(&waits[..4], &[5_000, 30_000, 120_000, 600_000]);
        assert_eq!(ledger.attempts(&target), MISSING_BODY_MAX_ATTEMPTS);
        assert!(ledger.try_begin(&target, i64::MAX).is_none());
    }

    #[test]
    fn success_forgets_the_hash_and_in_flight_blocks_a_second_attempt() {
        let ledger = MissingBodyLedger::default();
        let target = hash("b");
        let attempt = ledger.try_begin(&target, 0).expect("due");
        assert!(
            ledger.try_begin(&target, 0).is_none(),
            "a hash in flight is not started twice"
        );
        assert_eq!(ledger.len(), 1);
        attempt.succeed();
        assert_eq!(ledger.len(), 0);
        assert!(
            ledger.try_begin(&target, 0).is_some(),
            "a recovered hash starts from scratch"
        );
    }

    // 取得を待つ future が abort / cancel で破棄されても「取得中」のまま残らず、次の間隔で取り直せる。
    #[test]
    fn an_abandoned_attempt_is_recorded_as_a_failure() {
        let ledger = MissingBodyLedger::default();
        let target = hash("c");
        let attempt = ledger.try_begin(&target, 0).expect("due");
        drop(attempt);
        let next = ledger.next_attempt_at(&target).expect("scheduled");
        assert!(
            ledger.try_begin(&target, next - 1).is_none(),
            "cooling down"
        );
        assert!(
            ledger.try_begin(&target, next).is_some(),
            "an abandoned attempt must not block later attempts"
        );
        assert_eq!(ledger.attempts(&target), 2);
    }

    #[test]
    fn withdrawal_checks_are_spaced_per_object_and_bounded() {
        let ledger = WithdrawalCheckLedger::default();
        assert!(ledger.try_begin("object-a", 0));
        assert!(!ledger.try_begin("object-a", WITHDRAWAL_CHECK_INTERVAL_MS - 1));
        assert!(
            ledger.try_begin("object-b", 0),
            "another object is independent"
        );
        assert!(ledger.try_begin("object-a", WITHDRAWAL_CHECK_INTERVAL_MS));

        let ledger = WithdrawalCheckLedger::default();
        for index in 0..WITHDRAWAL_CHECK_LEDGER_LIMIT {
            assert!(ledger.try_begin(&format!("object-{index}"), 0));
        }
        assert!(
            !ledger.try_begin("one-too-many", 0),
            "a full ledger defers new checks instead of growing"
        );
        assert_eq!(ledger.len(), WITHDRAWAL_CHECK_LEDGER_LIMIT);
        assert!(
            ledger.try_begin("one-too-many", WITHDRAWAL_CHECK_INTERVAL_MS),
            "expired entries make room"
        );
        assert!(ledger.len() <= WITHDRAWAL_CHECK_LEDGER_LIMIT);
    }

    #[test]
    fn fingerprint_changes_with_keys_hashes_and_resolved_bodies() {
        let record = |key: &str, hash: &str, value: &[u8]| DocRecord {
            key: key.into(),
            value: value.to_vec(),
            content_hash: hash.into(),
            content_len: value.len() as u64,
        };
        let base = vec![record("objects/a/state", "h1", b"x")];
        assert_eq!(scan_fingerprint(&base), scan_fingerprint(&base.clone()));
        assert_ne!(
            scan_fingerprint(&base),
            scan_fingerprint(&[record("objects/a/state", "h2", b"x")])
        );
        assert_ne!(
            scan_fingerprint(&base),
            scan_fingerprint(&[record("objects/a/state", "h1", b"")])
        );
        assert_ne!(scan_fingerprint(&base), scan_fingerprint(&[]));
    }
}
