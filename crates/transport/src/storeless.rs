//! Web は peer candidate を端末に保存しない（ADR 0056 §5）。native と同じ経路を保ったまま、
//! store を持つ分岐へ入らないようにする、値を持たない型。

use anyhow::Result;

/// 値を作れないので、`Option<Arc<PeerCandidateStore>>` は常に `None` になる。
pub enum PeerCandidateStore {}

impl PeerCandidateStore {
    pub async fn put_peer_candidate(
        &self,
        _scope: &str,
        _source: &str,
        _endpoint_id: &str,
        _addr: &[u8],
        _now_ms: i64,
    ) -> Result<bool> {
        match *self {}
    }

    pub async fn peer_candidate_window(
        &self,
        _scope: &str,
        _source: &str,
        _after: Option<(i64, String)>,
        _limit: usize,
        _now_ms: i64,
    ) -> Result<Vec<(String, Vec<u8>, i64)>> {
        match *self {}
    }

    pub async fn peer_candidate_by_id(
        &self,
        _scope: &str,
        _source: &str,
        _endpoint_id: &str,
        _now_ms: i64,
    ) -> Result<Option<Vec<u8>>> {
        match *self {}
    }

    pub async fn imported_peer_candidate_ids(
        &self,
        _scope: &str,
        _after: Option<&str>,
        _limit: usize,
    ) -> Result<Vec<String>> {
        match *self {}
    }

    pub async fn imported_peer_candidate_window(
        &self,
        _scope: &str,
        _after: Option<&str>,
        _limit: usize,
    ) -> Result<Vec<(String, Vec<u8>)>> {
        match *self {}
    }

    pub async fn replace_seed_candidates(
        &self,
        _scope: &str,
        _seeds: Vec<(String, Vec<u8>)>,
        _now_ms: i64,
    ) -> Result<()> {
        match *self {}
    }
}
