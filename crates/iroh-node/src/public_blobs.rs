//! 公開 blob の発見（#1632、ADR 0063）。node と同じ寿命の DHT と補助 index の client で、公開 blob の保持端末を
//! Mainline から探し、手元に保持する公開 blob を告知する。
//!
//! 補助 index が見つかるまで（一覧が引けなければ 30 秒ごとにやり直す）、検索も告知もしない。告知は store の告知の
//! 予定から時刻の来た hash を少しずつ行い、取得と同じ実行枠を、表示・操作の取得より後の lane で使う。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use iroh::{EndpointId, SecretKey};
use iroh_mainline_endpoint_discovery::{AddrIndex, Announcer, Resolver, infohash_from_blake3};
use kukuri_store::ContentCacheStore;
use kukuri_transport::PublicBlobIndex;
use n0_future::task::AbortOnDropHandle;
use n0_future::time::{Instant, sleep, timeout};
use n0_mainline::Dht;
use tokio::sync::watch;
use tracing::{debug, warn};

use crate::network_work::NetworkWorkRuntime;
use crate::remote_fetch::current_time_ms;

/// 補助 index の一覧が引けないときに、やり直すまでの間。
const INDEX_RETRY: Duration = Duration::from_secs(30);
/// 補助 index の照会の結果を覚える時間。
const INDEX_LOOKUP_CACHE: Duration = Duration::from_secs(5 * 60);
/// 告知を更新する間隔と、hash ごとに時刻をずらす幅。
const ANNOUNCE_INTERVAL_MS: i64 = 10 * 60 * 1000;
const ANNOUNCE_JITTER_MS: i64 = 60 * 1000;
/// 告知に失敗した hash を、やり直すまでの間。
const ANNOUNCE_RETRY: Duration = Duration::from_secs(30);
/// 1 件の告知の期限（受付の待ちを含む）。
const ANNOUNCE_TIMEOUT: Duration = Duration::from_secs(30);
/// 時刻の来た告知が無いとき、新しく予定に載った hash を見に行くまでの最長の間。
const ANNOUNCE_IDLE: Duration = Duration::from_secs(5);
/// 同時に行う告知の数。
const ANNOUNCE_CONCURRENCY: usize = 2;
/// 導入前の公開記録の取込みで、1 回に読む行数（種類ごと）と、次の取込みまでの間。
const BACKFILL_ROWS: usize = 128;
const BACKFILL_PAUSE: Duration = Duration::from_millis(100);

struct Ready {
    resolver: Resolver,
    announcer: Announcer,
}

/// node が持つ発見の部品。落とすと検索・告知・補助 index の client の task が止まる。
pub(crate) struct PublicBlobDiscovery {
    ready: watch::Receiver<Option<Arc<Ready>>>,
    tasks: Mutex<Vec<AbortOnDropHandle<()>>>,
    #[cfg(test)]
    pub(crate) lookups: std::sync::atomic::AtomicUsize,
}

impl PublicBlobDiscovery {
    /// 補助 index の client を組み立て始める。組み立てが済むまで、検索と告知はしない。
    pub(crate) fn start(dht: Dht, index: PublicBlobIndex, secret: SecretKey) -> Self {
        let (sender, ready) = watch::channel(None);
        let task = n0_future::task::spawn(async move {
            let addr_index = loop {
                let builder = AddrIndex::builder(dht.clone()).lookup_cache(INDEX_LOOKUP_CACHE);
                let builder = match &index {
                    PublicBlobIndex::ListKey(key) => builder.list_key(*key),
                    PublicBlobIndex::Servers(servers) => servers
                        .iter()
                        .fold(builder, |builder, server| builder.server(*server)),
                };
                match builder.build().await {
                    Ok(addr_index) => break addr_index,
                    Err(error) => {
                        debug!(%error, "public blob index unavailable");
                        sleep(INDEX_RETRY).await;
                    }
                }
            };
            sender.send_replace(Some(Arc::new(Ready {
                resolver: Resolver::new(dht.clone(), addr_index.clone()),
                announcer: Announcer::new(secret, dht, addr_index),
            })));
        });
        Self {
            ready,
            tasks: Mutex::new(vec![AbortOnDropHandle::new(task)]),
            #[cfg(test)]
            lookups: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// `hash` を告知した端末を、見つかった順に返す。補助 index がまだ無ければ `None`。stream を落とすと検索は止まる。
    pub(crate) fn providers(
        &self,
        hash: iroh_blobs::Hash,
    ) -> Option<n0_future::stream::Boxed<EndpointId>> {
        let ready = self.ready.borrow().clone()?;
        #[cfg(test)]
        self.lookups
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Some(ready.resolver.resolve_stream(infohash(hash)))
    }

    /// 補助 index の client が組み上がるまで待つ。
    #[cfg(test)]
    pub(crate) async fn ready(&self) {
        let _ = self.ready.clone().wait_for(Option::is_some).await;
    }

    /// 告知の予定（`cache`）から、時刻の来た hash を告知し続ける task を始める。
    pub(crate) fn start_announcing(
        &self,
        cache: Arc<dyn ContentCacheStore>,
        work: Arc<NetworkWorkRuntime>,
    ) {
        let mut ready = self.ready.clone();
        let task = n0_future::task::spawn(async move {
            let Ok(ready) = ready
                .wait_for(Option::is_some)
                .await
                .map(|ready| ready.clone())
            else {
                return;
            };
            if let Some(ready) = ready {
                announce_loop(&ready, cache.as_ref(), &work).await;
            }
        });
        self.tasks
            .lock()
            .expect("public blob discovery tasks poisoned")
            .push(AbortOnDropHandle::new(task));
    }
}

fn infohash(hash: iroh_blobs::Hash) -> n0_mainline::Id {
    infohash_from_blake3(&blake3::Hash::from_bytes(*hash.as_bytes())).into()
}

async fn announce_loop(
    ready: &Ready,
    cache: &dyn ContentCacheStore,
    work: &Arc<NetworkWorkRuntime>,
) {
    // 前の告知の成功（再起動・復元の前のもの）は信頼しない。予定のすべてを、すぐ告知し直す。
    if let Err(error) = async {
        cache
            .restart_public_blob_announcements(current_time_ms()?)
            .await
    }
    .await
    {
        warn!(%error, "failed to restart public blob announcements");
    }
    let mut backfilled = false;
    let mut announce_at = Instant::now();
    loop {
        if !backfilled {
            match cache.backfill_public_blob_refs_step(BACKFILL_ROWS).await {
                Ok(done) => backfilled = done,
                Err(error) => warn!(%error, "public blob reference backfill failed"),
            }
        }
        // 取込みは 100ms ごとに進め、告知は失敗の後や予定の待ちを守る。
        if Instant::now() >= announce_at {
            let wait = match announce_due(ready, cache, work).await {
                Ok(wait) => wait,
                Err(error) => {
                    warn!(%error, "public blob announcement failed");
                    ANNOUNCE_IDLE
                }
            };
            announce_at = Instant::now() + wait;
        }
        let wait = announce_at.saturating_duration_since(Instant::now());
        sleep(if backfilled {
            wait
        } else {
            wait.min(BACKFILL_PAUSE)
        })
        .await;
    }
}

/// 時刻の来た告知を行い、次に見に行くまでの間を返す。
async fn announce_due(
    ready: &Ready,
    cache: &dyn ContentCacheStore,
    work: &Arc<NetworkWorkRuntime>,
) -> anyhow::Result<Duration> {
    let now = current_time_ms()?;
    let (due, next_at) = cache
        .due_public_blob_announcements(now, ANNOUNCE_CONCURRENCY)
        .await?;
    if due.is_empty() {
        let until_next = next_at.map_or(ANNOUNCE_IDLE, |next_at| {
            Duration::from_millis(u64::try_from(next_at - now).unwrap_or(0))
        });
        return Ok(until_next.min(ANNOUNCE_IDLE));
    }
    let results = futures_util::future::join_all(
        due.iter()
            .map(|hash| announce_one(ready, work, hash.as_str())),
    )
    .await;
    // 失敗したら（補助 index がまだ記録を持たない等）、続きの告知も同じ間だけ待つ。
    let mut wait = Duration::ZERO;
    for (hash, result) in due.iter().zip(results) {
        let next_at = match result {
            Ok(jitter) => current_time_ms()? + ANNOUNCE_INTERVAL_MS + jitter,
            Err(error) => {
                debug!(%hash, %error, "public blob announcement will be retried");
                wait = ANNOUNCE_RETRY;
                current_time_ms()? + ANNOUNCE_RETRY.as_millis() as i64
            }
        };
        cache
            .reschedule_public_blob_announcement(hash, Some(next_at))
            .await?;
    }
    Ok(wait)
}

/// 1 件を告知し、次の更新の時刻をずらす幅（hash から決める）を返す。
async fn announce_one(
    ready: &Ready,
    work: &Arc<NetworkWorkRuntime>,
    hash: &str,
) -> anyhow::Result<i64> {
    let hash = crate::parse_blob_hash(hash)?;
    let deadline = Instant::now() + ANNOUNCE_TIMEOUT;
    let lease = work.acquire_announce(*hash.as_bytes(), deadline).await?;
    tokio::select! {
        biased;
        _ = lease.cancelled() => anyhow::bail!("public blob announcement cancelled"),
        result = timeout(deadline.saturating_duration_since(Instant::now()), ready.announcer.announce(infohash(hash))) => {
            result?.map_err(|error| anyhow::anyhow!("{error:#}"))?;
        }
    }
    lease.finish();
    let [first, second, ..] = *hash.as_bytes();
    Ok(i64::from(u16::from_be_bytes([first, second])) * ANNOUNCE_JITTER_MS / 65_536)
}
