use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transport_relay_only_peer_reports_relay_fallback() {
    let (_relay_map, relay_url, _guard) = iroh::test_utils::run_relay_server()
        .await
        .expect("relay server");
    let relay_config = TransportRelayConfig {
        iroh_relay_urls: vec![relay_url.to_string()],
    }
    .normalized();
    // transport_a は relay-only(IP transport なし)。実データは relay 経由でしか流れない。
    let transport_a = IrohGossipTransport::bind_relay_only_for_tests(
        TransportNetworkConfig::loopback(),
        relay_config.clone(),
    )
    .await
    .expect("transport a");
    let transport_b = IrohGossipTransport::bind_with_options(
        TransportNetworkConfig::loopback(),
        DhtDiscoveryOptions::disabled(),
        relay_config,
    )
    .await
    .expect("transport b");
    let topic = TopicId::new("kukuri:topic:relay-only-fallback");
    let peer_id_a = transport_a.endpoint.id().to_string();
    let peer_id_b = transport_b.endpoint.id().to_string();
    let (mut stream_a, mut stream_b) = tokio::try_join!(
        transport_a.subscribe_hints(&topic),
        transport_b.subscribe_hints(&topic)
    )
    .expect("subscribe hints");

    // endpoint id のみの seed(addr_hint なし)。到達情報は relay fallback lookup が供給する。
    transport_a
        .configure_discovery(
            DiscoveryMode::StaticPeer,
            false,
            vec![SeedPeer {
                endpoint_id: peer_id_b.clone(),
                addr_hint: None,
            }],
            Vec::new(),
        )
        .await
        .expect("configure a");
    transport_b
        .configure_discovery(
            DiscoveryMode::StaticPeer,
            false,
            vec![SeedPeer {
                endpoint_id: peer_id_a.clone(),
                addr_hint: None,
            }],
            Vec::new(),
        )
        .await
        .expect("configure b");

    wait_for_hint_roundtrip(
        HintRoundtripParticipant {
            transport: &transport_a,
            stream: &mut stream_a,
            expected_source_peer: Some(peer_id_a.as_str()),
        },
        HintRoundtripParticipant {
            transport: &transport_b,
            stream: &mut stream_b,
            expected_source_peer: Some(peer_id_b.as_str()),
        },
        &topic,
        Duration::from_secs(20),
        "relay-only fallback roundtrip",
    )
    .await;

    // relay-only 側: 相手への実データ経路は relay しかないので relay_fallback。
    let snapshot_a =
        wait_for_topic_active_path(&transport_a, &ConnectionPath::RelayFallback, "transport a")
            .await;
    assert_eq!(snapshot_a.active_path, ConnectionPath::RelayFallback);
    assert_eq!(snapshot_a.fallback_peer_count, 1);
    let topic_diag_a = snapshot_a
        .topic_diagnostics
        .iter()
        .find(|diag| diag.active_path == ConnectionPath::RelayFallback)
        .expect("topic diagnostic a");
    assert_eq!(topic_diag_a.fallback_peer_count, 1);
    assert_eq!(topic_diag_a.rendezvous_peer_count, 0);
    assert_eq!(
        transport_a
            .peer_page(
                ConnectivityPeerKind::Connected,
                Some(&topic_diag_a.topic),
                None,
                64
            )
            .await
            .expect("connected page")
            .peer_ids,
        vec![peer_id_b]
    );

    // IP transport を持つ側から見ても、相手には relay 経由でしか到達できないので relay_fallback。
    let snapshot_b =
        wait_for_topic_active_path(&transport_b, &ConnectionPath::RelayFallback, "transport b")
            .await;
    assert_eq!(snapshot_b.fallback_peer_count, 1);
    let _ = peer_id_a;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transport_custom_relay_bootstrap_seed_reports_relay_supported_p2p() {
    let (_relay_map, relay_url, _guard) = iroh::test_utils::run_relay_server()
        .await
        .expect("relay server");
    let relay_config = TransportRelayConfig {
        iroh_relay_urls: vec![relay_url.to_string()],
    }
    .normalized();
    let config = TransportNetworkConfig::loopback();
    let transport_a = IrohGossipTransport::bind_with_options(
        config.clone(),
        DhtDiscoveryOptions::disabled(),
        relay_config.clone(),
    )
    .await
    .expect("transport a");
    let transport_b = IrohGossipTransport::bind_with_options(
        config,
        DhtDiscoveryOptions::disabled(),
        relay_config,
    )
    .await
    .expect("transport b");
    let topic = TopicId::new("kukuri:topic:relay-bootstrap-seed-path");
    let peer_id_a = transport_a.endpoint.id().to_string();
    let peer_id_b = transport_b.endpoint.id().to_string();
    let (mut stream_a, mut stream_b) = tokio::try_join!(
        transport_a.subscribe_hints(&topic),
        transport_b.subscribe_hints(&topic)
    )
    .expect("subscribe hints");

    let ticket_a = transport_a
        .export_ticket()
        .await
        .expect("ticket a")
        .expect("ticket a value");
    let ticket_b = transport_b
        .export_ticket()
        .await
        .expect("ticket b")
        .expect("ticket b value");

    // rendezvous 由来の bootstrap seed として接続する(configured seed ではなく 4 番目の引数)。
    transport_a
        .configure_discovery(
            DiscoveryMode::StaticPeer,
            false,
            Vec::new(),
            vec![seed_peer_from_ticket(&ticket_b)],
        )
        .await
        .expect("configure a");
    transport_b
        .configure_discovery(
            DiscoveryMode::StaticPeer,
            false,
            Vec::new(),
            vec![seed_peer_from_ticket(&ticket_a)],
        )
        .await
        .expect("configure b");

    wait_for_hint_roundtrip(
        HintRoundtripParticipant {
            transport: &transport_a,
            stream: &mut stream_a,
            expected_source_peer: Some(peer_id_a.as_str()),
        },
        HintRoundtripParticipant {
            transport: &transport_b,
            stream: &mut stream_b,
            expected_source_peer: Some(peer_id_b.as_str()),
        },
        &topic,
        Duration::from_secs(20),
        "bootstrap seed relay supported roundtrip",
    )
    .await;

    // 両側 IP transport あり + bootstrap seed 経由 → 既存判定どおり relay_supported_p2p のまま
    // (relay URL が構成されていても fallback にはならない)。
    let snapshot_a = wait_for_topic_active_path(
        &transport_a,
        &ConnectionPath::RelaySupportedP2p,
        "transport a",
    )
    .await;
    assert_eq!(snapshot_a.active_path, ConnectionPath::RelaySupportedP2p);
    let topic_diag_a = snapshot_a
        .topic_diagnostics
        .iter()
        .find(|diag| diag.active_path == ConnectionPath::RelaySupportedP2p)
        .expect("topic diagnostic a");
    assert_eq!(topic_diag_a.rendezvous_peer_count, 1);
    let _ = peer_id_b;
    assert_ne!(topic_diag_a.active_path, ConnectionPath::RelayFallback);
}
