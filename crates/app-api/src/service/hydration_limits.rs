//! #1225: 欠損した本文 blob の取り直しを有限にするための状態。
//!
//! - `MissingBodyLedger`: 取得できない本文・返信先・session の試行を対象key単位で数え、間隔と回数に上限を置く。
//!
//! replica の全件走査と、その指紋の cache(`ReplicaScanCache`)は #1239 で削除した。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use kukuri_blob_service::RemoteCacheDeferred;
use kukuri_core::BlobHash;
use tokio::sync::Semaphore;

use super::*;

/// n 回目の失敗から次の試行までの待ち時間。最後の値を以後の間隔として使う。
pub(crate) const MISSING_BODY_RETRY_DELAYS_MS: [i64; 3] = [5_000, 30_000, 120_000];
/// 本文 blob 1 つあたりの最大試行数。docs の event・hint の個別反映、利用者の操作の対象の反映、表示した行の
/// 取り直しは、どれもこの台帳を通る。超えた後は、再起動(台帳は memory 上にある)まで取り直さない。
pub(crate) const MISSING_BODY_MAX_ATTEMPTS: u32 = 4;
/// 表示のときに取りに行き始めた本文を待つ時間の上限。件数に依存しない。過ぎた取得は背景で続く。
pub(crate) const MISSING_BODY_DISPLAY_GRACE_MS: u64 = 300;
/// 背景で同時に取り直す本文 blob の上限。
pub(crate) const MISSING_BODY_MAX_CONCURRENT_FETCHES: usize = 4;
/// 台帳の上限。超えた分は、取得中でない項目から捨てる。
pub(crate) const MISSING_BODY_LEDGER_LIMIT: usize = 1_024;
pub(crate) const MISSING_BODY_FAILURE_KEY_MAX_BYTES: usize = 256;

pub(crate) fn display_retry_key(kind: &str, scope: &str, target: &str) -> String {
    let mut hash = blake3::Hasher::new();
    for part in [kind, scope, target] {
        hash.update(&(part.len() as u64).to_be_bytes());
        hash.update(part.as_bytes());
    }
    format!("{kind}:{}", hash.finalize().to_hex())
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

    /// 共通network受付前の延期は試行に数えない。
    pub(crate) fn defer(mut self) {
        self.settled = true;
        let mut entries = self.entries.lock().expect("missing body ledger lock");
        if let Some(entry) = entries.get_mut(&self.hash) {
            entry.in_flight = false;
            entry.attempts = entry.attempts.saturating_sub(1);
            entry.next_attempt_at_ms = 0;
            if entry.attempts == 0 {
                entries.remove(&self.hash);
            }
        }
    }
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
    /// 利用者の明示再試行。自動試行の cooldown / 上限とは別に、この hash を 1 回だけ
    /// `try_begin` できる状態へ置く。取得中の要求は合流させ、台帳の上限も維持する。
    pub(crate) fn request_manual_retry(&self, hash: &BlobHash) -> bool {
        self.request_manual_retry_key(hash.as_str())
    }

    pub(crate) fn request_manual_retry_key(&self, key: &str) -> bool {
        if key.len() > MISSING_BODY_FAILURE_KEY_MAX_BYTES {
            return false;
        }
        let mut entries = self.entries.lock().expect("missing body ledger lock");
        if entries.get(key).is_some_and(|entry| entry.in_flight) {
            return false;
        }
        if !entries.contains_key(key) && entries.len() >= MISSING_BODY_LEDGER_LIMIT {
            let evictable = entries
                .iter()
                .find(|(_, entry)| !entry.in_flight)
                .map(|(key, _)| key.clone());
            let Some(key) = evictable else {
                return false;
            };
            entries.remove(&key);
        }
        entries.insert(
            key.to_string(),
            MissingBodyEntry {
                attempts: MISSING_BODY_MAX_ATTEMPTS.saturating_sub(1),
                next_attempt_at_ms: 0,
                in_flight: false,
            },
        );
        true
    }

    /// この hash を今 remote へ取りに行ってよいか。`Some` を返したときは試行を 1 回消費し、取得中にする。
    pub(crate) fn try_begin(&self, hash: &BlobHash, now_ms: i64) -> Option<MissingBodyAttempt> {
        self.try_begin_key(hash.as_str(), now_ms)
    }

    pub(crate) fn try_begin_key(&self, key: &str, now_ms: i64) -> Option<MissingBodyAttempt> {
        if key.len() > MISSING_BODY_FAILURE_KEY_MAX_BYTES {
            return None;
        }
        let mut entries = self.entries.lock().expect("missing body ledger lock");
        if !entries.contains_key(key) && entries.len() >= MISSING_BODY_LEDGER_LIMIT {
            let evictable = entries
                .iter()
                .find(|(_, entry)| !entry.in_flight)
                .map(|(key, _)| key.clone())?;
            entries.remove(&evictable);
        }
        let entry = entries.entry(key.to_string()).or_insert(MissingBodyEntry {
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
            hash: key.to_string(),
            settled: false,
        })
    }

    /// local から本文を読めた。台帳に残っている記録を消す。
    pub(crate) fn forget(&self, hash: &BlobHash) {
        self.forget_key(hash.as_str());
    }

    pub(crate) fn forget_key(&self, key: &str) {
        let mut entries = self.entries.lock().expect("missing body ledger lock");
        if entries.get(key).is_some_and(|entry| !entry.in_flight) {
            entries.remove(key);
        }
    }

    pub(crate) fn ready_key(&self, key: &str, now_ms: i64) -> bool {
        if key.len() > MISSING_BODY_FAILURE_KEY_MAX_BYTES {
            return false;
        }
        self.entries
            .lock()
            .expect("missing body ledger lock")
            .get(key)
            .is_none_or(|entry| {
                !entry.in_flight
                    && entry.attempts < MISSING_BODY_MAX_ATTEMPTS
                    && entry.next_attempt_at_ms <= now_ms
            })
    }

    pub(crate) fn next_attempt_at_key(&self, key: &str) -> Option<i64> {
        self.entries
            .lock()
            .expect("missing body ledger lock")
            .get(key)
            .filter(|entry| !entry.in_flight && entry.attempts < MISSING_BODY_MAX_ATTEMPTS)
            .map(|entry| entry.next_attempt_at_ms)
    }

    pub(crate) fn display_retry_at_key(&self, key: &str, now_ms: i64) -> Option<i64> {
        if key.len() > MISSING_BODY_FAILURE_KEY_MAX_BYTES {
            return None;
        }
        match self
            .entries
            .lock()
            .expect("missing body ledger lock")
            .get(key)
        {
            Some(entry) if entry.attempts >= MISSING_BODY_MAX_ATTEMPTS && !entry.in_flight => None,
            Some(entry) if entry.in_flight => Some(now_ms.saturating_add(5_000)),
            Some(entry) => Some(entry.next_attempt_at_ms.max(now_ms.saturating_add(1_000))),
            None => Some(now_ms.saturating_add(5_000)),
        }
    }

    /// 共有network受付の延期。失敗回数を増やさず、表示中の再確認期限だけを置く。
    pub(crate) fn defer_key(&self, key: &str, until_ms: i64) {
        if key.len() > MISSING_BODY_FAILURE_KEY_MAX_BYTES {
            return;
        }
        let mut entries = self.entries.lock().expect("missing body ledger lock");
        if !entries.contains_key(key) && entries.len() >= MISSING_BODY_LEDGER_LIMIT {
            if let Some(victim) = entries
                .iter()
                .find(|(_, entry)| !entry.in_flight)
                .map(|(key, _)| key.clone())
            {
                entries.remove(&victim);
            } else {
                return;
            }
        }
        let entry = entries.entry(key.to_owned()).or_insert(MissingBodyEntry {
            attempts: 0,
            next_attempt_at_ms: 0,
            in_flight: false,
        });
        if !entry.in_flight && entry.attempts < MISSING_BODY_MAX_ATTEMPTS {
            entry.next_attempt_at_ms = entry.next_attempt_at_ms.max(until_ms);
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
pub(crate) struct BoundedBody {
    pub(crate) text: String,
    pub(crate) remote_bytes: Option<Vec<u8>>,
    pub(crate) attempt: Option<MissingBodyAttempt>,
}

pub(crate) async fn fetch_projection_blob_text_bounded(
    blob_service: &dyn BlobService,
    missing_bodies: &MissingBodyLedger,
    hash: &kukuri_core::BlobHash,
) -> Option<BoundedBody> {
    let local = matches!(
        best_effort_blob_cache_status(blob_service, hash).await,
        BlobCacheStatus::Available | BlobCacheStatus::Pinned
    );
    if local {
        let payload = fetch_local_projection_blob_text(blob_service, hash).await;
        if payload.is_some() {
            missing_bodies.forget(hash);
        }
        return payload.map(|text| BoundedBody {
            text,
            remote_bytes: None,
            attempt: None,
        });
    }
    let deadline = n0_future::time::Instant::now() + projection_blob_fetch_timeout();
    let fetch = crate::timeout_at(deadline, blob_service.prepare_retry_fetch(hash))
        .await
        .ok()?
        .ok()?;
    // 共有network枠の受付後にだけ試行を数える。abort時はattemptのDropで失敗を記録する。
    let attempt = missing_bodies.try_begin(hash, Utc::now().timestamp_millis())?;
    let bytes = match crate::timeout_at(deadline, fetch).await {
        Ok(Err(error)) if error.is::<RemoteCacheDeferred>() => {
            attempt.defer();
            return None;
        }
        Ok(Ok(bytes)) => bytes,
        Ok(Err(_)) | Err(_) => None,
    };
    match bytes {
        Some(bytes) => Some(BoundedBody {
            text: String::from_utf8_lossy(&bytes).to_string(),
            remote_bytes: Some(bytes),
            attempt: Some(attempt),
        }),
        None => {
            attempt.fail();
            None
        }
    }
}

/// 手元にある本文だけを読む。remote からは取得しない。
pub(crate) async fn fetch_local_projection_blob_text(
    blob_service: &dyn BlobService,
    hash: &kukuri_core::BlobHash,
) -> Option<String> {
    match n0_future::time::timeout(
        projection_blob_fetch_timeout(),
        blob_service.fetch_local_blob(hash),
    )
    .await
    {
        Ok(Ok(Some(bytes))) => Some(String::from_utf8_lossy(&bytes).to_string()),
        Ok(Ok(None)) | Ok(Err(_)) | Err(_) => None,
    }
}

/// 背景の確認(取り下げの確認、返信先の反映)を、同じ確認先で繰り返す間隔(#1239)。
pub(crate) const BACKGROUND_CHECK_INTERVAL_MS: i64 = 60_000;
/// 背景の確認の台帳の上限(台帳ごと)。超えたら、期限の切れた項目を捨て、それでも超えるなら確認を見送る。
pub(crate) const BACKGROUND_CHECK_LEDGER_LIMIT: usize = 4_096;
/// 背景で同時に行う確認の上限(台帳ごと)。
pub(crate) const BACKGROUND_CHECK_MAX_CONCURRENT: usize = 4;

/// view の生成から背景へ出した取り下げ確認を、確認先(replica と object id の組)ごとに間隔を空ける台帳(#1239)。
/// 同じ object を表示し続けても確認は間隔ごとに 1 回で、replica は走査しない。
pub(crate) struct BackgroundCheckLedger {
    next_check_at_ms: Mutex<HashMap<String, i64>>,
    permits: Arc<Semaphore>,
}

impl Default for BackgroundCheckLedger {
    fn default() -> Self {
        Self {
            next_check_at_ms: Mutex::new(HashMap::new()),
            permits: Arc::new(Semaphore::new(BACKGROUND_CHECK_MAX_CONCURRENT)),
        }
    }
}

impl BackgroundCheckLedger {
    /// この確認先を今確認してよいか。`true` を返したときは、次の確認の時刻を先へ進める。
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
        if !entries.contains_key(check_key) && entries.len() >= BACKGROUND_CHECK_LEDGER_LIMIT {
            entries.retain(|_, next_check_at| *next_check_at > now_ms);
            if entries.len() >= BACKGROUND_CHECK_LEDGER_LIMIT {
                return false;
            }
        }
        entries.insert(
            check_key.to_string(),
            now_ms.saturating_add(BACKGROUND_CHECK_INTERVAL_MS),
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
        for _ in 0..4 {
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
        assert_eq!(waits[..3], [5_000, 30_000, 120_000]);
        assert_eq!(ledger.attempts(&target), 4);
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
    fn deferred_admission_does_not_spend_a_retry() {
        let ledger = MissingBodyLedger::default();
        let target = hash("d");
        ledger.try_begin(&target, 0).expect("admitted").defer();
        assert_eq!(ledger.attempts(&target), 0);
        assert!(ledger.try_begin(&target, 0).is_some());
    }

    #[test]
    fn deferred_network_admission_keeps_the_retry_budget() {
        let ledger = MissingBodyLedger::default();
        let key = "session:deferred";
        ledger.defer_key(key, 5_000);
        assert_eq!(ledger.attempts(&BlobHash::new(key)), 0);
        assert!(!ledger.ready_key(key, 4_999));
        assert!(ledger.ready_key(key, 5_000));
    }

    #[test]
    fn display_deadline_follows_real_attempts_and_stops_after_four() {
        let ledger = MissingBodyLedger::default();
        let hash = hash("b");
        assert_eq!(ledger.display_retry_at_key(hash.as_str(), 0), Some(5_000));
        for _ in 0..4 {
            ledger
                .try_begin(&hash, i64::MAX)
                .expect("network attempt")
                .fail();
        }
        assert_eq!(ledger.attempts(&hash), 4);
        assert_eq!(ledger.display_retry_at_key(hash.as_str(), 0), None);
    }

    #[test]
    fn failure_history_and_key_bytes_are_bounded() {
        let ledger = MissingBodyLedger::default();
        for index in 0..10_240 {
            let hash = BlobHash::new(format!("{index:064x}"));
            ledger.try_begin(&hash, 0).expect("admitted").fail();
        }
        assert_eq!(ledger.len(), MISSING_BODY_LEDGER_LIMIT);
        let oversized = BlobHash::new("あ".repeat(100));
        assert!(ledger.try_begin(&oversized, 0).is_none());
        assert!(!ledger.request_manual_retry(&oversized));
        assert_eq!(ledger.len(), MISSING_BODY_LEDGER_LIMIT);
    }

    #[test]
    fn scoped_failure_keys_keep_distinct_authorization_and_target() {
        let first = display_retry_key("reply", "private-epoch-a", "post");
        assert_ne!(first, display_retry_key("reply", "private-epoch-b", "post"));
        assert_ne!(
            first,
            display_retry_key("reply", "private-epoch-a", "other-post")
        );
        assert!(display_retry_key("session", &"scope".repeat(1_000), "id").len() <= 256);
    }

    #[test]
    fn withdrawal_checks_are_spaced_per_object_and_bounded() {
        let ledger = BackgroundCheckLedger::default();
        assert!(ledger.try_begin("object-a", 0));
        assert!(!ledger.try_begin("object-a", BACKGROUND_CHECK_INTERVAL_MS - 1));
        assert!(
            ledger.try_begin("object-b", 0),
            "another object is independent"
        );
        assert!(ledger.try_begin("object-a", BACKGROUND_CHECK_INTERVAL_MS));

        let ledger = BackgroundCheckLedger::default();
        for index in 0..BACKGROUND_CHECK_LEDGER_LIMIT {
            assert!(ledger.try_begin(&format!("object-{index}"), 0));
        }
        assert!(
            !ledger.try_begin("one-too-many", 0),
            "a full ledger defers new checks instead of growing"
        );
        assert_eq!(ledger.len(), BACKGROUND_CHECK_LEDGER_LIMIT);
        assert!(
            ledger.try_begin("one-too-many", BACKGROUND_CHECK_INTERVAL_MS),
            "expired entries make room"
        );
        assert!(ledger.len() <= BACKGROUND_CHECK_LEDGER_LIMIT);
    }
}
