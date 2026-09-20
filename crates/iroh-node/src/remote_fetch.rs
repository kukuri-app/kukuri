//! docs-sync / blob-service 共通のリモートフェッチ本体(WP-B14)。
//!
//! 「local miss → cooldown ゲート → peers 走査 → connect(5s)→ fetch(15s)→
//! ローカル再読込」のループは両 crate で同一アルゴリズム(どちらも
//! `iroh_blobs::ALPN` で connect し `blobs().remote().fetch` で取得)だったため、
//! ここに一本化した。呼び出し側に残るのはローカル取得と policy 判定のみ。
//! ピア台帳・リトライ状態そのものは kukuri-transport の共通実装(WP-H2)。

use std::fmt::Display;
use std::future::Future;
use std::time::Duration;

use anyhow::Result;
use kukuri_transport::{
    PeerAddrBook, PeerConnectionStatus, PeerFetchFailure, RemoteFetchRetryState, RemoteFetchStart,
    RequestRateDecision,
};
use tokio::sync::Mutex;
use tokio::time::{Instant, timeout};
use tracing::{info, warn};

use crate::IrohDocsNode;

pub const REMOTE_FETCH_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
pub const REMOTE_FETCH_TRANSFER_TIMEOUT: Duration = Duration::from_secs(15);
/// One blob must not consume the sum of every peer/candidate timeout. The caller keeps the
/// existing entry and retries later when this budget is exhausted.
pub const REMOTE_FETCH_TOTAL_TIMEOUT: Duration = Duration::from_secs(30);

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
    node: &IrohDocsNode,
    peers: &PeerAddrBook,
    retries: &Mutex<RemoteFetchRetryState>,
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
    node: &IrohDocsNode,
    peers: &PeerAddrBook,
    retries: &Mutex<RemoteFetchRetryState>,
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
    node: &IrohDocsNode,
    peers: &PeerAddrBook,
    retries: &Mutex<RemoteFetchRetryState>,
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
    node: &IrohDocsNode,
    peers: &PeerAddrBook,
    retries: &Mutex<RemoteFetchRetryState>,
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
    match retries.lock().await.try_begin(&retry_key, Instant::now()) {
        RemoteFetchStart::Ready => {}
        RemoteFetchStart::CoolingDown => {
            info!(
                subject,
                hash = %hash_text,
                error = %local_error,
                "remote fetch skipped during retry cooldown"
            );
            return Ok(None);
        }
    }
    let result = match within_remote_fetch_budget(fetch_bytes_from_remote(
        node,
        peers,
        subject,
        hash_text,
        hash,
        local_error,
        mode,
    ))
    .await
    {
        Some(result) => result,
        None => {
            warn!(
                subject,
                hash = %hash_text,
                timeout_ms = REMOTE_FETCH_TOTAL_TIMEOUT.as_millis(),
                "remote fetch exhausted the total attempt budget"
            );
            Ok(None)
        }
    };
    retries
        .lock()
        .await
        .finish(&retry_key, matches!(&result, Ok(Some(_))), Instant::now());
    result
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
}
