//! #1152: 表示用の状態確認（`local_blob_status`）が remote の blob を取得・永続化しないことを、
//! 実 Iroh 2 ノードで固定する。

use std::str::FromStr;

use kukuri_iroh_node::IrohDocsNode;
use kukuri_transport::TransportNetworkConfig;
use tempfile::tempdir;

use crate::tests::loopback_ticket;
use crate::{BlobService, BlobStatus, IrohBlobService};

#[tokio::test]
async fn local_blob_status_does_not_fetch_or_persist_remote_blob() {
    // #1152: 表示用の状態確認はローカルの有無だけを返し、remote peer が持つ blob を
    // 取得・永続化しない(`blob_status` は remote 取得で確かめるため挙動が異なる)。
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
        .put_blob(b"remote-only-attachment".to_vec(), "image/png")
        .await
        .expect("put blob");
    let hash = iroh_blobs::Hash::from_str(stored.hash.as_str()).expect("hash");

    assert_eq!(
        receiver
            .fetch_local_blob(&stored.hash)
            .await
            .expect("local read"),
        None
    );
    assert_eq!(
        sender
            .fetch_local_blob(&stored.hash)
            .await
            .expect("local bytes"),
        Some(b"remote-only-attachment".to_vec())
    );

    assert_eq!(
        receiver
            .local_blob_status(&stored.hash)
            .await
            .expect("receiver local status"),
        BlobStatus::Missing
    );
    assert!(
        receiver_node.blobs().blobs().get_bytes(hash).await.is_err(),
        "local status check must not persist the remote blob"
    );
    let display = receiver
        .prepare_display_fetch(&stored.hash)
        .await
        .expect("display admission");
    assert_eq!(
        display.await.expect("display bytes"),
        Some(b"remote-only-attachment".to_vec())
    );
    assert_eq!(
        receiver
            .fetch_local_blob(&stored.hash)
            .await
            .expect("display does not cache"),
        None
    );
    // pin記録だけがあり、実体は健全なremote peerにしか無い場合もローカル読取りは取得しない。
    receiver
        .pin_blob(&stored.hash)
        .await
        .expect("pin missing bytes");
    assert_eq!(
        receiver
            .local_blob_status(&stored.hash)
            .await
            .expect("pin state"),
        BlobStatus::Pinned
    );
    assert_eq!(
        receiver
            .fetch_local_blob(&stored.hash)
            .await
            .expect("local bytes"),
        None
    );
    assert!(receiver_node.blobs().blobs().get_bytes(hash).await.is_err());
    receiver
        .unpin_blob(&stored.hash)
        .await
        .expect("remove test pin");

    assert_eq!(
        sender
            .local_blob_status(&stored.hash)
            .await
            .expect("sender local status"),
        BlobStatus::Available
    );
    sender.pin_blob(&stored.hash).await.expect("pin blob");
    assert_eq!(
        sender
            .local_blob_status(&stored.hash)
            .await
            .expect("pinned local status"),
        BlobStatus::Pinned
    );

    // 取得を伴う `blob_status` とは区別される: remote から取得した後はローカルに在る。
    assert_eq!(
        receiver
            .blob_status(&stored.hash)
            .await
            .expect("receiver fetching status"),
        BlobStatus::Available
    );
    assert_eq!(
        receiver
            .local_blob_status(&stored.hash)
            .await
            .expect("receiver local status after fetch"),
        BlobStatus::Available
    );
    assert_eq!(
        receiver
            .fetch_local_blob(&stored.hash)
            .await
            .expect("downloaded local bytes"),
        Some(b"remote-only-attachment".to_vec())
    );
}

#[tokio::test]
async fn cancelling_display_fetch_closes_the_actual_blob_stream_without_caching() {
    use iroh::endpoint::presets;
    let server = iroh::Endpoint::builder(presets::Minimal)
        .alpns(vec![iroh_blobs::ALPN.to_vec()])
        .bind()
        .await
        .expect("server endpoint");
    let root = tempdir().expect("client root");
    let config = TransportNetworkConfig::loopback();
    let node = IrohDocsNode::persistent_with_config(root.path(), config.clone())
        .await
        .expect("client node");
    let client = std::sync::Arc::new(IrohBlobService::new(node));
    client
        .import_peer_ticket(&loopback_ticket(&server, &config))
        .await
        .expect("peer");
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (stopped_tx, stopped_rx) = tokio::sync::oneshot::channel();
    let endpoint = server.clone();
    let provider = tokio::spawn(async move {
        let connection = endpoint
            .accept()
            .await
            .expect("incoming")
            .await
            .expect("connection");
        let (send, mut recv) = connection.accept_bi().await.expect("blob stream");
        let mut request = [0_u8; 128];
        assert!(recv.read(&mut request).await.expect("request").is_some());
        let _ = started_tx.send(());
        // No blob header is sent. Cancellation must stop this real QUIC receive stream.
        // STOP_SENDINGと接続全体の終了は、どちらも転送を続けられない取消結果。
        let _ = send.stopped().await;
        let _ = stopped_tx.send(());
    });
    let hash = kukuri_core::BlobHash::new(blake3::hash(b"delayed manifest").to_hex().to_string());
    let reader = client.clone();
    let requested = hash.clone();
    let fetch = tokio::spawn(async move {
        reader
            .prepare_display_fetch(&requested)
            .await
            .expect("display admission")
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(10), started_rx)
        .await
        .expect("fetch started")
        .expect("start signal");
    fetch.abort();
    let _ = fetch.await;
    tokio::time::timeout(std::time::Duration::from_secs(5), stopped_rx)
        .await
        .expect("stream cancelled")
        .expect("stop signal");
    assert_eq!(
        client.fetch_local_blob(&hash).await.expect("local cache"),
        None
    );
    provider.await.expect("provider");
    server.close().await;
}
