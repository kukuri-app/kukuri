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

/// n 回目の失敗から次の試行までの待ち時間。最後の値を以後の間隔として使う。
pub(crate) const MISSING_BODY_RETRY_DELAYS_MS: [i64; 4] = [5_000, 30_000, 120_000, 600_000];
/// 本文 blob 1 つあたりの最大試行数。超えた後は、同じ行を指す docs event / hint の個別反映(その場で 1 回試す)と
/// 再起動でだけ取り直す。
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

    /// この replica の全 prefix を次回は必ず反映し直す。
    pub(crate) fn forget_replica(&self, replica: &str) {
        self.fingerprints
            .lock()
            .expect("replica scan cache lock")
            .retain(|(known, _), _| known != replica);
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
    entries: Mutex<HashMap<String, MissingBodyEntry>>,
    fetch_permits: Arc<Semaphore>,
}

impl Default for MissingBodyLedger {
    fn default() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            fetch_permits: Arc::new(Semaphore::new(MISSING_BODY_MAX_CONCURRENT_FETCHES)),
        }
    }
}

impl MissingBodyLedger {
    /// この hash を今 remote へ取りに行ってよいか。`true` を返したときは試行を 1 回消費し、取得中にする。
    pub(crate) fn try_begin(&self, hash: &BlobHash, now_ms: i64) -> bool {
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
                None => return false,
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
            return false;
        }
        entry.attempts += 1;
        entry.in_flight = true;
        true
    }

    pub(crate) fn succeed(&self, hash: &BlobHash) {
        self.entries
            .lock()
            .expect("missing body ledger lock")
            .remove(hash.as_str());
    }

    pub(crate) fn fail(&self, hash: &BlobHash, now_ms: i64) {
        let mut entries = self.entries.lock().expect("missing body ledger lock");
        if let Some(entry) = entries.get_mut(hash.as_str()) {
            entry.in_flight = false;
            let step = (entry.attempts.saturating_sub(1) as usize)
                .min(MISSING_BODY_RETRY_DELAYS_MS.len() - 1);
            entry.next_attempt_at_ms = now_ms.saturating_add(MISSING_BODY_RETRY_DELAYS_MS[step]);
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
        let mut now = 1_000;
        let mut waits = Vec::new();
        for _ in 0..MISSING_BODY_MAX_ATTEMPTS {
            assert!(ledger.try_begin(&target, now));
            assert!(!ledger.try_begin(&target, now), "in flight");
            ledger.fail(&target, now);
            let next = ledger.next_attempt_at(&target).expect("scheduled");
            assert!(!ledger.try_begin(&target, next - 1), "still waiting");
            waits.push(next - now);
            now = next;
        }
        assert_eq!(&waits[..4], &[5_000, 30_000, 120_000, 600_000]);
        assert_eq!(ledger.attempts(&target), MISSING_BODY_MAX_ATTEMPTS);
        assert!(!ledger.try_begin(&target, i64::MAX));
    }

    #[test]
    fn success_forgets_the_hash_and_in_flight_blocks_a_second_attempt() {
        let ledger = MissingBodyLedger::default();
        let target = hash("b");
        assert!(ledger.try_begin(&target, 0));
        assert!(
            !ledger.try_begin(&target, 0),
            "a hash in flight is not started twice"
        );
        assert_eq!(ledger.len(), 1);
        ledger.succeed(&target);
        assert_eq!(ledger.len(), 0);
        assert!(
            ledger.try_begin(&target, 0),
            "a recovered hash starts from scratch"
        );
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
