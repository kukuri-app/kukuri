use super::*;
use futures_util::StreamExt;
use kukuri_app_api::{AppService, ServiceHandles};
use kukuri_blob_service::BlobService;
use kukuri_docs_sync::DocsSync;
use kukuri_store::MemoryStore;
use kukuri_transport::Transport;
use tempfile::tempdir;
use tokio::time::{Duration, timeout};

mod account_docs_author;

// #1152 / ADR 0046 §6.2: desktop が実際に使う `ReloadableBlobService` 越しでも、
// ephemeral 取得は remote の bytes をローカルへ保存せず、状態確認は remote から取得しない。
// 既定実装へ落ちる method があると、黙って永続化する `fetch_blob` に戻る。
#[tokio::test]
async fn reloadable_blob_service_keeps_ephemeral_fetch_and_local_status_non_persistent() {
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
    let receiver =
        ReloadableBlobService::new(Arc::new(IrohBlobService::new(receiver_node.clone())));

    let bound_port = sender_node
        .endpoint()
        .bound_sockets()
        .into_iter()
        .find(|addr| addr.port() != 0)
        .map(|addr| addr.port());
    let ticket = kukuri_transport::encode_endpoint_ticket(
        &sender_node.endpoint().addr(),
        &TransportNetworkConfig {
            advertised_host: Some("127.0.0.1".to_string()),
            advertised_port: bound_port,
            ..config.clone()
        },
    )
    .expect("sender ticket");
    receiver
        .import_peer_ticket(&ticket)
        .await
        .expect("import ticket");

    let stored = sender
        .put_blob(b"adult-display-enabled-media".to_vec(), "image/png")
        .await
        .expect("put blob");
    let inner = receiver.current().await;

    assert_eq!(
        receiver
            .local_blob_status(&stored.hash)
            .await
            .expect("local status"),
        BlobStatus::Missing
    );
    assert_eq!(
        inner
            .local_blob_status(&stored.hash)
            .await
            .expect("inner local status"),
        BlobStatus::Missing,
        "local status check must not persist the remote blob"
    );

    let payload = timeout(
        Duration::from_secs(20),
        receiver.fetch_blob_ephemeral(&stored.hash),
    )
    .await
    .expect("ephemeral fetch timeout")
    .expect("ephemeral fetch");
    assert_eq!(payload, Some(b"adult-display-enabled-media".to_vec()));
    assert_eq!(
        inner
            .local_blob_status(&stored.hash)
            .await
            .expect("inner local status after ephemeral fetch"),
        BlobStatus::Missing,
        "ephemeral fetch through the reloadable wrapper must not persist the blob"
    );
}

#[test]
fn runtime_connectivity_rebuild_helper_skips_rebuild_when_relay_urls_are_unchanged() {
    let relay_url = "https://relay.example.com".to_string();
    assert!(!should_rebuild_runtime_connectivity(
        std::slice::from_ref(&relay_url),
        std::slice::from_ref(&relay_url),
    ));
}

#[test]
fn runtime_connectivity_rebuild_helper_rebuilds_for_static_peer_relay_change() {
    let current = "https://relay-a.example.com".to_string();
    let next = "https://relay-b.example.com".to_string();
    assert!(should_rebuild_runtime_connectivity(
        std::slice::from_ref(&current),
        std::slice::from_ref(&next),
    ));
}

#[test]
fn runtime_connectivity_rebuild_helper_rebuilds_for_non_static_peer_relay_change() {
    let current = "https://relay-a.example.com".to_string();
    let next = "https://relay-b.example.com".to_string();
    assert!(should_rebuild_runtime_connectivity(
        std::slice::from_ref(&current),
        std::slice::from_ref(&next),
    ));
}

#[tokio::test]
async fn runtime_connectivity_rebuild_preserves_manual_ticket_peers() {
    let (_relay_map, relay_url, _guard) = iroh::test_utils::run_relay_server()
        .await
        .expect("relay server");
    let dir = tempdir().expect("tempdir");
    let discovery_config = DiscoveryConfig::static_peer_default();
    let candidate_store = Arc::new(
        SqliteStore::connect_file(dir.path().join("account.db"))
            .await
            .expect("account store"),
    );
    let stack_a = SharedIrohStack::new(
        &dir.path().join("stack-a"),
        TransportNetworkConfig::loopback(),
        &discovery_config,
        &[],
        DhtDiscoveryOptions::disabled(),
        TransportRelayConfig::default(),
        Some(candidate_store),
    )
    .await
    .expect("stack a");
    let stack_b = SharedIrohStack::new(
        &dir.path().join("stack-b"),
        TransportNetworkConfig::loopback(),
        &discovery_config,
        &[],
        DhtDiscoveryOptions::disabled(),
        TransportRelayConfig::default(),
        None,
    )
    .await
    .expect("stack b");

    let ticket_b = stack_b
        .transport
        .current()
        .await
        .export_ticket()
        .await
        .expect("export ticket b")
        .expect("ticket b value");
    stack_a
        .transport
        .current()
        .await
        .import_ticket(ticket_b.as_str())
        .await
        .expect("import transport ticket");
    stack_a
        .docs_sync
        .current()
        .await
        .import_peer_ticket(ticket_b.as_str())
        .await
        .expect("import docs ticket");
    stack_a
        .blob_service
        .current()
        .await
        .import_peer_ticket(ticket_b.as_str())
        .await
        .expect("import blob ticket");

    timeout(
        Duration::from_secs(30),
        stack_a.rebuild(
            &discovery_config,
            &[],
            TransportRelayConfig {
                iroh_relay_urls: vec![relay_url.to_string()],
            },
        ),
    )
    .await
    .expect("stack rebuild timeout")
    .expect("stack rebuild");

    let peer_id_b = stack_b
        .current
        .lock()
        .await
        .as_ref()
        .expect("stack b")
        .node
        .endpoint()
        .id();
    assert!(
        stack_a
            .transport
            .discovery()
            .await
            .expect("discovery")
            .manual_ticket_peer_ids
            .contains(&peer_id_b.to_string())
    );
    assert!(
        stack_a
            .docs_sync
            .current()
            .await
            .remote_read_candidates()
            .await
            .iter()
            .any(|peer| peer.id == peer_id_b)
    );
    let stored = stack_b
        .blob_service
        .put_blob(b"candidate-ledger".to_vec(), "text/plain")
        .await
        .expect("remote blob");
    assert_eq!(
        stack_a
            .blob_service
            .fetch_blob(&stored.hash)
            .await
            .expect("remote fetch"),
        Some(b"candidate-ledger".to_vec())
    );

    timeout(Duration::from_secs(30), stack_a.shutdown_checked())
        .await
        .expect("stack a shutdown timeout")
        .expect("stack a shutdown");
    timeout(Duration::from_secs(30), stack_b.shutdown_checked())
        .await
        .expect("stack b shutdown timeout")
        .expect("stack b shutdown");
}

#[tokio::test]
async fn reloadable_account_offer_route_recovers_only_when_new_stack_is_vacant() {
    let dir = tempdir().expect("tempdir");
    let discovery_config = DiscoveryConfig::static_peer_default();
    let stack = SharedIrohStack::new(
        &dir.path().join("account-route-rebuild"),
        TransportNetworkConfig::loopback(),
        &discovery_config,
        &[],
        DhtDiscoveryOptions::disabled(),
        TransportRelayConfig::default(),
        None,
    )
    .await
    .expect("stack");
    let account = KukuriKeys::generate().public_key();
    let (old_lease, mut old_stream, _) = stack
        .transport
        .subscribe_receive_offers(&account)
        .await
        .expect("old route");

    timeout(
        Duration::from_secs(30),
        stack.rebuild(&discovery_config, &[], TransportRelayConfig::default()),
    )
    .await
    .expect("rebuild timeout")
    .expect("rebuild");
    assert_ne!(
        stack
            .transport
            .receive_offer_transport_instance()
            .await
            .unwrap(),
        old_lease.transport_instance()
    );
    assert!(
        timeout(Duration::from_secs(1), old_stream.next())
            .await
            .expect("old stream closes")
            .is_none()
    );
    assert!(
        stack
            .transport
            .resubscribe_receive_offers_if_current(&account, old_lease)
            .await
            .unwrap()
            .is_none()
    );
    let (new_lease, _new_stream, _) = stack
        .transport
        .subscribe_receive_offers_if_vacant(&account)
        .await
        .unwrap()
        .expect("vacant rebuilt route");
    assert_eq!(
        new_lease.transport_instance(),
        stack
            .transport
            .receive_offer_transport_instance()
            .await
            .unwrap()
    );
    assert!(
        stack
            .transport
            .subscribe_receive_offers_if_vacant(&account)
            .await
            .unwrap()
            .is_none(),
        "another owner already holds the rebuilt route"
    );
    stack
        .transport
        .unsubscribe_receive_offers(&account, old_lease)
        .await
        .unwrap();
    stack
        .transport
        .unsubscribe_receive_offers(&account, new_lease)
        .await
        .unwrap();
    timeout(Duration::from_secs(30), stack.shutdown_checked())
        .await
        .expect("shutdown timeout")
        .expect("shutdown");
}

#[tokio::test]
async fn app_account_listener_reclaims_vacant_route_after_stack_rebuild() {
    let dir = tempdir().expect("tempdir");
    let discovery_config = DiscoveryConfig::static_peer_default();
    let stack = SharedIrohStack::new(
        &dir.path().join("app-offer-rebuild"),
        TransportNetworkConfig::loopback(),
        &discovery_config,
        &[],
        DhtDiscoveryOptions::disabled(),
        TransportRelayConfig::default(),
        None,
    )
    .await
    .expect("stack");
    let keys = KukuriKeys::generate();
    let account = keys.public_key();
    let store = Arc::new(MemoryStore::default());
    let app = AppService::from_handles(ServiceHandles::new(
        store.clone(),
        store,
        stack.transport.clone(),
        stack.transport.clone(),
        stack.docs_sync.clone(),
        stack.blob_service.clone(),
        keys,
    ));
    app.start_account_receive_offers().await.unwrap();
    stack
        .rebuild(&discovery_config, &[], TransportRelayConfig::default())
        .await
        .expect("rebuild");
    tokio::time::sleep(Duration::from_millis(3_500)).await;
    assert!(
        stack
            .transport
            .subscribe_receive_offers_if_vacant(&account)
            .await
            .unwrap()
            .is_none(),
        "the running app should reclaim the rebuilt route first"
    );
    app.shutdown().await;
    stack.shutdown_checked().await.unwrap();
}

/// `ReloadableBlobService` 越しの `unpin_blob` が内側の `IrohBlobService` まで届き、
/// Metaverse の pin tag と pin 状態を解放すること（#1157）。宣言漏れだと trait の既定実装
/// （no-op の `Ok(())`）に落ち、GC 後も blob が pin されたまま残る。
#[tokio::test]
async fn reloadable_blob_service_forwards_unpin_to_inner_service() {
    let dir = tempdir().expect("tempdir");
    let stack = SharedIrohStack::new(
        &dir.path().join("stack-unpin"),
        TransportNetworkConfig::loopback(),
        &DiscoveryConfig::static_peer_default(),
        &[],
        DhtDiscoveryOptions::disabled(),
        TransportRelayConfig::default(),
        None,
    )
    .await
    .expect("stack");
    let (node, inner) = {
        let current_guard = stack.current.lock().await;
        let current = current_guard.as_ref().expect("current stack");
        (current.node.clone(), current.blob_service.clone())
    };
    let pin_tag = |hash: &BlobHash| format!("kukuri/metaverse/pin/{}", hash.as_str());

    let stored = stack
        .blob_service
        .put_blob(b"metaverse asset".to_vec(), "application/octet-stream")
        .await
        .expect("put blob");
    stack
        .blob_service
        .pin_blob(&stored.hash)
        .await
        .expect("pin blob");
    assert_eq!(
        stack
            .blob_service
            .blob_status(&stored.hash)
            .await
            .expect("status after pin"),
        BlobStatus::Pinned
    );
    assert!(
        node.blobs()
            .tags()
            .get(pin_tag(&stored.hash))
            .await
            .expect("pin tag after pin")
            .is_some()
    );

    stack
        .blob_service
        .unpin_blob(&stored.hash)
        .await
        .expect("unpin blob");

    assert!(
        node.blobs()
            .tags()
            .get(pin_tag(&stored.hash))
            .await
            .expect("pin tag after unpin")
            .is_none(),
        "unpin_blob must delete the metaverse pin tag of the inner service"
    );
    assert_eq!(
        inner
            .blob_status(&stored.hash)
            .await
            .expect("inner status after unpin"),
        BlobStatus::Available
    );
    assert_eq!(
        stack
            .blob_service
            .blob_status(&stored.hash)
            .await
            .expect("wrapper status after unpin"),
        BlobStatus::Available
    );

    timeout(Duration::from_secs(30), stack.shutdown_checked())
        .await
        .expect("stack shutdown timeout")
        .expect("stack shutdown");
}

#[tokio::test]
async fn shared_stack_initializes_with_configured_relay_on_first_bind() {
    let (_relay_map, relay_url, _guard) = iroh::test_utils::run_relay_server()
        .await
        .expect("relay server");
    let dir = tempdir().expect("tempdir");
    let discovery_config = DiscoveryConfig::static_peer_default();
    let relay_config = TransportRelayConfig {
        iroh_relay_urls: vec![relay_url.to_string()],
    };
    let stack = SharedIrohStack::new(
        &dir.path().join("stack-relay"),
        TransportNetworkConfig::loopback(),
        &discovery_config,
        &[],
        DhtDiscoveryOptions::disabled(),
        relay_config,
        None,
    )
    .await
    .expect("stack");

    let current_guard = stack.current.lock().await;
    let current = current_guard.as_ref().expect("current stack");
    assert_eq!(current.node.relay_urls().await, vec![relay_url.clone()]);
    assert_eq!(
        current
            .transport
            .discovery()
            .await
            .expect("discovery")
            .connect_mode,
        ConnectMode::DirectOrRelay
    );
    drop(current_guard);

    timeout(Duration::from_secs(30), stack.shutdown_checked())
        .await
        .expect("stack shutdown timeout")
        .expect("stack shutdown");
}
