//! #1632: 公開 blob の発見（ADR 0063）。loopback の Testnet と補助 index の server で、node に組み込んだ発見・告知と
//! 共通の取得を通す。
use super::*;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::Path;

use iroh::{EndpointId, SecretKey};
use kukuri_core::{AssetRef, AssetRole, BlobHash, EnvelopeId, PayloadRef, ReplicaId};
use kukuri_store::{ObjectProjectionRow, ObjectProjectionStore, SqliteStore};
use kukuri_transport::{
    DhtDiscoveryOptions, PublicBlobIndex, TransportNetworkConfig, TransportRelayConfig,
};
use n0_mainline::{DhtBuilder, Testnet};
use udp_addr_index::{Limits, Server, UdpHandle};

fn dht_builder(testnet: &Testnet, port: u16) -> DhtBuilder {
    let mut builder = DhtBuilder::default();
    builder
        .bootstrap(&testnet.bootstrap)
        .port(port)
        .public_ip(Ipv4Addr::LOCALHOST);
    builder
}

fn infohash(hash: iroh_blobs::Hash) -> n0_mainline::Id {
    iroh_mainline_endpoint_discovery::infohash_from_blake3(&blake3::Hash::from_bytes(
        *hash.as_bytes(),
    ))
    .into()
}

async fn index_server(testnet: &Testnet) -> Result<(SocketAddrV4, UdpHandle)> {
    let dht = dht_builder(testnet, 0).build()?;
    let index = SocketAddrV4::new(Ipv4Addr::LOCALHOST, dht.info().await?.local_addr().port());
    Ok((index, Server::new(Limits::for_tests()).attach(dht).await?))
}

/// 公開 blob の発見を持つ node と、その account の store。
async fn discovery_node(
    root: &Path,
    testnet: &Testnet,
    index: SocketAddrV4,
    port: u16,
) -> Result<(Arc<IrohDocsNode>, Arc<SqliteStore>)> {
    let node = IrohDocsNode::persistent_with_discovery_config(
        root,
        TransportNetworkConfig::loopback(),
        DhtDiscoveryOptions {
            enabled: true,
            dht_builder: Some(dht_builder(testnet, port)),
            public_blob_index: Some(PublicBlobIndex::Servers(vec![index])),
            ..DhtDiscoveryOptions::default()
        },
        TransportRelayConfig::default(),
        false,
    )
    .await?;
    let store = Arc::new(SqliteStore::connect_memory().await?);
    node.install_remote_cache(store.clone())?;
    Ok((node, store))
}

/// `hash` を添付に持つ公開投稿（検証済みの公開記録）を置く。
async fn publish(store: &SqliteStore, hash: iroh_blobs::Hash) -> Result<()> {
    let object_id = EnvelopeId::from(format!("post-{hash}"));
    store
        .put_object_projection(ObjectProjectionRow {
            object_id: object_id.clone(),
            topic_id: "topic".into(),
            channel_id: "public".into(),
            author_pubkey: "a".repeat(64),
            created_at: 1,
            object_kind: "post".into(),
            root_object_id: None,
            reply_to_object_id: None,
            payload_ref: PayloadRef::InlineText {
                text: "body".into(),
            },
            content: Some("body".into()),
            attachments: vec![AssetRef {
                hash: BlobHash::new(hash.to_string()),
                mime: "image/png".into(),
                bytes: 1,
                role: AssetRole::ImageOriginal,
            }],
            repost_of: None,
            content_labels: Vec::new(),
            source_replica_id: ReplicaId::new("bucket::v1::topic::746f706963::1"),
            source_key: format!("objects/post-{hash}/envelope"),
            source_envelope_id: object_id,
            source_blob_hash: None,
            source_docs_author: None,
            derived_at: 1,
            projection_version: 3,
        })
        .await
}

/// 告知に成功して、次の更新が 10 分後へ移るまで待つ。
async fn wait_announced(store: &SqliteStore, hash: iroh_blobs::Hash) -> Result<()> {
    timeout(Duration::from_secs(120), async {
        loop {
            let soon = current_time_ms()? + 9 * 60 * 1000;
            let (due, next_at) = store.due_public_blob_announcements(soon, 1).await?;
            anyhow::ensure!(
                !due.is_empty() || next_at.is_some(),
                "{hash} left the announcement schedule"
            );
            if due.is_empty() {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .context("the holder did not announce the public blob in time")?
}

fn lookups(node: &IrohDocsNode) -> usize {
    node.public_blobs()
        .expect("public blob discovery")
        .lookups
        .load(Ordering::Relaxed)
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

/// 既知の候補に無い公開 blob を、DHT で見つけた未知の保持端末（SDK と保護つきの cache、cache だけ）から取得する。
/// 保持端末は、公開参照と保持の両方がある hash を自分で告知する。2 回目は覚えた取得元から取れるので、検索しない。
#[tokio::test]
async fn a_public_blob_comes_from_an_unknown_holder_found_through_the_dht() -> Result<()> {
    let testnet = Testnet::new(3).await?;
    let (index, _server) = index_server(&testnet).await?;
    let client_dir = tempfile::tempdir()?;
    let (client, client_store) = discovery_node(client_dir.path(), &testnet, index, 0).await?;
    let mut holders = Vec::new();
    for cache_only in [false, true] {
        let dir = tempfile::tempdir()?;
        let (provider, provider_store) = discovery_node(dir.path(), &testnet, index, 0).await?;
        let bytes = format!("kukuri-1632-public-blob-{cache_only}").into_bytes();
        let hash = iroh_blobs::Hash::new(&bytes);
        if cache_only {
            assert!(
                provider_store
                    .put_remote_content("blob", &hash.to_string(), "blob", &bytes)
                    .await?
            );
        } else {
            provider_store
                .put_owned_blob(&format!("own_blob:{hash}"), &hash.to_string(), &bytes)
                .await?;
            provider.blobs().blobs().add_bytes(bytes.clone()).await?;
        }
        publish(&provider_store, hash).await?;
        publish(&client_store, hash).await?;
        holders.push((dir, provider, provider_store, bytes, hash));
    }
    for (_, _, provider_store, _, hash) in &holders {
        wait_announced(provider_store, *hash).await?;
    }

    let peers = Arc::new(PeerAddrBook::new(
        client.endpoint().clone(),
        client.discovery(),
    ));
    for (round, (_, _, _, bytes, hash)) in holders.iter().enumerate() {
        assert!(peers.ranked_peers_for(&hash.to_string()).await.is_empty());
        let fetched = prepare_display_fetch(&client, &peers, *hash).await?.await?;
        assert_eq!(fetched.as_deref(), Some(bytes.as_slice()));
        assert_eq!(lookups(&client), round + 1);
        let again = prepare_display_fetch(&client, &peers, *hash).await?.await?;
        assert_eq!(again.as_deref(), Some(bytes.as_slice()));
        assert_eq!(
            lookups(&client),
            round + 1,
            "a known holder must not be searched"
        );
    }
    assert_all_owner_slots_released(&client).await;
    client.shutdown().await?;
    for (_, provider, ..) in holders {
        provider.shutdown().await?;
    }
    Ok(())
}

/// 検証済みの公開記録に参照されていない blob（DM・非公開の内容）は、DHT で探さない。
#[tokio::test]
async fn a_blob_without_a_public_record_is_not_searched() -> Result<()> {
    let testnet = Testnet::new(2).await?;
    let (index, _server) = index_server(&testnet).await?;
    let dir = tempfile::tempdir()?;
    let (client, _store) = discovery_node(dir.path(), &testnet, index, 0).await?;
    client
        .public_blobs()
        .expect("public blob discovery")
        .ready()
        .await;
    let peers = Arc::new(PeerAddrBook::new(
        client.endpoint().clone(),
        client.discovery(),
    ));
    let hash = iroh_blobs::Hash::new(b"kukuri-1632-private-blob");
    assert_eq!(
        prepare_display_fetch(&client, &peers, hash).await?.await?,
        None
    );
    assert_eq!(lookups(&client), 0);
    client.shutdown().await?;
    Ok(())
}

/// 検索の途中で取得を落とすと、検索が止まって実行枠が空く。node を止めると、DHT の socket も空く。
#[tokio::test]
async fn dropping_a_fetch_and_stopping_the_node_leave_no_search_behind() -> Result<()> {
    let testnet = Testnet::new(2).await?;
    // 応答しない補助 index。照会が届いた時点で、検索は保持端末の endpoint ID を待っている。
    let silent = tokio::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let silent_addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, silent.local_addr()?.port());
    let hash = iroh_blobs::Hash::new(b"kukuri-1632-cancelled-search");
    let holder = dht_builder(&testnet, 0).build()?;
    holder.announce_peer(infohash(hash), None).await?;
    let port = std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?
        .local_addr()?
        .port();
    let dir = tempfile::tempdir()?;
    let (client, store) = discovery_node(dir.path(), &testnet, silent_addr, port).await?;
    publish(&store, hash).await?;
    client
        .public_blobs()
        .expect("public blob discovery")
        .ready()
        .await;
    let peers = Arc::new(PeerAddrBook::new(
        client.endpoint().clone(),
        client.discovery(),
    ));
    let mut fetch = prepare_display_fetch(&client, &peers, hash).await?;
    let mut packet = [0; 2_048];
    tokio::select! {
        result = fetch.as_mut() => panic!("the search must still wait for the index: {result:?}"),
        received = timeout(Duration::from_secs(20), silent.recv_from(&mut packet)) => { received??; }
    }
    drop(fetch);
    assert_all_owner_slots_released(&client).await;

    // 停止の後も node を別に保持する（作り直しの間は古い stack が残る）。
    let held = client.clone();
    client.shutdown().await?;
    timeout(Duration::from_secs(5), async {
        while std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, port)).is_err() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .context("the stopped node must release its DHT socket")?;
    drop((held, holder));
    Ok(())
}

/// 発見の stream が 20・200・2000 件を返しても、読むのは重複を含めて 16 件まで、試すのは既知と合わせて 4 端末まで。
#[tokio::test]
async fn discovered_holders_use_a_fixed_window() -> Result<()> {
    let client = IrohDocsNode::memory().await?;
    let hash = iroh_blobs::Hash::new(b"kukuri-1632-window");
    let hash_text = hash.to_string();
    let holders = (0..6)
        .map(|_| SecretKey::generate().public())
        .collect::<Vec<EndpointId>>();
    for total in [20, 200, 2_000] {
        for (repeated, known) in [(false, 1), (true, 0)] {
            let peers = PeerAddrBook::new(client.endpoint().clone(), client.discovery());
            let fetch = PeerFetch {
                node: &client,
                peers: &peers,
                subject: "discovered holders",
                hash_text: &hash_text,
                hash,
                mode: FetchMode::Ephemeral,
                file_path: None,
            };
            let read = Arc::new(AtomicUsize::new(0));
            let stream = futures_util::stream::iter((0..total).map({
                let (read, holders) = (read.clone(), holders.clone());
                move |index: usize| {
                    read.fetch_add(1, Ordering::Relaxed);
                    holders[if repeated { 0 } else { index % holders.len() }]
                }
            }));
            let mut tried = vec![SecretKey::generate().public(); known];
            let mut failed = false;
            assert_eq!(
                fetch.try_providers(stream, &mut tried, &mut failed).await?,
                None
            );
            if repeated {
                assert_eq!(read.load(Ordering::Relaxed), MAX_PROVIDER_ITEMS);
                assert_eq!(tried, [holders[0]]);
            } else {
                assert_eq!(read.load(Ordering::Relaxed), MAX_FETCH_PEERS);
                assert_eq!(tried.len(), MAX_FETCH_PEERS);
            }
        }
    }
    client.shutdown().await?;
    Ok(())
}
