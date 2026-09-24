use super::*;

use n0_mainline::{DhtBuilder, Testnet};

use crate::test_support::{
    HintRoundtripParticipant, format_peer_snapshot, wait_for_hint_roundtrip,
};

async fn wait_for_endpoint_in_testnet(endpoint: &Endpoint, testnet: &Testnet) {
    let mut dht_builder = DhtBuilder::default();
    dht_builder.bootstrap(&testnet.bootstrap);
    let lookup = DhtAddressLookup::builder()
        .dht_builder(dht_builder)
        .no_publish()
        .addr_filter(AddrFilter::unfiltered())
        .build()
        .expect("dht lookup");
    timeout(Duration::from_secs(30), async {
        loop {
            if let Some(mut resolved) = lookup.resolve(endpoint.id())
                && let Some(Ok(item)) = resolved.next().await
                && item.endpoint_info().endpoint_id == endpoint.id()
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("resolve endpoint info from DHT");
}

fn seed_peer_from_ticket(ticket: &str) -> SeedPeer {
    let (endpoint_id, addr_hint) = ticket.split_once('@').expect("ticket host");
    SeedPeer {
        endpoint_id: endpoint_id.to_string(),
        addr_hint: Some(addr_hint.to_string()),
    }
}

#[tokio::test]
async fn bootstrap_candidates_read_four_peers_from_large_history() {
    let transport = IrohGossipTransport::bind_local().await.expect("transport");
    for index in 0_u64..1_000 {
        let mut bytes = [0; 32];
        bytes[..8].copy_from_slice(&index.to_be_bytes());
        let id = iroh::SecretKey::from_bytes(&bytes).public();
        transport
            .configured_seed_peers
            .lock()
            .await
            .insert(id.to_string(), EndpointAddr::new(id));
    }
    let first = transport.bootstrap_peers().await;
    assert!(
        first.len() <= 4,
        "one topic join should not materialize all known peers"
    );
    transport.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transport_two_process_hint_roundtrip_static_peer() {
    if std::env::var_os("GITHUB_ACTIONS").is_some() {
        return;
    }
    let transport_a = IrohGossipTransport::bind_local()
        .await
        .expect("transport a");
    let transport_b = IrohGossipTransport::bind_local()
        .await
        .expect("transport b");
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
    transport_a
        .import_ticket(&ticket_b)
        .await
        .expect("import b");
    transport_b
        .import_ticket(&ticket_a)
        .await
        .expect("import a");
    let topic = TopicId::new("kukuri:topic:transport");
    let join_timeout = initial_topic_join_timeout();
    let peer_id_a = transport_a.endpoint.id().to_string();
    let peer_id_b = transport_b.endpoint.id().to_string();
    let (mut stream_a, mut stream_b) = tokio::try_join!(
        transport_a.subscribe_hints(&topic),
        transport_b.subscribe_hints(&topic)
    )
    .expect("subscribe both");
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
        join_timeout,
        "static-peer",
    )
    .await;

    match timeout(join_timeout, async {
        loop {
            let peers_a = transport_a.peers().await.expect("peers a");
            let peers_b = transport_b.peers().await.expect("peers b");
            let diag_a = peers_a
                .topic_diagnostics
                .iter()
                .find(|topic| topic.topic == "hint/kukuri:topic:transport");
            let diag_b = peers_b
                .topic_diagnostics
                .iter()
                .find(|topic| topic.topic == "hint/kukuri:topic:transport");
            if peers_a.peer_count >= 1
                && peers_b.peer_count >= 1
                && diag_a.is_some_and(|topic| topic.peer_count >= 1)
                && diag_b.is_some_and(|topic| topic.peer_count >= 1)
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    {
        Ok(()) => {}
        Err(_) => {
            let peers_a = transport_a.peers().await.expect("peers a");
            let peers_b = transport_b.peers().await.expect("peers b");
            panic!(
                "peer snapshot timeout: a={} b={}",
                format_peer_snapshot(&peers_a),
                format_peer_snapshot(&peers_b)
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transport_import_ticket_updates_existing_topic_subscription() {
    let transport_a = IrohGossipTransport::bind_local()
        .await
        .expect("transport a");
    let transport_b = IrohGossipTransport::bind_local()
        .await
        .expect("transport b");
    let topic = TopicId::new("kukuri:topic:import-update");
    let join_timeout = Duration::from_secs(10);
    let peer_id_a = transport_a.endpoint.id().to_string();
    let peer_id_b = transport_b.endpoint.id().to_string();
    let (mut stream_a, mut stream_b) = tokio::try_join!(
        transport_a.subscribe_hints(&topic),
        transport_b.subscribe_hints(&topic)
    )
    .expect("subscribe both before import");

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
    transport_a
        .import_ticket(&ticket_b)
        .await
        .expect("import b after subscribe");
    transport_b
        .import_ticket(&ticket_a)
        .await
        .expect("import a after subscribe");

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
        join_timeout,
        "import-update",
    )
    .await;

    timeout(join_timeout, async {
        loop {
            let peers_a = transport_a.peers().await.expect("peers a");
            let peers_b = transport_b.peers().await.expect("peers b");
            let direct_a = peers_a.topic_diagnostics.iter().any(|diag| {
                diag.topic == "hint/kukuri:topic:import-update" && !diag.connected_peers.is_empty()
            });
            let direct_b = peers_b.topic_diagnostics.iter().any(|diag| {
                diag.topic == "hint/kukuri:topic:import-update" && !diag.connected_peers.is_empty()
            });
            if direct_a && direct_b {
                return;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("direct topic update timeout");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transport_seed_update_updates_existing_topic_subscription() {
    let transport_a = IrohGossipTransport::bind_local()
        .await
        .expect("transport a");
    let transport_b = IrohGossipTransport::bind_local()
        .await
        .expect("transport b");
    let topic = TopicId::new("kukuri:topic:seed-update");
    let join_timeout = Duration::from_secs(10);
    let peer_id_a = transport_a.endpoint.id().to_string();
    let peer_id_b = transport_b.endpoint.id().to_string();
    let (mut stream_a, mut stream_b) = tokio::try_join!(
        transport_a.subscribe_hints(&topic),
        transport_b.subscribe_hints(&topic)
    )
    .expect("subscribe both before seed update");

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
    transport_a
        .configure_discovery(
            DiscoveryMode::StaticPeer,
            false,
            vec![seed_peer_from_ticket(&ticket_b)],
            Vec::new(),
        )
        .await
        .expect("configure a after subscribe");
    transport_b
        .configure_discovery(
            DiscoveryMode::StaticPeer,
            false,
            vec![seed_peer_from_ticket(&ticket_a)],
            Vec::new(),
        )
        .await
        .expect("configure b after subscribe");

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
        join_timeout,
        "seed-update",
    )
    .await;

    timeout(join_timeout, async {
        loop {
            let peers_a = transport_a.peers().await.expect("peers a");
            let peers_b = transport_b.peers().await.expect("peers b");
            let direct_a = peers_a.topic_diagnostics.iter().any(|diag| {
                diag.topic == "hint/kukuri:topic:seed-update" && !diag.connected_peers.is_empty()
            });
            let direct_b = peers_b.topic_diagnostics.iter().any(|diag| {
                diag.topic == "hint/kukuri:topic:seed-update" && !diag.connected_peers.is_empty()
            });
            if direct_a && direct_b {
                return;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("direct seed update timeout");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transport_resubscribe_recreates_timed_out_topic_state() {
    let transport_a = IrohGossipTransport::bind_local()
        .await
        .expect("transport a");
    let transport_b = IrohGossipTransport::bind_local()
        .await
        .expect("transport b");
    let ticket_b = transport_b
        .export_ticket()
        .await
        .expect("ticket b")
        .expect("ticket b value");
    transport_b.shutdown().await;

    transport_a
        .configure_discovery(
            DiscoveryMode::StaticPeer,
            false,
            vec![seed_peer_from_ticket(&ticket_b)],
            Vec::new(),
        )
        .await
        .expect("configure a");

    let topic = TopicId::new("kukuri:topic:timed-out-resubscribe");
    let _stream = transport_a
        .subscribe_hints(&topic)
        .await
        .expect("initial subscribe");
    let topic_key = "hint/kukuri:topic:timed-out-resubscribe";
    let initial_last_error = {
        let topics = transport_a.topic_states.lock().await;
        topics
            .get(topic_key)
            .expect("initial topic state")
            .last_error
            .clone()
    };
    *initial_last_error.lock().await = Some("timed out waiting for initial topic join".to_string());

    let _stream = transport_a
        .subscribe_hints(&topic)
        .await
        .expect("resubscribe after join timeout");
    let recreated_last_error = {
        let topics = transport_a.topic_states.lock().await;
        topics
            .get(topic_key)
            .expect("recreated topic state")
            .last_error
            .clone()
    };

    assert_eq!(
        *recreated_last_error.lock().await,
        None,
        "resubscribe should recreate timed-out topic state so future joins can retry cleanly"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transport_seeded_dht_can_connect_by_endpoint_id_without_ticket() {
    let testnet = Testnet::new(5).await.expect("testnet");
    let config = TransportNetworkConfig::loopback();
    let transport_a = IrohGossipTransport::bind_with_discovery(
        config.clone(),
        DhtDiscoveryOptions::with_bootstrap(&testnet.bootstrap),
    )
    .await
    .expect("transport a");
    let transport_b = IrohGossipTransport::bind_with_discovery(
        config,
        DhtDiscoveryOptions::with_bootstrap(&testnet.bootstrap),
    )
    .await
    .expect("transport b");
    let discovery_a = transport_a.discovery().await.expect("discovery a");
    let discovery_b = transport_b.discovery().await.expect("discovery b");
    wait_for_endpoint_in_testnet(&transport_a.endpoint, &testnet).await;
    wait_for_endpoint_in_testnet(&transport_b.endpoint, &testnet).await;

    transport_a
        .configure_discovery(
            DiscoveryMode::SeededDht,
            false,
            vec![SeedPeer {
                endpoint_id: discovery_b.local_endpoint_id.clone(),
                addr_hint: None,
            }],
            Vec::new(),
        )
        .await
        .expect("configure a");
    transport_b
        .configure_discovery(
            DiscoveryMode::SeededDht,
            false,
            vec![SeedPeer {
                endpoint_id: discovery_a.local_endpoint_id.clone(),
                addr_hint: None,
            }],
            Vec::new(),
        )
        .await
        .expect("configure b");

    let endpoint_b = EndpointId::from_str(&discovery_b.local_endpoint_id).expect("endpoint b");
    let connection = timeout(Duration::from_secs(20), async {
        loop {
            match transport_a
                .endpoint
                .connect(EndpointAddr::new(endpoint_b), GOSSIP_ALPN)
                .await
            {
                Ok(connection) => return connection,
                Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        }
    })
    .await
    .expect("seeded dht connect timeout");

    drop(connection);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transport_custom_relay_static_peer_seed_peers_connect_without_ticket_import() {
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
        relay_config.clone(),
    )
    .await
    .expect("transport b");
    let discovery_a = transport_a.discovery().await.expect("discovery a");
    let discovery_b = transport_b.discovery().await.expect("discovery b");

    transport_a
        .configure_discovery(
            DiscoveryMode::StaticPeer,
            false,
            vec![SeedPeer {
                endpoint_id: discovery_b.local_endpoint_id.clone(),
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
                endpoint_id: discovery_a.local_endpoint_id.clone(),
                addr_hint: None,
            }],
            Vec::new(),
        )
        .await
        .expect("configure b");

    let endpoint_b = EndpointId::from_str(&discovery_b.local_endpoint_id).expect("endpoint b");
    let connection = timeout(Duration::from_secs(20), async {
        loop {
            match transport_a
                .endpoint
                .connect(EndpointAddr::new(endpoint_b), GOSSIP_ALPN)
                .await
            {
                Ok(connection) => return connection,
                Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        }
    })
    .await
    .expect("custom relay seed connect timeout");

    drop(connection);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_hint_payload_increments_counter_and_keeps_stream_healthy() {
    let transport_a = IrohGossipTransport::bind_local()
        .await
        .expect("transport a");
    let transport_b = IrohGossipTransport::bind_local()
        .await
        .expect("transport b");
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
    transport_a
        .import_ticket(&ticket_b)
        .await
        .expect("import b");
    transport_b
        .import_ticket(&ticket_a)
        .await
        .expect("import a");
    let topic = TopicId::new("kukuri:topic:invalid-hint");
    let join_timeout = initial_topic_join_timeout();
    let (mut stream_a, mut stream_b) = tokio::try_join!(
        transport_a.subscribe_hints(&topic),
        transport_b.subscribe_hints(&topic)
    )
    .expect("subscribe both");
    // gossip メッシュ確立と正常経路の成立を先に確認する。
    wait_for_hint_roundtrip(
        HintRoundtripParticipant {
            transport: &transport_a,
            stream: &mut stream_a,
            expected_source_peer: None,
        },
        HintRoundtripParticipant {
            transport: &transport_b,
            stream: &mut stream_b,
            expected_source_peer: None,
        },
        &topic,
        join_timeout,
        "invalid-hint-setup",
    )
    .await;

    // B の gossip sender から wire に生の不正 bytes を注入する
    // (hint_publish_hint_impl と同一経路。GossipHint の serde には触れない)。
    let hint_topic = kukuri_core::wire::hint_topic_id(&topic);
    {
        let states = transport_b.topic_states.lock().await;
        let state = states
            .get(hint_topic.as_str())
            .expect("hint topic state on b");
        let sender = state.sender.lock().await;
        sender
            .broadcast(b"not-a-gossip-hint".to_vec().into())
            .await
            .expect("broadcast invalid payload");
    }

    // A 側: 不正 hint がカウンタと last_error で観測できる(WP-C4)。
    timeout(join_timeout, async {
        loop {
            if transport_a.invalid_hint_count(&topic).await >= 1 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("invalid hint counter timeout");
    timeout(join_timeout, async {
        loop {
            let peers = transport_a.peers().await.expect("peers a");
            let diag = peers
                .topic_diagnostics
                .iter()
                .find(|diag| diag.topic == hint_topic.as_str());
            if diag.is_some_and(|diag| {
                diag.last_error.as_deref() == Some("failed to decode hint payload")
            }) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("invalid hint last_error timeout");

    // 正常経路は不変: 不正 hint の後も正常 hint が配送され、last_error が消える。
    let recovery_hint = GossipHint::TopicObjectsChanged {
        topic_id: topic.clone(),
        objects: vec![HintObjectRef {
            object_id: "invalid-hint-recovery".into(),
            object_kind: "post".into(),
            docs_author: None,
        }],
    };
    timeout(join_timeout, async {
        loop {
            transport_b
                .publish_hint(&topic, recovery_hint.clone())
                .await
                .expect("publish recovery hint");
            if let Ok(Some(envelope)) = timeout(Duration::from_millis(500), stream_a.next()).await
                && envelope.hint == recovery_hint
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("recovery hint delivery timeout");
    timeout(join_timeout, async {
        loop {
            let peers = transport_a.peers().await.expect("peers a");
            let diag = peers
                .topic_diagnostics
                .iter()
                .find(|diag| diag.topic == hint_topic.as_str());
            if diag.is_some_and(|diag| diag.last_error.is_none()) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("last_error clear timeout");
    assert!(transport_a.invalid_hint_count(&topic).await >= 1);
}

/// 単一 topic のスナップショットが expected の active_path に到達するまで待つ。
/// 接続直後は relay 経由 → holepunch で direct 化のような遷移があり得るため、
/// 経路判定の assert は一発判定ではなく収束待ちで行う。
async fn wait_for_topic_active_path(
    transport: &IrohGossipTransport,
    expected: &ConnectionPath,
    context: &str,
) -> PeerSnapshot {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let snapshot = transport.peers().await.expect("peer snapshot");
        if snapshot
            .topic_diagnostics
            .iter()
            .any(|topic| &topic.active_path == expected)
        {
            return snapshot;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!(
                "{context}: active_path did not reach {expected:?}: aggregate={:?}, topic_paths={:?}, fallback_peer_ids={:?}, snapshot={}",
                snapshot.active_path,
                snapshot
                    .topic_diagnostics
                    .iter()
                    .map(|topic| topic.active_path.clone())
                    .collect::<Vec<_>>(),
                snapshot.fallback_peer_ids,
                format_peer_snapshot(&snapshot)
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

mod connection_path;
mod connection_release;
mod controlled_gossip;
mod receive_offer;
mod relay_connectivity;
mod relay_overload;
