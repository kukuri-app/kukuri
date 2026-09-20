//! docs-sync / blob-service 共通のリモートフェッチ本体(WP-B14)。
//!
//! 「local miss → cooldown ゲート → peers 走査 → connect(5s)→ fetch(15s)→
//! ローカル再読込」のループは両 crate で同一アルゴリズム(どちらも
//! `iroh_blobs::ALPN` で connect し `blobs().remote().fetch` で取得)だったため、
//! ここに一本化した。呼び出し側に残るのはローカル取得と policy 判定のみ。
//! ピア台帳・リトライ状態そのものは kukuri-transport の共通実装(WP-H2)。

use std::fmt::Display;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use kukuri_transport::{
    PeerAddrBook, PeerConnectionStatus, PeerFetchFailure, RemoteFetchBegin, RemoteFetchRetryState,
    RequestRateDecision, SharedRemoteFetchResult,
};
use tokio::sync::{Mutex, Semaphore};
use tokio::time::{Instant, timeout};
use tracing::{info, warn};

use crate::IrohDocsNode;

pub const REMOTE_FETCH_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
pub const REMOTE_FETCH_TRANSFER_TIMEOUT: Duration = Duration::from_secs(15);
/// One blob must not consume the sum of every peer/candidate timeout. The caller keeps the
/// existing entry and retries later when this budget is exhausted.
pub const REMOTE_FETCH_TOTAL_TIMEOUT: Duration = Duration::from_secs(30);

/// 同時に実行する remote 走査の上限(#1207)。超過分は順番を待つ。
/// 走査は呼び出し側から切り離した task で動くため、task 数をここで有限にする。
const REMOTE_FETCH_MAX_CONCURRENT_WALKS: usize = 8;
static REMOTE_FETCH_WALK_PERMITS: std::sync::LazyLock<Arc<Semaphore>> =
    std::sync::LazyLock::new(|| Arc::new(Semaphore::new(REMOTE_FETCH_MAX_CONCURRENT_WALKS)));

async fn within_remote_fetch_budget<T>(future: impl Future<Output = T>) -> Option<T> {
    timeout(REMOTE_FETCH_TOTAL_TIMEOUT, future).await.ok()
}

/// remote fetch の取得モード。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FetchMode {
    /// ローカルストアへ取り込んでから読む(docs-sync / blob-service の恒久取得)。
    Store,
    /// ストアを経由せず memory に直接取得する(safety scan 用の一時 fetch。#609)。
    Ephemeral,
    EphemeralBounded(u64),
}

#[derive(Debug)]
pub struct BlobTooLarge {
    pub limit: u64,
}

impl std::fmt::Display for BlobTooLarge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "blob exceeds ephemeral byte limit {}", self.limit)
    }
}
impl std::error::Error for BlobTooLarge {}

/// CN scan ingress: stop consuming verified leaves before exceeding the bound.
pub async fn fetch_bytes_ephemeral_bounded_with_cooldown(
    node: &Arc<IrohDocsNode>,
    peers: &Arc<PeerAddrBook>,
    retries: &Arc<Mutex<RemoteFetchRetryState>>,
    hash: iroh_blobs::Hash,
    max_bytes: u64,
) -> Result<Option<Vec<u8>>> {
    fetch_bytes_with_cooldown_mode(
        node,
        peers,
        retries,
        "bounded scan blob",
        &hash.to_string(),
        hash,
        "local blob unavailable",
        FetchMode::EphemeralBounded(max_bytes),
    )
    .await
}

async fn fetch_ephemeral(
    connection: iroh::endpoint::Connection,
    hash: iroh_blobs::Hash,
    mode: FetchMode,
) -> Result<Vec<u8>> {
    use bao_tree::io::BaoContentItem;
    use futures_util::StreamExt;
    use iroh_blobs::get::request::{GetBlobItem, get_blob};
    let max_bytes = match mode {
        FetchMode::EphemeralBounded(limit) => limit,
        _ => u64::MAX,
    };
    let mut stream = get_blob(connection, hash);
    let mut bytes = Vec::new();
    while let Some(item) = stream.next().await {
        match item {
            GetBlobItem::Item(BaoContentItem::Leaf(leaf)) => {
                if (bytes.len() as u64).saturating_add(leaf.data.len() as u64) > max_bytes {
                    return Err(BlobTooLarge { limit: max_bytes }.into());
                }
                anyhow::ensure!(
                    leaf.offset == bytes.len() as u64,
                    "non-contiguous ephemeral blob stream"
                );
                bytes.extend_from_slice(&leaf.data);
            }
            GetBlobItem::Item(_) => {}
            GetBlobItem::Done(_) => return Ok(bytes),
            GetBlobItem::Error(error) => return Err(error.into()),
        }
    }
    anyhow::bail!("incomplete ephemeral blob stream")
}

/// local miss 後のリモートフェッチ一式(cooldown ゲート込み)。
///
/// `subject` はログ上の対象名("docs entry" / "blob")。`local_error` は
/// 呼び出し側のローカル取得が返したエラーで、ログにのみ使う。
/// 成功時はローカルストアへ取り込んだうえで bytes を返す。
pub async fn fetch_bytes_with_cooldown(
    node: &Arc<IrohDocsNode>,
    peers: &Arc<PeerAddrBook>,
    retries: &Arc<Mutex<RemoteFetchRetryState>>,
    subject: &str,
    hash_text: &str,
    hash: iroh_blobs::Hash,
    local_error: impl Display,
) -> Result<Option<Vec<u8>>> {
    fetch_bytes_with_cooldown_mode(
        node,
        peers,
        retries,
        subject,
        hash_text,
        hash,
        local_error,
        FetchMode::Store,
    )
    .await
}

/// `fetch_bytes_with_cooldown` の一時取得版: 取得した bytes をローカルストアへ**書き込まない**。
///
/// safety scan の一時 fetch(#609)用。community node の no-permanent-blob-storage 前提を
/// 構造的に守る(スキャン後の破棄処理が不要になる)。
pub async fn fetch_bytes_ephemeral_with_cooldown(
    node: &Arc<IrohDocsNode>,
    peers: &Arc<PeerAddrBook>,
    retries: &Arc<Mutex<RemoteFetchRetryState>>,
    subject: &str,
    hash_text: &str,
    hash: iroh_blobs::Hash,
    local_error: impl Display,
) -> Result<Option<Vec<u8>>> {
    fetch_bytes_with_cooldown_mode(
        node,
        peers,
        retries,
        subject,
        hash_text,
        hash,
        local_error,
        FetchMode::Ephemeral,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn fetch_bytes_with_cooldown_mode(
    node: &Arc<IrohDocsNode>,
    peers: &Arc<PeerAddrBook>,
    retries: &Arc<Mutex<RemoteFetchRetryState>>,
    subject: &str,
    hash_text: &str,
    hash: iroh_blobs::Hash,
    local_error: impl Display,
    mode: FetchMode,
) -> Result<Option<Vec<u8>>> {
    // A scan's size rejection must not throttle a reader with a different limit.
    let retry_key = match mode {
        FetchMode::EphemeralBounded(limit) => format!("bounded:{limit}:{hash_text}"),
        _ => hash_text.to_owned(),
    };
    // 保存先が違う取得を合流させない。永続取得へ一時取得が合流すると、保存しないはずの
    // bytes がローカルストアへ残る(#1207 INVAR-2)。
    let flight_key = match mode {
        FetchMode::Store => format!("store:{hash_text}"),
        FetchMode::Ephemeral => format!("ephemeral:{hash_text}"),
        FetchMode::EphemeralBounded(_) => retry_key.clone(),
    };
    let walk = {
        let node = Arc::clone(node);
        let peers = Arc::clone(peers);
        let subject = subject.to_owned();
        let hash_text = hash_text.to_owned();
        let local_error = local_error.to_string();
        async move {
            fetch_bytes_from_remote(&node, &peers, &subject, &hash_text, hash, local_error, mode)
                .await
        }
    };
    let Some(result) = run_single_flight(
        retries,
        &REMOTE_FETCH_WALK_PERMITS,
        &retry_key,
        &flight_key,
        subject,
        hash_text,
        walk,
    )
    .await
    else {
        info!(
            subject,
            hash = %hash_text,
            "remote fetch skipped during retry cooldown"
        );
        return Ok(None);
    };
    match result {
        Ok(bytes) => Ok(bytes.map(Arc::unwrap_or_clone)),
        Err(error) => match error.downcast_ref::<BlobTooLarge>() {
            // 呼び出し側(CN scan)が型で判定するため、合流した側にも同じ型で返す。
            Some(too_large) => Err(BlobTooLarge {
                limit: too_large.limit,
            }
            .into()),
            None => Err(anyhow::anyhow!("{error:#}")),
        },
    }
}

/// 同じ対象の走査を 1 本にまとめ、結果を全呼び出しへ配る(#1207 AC-6)。
///
/// 走査は呼び出し側の future から切り離した task で最後まで実行する。呼び出し側が外側の
/// timeout や cancel で待つのをやめても、クールダウンと peer 単位の成否は必ず記録される。
/// 戻り値が `None` のときはクールダウン中で、走査を行っていない。
async fn run_single_flight<F>(
    retries: &Arc<Mutex<RemoteFetchRetryState>>,
    permits: &Arc<Semaphore>,
    retry_key: &str,
    flight_key: &str,
    subject: &str,
    hash_text: &str,
    walk: F,
) -> Option<SharedRemoteFetchResult>
where
    F: Future<Output = Result<Option<Vec<u8>>>> + Send + 'static,
{
    let mut receiver = match retries
        .lock()
        .await
        .begin(retry_key, flight_key, Instant::now())
    {
        RemoteFetchBegin::CoolingDown => return None,
        RemoteFetchBegin::Join(receiver) => receiver,
        RemoteFetchBegin::Lead(sender) => {
            let receiver = sender.subscribe();
            let retries = Arc::clone(retries);
            let permits = Arc::clone(permits);
            let retry_key = retry_key.to_owned();
            let flight_key = flight_key.to_owned();
            let subject = subject.to_owned();
            let hash_text = hash_text.to_owned();
            tokio::spawn(async move {
                let result = match permits.acquire().await {
                    Ok(_permit) => match within_remote_fetch_budget(walk).await {
                        Some(result) => result,
                        None => {
                            warn!(
                                subject = %subject,
                                hash = %hash_text,
                                timeout_ms = REMOTE_FETCH_TOTAL_TIMEOUT.as_millis(),
                                "remote fetch exhausted the total attempt budget"
                            );
                            Ok(None)
                        }
                    },
                    Err(_) => Ok(None),
                };
                retries.lock().await.finish(
                    &retry_key,
                    &flight_key,
                    matches!(&result, Ok(Some(_))),
                    Instant::now(),
                );
                let _ = sender.send(Some(
                    result.map(|bytes| bytes.map(Arc::new)).map_err(Arc::new),
                ));
            });
            receiver
        }
    };
    match receiver.wait_for(Option::is_some).await {
        Ok(result) => result.clone(),
        // 走査 task が結果を流さずに終わった(異常終了)。取得できなかったものとして扱う。
        Err(_) => Some(Ok(None)),
    }
}

async fn fetch_bytes_from_remote(
    node: &IrohDocsNode,
    peers: &PeerAddrBook,
    subject: &str,
    hash_text: &str,
    hash: iroh_blobs::Hash,
    local_error: impl Display,
    mode: FetchMode,
) -> Result<Option<Vec<u8>>> {
    let imported_peers = peers.ranked_peers().await;
    info!(
        subject,
        hash = %hash_text,
        error = %local_error,
        configured_peer_count = imported_peers.len(),
        "fetch local miss, trying remote peers"
    );
    for imported_peer in imported_peers {
        let candidates = peers.connect_candidates(&imported_peer).await;
        info!(
            subject,
            hash = %hash_text,
            peer_id = %imported_peer.id,
            imported_addrs = ?imported_peer.addrs,
            candidate_count = candidates.len(),
            "fetch prepared remote peer candidates"
        );
        for peer in candidates {
            if let RequestRateDecision::Limited { retry_after } =
                peers.record_peer_fetch_request(imported_peer.id).await
            {
                info!(
                    subject,
                    hash = %hash_text,
                    peer_id = %imported_peer.id,
                    retry_after_ms = retry_after.as_millis(),
                    "peer fetch request deferred by the shared request-frequency ledger"
                );
                break;
            }
            let connection_generation = peers.begin_connection_attempt(imported_peer.id).await;
            match timeout(
                REMOTE_FETCH_CONNECT_TIMEOUT,
                node.endpoint().connect(peer.clone(), iroh_blobs::ALPN),
            )
            .await
            {
                Ok(Ok(conn)) => {
                    peers
                        .record_connection_state(
                            imported_peer.id,
                            connection_generation,
                            PeerConnectionStatus::Connected,
                        )
                        .await;
                    info!(
                        subject,
                        hash = %hash_text,
                        peer_id = %peer.id,
                        addrs = ?peer.addrs,
                        "fetch connected to remote peer"
                    );
                    match mode {
                        FetchMode::Store => {
                            let transfer_started = Instant::now();
                            match timeout(
                                REMOTE_FETCH_TRANSFER_TIMEOUT,
                                node.blobs().remote().fetch(conn, hash),
                            )
                            .await
                            {
                                Ok(Ok(_)) => {
                                    peers
                                        .record_fetch_success(
                                            imported_peer.id,
                                            transfer_started.elapsed(),
                                        )
                                        .await;
                                    info!(
                                        subject,
                                        hash = %hash_text,
                                        peer_id = %peer.id,
                                        "fetch remote transfer completed"
                                    );
                                }
                                Ok(Err(error)) => {
                                    peers
                                        .record_fetch_failure(
                                            imported_peer.id,
                                            PeerFetchFailure::TransferFailed,
                                        )
                                        .await;
                                    warn!(
                                        subject,
                                        hash = %hash_text,
                                        peer_id = %peer.id,
                                        addrs = ?peer.addrs,
                                        error = %error,
                                        "fetch remote transfer failed"
                                    );
                                    continue;
                                }
                                Err(_) => {
                                    peers
                                        .record_fetch_failure(
                                            imported_peer.id,
                                            PeerFetchFailure::TransferTimeout,
                                        )
                                        .await;
                                    warn!(
                                        subject,
                                        hash = %hash_text,
                                        peer_id = %peer.id,
                                        addrs = ?peer.addrs,
                                        timeout_ms = REMOTE_FETCH_TRANSFER_TIMEOUT.as_millis(),
                                        "fetch remote transfer timed out"
                                    );
                                    continue;
                                }
                            }
                            match node.blobs().blobs().get_bytes(hash).await {
                                Ok(bytes) => return Ok(Some(bytes.to_vec())),
                                Err(error) => {
                                    warn!(
                                        subject,
                                        hash = %hash_text,
                                        peer_id = %peer.id,
                                        error = %error,
                                        "fetch transfer completed but content is still missing locally"
                                    );
                                }
                            }
                        }
                        FetchMode::Ephemeral | FetchMode::EphemeralBounded(_) => {
                            // ストアへ書き込まず、検証付きで memory へ直接取得する。
                            let transfer_started = Instant::now();
                            match timeout(
                                REMOTE_FETCH_TRANSFER_TIMEOUT,
                                fetch_ephemeral(conn, hash, mode),
                            )
                            .await
                            {
                                Ok(Ok(bytes)) => {
                                    peers
                                        .record_fetch_success(
                                            imported_peer.id,
                                            transfer_started.elapsed(),
                                        )
                                        .await;
                                    info!(
                                        subject,
                                        hash = %hash_text,
                                        peer_id = %peer.id,
                                        "ephemeral fetch remote transfer completed"
                                    );
                                    return Ok(Some(bytes));
                                }
                                Ok(Err(error)) => {
                                    if error.is::<BlobTooLarge>() {
                                        return Err(error);
                                    }
                                    peers
                                        .record_fetch_failure(
                                            imported_peer.id,
                                            PeerFetchFailure::TransferFailed,
                                        )
                                        .await;
                                    warn!(
                                        subject,
                                        hash = %hash_text,
                                        peer_id = %peer.id,
                                        addrs = ?peer.addrs,
                                        error = %error,
                                        "ephemeral fetch remote transfer failed"
                                    );
                                    continue;
                                }
                                Err(_) => {
                                    peers
                                        .record_fetch_failure(
                                            imported_peer.id,
                                            PeerFetchFailure::TransferTimeout,
                                        )
                                        .await;
                                    warn!(
                                        subject,
                                        hash = %hash_text,
                                        peer_id = %peer.id,
                                        addrs = ?peer.addrs,
                                        timeout_ms = REMOTE_FETCH_TRANSFER_TIMEOUT.as_millis(),
                                        "ephemeral fetch remote transfer timed out"
                                    );
                                    continue;
                                }
                            }
                        }
                    }
                }
                Ok(Err(error)) => {
                    peers
                        .record_fetch_failure(imported_peer.id, PeerFetchFailure::ConnectFailed)
                        .await;
                    warn!(
                        subject,
                        hash = %hash_text,
                        peer_id = %peer.id,
                        addrs = ?peer.addrs,
                        error = %error,
                        "fetch connect failed"
                    );
                }
                Err(_) => {
                    peers
                        .record_fetch_failure(imported_peer.id, PeerFetchFailure::ConnectTimeout)
                        .await;
                    warn!(
                        subject,
                        hash = %hash_text,
                        peer_id = %peer.id,
                        addrs = ?peer.addrs,
                        timeout_ms = REMOTE_FETCH_CONNECT_TIMEOUT.as_millis(),
                        "fetch connect timed out"
                    );
                }
            }
        }
    }
    warn!(
        subject,
        hash = %hash_text,
        "fetch exhausted remote peers without success"
    );
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn total_budget_cancels_a_fetch_that_never_completes() {
        let result = within_remote_fetch_budget(async {
            tokio::time::sleep(REMOTE_FETCH_TOTAL_TIMEOUT + Duration::from_secs(1)).await;
            1_u8
        })
        .await;
        assert_eq!(result, None);
    }

    use std::sync::atomic::{AtomicUsize, Ordering};

    fn retries() -> Arc<Mutex<RemoteFetchRetryState>> {
        Arc::new(Mutex::new(RemoteFetchRetryState::default()))
    }

    /// 応答しない peer を模した走査。開始回数だけを数え、総予算まで完了しない。
    fn stalled_walk(
        started: &Arc<AtomicUsize>,
    ) -> impl Future<Output = Result<Option<Vec<u8>>>> + Send + 'static {
        let started = Arc::clone(started);
        async move {
            started.fetch_add(1, Ordering::SeqCst);
            std::future::pending::<()>().await;
            Ok(None)
        }
    }

    // #1207 TR-10: 呼び出し側が外側の timeout で待つのをやめても、失敗のクールダウンが残る。
    #[tokio::test(start_paused = true)]
    async fn dropped_caller_still_records_the_failure_cooldown() {
        let retries = retries();
        let permits = Arc::new(Semaphore::new(REMOTE_FETCH_MAX_CONCURRENT_WALKS));
        let started = Arc::new(AtomicUsize::new(0));

        let abandoned = timeout(
            Duration::from_secs(2),
            run_single_flight(
                &retries,
                &permits,
                "hash-a",
                "store:hash-a",
                "blob",
                "hash-a",
                stalled_walk(&started),
            ),
        )
        .await;
        assert!(
            abandoned.is_err(),
            "the caller gives up before the walk ends"
        );

        // 走査の総予算が尽きるまで進める。走査 task は呼び出し側と無関係に終わる。
        tokio::time::sleep(REMOTE_FETCH_TOTAL_TIMEOUT).await;
        tokio::task::yield_now().await;
        assert_eq!(retries.lock().await.in_flight_len(), 0);

        let next = run_single_flight(
            &retries,
            &permits,
            "hash-a",
            "store:hash-a",
            "blob",
            "hash-a",
            stalled_walk(&started),
        )
        .await;
        assert!(next.is_none(), "the next call must be inside the cooldown");
        assert_eq!(started.load(Ordering::SeqCst), 1);
    }

    // #1207 TR-10 / TR-11: 待つのをやめた直後の再要求は、実行中の走査へ合流し新しい走査を始めない。
    #[tokio::test(start_paused = true)]
    async fn repeated_callers_join_the_walk_in_flight() {
        let retries = retries();
        let permits = Arc::new(Semaphore::new(REMOTE_FETCH_MAX_CONCURRENT_WALKS));
        let started = Arc::new(AtomicUsize::new(0));

        for _ in 0..5 {
            let attempt = timeout(
                Duration::from_secs(3),
                run_single_flight(
                    &retries,
                    &permits,
                    "hash-a",
                    "store:hash-a",
                    "blob",
                    "hash-a",
                    stalled_walk(&started),
                ),
            )
            .await;
            assert!(attempt.is_err());
        }
        assert_eq!(started.load(Ordering::SeqCst), 1);
        assert_eq!(retries.lock().await.in_flight_len(), 1);
    }

    // #1207 TR-11: 合流した全呼び出しが同じ結果を受け取り、成功後は予約もクールダウンも残らない。
    #[tokio::test(start_paused = true)]
    async fn joined_callers_share_one_result() {
        let retries = retries();
        let permits = Arc::new(Semaphore::new(REMOTE_FETCH_MAX_CONCURRENT_WALKS));
        let started = Arc::new(AtomicUsize::new(0));
        let walk = |started: &Arc<AtomicUsize>| {
            let started = Arc::clone(started);
            async move {
                started.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_secs(1)).await;
                Ok(Some(vec![1_u8, 2, 3]))
            }
        };

        let (first, second) = tokio::join!(
            run_single_flight(
                &retries,
                &permits,
                "hash-a",
                "store:hash-a",
                "blob",
                "hash-a",
                walk(&started)
            ),
            run_single_flight(
                &retries,
                &permits,
                "hash-a",
                "store:hash-a",
                "blob",
                "hash-a",
                walk(&started)
            ),
        );
        for result in [first, second] {
            let bytes = result.expect("not cooling down").expect("walk succeeded");
            assert_eq!(bytes.as_deref(), Some(&vec![1_u8, 2, 3]));
        }
        assert_eq!(started.load(Ordering::SeqCst), 1);
        let state = retries.lock().await;
        assert_eq!(state.in_flight_len(), 0);
        assert_eq!(state.cooldown_len(), 0);
    }

    // #1207 INVAR-2: 永続取得と一時取得は合流しない(一時取得の bytes を保存経路へ混ぜない)。
    #[tokio::test(start_paused = true)]
    async fn store_and_ephemeral_walks_do_not_join() {
        let retries = retries();
        let permits = Arc::new(Semaphore::new(REMOTE_FETCH_MAX_CONCURRENT_WALKS));
        let started = Arc::new(AtomicUsize::new(0));
        for flight_key in ["store:hash-a", "ephemeral:hash-a"] {
            let attempt = timeout(
                Duration::from_secs(1),
                run_single_flight(
                    &retries,
                    &permits,
                    "hash-a",
                    flight_key,
                    "blob",
                    "hash-a",
                    stalled_walk(&started),
                ),
            )
            .await;
            assert!(attempt.is_err());
        }
        assert_eq!(started.load(Ordering::SeqCst), 2);
    }

    // #1207 INVAR-3: 同時に動く走査は上限までで、超過分は先行の終了を待つ。
    #[tokio::test(start_paused = true)]
    async fn concurrent_walks_are_bounded() {
        let retries = retries();
        let permits = Arc::new(Semaphore::new(REMOTE_FETCH_MAX_CONCURRENT_WALKS));
        let started = Arc::new(AtomicUsize::new(0));
        let total = REMOTE_FETCH_MAX_CONCURRENT_WALKS + 3;
        for index in 0..total {
            let key = format!("hash-{index}");
            let flight_key = format!("store:{key}");
            let attempt = timeout(
                Duration::from_millis(10),
                run_single_flight(
                    &retries,
                    &permits,
                    &key,
                    &flight_key,
                    "blob",
                    &key,
                    stalled_walk(&started),
                ),
            )
            .await;
            assert!(attempt.is_err());
        }
        assert_eq!(
            started.load(Ordering::SeqCst),
            REMOTE_FETCH_MAX_CONCURRENT_WALKS
        );

        tokio::time::sleep(REMOTE_FETCH_TOTAL_TIMEOUT).await;
        tokio::task::yield_now().await;
        assert_eq!(started.load(Ordering::SeqCst), total);
    }

    // 合流した側にも、大きさ超過を同じ型で返す(CN scan が型で判定する)。
    #[tokio::test(start_paused = true)]
    async fn shared_error_keeps_the_too_large_type() {
        let retries = retries();
        let permits = Arc::new(Semaphore::new(REMOTE_FETCH_MAX_CONCURRENT_WALKS));
        let result = run_single_flight(
            &retries,
            &permits,
            "bounded:8:hash-a",
            "bounded:8:hash-a",
            "blob",
            "hash-a",
            async { Err(BlobTooLarge { limit: 8 }.into()) },
        )
        .await
        .expect("not cooling down");
        let error = result.expect_err("walk failed");
        assert!(error.downcast_ref::<BlobTooLarge>().is_some());
    }
}
