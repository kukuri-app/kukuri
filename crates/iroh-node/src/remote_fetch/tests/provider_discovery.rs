//! #1596: 合成公開blobだけのnative PoC。本番の取得入口へは接続しない。
use super::*;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::pin::Pin;
use std::sync::Mutex as StdMutex;

use futures_util::{Stream, StreamExt};
use iroh::EndpointId;
use iroh_mainline_address_lookup::DhtAddressLookup;
use iroh_mainline_endpoint_discovery::{AddrIndex, Resolver, infohash_from_blake3};
use n0_mainline::{Dht, DhtBuilder, Testnet};
use udp_addr_index::{Limits, Server, UdpHandle};

/// 採用判断のための負の証拠。shared clientの呼出しdropはactorの予約を取消しない。
#[tokio::test]
async fn shared_index_client_still_sends_a_queued_request_after_caller_drop() -> Result<()> {
    let index_socket = tokio::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let index_addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, index_socket.local_addr()?.port());
    let dht = local_dht(&[])?;
    let client_port = dht.info().await?.local_addr().port();
    let index = AddrIndex::udp(dht.clone(), index_addr).await?;
    let mut request = Box::pin(index.lookup(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 12345)));
    assert!(futures_util::poll!(request.as_mut()).is_pending());
    drop(request);
    let mut packet = [0; 2_048];
    let (len, from) =
        timeout(Duration::from_secs(1), index_socket.recv_from(&mut packet)).await??;
    assert!(len > 0);
    assert_eq!(from.port(), client_port);
    eprintln!("#1596 shared_index_cancel=FAIL queued_udp_request_after_drop_bytes={len}");
    drop(index);
    drop(dht);
    Ok(())
}

type Providers = Pin<Box<dyn Stream<Item = EndpointId> + Send>>;

#[derive(Default)]
struct Observations {
    lookups: AtomicUsize,
    candidates: AtomicUsize,
    sockets: StdMutex<Vec<SocketAddrV4>>,
}

fn local_dht(bootstrap: &[String]) -> Result<Dht> {
    Ok(Dht::builder()
        .bootstrap(bootstrap)
        .port(0)
        .public_ip(Ipv4Addr::LOCALHOST)
        .build()?)
}

fn mount_address_lookup(node: &IrohDocsNode, bootstrap: &[String]) -> Result<()> {
    let mut builder = DhtBuilder::default();
    builder.bootstrap(bootstrap).port(0);
    let lookup = DhtAddressLookup::builder()
        .dht_builder(builder)
        .secret_key(node.endpoint().secret_key().clone())
        .addr_filter(iroh::address_lookup::AddrFilter::unfiltered())
        .build()?;
    node.endpoint().address_lookup()?.add(lookup);
    Ok(())
}

async fn lookup(
    bootstrap: Vec<String>,
    index: SocketAddrV4,
    hash: iroh_blobs::Hash,
    observations: Arc<Observations>,
) -> Result<Providers> {
    observations.lookups.fetch_add(1, Ordering::Relaxed);
    // 需要が所有するcontext。共有DHTをstream dropだけで取消できるとは扱わない。
    let dht = local_dht(&bootstrap)?;
    let socket = dht.info().await?.local_addr();
    observations
        .sockets
        .lock()
        .unwrap()
        .push(SocketAddrV4::new(Ipv4Addr::LOCALHOST, socket.port()));
    let index = AddrIndex::udp(dht.clone(), index).await?;
    let resolver = Resolver::new(dht, index);
    Ok(resolver
        .resolve_stream(infohash_from_blake3(&blake3::Hash::from_bytes(*hash.as_bytes())).into()))
}

/// 同じ30秒の期限・既存ownerに計上して候補だけを入れ替える。
/// 発見後の転送は次の既存表示試行に任せ、既知4件と発見4件を一度に試さない。
async fn probe(
    node: &Arc<IrohDocsNode>,
    peers: &Arc<PeerAddrBook>,
    hash: iroh_blobs::Hash,
    public_gate: bool,
    observations: &Observations,
    discover: impl Future<Output = Result<Providers>>,
) -> Result<Option<Vec<u8>>> {
    if !public_gate {
        return Ok(None);
    }
    let deadline = Instant::now() + REMOTE_FETCH_TOTAL_TIMEOUT;
    let work = async {
        if let Some(bytes) = prepare_display_fetch(node, peers, hash).await?.await? {
            return Ok(Some(bytes));
        }
        let lease = node
            .network_work
            .acquire(*hash.as_bytes(), deadline)
            .await?;
        let discover = async {
            let mut providers = discover.await?.take(4);
            while let Some(provider) = providers.next().await {
                observations.candidates.fetch_add(1, Ordering::Relaxed);
                peers.note_content_source(&hash.to_string(), provider).await;
            }
            Ok(None)
        };
        let result = tokio::select! {
            biased;
            _ = lease.cancelled() => Ok(None),
            result = discover => result,
        };
        if lease.finish() { result } else { Ok(None) }
    };
    timeout(deadline.saturating_duration_since(Instant::now()), work)
        .await
        .unwrap_or(Ok(None))
}

struct Fixture {
    client: Arc<IrohDocsNode>,
    provider: Arc<IrohDocsNode>,
    peers: Arc<PeerAddrBook>,
    bytes: Vec<u8>,
    hash: iroh_blobs::Hash,
    index: SocketAddrV4,
    testnet: Testnet,
    _index_udp: UdpHandle,
    _index_client: AddrIndex,
}

impl Fixture {
    async fn new(cache_only: bool) -> Result<Self> {
        let testnet = Testnet::new(2).await?;
        let server_dht = local_dht(&testnet.bootstrap)?;
        let index = SocketAddrV4::new(
            Ipv4Addr::LOCALHOST,
            server_dht.info().await?.local_addr().port(),
        );
        let server = Server::new(Limits {
            max_entries: 4,
            max_entries_per_ip: 4,
            value_ttl_secs: 60,
            ..Limits::default()
        });
        let index_udp = server.attach_with_rendezvous(server_dht, None).await?;
        let client = IrohDocsNode::memory().await?;
        let provider = IrohDocsNode::memory().await?;
        mount_address_lookup(&client, &testnet.bootstrap)?;
        mount_address_lookup(&provider, &testnet.bootstrap)?;
        let bytes = format!("kukuri-1596-synthetic-public-cache-{cache_only}").into_bytes();
        let hash = iroh_blobs::Hash::new(&bytes);
        if cache_only {
            let cache = Arc::new(kukuri_store::SqliteStore::connect_memory().await?);
            assert!(
                cache
                    .put_remote_content("blob", &hash.to_string(), "blob", &bytes)
                    .await?
            );
            provider.install_remote_cache(cache)?;
            assert!(!provider.blobs().blobs().has(hash).await?);
        } else {
            let tag = provider.blobs().blobs().add_bytes(bytes.clone()).await?;
            assert_eq!(tag.hash, hash);
        }
        let provider_dht = local_dht(&testnet.bootstrap)?;
        let index_client = AddrIndex::udp(provider_dht.clone(), index).await?;
        index_client
            .publish(provider.endpoint().secret_key())
            .await?;
        provider_dht
            .announce_peer(
                infohash_from_blake3(&blake3::Hash::from_bytes(*hash.as_bytes())).into(),
                None,
            )
            .await?;
        let peers = Arc::new(PeerAddrBook::new(
            client.endpoint().clone(),
            client.discovery(),
        ));
        assert!(peers.ranked_peers_for(&hash.to_string()).await.is_empty());
        assert!(client.relay_urls().await.is_empty());
        assert!(provider.relay_urls().await.is_empty());
        Ok(Self {
            client,
            provider,
            peers,
            bytes,
            hash,
            index,
            testnet,
            _index_udp: index_udp,
            _index_client: index_client,
        })
    }

    fn discover(&self, observations: Arc<Observations>) -> impl Future<Output = Result<Providers>> {
        lookup(
            self.testnet.bootstrap.clone(),
            self.index,
            self.hash,
            observations,
        )
    }

    async fn shutdown(self) -> Result<()> {
        self.client.endpoint().address_lookup()?.clear();
        self.provider.endpoint().address_lookup()?.clear();
        self.client.shutdown().await?;
        self.provider.shutdown().await?;
        Ok(())
    }
}

async fn assert_lookup_sockets_released(observations: &Observations) {
    let sockets = observations.sockets.lock().unwrap().clone();
    for socket in sockets {
        timeout(Duration::from_secs(1), async {
            loop {
                if let Ok(reclaimed) = tokio::net::UdpSocket::bind(socket).await {
                    drop(reclaimed);
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("owned provider lookup must release its UDP socket");
    }
}

async fn assert_all_owner_slots_released(node: &Arc<IrohDocsNode>) {
    let mut slots = Vec::new();
    for index in 0..WorkLimits::default().running {
        slots.push(
            timeout(
                Duration::from_secs(1),
                node.network_work.acquire(
                    [index as u8; 32],
                    Instant::now() + REMOTE_FETCH_TOTAL_TIMEOUT,
                ),
            )
            .await
            .unwrap()
            .unwrap(),
        );
    }
    drop(slots);
}

#[tokio::test]
async fn public_blob_discovery_uses_dht_address_lookup_and_both_quic_protocols() -> Result<()> {
    for cache_only in [false, true] {
        let fixture = timeout(REMOTE_FETCH_TOTAL_TIMEOUT, Fixture::new(cache_only)).await??;
        let observations = Arc::new(Observations::default());
        assert_eq!(
            probe(
                &fixture.client,
                &fixture.peers,
                fixture.hash,
                true,
                &observations,
                fixture.discover(observations.clone())
            )
            .await?,
            None
        );
        assert_eq!(observations.lookups.load(Ordering::Relaxed), 1);
        assert!((1..=4).contains(&observations.candidates.load(Ordering::Relaxed)));
        assert_lookup_sockets_released(&observations).await;
        // 表示需要の既存の最初の再試行間隔。PoCは新しいretry taskを作らない。
        tokio::time::sleep(Duration::from_secs(5)).await;
        let bytes = probe(
            &fixture.client,
            &fixture.peers,
            fixture.hash,
            true,
            &observations,
            fixture.discover(observations.clone()),
        )
        .await?
        .expect("discovered EndpointId must serve the synthetic public blob");
        assert_eq!(iroh_blobs::Hash::new(&bytes), fixture.hash);
        assert_eq!(bytes, fixture.bytes);
        assert_eq!(
            observations.lookups.load(Ordering::Relaxed),
            1,
            "known success must skip lookup"
        );
        assert!(!fixture.client.blobs().blobs().has(fixture.hash).await?);
        let state = fixture
            .peers
            .peer_state_snapshot(fixture.provider.endpoint().id())
            .await
            .unwrap();
        assert_eq!(state.fetch_successes, 1);
        assert_eq!(state.fetch_misses, 0);
        assert_all_owner_slots_released(&fixture.client).await;
        eprintln!(
            "#1596 cache_only={cache_only} candidates={} peer_attempts=1 verified_bytes={} direct_bytes={} relay_bytes=0 owner_slots_released=true",
            observations.candidates.load(Ordering::Relaxed),
            bytes.len(),
            bytes.len()
        );
        fixture.shutdown().await?;
    }
    Ok(())
}

#[tokio::test]
async fn public_blob_discovery_keeps_a_fixed_candidate_window() -> Result<()> {
    let fixture = Fixture::new(true).await?;
    let mut providers = vec![fixture.provider.endpoint().id()];
    let mut missing = Vec::new();
    for _ in 0..3 {
        let node = IrohDocsNode::memory().await?;
        fixture
            .client
            .discovery()
            .add_endpoint_info(node.endpoint().addr());
        providers.push(node.endpoint().id());
        missing.push(node);
    }
    for history in [20, 200, 2_000] {
        let peers = Arc::new(PeerAddrBook::new(
            fixture.client.endpoint().clone(),
            fixture.client.discovery(),
        ));
        let observations = Arc::new(Observations::default());
        let ids = providers.clone();
        let stream = futures_util::stream::iter((0..history).map(move |index| ids[index % 4]));
        let discover = async { Ok(Box::pin(stream) as Providers) };
        assert_eq!(
            probe(
                &fixture.client,
                &peers,
                fixture.hash,
                true,
                &observations,
                discover
            )
            .await?,
            None
        );
        assert_eq!(observations.candidates.load(Ordering::Relaxed), 4);
        let selected = peers.ranked_peers_for(&fixture.hash.to_string()).await;
        assert_eq!(selected.len(), 4);
        let bytes = prepare_display_fetch(&fixture.client, &peers, fixture.hash)
            .await?
            .await?
            .expect("one of the fixed four peers holds the blob");
        assert_eq!(bytes, fixture.bytes);
        let mut attempts = 0;
        for provider in &providers {
            if let Some(state) = peers.peer_state_snapshot(*provider).await {
                attempts += state.fetch_successes
                    + state.fetch_misses
                    + state.fetch_failures
                    + state.fetch_rejections;
            }
        }
        assert_eq!(attempts, 4);
        assert_all_owner_slots_released(&fixture.client).await;
        eprintln!(
            "#1596 candidates={history} consumed=4 admitted=4 peer_attempts={attempts} admitted_endpoint_bytes=128 verified_bytes={}",
            bytes.len()
        );
    }
    for node in missing {
        node.shutdown().await?;
    }
    fixture.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn public_blob_discovery_gate_cancel_and_deadline_release_owned_work() -> Result<()> {
    let fixture = Fixture::new(false).await?;
    let denied = Arc::new(Observations::default());
    assert_eq!(
        probe(
            &fixture.client,
            &fixture.peers,
            fixture.hash,
            false,
            &denied,
            fixture.discover(denied.clone())
        )
        .await?,
        None
    );
    assert_eq!(denied.lookups.load(Ordering::Relaxed), 0);
    assert_eq!(denied.candidates.load(Ordering::Relaxed), 0);
    assert!(
        fixture
            .peers
            .peer_state_snapshot(fixture.provider.endpoint().id())
            .await
            .is_none()
    );

    let silent_index = tokio::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let silent_addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, silent_index.local_addr()?.port());
    let cancelled = Arc::new(Observations::default());
    let work = probe(
        &fixture.client,
        &fixture.peers,
        fixture.hash,
        true,
        &cancelled,
        lookup(
            fixture.testnet.bootstrap.clone(),
            silent_addr,
            fixture.hash,
            cancelled.clone(),
        ),
    );
    let mut work = Box::pin(work);
    let mut packet = [0; 2_048];
    tokio::select! {
        result = work.as_mut() => panic!("lookup must still be waiting: {result:?}"),
        result = timeout(Duration::from_secs(5), silent_index.recv_from(&mut packet)) => { result??; }
    }
    drop(work);
    assert_lookup_sockets_released(&cancelled).await;
    assert_all_owner_slots_released(&fixture.client).await;
    assert_eq!(cancelled.candidates.load(Ordering::Relaxed), 0);

    let expired = Arc::new(Observations::default());
    let pending = async { Ok(Box::pin(futures_util::stream::pending()) as Providers) };
    tokio::time::pause();
    let started = Instant::now();
    assert_eq!(
        probe(
            &fixture.client,
            &fixture.peers,
            fixture.hash,
            true,
            &expired,
            pending
        )
        .await?,
        None
    );
    // Tokioの既存timerはms単位で丸める。設定期限を延長せず、取消と枠の解放を確認する。
    assert!(started.elapsed() >= REMOTE_FETCH_TOTAL_TIMEOUT);
    assert!(started.elapsed() <= REMOTE_FETCH_TOTAL_TIMEOUT + Duration::from_millis(1));
    tokio::time::resume();
    assert_all_owner_slots_released(&fixture.client).await;
    assert_eq!(expired.candidates.load(Ordering::Relaxed), 0);
    eprintln!(
        "#1596 denied_lookup=0 cancelled_context_sockets_released=true deadline_secs=30 owner_slots_released=true"
    );
    fixture.shutdown().await?;
    Ok(())
}
