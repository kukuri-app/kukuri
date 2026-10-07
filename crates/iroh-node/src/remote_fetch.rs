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

use anyhow::{Context, Result};
use kukuri_core::{ReceiveOfferScopeV1, VerifiedReceiveOffer};
use kukuri_transport::{
    EndpointAddr, PeerAddrBook, PeerConnectionStatus, PeerFetchFailure, RemoteFetchRetryState,
    RequestRateDecision, SharedRemoteFetchResult, fetch_receive_endpoint_binding,
};
use n0_future::time::{Instant, timeout};
use tokio::sync::Mutex;
use tracing::{info, warn};

use crate::IrohDocsNode;
use crate::network_work::{FetchIdentity, FetchRequest, NetworkWorkRuntime};
use crate::remote_blob::{self, BlobOutput};
use kukuri_transport::work_admission::WorkPersistence;

pub const REMOTE_FETCH_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
pub const REMOTE_FETCH_TRANSFER_TIMEOUT: Duration = Duration::from_secs(15);
/// One blob must not consume the sum of every peer/candidate timeout. The caller keeps the
/// existing entry and retries later when this budget is exhausted.
pub const REMOTE_FETCH_TOTAL_TIMEOUT: Duration = Duration::from_secs(30);
/// 1 要求で試す端末の上限（既知の候補と、発見した候補を合わせて。#1632 D1）。
const MAX_FETCH_PEERS: usize = 4;
/// 1 要求で、発見の stream から読む件数の上限（同じ端末の重複を含む）。
const MAX_PROVIDER_ITEMS: usize = 16;

mod providers;
pub use providers::PublicBlobProviders;
#[cfg(target_family = "wasm")]
pub(crate) use providers::backfill_public_refs;
use providers::public_blob_providers;
#[cfg(not(target_family = "wasm"))]
pub(crate) use providers::{BACKFILL_PAUSE, BACKFILL_ROWS};

async fn within_remote_fetch_budget<T>(future: impl Future<Output = T>) -> Option<T> {
    timeout(REMOTE_FETCH_TOTAL_TIMEOUT, future).await.ok()
}

/// 表示要求が所有する取得。共有walkと合流/切り離しをせず、取消で待機permitとQUIC streamもdropする。
/// bytesの検証だけを行い、保存は表示権限を再確認する呼出元が所有する。
pub type DisplayBlobFetch = std::pin::Pin<Box<dyn Future<Output = Result<Option<Vec<u8>>>> + Send>>;
pub type DisplayBlobFileFetch = std::pin::Pin<Box<dyn Future<Output = Result<Option<u64>>> + Send>>;

// 表示用の file への取得は native だけ（ADR 0056 §5）。
#[cfg(not(target_family = "wasm"))]
mod display_file;
#[cfg(not(target_family = "wasm"))]
pub use display_file::prepare_display_file_fetch;

pub async fn prepare_display_fetch(
    node: &Arc<IrohDocsNode>,
    peers: &Arc<PeerAddrBook>,
    hash: iroh_blobs::Hash,
) -> Result<DisplayBlobFetch> {
    let deadline = Instant::now() + REMOTE_FETCH_TOTAL_TIMEOUT;
    let lease = node
        .network_work
        .acquire(*hash.as_bytes(), deadline)
        .await?;
    let node = node.clone();
    let peers = peers.clone();
    Ok(Box::pin(async move {
        let hash_text = hash.to_string();
        let walk = run_display_fetch(fetch_bytes_from_remote(
            &node,
            &peers,
            "displayed session",
            &hash_text,
            hash,
            "local manifest unavailable",
            FetchMode::Ephemeral,
            None,
        ));
        let result = tokio::select! {
            biased;
            _ = lease.cancelled() => Ok(None),
            result = n0_future::time::timeout(deadline.saturating_duration_since(Instant::now()), walk) => result.unwrap_or(Ok(None)),
        };
        if lease.finish() { result } else { Ok(None) }
    }))
}

async fn run_display_fetch(
    future: impl Future<Output = Result<Option<Vec<u8>>>>,
) -> Result<Option<Vec<u8>>> {
    within_remote_fetch_budget(future).await.unwrap_or(Ok(None))
}

/// One signed provider and one bounded ephemeral manifest. The account binding
/// and Bao content hash are checked on the same selected endpoint, without
/// falling back to the general peer walk or persisting the result.
pub async fn fetch_verified_receive_offer_payload(
    node: &Arc<IrohDocsNode>,
    offer: &VerifiedReceiveOffer,
    provider: EndpointAddr,
) -> Result<Vec<u8>> {
    let reference = offer.reference();
    anyhow::ensure!(
        provider.id.to_string() == reference.provider_endpoint_id,
        "receive offer provider endpoint mismatch"
    );
    let max_bytes = reference.payload_bytes as u64;
    anyhow::ensure!(
        (1..=kukuri_core::RECEIVE_PAYLOAD_MAX_BYTES as u64).contains(&max_bytes),
        "invalid receive offer payload size"
    );
    let hash = crate::parse_blob_hash(reference.payload_hash.as_str())?;
    anyhow::ensure!(
        current_time_ms()? < offer.expires_at_ms(),
        "receive offer expired before fetch"
    );
    let deadline = Instant::now() + REMOTE_FETCH_TOTAL_TIMEOUT;
    let lease = node
        .network_work
        .acquire_bounded_blob(*hash.as_bytes(), max_bytes, deadline)
        .await?;
    let work = async {
        anyhow::ensure!(
            current_time_ms()? < offer.expires_at_ms(),
            "receive offer expired before fetch"
        );
        let binding = fetch_receive_endpoint_binding(
            node.endpoint(),
            provider.clone(),
            offer.sender(),
            deadline,
        )
        .await?;
        anyhow::ensure!(
            binding.endpoint_id() == reference.provider_endpoint_id,
            "receive offer binding provider mismatch"
        );
        let now_ms = current_time_ms()?;
        anyhow::ensure!(
            now_ms < binding.expires_at_ms() && now_ms < offer.expires_at_ms(),
            "receive offer or provider binding expired before payload request"
        );
        let cached = || async {
            timeout(
                REMOTE_FETCH_TRANSFER_TIMEOUT,
                remote_blob::fetch(
                    node.endpoint(),
                    provider.clone(),
                    hash,
                    Some(max_bytes),
                    None,
                ),
            )
            .await
            .context("receive offer cached payload transfer timed out")?
            .and_then(|bytes| bytes.context("receive offer payload missing from provider"))
        };
        let bytes = if matches!(reference.scope, ReceiveOfferScopeV1::PublicSource) {
            match cached().await {
                Ok(bytes) => bytes,
                Err(_) => fetch_offer_sdk_bytes(node.endpoint(), provider, hash, max_bytes).await?,
            }
        } else {
            match fetch_offer_sdk_bytes(node.endpoint(), provider.clone(), hash, max_bytes).await {
                Ok(bytes) => bytes,
                Err(_) => cached().await?,
            }
        };
        anyhow::ensure!(
            bytes.len() as u64 == max_bytes,
            "receive offer payload size mismatch"
        );
        anyhow::ensure!(
            iroh_blobs::Hash::new(&bytes) == hash,
            "receive offer payload hash mismatch"
        );
        let now_ms = current_time_ms()?;
        anyhow::ensure!(
            now_ms < binding.expires_at_ms() && now_ms < offer.expires_at_ms(),
            "receive offer or provider binding expired"
        );
        Ok(bytes)
    };
    let result = tokio::select! {
        biased;
        _ = lease.cancelled() => anyhow::bail!("receive offer payload fetch cancelled"),
        result = n0_future::time::timeout(deadline.saturating_duration_since(Instant::now()), work) => {
            result.context("receive offer payload fetch timed out")?
        }
    };
    anyhow::ensure!(lease.finish(), "receive offer payload fetch scope ended");
    result
}

async fn fetch_offer_sdk_bytes(
    endpoint: &iroh::Endpoint,
    provider: EndpointAddr,
    hash: iroh_blobs::Hash,
    max_bytes: u64,
) -> Result<Vec<u8>> {
    let connection = timeout(
        REMOTE_FETCH_CONNECT_TIMEOUT,
        endpoint.connect(provider, iroh_blobs::ALPN),
    )
    .await
    .context("receive offer provider connect timed out")??;
    let close = CloseOfferConnection(connection.clone());
    let bytes = timeout(
        REMOTE_FETCH_TRANSFER_TIMEOUT,
        fetch_ephemeral(
            connection,
            hash,
            FetchMode::EphemeralBounded(max_bytes),
            None,
        ),
    )
    .await
    .context("receive offer payload transfer timed out")??;
    drop(close);
    Ok(bytes)
}

pub(crate) fn current_time_ms() -> Result<i64> {
    Ok(web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)?
        .as_millis()
        .try_into()?)
}

struct CloseOfferConnection(iroh::endpoint::Connection);

impl Drop for CloseOfferConnection {
    fn drop(&mut self) {
        self.0.close(0u32.into(), b"receive offer payload complete");
    }
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

#[derive(Debug, thiserror::Error)]
#[error("remote cache capacity temporarily unavailable")]
pub struct RemoteCacheDeferred;

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
    cache: Option<&dyn kukuri_store::ContentCacheStore>,
) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    fetch_ephemeral_into(
        connection,
        hash,
        mode,
        cache,
        BlobOutput::Memory(&mut bytes),
    )
    .await?;
    Ok(bytes)
}

#[cfg(not(target_family = "wasm"))]
async fn fetch_ephemeral_to_file(
    connection: iroh::endpoint::Connection,
    hash: iroh_blobs::Hash,
    path: &std::path::Path,
    cache: Option<&dyn kukuri_store::ContentCacheStore>,
) -> Result<u64> {
    let mut file = tokio::fs::File::create(path).await?;
    fetch_ephemeral_into(
        connection,
        hash,
        FetchMode::Ephemeral,
        cache,
        BlobOutput::File(&mut file),
    )
    .await
}

async fn fetch_ephemeral_into(
    connection: iroh::endpoint::Connection,
    hash: iroh_blobs::Hash,
    mode: FetchMode,
    cache: Option<&dyn kukuri_store::ContentCacheStore>,
    mut output: BlobOutput<'_>,
) -> Result<u64> {
    use bao_tree::io::BaoContentItem;
    use futures_util::StreamExt;
    use iroh_blobs::get::request::{GetBlobItem, get_blob};

    let max_bytes = match mode {
        FetchMode::EphemeralBounded(limit) => limit,
        _ => u64::MAX,
    };
    let mut stream = get_blob(connection, hash);
    let mut received = 0u64;
    let mut hasher = blake3::Hasher::new();
    let mut reservation = cache.map(|cache| cache.empty_remote_cache_reservation());
    while let Some(item) = stream.next().await {
        match item {
            GetBlobItem::Item(BaoContentItem::Leaf(leaf)) => {
                let incoming = received.saturating_add(leaf.data.len() as u64);
                if incoming > max_bytes {
                    return Err(BlobTooLarge { limit: max_bytes }.into());
                }
                anyhow::ensure!(
                    leaf.offset == received,
                    "non-contiguous ephemeral blob stream"
                );
                let capacity = cache.map_or(0, |cache| cache.remote_cache_capacity());
                if incoming > capacity {
                    reservation = None;
                } else if let (Some(cache), Some(reservation)) = (cache, reservation.as_mut())
                    && incoming > reservation.bytes()
                {
                    let target = incoming
                        .div_ceil(1024 * 1024)
                        .saturating_mul(1024 * 1024)
                        .min(capacity);
                    let additional = target - reservation.bytes();
                    if !cache
                        .reserve_remote_cache_bytes(reservation, additional)
                        .await?
                    {
                        return Err(RemoteCacheDeferred.into());
                    }
                }
                hasher.update(&leaf.data);
                output.write(&leaf.data).await?;
                received = incoming;
            }
            GetBlobItem::Item(_) => {}
            GetBlobItem::Done(_) => {
                anyhow::ensure!(
                    hasher.finalize().as_bytes() == hash.as_bytes(),
                    "ephemeral blob hash mismatch"
                );
                output.flush().await?;
                return Ok(received);
            }
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
        let subject = bounded_fetch_log_text(subject, 128);
        let hash_text = hash_text.to_owned();
        let local_error = bounded_fetch_log_text(local_error, 4096);
        async move {
            fetch_bytes_from_remote(
                &node,
                &peers,
                &subject,
                &hash_text,
                hash,
                local_error,
                mode,
                None,
            )
            .await
        }
    };
    let Some(result) = run_single_flight(
        &node.network_work,
        retries,
        &retry_key,
        &flight_key,
        subject,
        hash_text,
        mode,
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
        Err(error) if error.is::<RemoteCacheDeferred>() => Err(RemoteCacheDeferred.into()),
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

/// Queue only bounded diagnostic labels; never retain an arbitrarily large
/// formatted error behind a pending fetch. This does not truncate user content.
fn bounded_fetch_log_text(value: impl Display, limit: usize) -> String {
    use std::fmt::Write;
    struct LimitedText {
        value: String,
        limit: usize,
    }
    impl std::fmt::Write for LimitedText {
        fn write_str(&mut self, text: &str) -> std::fmt::Result {
            let remaining = self.limit - self.value.len();
            if text.len() <= remaining {
                self.value.push_str(text);
                return Ok(());
            }
            let mut end = remaining;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            self.value.push_str(&text[..end]);
            Err(std::fmt::Error)
        }
    }
    let mut text = LimitedText {
        value: String::with_capacity(limit),
        limit,
    };
    let _ = write!(&mut text, "{value}");
    text.value
}

/// 同じ対象の走査を 1 本にまとめ、結果を全呼び出しへ配る(#1207 AC-6)。
///
/// 走査は呼び出し側の future から切り離した task で最後まで実行する。呼び出し側が外側の
/// timeout や cancel で待つのをやめても、クールダウンと peer 単位の成否は必ず記録される。
/// 戻り値が `None` のときはクールダウン中で、走査を行っていない。
#[allow(clippy::too_many_arguments)]
async fn run_single_flight<F>(
    admission: &Arc<NetworkWorkRuntime>,
    retries: &Arc<Mutex<RemoteFetchRetryState>>,
    retry_key: &str,
    flight_key: &str,
    subject: &str,
    hash_text: &str,
    mode: FetchMode,
    walk: F,
) -> Option<SharedRemoteFetchResult>
where
    F: Future<Output = Result<Option<Vec<u8>>>> + Send + 'static,
{
    // Hold the retry guard through synchronous admission. Completion records
    // cooldown before retiring the identity, so it cannot race a new attempt.
    let retry_state = retries.lock().await;
    let identity = FetchIdentity {
        service: retry_state.instance_id(),
        key: flight_key.to_owned(),
    };
    let cooling_down = retry_state.is_cooling_down(retry_key, Instant::now());
    let persistence = if mode == FetchMode::Store {
        WorkPersistence::Store
    } else {
        WorkPersistence::Ephemeral
    };
    let byte_limit = match mode {
        FetchMode::EphemeralBounded(limit) => limit,
        _ => u64::MAX,
    };
    let completion_retries = retries.clone();
    let retry_key = retry_key.to_owned();
    let completion_key = flight_key.to_owned();
    let finished = Box::new(move |success| {
        Box::pin(async move {
            completion_retries.lock().await.finish(
                &retry_key,
                &completion_key,
                success,
                Instant::now(),
            );
        }) as std::pin::Pin<Box<dyn Future<Output = ()> + Send>>
    });
    let admitted = admission.submit_fetch(
        FetchRequest {
            identity,
            protocol: kukuri_transport::work_admission::WorkProtocol::Blob,
            object: *iroh_blobs::Hash::new(flight_key.as_bytes()).as_bytes(),
            persistence,
            byte_limit,
            cooling_down,
        },
        Box::pin(walk),
        finished,
    );
    drop(retry_state);
    match admitted {
        Ok(waiter) => Some(waiter.result().await),
        Err(crate::NetworkAdmissionError::CoolingDown) => None,
        Err(error) => {
            info!(subject, hash = %hash_text, %error, "remote fetch not admitted");
            Some(Err(Arc::new(error.into())))
        }
    }
}

enum BlobTransferFailure {
    Missing,
    Rejected,
    Local,
    Remote,
}

fn classify_blob_transfer(error: &iroh_blobs::get::GetError) -> BlobTransferFailure {
    use iroh_blobs::protocol::{ERR_INTERNAL, ERR_LIMIT, ERR_PERMISSION};
    // The pinned provider reports absent data as ERR_INTERNAL, which also
    // covers genuine server faults. Preserve that ambiguity: a stream-level
    // application reply is neither proven NotFound nor a transport failure.
    // Connection-close codes occupy a different namespace, so do not use them.
    if (error.remote_read().is_some() || error.remote_write().is_some())
        && error
            .iroh_error_code()
            .is_some_and(|code| code == ERR_INTERNAL || code == ERR_LIMIT || code == ERR_PERMISSION)
    {
        return BlobTransferFailure::Rejected;
    }
    use iroh_blobs::get::{
        GetError,
        fsm::{AtBlobHeaderNextError, DecodeError},
    };
    match error {
        GetError::AtBlobHeaderNext {
            source: AtBlobHeaderNextError::NotFound { .. },
            ..
        }
        | GetError::Decode {
            source:
                DecodeError::ChunkNotFound { .. }
                | DecodeError::ParentNotFound { .. }
                | DecodeError::LeafNotFound { .. },
            ..
        } => BlobTransferFailure::Missing,
        GetError::LocalFailure { .. }
        | GetError::IrpcSend { .. }
        | GetError::BadRequest { .. }
        | GetError::Decode {
            source: DecodeError::Write { .. },
            ..
        } => BlobTransferFailure::Local,
        _ => BlobTransferFailure::Remote,
    }
}

/// Only an explicit cache miss is NotFound; timeout and protocol/transport
/// errors keep the provider eligible after the hash-specific cooldown.
fn cached_transfer_outcome(
    result: Option<Result<Option<Vec<u8>>>>,
) -> Result<Result<Vec<u8>, PeerFetchFailure>> {
    Ok(match result {
        Some(Ok(Some(bytes))) => Ok(bytes),
        Some(Ok(None)) => Err(PeerFetchFailure::NotFound),
        Some(Err(error)) if error.is::<BlobTooLarge>() || error.is::<RemoteCacheDeferred>() => {
            return Err(error);
        }
        Some(Err(error)) => {
            warn!(%error, "fetch cached transfer failed");
            Err(PeerFetchFailure::TransferFailed)
        }
        None => Err(PeerFetchFailure::TransferTimeout),
    })
}

#[allow(clippy::too_many_arguments)]
async fn fetch_bytes_from_remote(
    node: &IrohDocsNode,
    peers: &PeerAddrBook,
    subject: &str,
    hash_text: &str,
    hash: iroh_blobs::Hash,
    local_error: impl Display,
    mode: FetchMode,
    file_path: Option<&std::path::Path>,
) -> Result<Option<Vec<u8>>> {
    let started = Instant::now();
    let imported_peers = peers.ranked_peers_for(hash_text).await;
    let mut had_transport_failure = false;
    info!(
        subject,
        hash = %hash_text,
        error = %local_error,
        selected_peer_count = imported_peers.len(),
        "fetch local miss, trying remote peers"
    );
    let fetch = PeerFetch {
        node,
        peers,
        subject,
        hash_text,
        hash,
        mode,
        file_path,
    };
    let mut tried = Vec::new();
    for imported_peer in imported_peers {
        tried.push(imported_peer.id);
        if let Some(bytes) = fetch
            .try_peer(&imported_peer, &mut had_transport_failure)
            .await?
        {
            return Ok(Some(bytes));
        }
    }
    // #1632 D1: 既知の候補で取れない公開 blob は、保持端末を探す（native は DHT、Web は Community Node）。
    let budget = REMOTE_FETCH_TOTAL_TIMEOUT.saturating_sub(started.elapsed());
    if let Some(providers) = public_blob_providers(node, hash, budget).await
        && let Some(bytes) = fetch
            .try_providers(providers, &mut tried, &mut had_transport_failure)
            .await?
    {
        return Ok(Some(bytes));
    }
    if had_transport_failure {
        warn!(subject, hash = %hash_text, "fetch exhausted selected peers after transport failures");
    } else {
        info!(subject, hash = %hash_text, "fetch ended without content from the selected peer window");
    }
    Ok(None)
}

/// 1 要求の取得の対象。端末ごとの試行（接続の候補を順に試す）を行う。
struct PeerFetch<'a> {
    node: &'a IrohDocsNode,
    peers: &'a PeerAddrBook,
    subject: &'a str,
    hash_text: &'a str,
    hash: iroh_blobs::Hash,
    mode: FetchMode,
    file_path: Option<&'a std::path::Path>,
}

impl PeerFetch<'_> {
    /// 発見した端末を hash 別の取得元として覚え（10 分。次の試行で先に使う）、端末の枠（既知と合わせて 4 件）が
    /// 残っていればこの要求で試す。stream から読むのは、重複を含めて `MAX_PROVIDER_ITEMS` 件まで。
    async fn try_providers(
        &self,
        providers: impl futures_util::Stream<Item = EndpointAddr> + Unpin,
        tried: &mut Vec<iroh::EndpointId>,
        had_transport_failure: &mut bool,
    ) -> Result<Option<Vec<u8>>> {
        let mut providers = futures_util::StreamExt::take(providers, MAX_PROVIDER_ITEMS);
        let mut found = Vec::new();
        while found.len() < MAX_FETCH_PEERS
            && let Some(provider) = futures_util::StreamExt::next(&mut providers).await
        {
            if provider.id == self.node.endpoint().id() || found.contains(&provider.id) {
                continue;
            }
            found.push(provider.id);
            self.peers
                .note_content_source(self.hash_text, provider.clone())
                .await;
            if tried.len() < MAX_FETCH_PEERS && !tried.contains(&provider.id) {
                tried.push(provider.id);
                if let Some(bytes) = self.try_peer(&provider, had_transport_failure).await? {
                    return Ok(Some(bytes));
                }
            }
        }
        Ok(None)
    }

    /// 端末 1 件から取得する。取れなければ `None`。
    async fn try_peer(
        &self,
        imported_peer: &EndpointAddr,
        had_transport_failure: &mut bool,
    ) -> Result<Option<Vec<u8>>> {
        let Self {
            node,
            peers,
            subject,
            hash_text,
            hash,
            mode,
            file_path,
        } = *self;
        for peer in peers.connect_candidates(imported_peer).await {
            if let RequestRateDecision::Limited { retry_after } =
                peers.record_peer_fetch_request(imported_peer.id).await
            {
                info!(subject, hash = %hash_text, peer_id = %peer.id,
                    retry_after_ms = retry_after.as_millis(), "peer fetch request deferred");
                break;
            }
            let Some(attempt) = peers.begin_fetch_attempt(imported_peer.id).await else {
                break;
            };
            let mut attempt_started = Instant::now();
            let outcome = match timeout(
                REMOTE_FETCH_CONNECT_TIMEOUT,
                node.endpoint().connect(peer.clone(), iroh_blobs::ALPN),
            )
            .await
            {
                Ok(Ok(conn)) => {
                    attempt.connection(PeerConnectionStatus::Connected).await;
                    attempt_started = Instant::now();
                    let transfer = timeout(REMOTE_FETCH_TRANSFER_TIMEOUT, async {
                        match mode {
                            FetchMode::Store => {
                                let blobs = node.blobs().clone();
                                crate::confine_local(async move {
                                    blobs.remote().fetch(conn, hash).await
                                })
                                .await
                                .map(|_| Vec::new())
                                .map_err(anyhow::Error::from)
                            }
                            _ => match file_path {
                                #[cfg(not(target_family = "wasm"))]
                                Some(path) => fetch_ephemeral_to_file(
                                    conn,
                                    hash,
                                    path,
                                    node.remote_cache().map(AsRef::as_ref),
                                )
                                .await
                                .map(|_| Vec::new()),
                                _ => {
                                    fetch_ephemeral(
                                        conn,
                                        hash,
                                        mode,
                                        node.remote_cache().map(AsRef::as_ref),
                                    )
                                    .await
                                }
                            },
                        }
                    })
                    .await;
                    match transfer {
                        Ok(Ok(bytes)) => Ok(bytes),
                        Ok(Err(error)) => {
                            if error.is::<BlobTooLarge>() || error.is::<RemoteCacheDeferred>() {
                                return Err(error);
                            }
                            let failure = match error
                                .downcast_ref::<iroh_blobs::get::GetError>()
                                .map(classify_blob_transfer)
                                .unwrap_or(BlobTransferFailure::Remote)
                            {
                                BlobTransferFailure::Missing => PeerFetchFailure::NotFound,
                                BlobTransferFailure::Rejected => PeerFetchFailure::Rejected,
                                BlobTransferFailure::Local => return Err(error),
                                BlobTransferFailure::Remote => PeerFetchFailure::TransferFailed,
                            };
                            warn!(subject, hash = %hash_text, peer_id = %peer.id, %error,
                                "fetch SDK transfer failed");
                            if mode != FetchMode::Store
                                && matches!(
                                    failure,
                                    PeerFetchFailure::NotFound | PeerFetchFailure::Rejected,
                                )
                            {
                                let limit = match mode {
                                    FetchMode::EphemeralBounded(limit) => Some(limit),
                                    _ => None,
                                };
                                cached_transfer_outcome(
                                    timeout(
                                        REMOTE_FETCH_CONNECT_TIMEOUT
                                            + REMOTE_FETCH_TRANSFER_TIMEOUT,
                                        async {
                                            match file_path {
                                                #[cfg(not(target_family = "wasm"))]
                                                Some(path) => remote_blob::fetch_to_file(
                                                    node.endpoint(),
                                                    peer.clone(),
                                                    hash,
                                                    path,
                                                )
                                                .await
                                                .map(|found| found.map(|_| Vec::new())),
                                                _ => {
                                                    remote_blob::fetch(
                                                        node.endpoint(),
                                                        peer.clone(),
                                                        hash,
                                                        limit,
                                                        node.remote_cache().map(AsRef::as_ref),
                                                    )
                                                    .await
                                                }
                                            }
                                        },
                                    )
                                    .await
                                    .ok(),
                                )?
                            } else {
                                Err(failure)
                            }
                        }
                        Err(_) => Err(PeerFetchFailure::TransferTimeout),
                    }
                }
                Ok(Err(error)) => {
                    warn!(subject, hash = %hash_text, peer_id = %peer.id, %error,
                        "fetch connect failed");
                    Err(PeerFetchFailure::ConnectFailed)
                }
                Err(_) => Err(PeerFetchFailure::ConnectTimeout),
            };
            match outcome {
                Ok(bytes) => {
                    attempt.success(attempt_started.elapsed()).await;
                    peers
                        .record_content_fetch_result(hash_text, peer.id, Ok(()))
                        .await;
                    if mode != FetchMode::Store {
                        return Ok(Some(bytes));
                    }
                    match node.blobs().blobs().get_bytes(hash).await {
                        Ok(bytes) => return Ok(Some(bytes.to_vec())),
                        Err(error) => warn!(subject, hash = %hash_text, peer_id = %peer.id, %error,
                            "fetch transfer completed but content is still missing locally"),
                    }
                }
                Err(failure) => {
                    peers
                        .record_content_fetch_result(hash_text, peer.id, Err(failure))
                        .await;
                    attempt.failure(failure).await;
                    if matches!(
                        failure,
                        PeerFetchFailure::NotFound | PeerFetchFailure::Rejected
                    ) {
                        break;
                    }
                    *had_transport_failure = true;
                    warn!(subject, hash = %hash_text, peer_id = %peer.id, ?failure,
                        "fetch peer attempt failed");
                }
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
#[cfg(not(target_family = "wasm"))]
mod tests;
