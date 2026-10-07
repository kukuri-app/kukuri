//! 公開コンテンツの発見の設定（#1632 D2、ADR 0063）。
use super::*;
use crate::discovery::{SetPublicBlobDiscoveryRequest, load_discovery_config_from_file};
use crate::stack::effective_dht_options;
use kukuri_transport::{PublicBlobIndex, SeedPeer, TransportRelayConfig};

/// 試験の DHT（bootstrap 無しの孤立した node）と、照会を送らない補助 index の server。
fn public_blob_options() -> DhtDiscoveryOptions {
    let mut builder = DhtBuilder::default();
    builder.bootstrap::<String>(&[]).port(0);
    DhtDiscoveryOptions {
        enabled: true,
        dht_builder: Some(builder),
        public_blob_index: Some(PublicBlobIndex::Servers(vec![std::net::SocketAddrV4::new(
            std::net::Ipv4Addr::LOCALHOST,
            9,
        )])),
        ..DhtDiscoveryOptions::default()
    }
}

/// 新しい profile も、欄の無い既存の profile も、公開コンテンツの発見をオンで始める。
#[tokio::test]
async fn existing_and_new_profiles_start_with_public_content_discovery_on() {
    let dir = tempdir().expect("tempdir");
    let db = dir.path().join("existing.db");
    fs::write(
        discovery_config_path(&db),
        br#"{"mode":"static_peer","seed_peers":[]}"#,
    )
    .expect("write an existing discovery config");
    assert!(
        resolve_discovery_config_from_env(&db)
            .await
            .expect("existing config")
            .public_blob_discovery
    );
    assert!(DiscoveryConfig::static_peer_default().public_blob_discovery);
    assert!(DiscoveryConfig::seeded_dht_default().public_blob_discovery);
}

/// オンで補助 index があれば、Community Node の利用中・static_peer でも DHT を組み立てる。オフか補助 index が無ければ、
/// 今までどおり（Community Node の利用中は DHT を使わず、発見もしない）。
#[test]
fn public_content_discovery_decides_whether_to_build_the_dht() {
    let community_node = TransportRelayConfig {
        iroh_relay_urls: vec!["https://relay.example".to_string()],
    };
    let seeds = [SeedPeer {
        endpoint_id: "1".repeat(64),
        addr_hint: None,
    }];
    let mut config = DiscoveryConfig::static_peer_default();
    let on = effective_dht_options(&public_blob_options(), &config, &seeds, &community_node);
    assert!(on.enabled && on.public_blob_index.is_some());

    config.public_blob_discovery = false;
    let off = effective_dht_options(&public_blob_options(), &config, &seeds, &community_node);
    assert!(!off.enabled && off.public_blob_index.is_none());
    let off_direct = effective_dht_options(
        &public_blob_options(),
        &config,
        &[],
        &TransportRelayConfig::default(),
    );
    assert!(off_direct.enabled && off_direct.public_blob_index.is_none());

    config.public_blob_discovery = true;
    let no_index = effective_dht_options(
        &DhtDiscoveryOptions::disabled(),
        &config,
        &seeds,
        &community_node,
    );
    assert!(!no_index.enabled && no_index.public_blob_index.is_none());
}

/// 切替は設定を保存し、関連する要求を止めてから stack を組み直す。オフの stack は発見を持たない。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn switching_public_content_discovery_rebuilds_the_stack() {
    let dir = tempdir().expect("tempdir");
    let db = dir.path().join("public-blobs.db");
    let runtime = DesktopRuntime::new_with_config_and_identity_and_discovery(
        &db,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
        DiscoveryConfig::static_peer_default(),
        public_blob_options(),
        None,
    )
    .await
    .expect("runtime");
    assert!(runtime.iroh_stack.public_blobs_enabled());
    let generation = runtime.iroh_stack.generation();

    let config = runtime
        .set_public_blob_discovery(SetPublicBlobDiscoveryRequest { enabled: false })
        .await
        .expect("turn discovery off");
    assert!(!config.public_blob_discovery);
    assert_eq!(runtime.iroh_stack.generation(), generation + 1);
    assert!(!runtime.iroh_stack.public_blobs_enabled());
    let stored = load_discovery_config_from_file(&db)
        .await
        .expect("read stored config")
        .expect("stored config");
    assert!(!DiscoveryConfig::from_stored(stored, false).public_blob_discovery);

    runtime
        .set_public_blob_discovery(SetPublicBlobDiscoveryRequest { enabled: true })
        .await
        .expect("turn discovery on");
    assert_eq!(runtime.iroh_stack.generation(), generation + 2);
    assert!(runtime.iroh_stack.public_blobs_enabled());
    runtime.shutdown().await;
}
