use super::*;
use std::net::SocketAddr;

use iroh::Endpoint;
use kukuri_transport::{TransportNetworkConfig, encode_endpoint_ticket};
use tempfile::tempdir;
use tokio::time::{Duration, sleep, timeout};

pub(crate) fn loopback_ticket(endpoint: &Endpoint, config: &TransportNetworkConfig) -> String {
    let endpoint_addr = endpoint.addr();
    let bound_sockets = endpoint.bound_sockets();
    let ticket_config = TransportNetworkConfig {
        bind_addr: config.bind_addr,
        advertised_host: config.advertised_host.clone().or_else(|| {
            bound_sockets
                .iter()
                .find(|addr| addr.ip().is_loopback())
                .or_else(|| {
                    bound_sockets
                        .iter()
                        .find(|addr| is_ticket_host_candidate(**addr))
                })
                .map(|addr| addr.ip().to_string())
        }),
        advertised_port: config.advertised_port.or_else(|| {
            bound_sockets
                .iter()
                .find(|addr| addr.port() != 0)
                .map(|addr| addr.port())
        }),
    };
    encode_endpoint_ticket(&endpoint_addr, &ticket_config).expect("sender ticket")
}

fn is_ticket_host_candidate(addr: SocketAddr) -> bool {
    !addr.ip().is_unspecified()
}

// リトライ状態のテストは共通実装側(kukuri-transport::peers)へ移動した(WP-H2)。

// 画面の送信中の添付の仮の hash（`<id>-attachment-0`）のような、hash でない文字列は panic せずにエラーにする。
// `iroh_blobs::Hash::from_str` は 64・52 文字以外の長さで panic し、wasm では runtime の task の実行器を壊す
// （#1220 AC-4）。
#[tokio::test]
async fn a_string_that_is_not_a_hash_is_an_error() {
    let blobs = IrohBlobService::new(IrohDocsNode::memory().await.expect("memory node"));
    for text in ["", "x", &"a".repeat(46), &"f".repeat(63), &"0".repeat(65)] {
        let hash = BlobHash::new(text);
        assert!(blobs.fetch_blob_ephemeral(&hash).await.is_err(), "{text}");
        assert!(blobs.fetch_local_blob(&hash).await.is_err(), "{text}");
    }
}

#[tokio::test]
async fn blob_roundtrip_basic() {
    let node = IrohDocsNode::memory().await.expect("memory node");
    let blobs = IrohBlobService::new(node);
    let stored = blobs
        .put_blob(b"hello blob".to_vec(), "text/plain")
        .await
        .expect("put blob");

    let payload = blobs
        .fetch_blob(&stored.hash)
        .await
        .expect("fetch blob")
        .expect("blob bytes");
    assert_eq!(payload, b"hello blob".to_vec());

    assert_eq!(
        blobs.blob_status(&stored.hash).await.expect("blob status"),
        BlobStatus::Available
    );
    blobs.pin_blob(&stored.hash).await.expect("pin blob");
    assert_eq!(
        blobs.blob_status(&stored.hash).await.expect("blob status"),
        BlobStatus::Pinned
    );
    blobs.unpin_blob(&stored.hash).await.expect("unpin blob");
    assert_eq!(
        blobs.blob_status(&stored.hash).await.expect("blob status"),
        BlobStatus::Available
    );
}

#[test]
fn metaverse_cache_deduplicates_pins_and_only_collects_unreferenced_grace_candidates() {
    let mut cache = MetaverseBlobCacheIndex::new(1_000).unwrap();
    let hash = BlobHash::new("asset-hash");
    let current = MetaverseBlobPin {
        reason: MetaverseBlobPinReason::Current,
        reference_id: "preset:2".into(),
    };
    let active = MetaverseBlobPin {
        reason: MetaverseBlobPinReason::ActiveLease,
        reference_id: "dome:1".into(),
    };
    cache.pin(&hash, 400, current.clone(), 1_000);
    cache.pin(&hash, 400, active.clone(), 1_100);
    assert_eq!(cache.total_bytes(), 400);
    assert!(
        cache
            .collect_garbage(1_000 + METAVERSE_BLOB_GC_GRACE_MILLIS)
            .is_empty()
    );

    cache.unpin_reference(&current, 2_000);
    assert!(
        cache
            .collect_garbage(2_000 + METAVERSE_BLOB_GC_GRACE_MILLIS)
            .is_empty()
    );
    cache.unpin_reference(&active, 3_000);
    assert!(
        cache
            .collect_garbage(3_000 + METAVERSE_BLOB_GC_GRACE_MILLIS - 1)
            .is_empty()
    );
    assert_eq!(
        cache.collect_garbage(3_000 + METAVERSE_BLOB_GC_GRACE_MILLIS),
        vec![hash]
    );
    assert_eq!(cache.total_bytes(), 0);
}

#[test]
fn metaverse_cache_fails_staging_before_exceeding_hard_capacity() {
    let mut cache = MetaverseBlobCacheIndex::new(500).unwrap();
    let current = MetaverseBlobPin {
        reason: MetaverseBlobPinReason::Current,
        reference_id: "preset:1".into(),
    };
    cache.pin(&BlobHash::new("manifest"), 400, current, 1_000);
    assert!(
        cache
            .ensure_staging_capacity(&[(BlobHash::new("asset"), 101)])
            .is_err()
    );
    assert!(
        cache
            .ensure_staging_capacity(&[(BlobHash::new("manifest"), 400)])
            .is_ok()
    );
}

#[tokio::test]
async fn remote_fetch_roundtrip_after_ticket_import() {
    let sender_dir = tempdir().expect("sender tempdir");
    let receiver_dir = tempdir().expect("receiver tempdir");
    let config = TransportNetworkConfig::loopback();

    let sender_node = IrohDocsNode::persistent_with_config(sender_dir.path(), config.clone())
        .await
        .expect("sender node");
    let receiver_node = IrohDocsNode::persistent_with_config(receiver_dir.path(), config.clone())
        .await
        .expect("receiver node");

    let sender = IrohBlobService::new(sender_node.clone());
    let receiver = IrohBlobService::new(receiver_node);

    let ticket = loopback_ticket(sender_node.endpoint(), &config);
    receiver
        .import_peer_ticket(&ticket)
        .await
        .expect("import ticket");

    let stored = sender
        .put_blob(b"video-remote-roundtrip".to_vec(), "video/mp4")
        .await
        .expect("put blob");

    let payload = receiver.fetch_blob(&stored.hash).await.expect("fetch blob");

    assert_eq!(payload, Some(b"video-remote-roundtrip".to_vec()));
}

#[tokio::test]
async fn ephemeral_fetch_returns_bytes_without_persisting_them_locally() {
    // safety scan の一時 fetch(#609): remote から取得できること、かつ取得後も
    // ローカルストアに blob が残らないこと(no-permanent-blob-storage)を固定する。
    let sender_dir = tempdir().expect("sender tempdir");
    let receiver_dir = tempdir().expect("receiver tempdir");
    let config = TransportNetworkConfig::loopback();

    let sender_node = IrohDocsNode::persistent_with_config(sender_dir.path(), config.clone())
        .await
        .expect("sender node");
    let receiver_node = IrohDocsNode::persistent_with_config(receiver_dir.path(), config.clone())
        .await
        .expect("receiver node");

    let sender = IrohBlobService::new(sender_node.clone());
    let receiver = IrohBlobService::new(receiver_node.clone());

    let ticket = loopback_ticket(sender_node.endpoint(), &config);
    receiver
        .import_peer_ticket(&ticket)
        .await
        .expect("import ticket");

    let stored = sender
        .put_blob(b"scan-only-media".to_vec(), "image/png")
        .await
        .expect("put blob");

    let payload = receiver
        .fetch_blob_ephemeral(&stored.hash)
        .await
        .expect("ephemeral fetch");
    assert_eq!(payload, Some(b"scan-only-media".to_vec()));

    // ローカルストアには取り込まれていない(fetch_blob 経由だと remote fetch に
    // フォールバックしてしまうため、store を直接確認する)。
    let hash = iroh_blobs::Hash::from_str(stored.hash.as_str()).expect("hash");
    assert!(
        receiver_node.blobs().blobs().get_bytes(hash).await.is_err(),
        "ephemeral fetch must not persist the blob into the local store"
    );

    // #1060: both local and remote ingress stop at the byte bound, and a
    // rejected small scan must not poison a later permitted acquisition.
    let large = sender
        .put_blob(vec![42; 2 * 1024 * 1024], "video/mp4")
        .await
        .expect("large blob");
    let local_error = sender
        .fetch_blob_ephemeral_bounded(&large.hash, 1024)
        .await
        .expect_err("bounded local read");
    assert!(local_error.is::<remote_fetch::BlobTooLarge>());
    let remote_error = receiver
        .fetch_blob_ephemeral_bounded(&large.hash, 1024)
        .await
        .expect_err("bounded remote stream");
    assert!(remote_error.is::<remote_fetch::BlobTooLarge>());
    let recovered = receiver
        .fetch_blob_ephemeral_bounded(&large.hash, 3 * 1024 * 1024)
        .await
        .expect("larger permitted read")
        .expect("bytes");
    assert_eq!(recovered.len(), 2 * 1024 * 1024);
    assert!(recovered.iter().all(|byte| *byte == 42));
    let large_hash = iroh_blobs::Hash::from_str(large.hash.as_str()).expect("hash");
    assert!(
        receiver_node
            .blobs()
            .blobs()
            .get_bytes(large_hash)
            .await
            .is_err()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_fetch_uses_learned_remote_info_when_imported_ticket_is_stale() {
    let sender_dir = tempdir().expect("sender tempdir");
    let receiver_dir = tempdir().expect("receiver tempdir");
    let config = TransportNetworkConfig::loopback();

    let sender_node = IrohDocsNode::persistent_with_config(sender_dir.path(), config.clone())
        .await
        .expect("sender node");
    let receiver_node = IrohDocsNode::persistent_with_config(receiver_dir.path(), config.clone())
        .await
        .expect("receiver node");

    let sender = IrohBlobService::new(sender_node.clone());
    let receiver = IrohBlobService::new(receiver_node.clone());

    let stale_sender_ticket = format!("{}@127.0.0.1:1", sender_node.endpoint().addr().id);
    receiver
        .import_peer_ticket(&stale_sender_ticket)
        .await
        .expect("import stale sender ticket");

    let receiver_addr = receiver_node.endpoint().addr();
    let connection = sender_node
        .endpoint()
        .connect(receiver_addr, iroh_blobs::ALPN)
        .await
        .expect("seed incoming sender connection");
    drop(connection);

    timeout(Duration::from_secs(5), async {
        loop {
            if receiver_node
                .endpoint()
                .remote_info(sender_node.endpoint().addr().id)
                .await
                .is_some()
            {
                return;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("receiver should learn sender remote info");

    let stored = sender
        .put_blob(b"stale-ticket-fallback".to_vec(), "video/mp4")
        .await
        .expect("put blob");

    let payload = receiver.fetch_blob(&stored.hash).await.expect("fetch blob");

    assert_eq!(payload, Some(b"stale-ticket-fallback".to_vec()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connect_candidates_prefers_direct_remote_info_before_relay_hint() {
    let sender_dir = tempdir().expect("sender tempdir");
    let receiver_dir = tempdir().expect("receiver tempdir");
    let config = TransportNetworkConfig::loopback();

    let sender_node = IrohDocsNode::persistent_with_config(sender_dir.path(), config.clone())
        .await
        .expect("sender node");
    let receiver_node = IrohDocsNode::persistent_with_config(receiver_dir.path(), config)
        .await
        .expect("receiver node");

    let receiver = IrohBlobService::new(receiver_node.clone());
    let relay_url = "https://relay.example.invalid/".parse().expect("relay url");
    let sender_addr =
        iroh::EndpointAddr::new(sender_node.endpoint().id()).with_relay_url(relay_url);

    let seeded = sender_node
        .endpoint()
        .connect(receiver_node.endpoint().addr(), iroh_blobs::ALPN)
        .await
        .expect("seed connection");
    drop(seeded);

    timeout(Duration::from_secs(5), async {
        loop {
            if receiver_node
                .endpoint()
                .remote_info(sender_node.endpoint().id())
                .await
                .is_some()
            {
                return;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("receiver should learn sender remote info");

    let candidates = receiver.connect_candidates(&sender_addr).await;
    assert!(!candidates.is_empty());
    assert_ne!(candidates[0], sender_addr);
    assert!(candidates[0].relay_urls().next().is_none());
    assert_eq!(candidates.last(), Some(&sender_addr));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn learn_peer_snapshots_remote_info_addrs_for_future_blob_fetches() {
    let sender_dir = tempdir().expect("sender tempdir");
    let receiver_dir = tempdir().expect("receiver tempdir");
    let config = TransportNetworkConfig::loopback();

    let sender_node = IrohDocsNode::persistent_with_config(sender_dir.path(), config.clone())
        .await
        .expect("sender node");
    let receiver_node = IrohDocsNode::persistent_with_config(receiver_dir.path(), config)
        .await
        .expect("receiver node");

    let sender = IrohBlobService::new(sender_node.clone());
    let receiver = IrohBlobService::new(receiver_node.clone());

    let seeded = sender_node
        .endpoint()
        .connect(receiver_node.endpoint().addr(), iroh_blobs::ALPN)
        .await
        .expect("seed connection");
    drop(seeded);

    timeout(Duration::from_secs(5), async {
        loop {
            if receiver_node
                .endpoint()
                .remote_info(sender_node.endpoint().id())
                .await
                .is_some()
            {
                return;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("receiver should learn sender remote info");

    receiver
        .learn_peer(&sender_node.endpoint().id().to_string())
        .await
        .expect("learn sender peer");

    let learned = receiver.fetch_peers().await;
    assert!(
        learned
            .iter()
            .find(|peer| peer.id == sender_node.endpoint().id())
            .is_some_and(|peer| !peer.is_empty()),
        "learned peer should retain usable address information"
    );

    let stored = sender
        .put_blob(b"learned-peer-fetch".to_vec(), "image/png")
        .await
        .expect("put blob");

    let payload = receiver.fetch_blob(&stored.hash).await.expect("fetch blob");
    assert_eq!(payload, Some(b"learned-peer-fetch".to_vec()));
}
