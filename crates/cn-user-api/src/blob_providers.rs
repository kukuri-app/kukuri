//! 公開 blob の保持端末の検索（#1632 AC-5、ADR 0063 §7）。
//!
//! `POST /v1/blob-providers/search` は認証・同意済みの client から公開 blob の hash を受け、Mainline DHT（client）と
//! kukuri の補助 index で告知した端末を探して、署名つきの住所 record に relay URL を持つ候補だけを返す（D5）。
//! iroh の endpoint を持たないので、blob の取得・保存・size の確認はしない。受付は全体と端末ごとの同時数で打ち切り、
//! 期限か HTTP の切断で検索ごと止まる。

use std::collections::{HashMap, HashSet};
use std::future::{Future, ready};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use anyhow::Result;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use futures_util::{Stream, StreamExt};
use iroh::EndpointId;
use iroh::address_lookup::AddressLookup;
use iroh_mainline_address_lookup::DhtAddressLookup;
use iroh_mainline_endpoint_discovery::{Hash, Resolver, infohash_from_blake3};
use kukuri_cn_core::{ApiError, ApiResult, require_bearer_identity, require_consents};
use kukuri_cn_protocol::{
    BLOB_PROVIDER_MAX_DIRECT_ADDRS, BLOB_PROVIDER_MAX_RELAY_URLS, BLOB_PROVIDER_SEARCH_BUSY_CODE,
    BLOB_PROVIDER_SEARCH_MAX_BUDGET_MS, BLOB_PROVIDER_SEARCH_MAX_CANDIDATES,
    BLOB_PROVIDER_SEARCH_NOT_CONFIGURED_CODE, BlobProviderCandidate, BlobProviderSearchRequest,
    BlobProviderSearchResponse, INVALID_BLOB_PROVIDER_SEARCH_CODE,
};
use kukuri_transport::PublicBlobIndex;
use n0_mainline::DhtBuilder;
use tokio::time::{Instant, timeout_at};

use crate::state::UserApiState;

/// 同時に進める検索の数（全体と端末ごと）。端末ごとの数は client の取得の実行枠（8）と同じ。
const RUNNING: usize = 32;
const RUNNING_PER_DEVICE: usize = 8;
/// 1 検索で読む発見の件数（重複を含む）と、同時に引く住所 record の数。
const MAX_DISCOVERED: usize = 16;
const RESOLVING: usize = 4;
/// 満杯の応答で、次に試すまでの目安（秒）。取得の次の試行（5 秒）と同じ。
const RETRY_AFTER_SECS: &str = "5";

/// 保持端末の検索。DHT と補助 index の client を持つ。
pub struct BlobProviderSearch {
    resolver: Arc<OnceLock<Resolver>>,
    addresses: DhtAddressLookup,
    /// 端末ごとの進行中の検索の数。全体で `RUNNING` 件までなので、行も `RUNNING` 件まで。
    running: Mutex<HashMap<String, usize>>,
}

impl BlobProviderSearch {
    /// `dht` で DHT を組み立て、`index` の補助 index へ背景でつなぐ。つながるまでの検索は候補なしの `partial` を返す。
    pub fn start(dht: &DhtBuilder, index: PublicBlobIndex) -> Result<Self> {
        let dht = dht.build()?;
        let resolver = Arc::new(OnceLock::new());
        tokio::spawn({
            let (resolver, dht) = (resolver.clone(), dht.clone());
            async move {
                let index = index.connect(dht.clone()).await;
                let _ = resolver.set(Resolver::new(dht, index));
            }
        });
        Ok(Self {
            resolver,
            addresses: DhtAddressLookup::builder().dht(dht).no_publish().build()?,
            running: Mutex::default(),
        })
    }

    /// `device` の要求として、`hash` を告知した端末を `deadline` まで探す。
    async fn search(
        &self,
        device: String,
        hash: Hash,
        deadline: Instant,
    ) -> ApiResult<BlobProviderSearchResponse> {
        let _admitted = self.admit(device)?;
        let Some(resolver) = self.resolver.get() else {
            return Ok(BlobProviderSearchResponse {
                candidates: Vec::new(),
                partial: true,
            });
        };
        let discovered = resolver.resolve_stream(infohash_from_blake3(&hash).into());
        Ok(collect(discovered, |id| self.candidate(id), deadline).await)
    }

    /// 全体か `device` の同時数が上限なら 429。受け付けた分は、返した値を落とすと（応答・期限・切断）戻る。
    fn admit(&self, device: String) -> ApiResult<Admitted<'_>> {
        let mut running = self.running.lock().expect("blob provider search poisoned");
        if running.values().sum::<usize>() >= RUNNING
            || running.get(&device).copied().unwrap_or(0) >= RUNNING_PER_DEVICE
        {
            return Err(ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                BLOB_PROVIDER_SEARCH_BUSY_CODE,
                "public blob holder search is busy",
            )
            .with_header(header::RETRY_AFTER, RETRY_AFTER_SECS));
        }
        *running.entry(device.clone()).or_default() += 1;
        Ok(Admitted {
            search: self,
            device,
        })
    }

    /// `id` の署名つきの住所 record を引き、relay URL があれば候補にする（D5）。
    async fn candidate(&self, id: EndpointId) -> Option<BlobProviderCandidate> {
        let item = self
            .addresses
            .resolve(id)?
            .filter_map(|item| ready(item.ok()))
            .next()
            .await?;
        let info = item.endpoint_info();
        let relay_urls = info
            .relay_urls()
            .take(BLOB_PROVIDER_MAX_RELAY_URLS)
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        (!relay_urls.is_empty()).then(|| BlobProviderCandidate {
            endpoint_id: id.to_string(),
            relay_urls,
            direct_addrs: info
                .ip_addrs()
                .take(BLOB_PROVIDER_MAX_DIRECT_ADDRS)
                .map(ToString::to_string)
                .collect(),
        })
    }
}

struct Admitted<'a> {
    search: &'a BlobProviderSearch,
    device: String,
}

impl Drop for Admitted<'_> {
    fn drop(&mut self) {
        let mut running = self
            .search
            .running
            .lock()
            .expect("blob provider search poisoned");
        if let Some(count) = running.get_mut(&self.device) {
            *count -= 1;
            if *count == 0 {
                running.remove(&self.device);
            }
        }
    }
}

/// 発見した端末を 16 件まで読み、重複を除いて住所を同時 4 件まで引き、候補を最大 4 件集める。`deadline` を過ぎたら
/// 集めた分で打ち切り、`partial` を立てる。
async fn collect<F, Fut>(
    discovered: impl Stream<Item = EndpointId>,
    resolve: F,
    deadline: Instant,
) -> BlobProviderSearchResponse
where
    F: Fn(EndpointId) -> Fut,
    Fut: Future<Output = Option<BlobProviderCandidate>>,
{
    let mut seen = HashSet::new();
    let mut found = std::pin::pin!(
        discovered
            .take(MAX_DISCOVERED)
            .filter(move |id| ready(seen.insert(*id)))
            .map(resolve)
            .buffer_unordered(RESOLVING)
            .filter_map(ready)
            .take(BLOB_PROVIDER_SEARCH_MAX_CANDIDATES)
    );
    let mut candidates = Vec::new();
    let partial = timeout_at(deadline, async {
        while let Some(candidate) = found.next().await {
            candidates.push(candidate);
        }
    })
    .await
    .is_err();
    BlobProviderSearchResponse {
        candidates,
        partial,
    }
}

pub(crate) async fn search_blob_providers(
    State(state): State<UserApiState>,
    headers: HeaderMap,
    Json(request): Json<BlobProviderSearchRequest>,
) -> ApiResult<Json<BlobProviderSearchResponse>> {
    let received = Instant::now();
    let Some(search) = state.blob_provider_search.clone() else {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            BLOB_PROVIDER_SEARCH_NOT_CONFIGURED_CODE,
            "this community node does not search public blob holders",
        ));
    };
    let identity = require_bearer_identity(&state.pool, &state.jwt_config, &headers).await?;
    let _ = require_consents(&state.pool, identity.pubkey.as_str()).await?;
    let hash = Hash::from_hex(request.hash.as_str())
        .ok()
        .filter(|_| request.budget_ms > 0)
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::BAD_REQUEST,
                INVALID_BLOB_PROVIDER_SEARCH_CODE,
                "hash must be 64 hex digits and budget_ms must be positive",
            )
        })?;
    let deadline =
        received + Duration::from_millis(request.budget_ms.min(BLOB_PROVIDER_SEARCH_MAX_BUDGET_MS));
    let device = identity.endpoint_id.unwrap_or(identity.pubkey);
    search.search(device, hash, deadline).await.map(Json)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use iroh::SecretKey;

    use super::*;

    fn candidate(id: EndpointId) -> BlobProviderCandidate {
        BlobProviderCandidate {
            endpoint_id: id.to_string(),
            relay_urls: vec!["https://relay.example/".to_string()],
            direct_addrs: Vec::new(),
        }
    }

    /// 発見が 20・200・2000 件でも、読むのは 16 件まで・同時に引く住所は 4 件まで・候補は 4 件まで。
    #[tokio::test]
    async fn a_search_reads_a_fixed_window() {
        let ids = (0..32)
            .map(|_| SecretKey::generate().public())
            .collect::<Vec<_>>();
        let deadline = Instant::now() + Duration::from_secs(10);
        for total in [20, 200, 2_000] {
            for (repeated, relay) in [(false, false), (true, false), (false, true)] {
                let read = Arc::new(AtomicUsize::new(0));
                let discovered = futures_util::stream::iter(0..total).map({
                    let (read, ids) = (read.clone(), ids.clone());
                    move |index: usize| {
                        read.fetch_add(1, Ordering::Relaxed);
                        ids[if repeated { 0 } else { index % ids.len() }]
                    }
                });
                let (resolved, active, peak) = (
                    AtomicUsize::new(0),
                    AtomicUsize::new(0),
                    AtomicUsize::new(0),
                );
                let response = collect(
                    discovered,
                    |id| {
                        let (resolved, active, peak) = (&resolved, &active, &peak);
                        async move {
                            resolved.fetch_add(1, Ordering::Relaxed);
                            peak.fetch_max(
                                active.fetch_add(1, Ordering::Relaxed) + 1,
                                Ordering::Relaxed,
                            );
                            tokio::task::yield_now().await;
                            active.fetch_sub(1, Ordering::Relaxed);
                            relay.then(|| candidate(id))
                        }
                    },
                    deadline,
                )
                .await;
                assert!(!response.partial);
                assert!(read.load(Ordering::Relaxed) <= MAX_DISCOVERED);
                assert!(peak.load(Ordering::Relaxed) <= RESOLVING);
                if relay {
                    assert_eq!(
                        response.candidates.len(),
                        BLOB_PROVIDER_SEARCH_MAX_CANDIDATES
                    );
                } else {
                    assert!(response.candidates.is_empty());
                    assert_eq!(read.load(Ordering::Relaxed), MAX_DISCOVERED);
                    assert_eq!(
                        resolved.load(Ordering::Relaxed),
                        if repeated { 1 } else { MAX_DISCOVERED }
                    );
                }
            }
        }
    }

    /// 期限までに発見が終わらなければ、集めた分で打ち切って `partial` を立てる。
    #[tokio::test(start_paused = true)]
    async fn a_search_ends_at_the_deadline() {
        let id = SecretKey::generate().public();
        let discovered = futures_util::stream::iter([id]).chain(futures_util::stream::pending());
        let started = Instant::now();
        let response = collect(
            discovered,
            |id| ready(Some(candidate(id))),
            started + Duration::from_secs(3),
        )
        .await;
        assert!(response.partial);
        assert_eq!(response.candidates, [candidate(id)]);
        assert_eq!(started.elapsed(), Duration::from_secs(3));
    }

    /// 20・200・2000 件の要求でも、進行中の検索は全体 32 件・端末ごと 8 件までで、行も同じ数まで。応答を落とすと戻る。
    #[tokio::test]
    async fn admission_holds_a_fixed_number_of_searches() -> Result<()> {
        let mut dht = DhtBuilder::default();
        dht.no_bootstrap().port(0);
        let search = BlobProviderSearch::start(&dht, PublicBlobIndex::Servers(Vec::new()))?;
        let mut held = (0..RUNNING_PER_DEVICE)
            .map(|_| search.admit("device".to_string()))
            .collect::<ApiResult<Vec<_>>>()
            .map_err(|_| anyhow::anyhow!("a device must get its own slots"))?;
        assert!(search.admit("device".to_string()).is_err());
        for total in [20, 200, 2_000] {
            let rejected = (0..total)
                .filter(|index| match search.admit(format!("device-{index}")) {
                    Ok(slot) => {
                        held.push(slot);
                        false
                    }
                    Err(_) => true,
                })
                .count();
            let admitted = total.min(RUNNING - RUNNING_PER_DEVICE);
            assert_eq!(
                (held.len(), rejected),
                (RUNNING_PER_DEVICE + admitted, total - admitted)
            );
            assert_eq!(search.running.lock().unwrap().len(), 1 + admitted);
            held.truncate(RUNNING_PER_DEVICE);
        }
        drop(held);
        assert!(search.running.lock().unwrap().is_empty());
        Ok(())
    }
}
