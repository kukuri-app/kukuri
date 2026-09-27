use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transport_custom_relay_static_peer_seed_peers_with_addr_hints_sync_hints() {
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
    let topic = TopicId::new("kukuri:topic:relay-seed-hint-roundtrip");
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

    transport_a
        .configure_discovery(
            DiscoveryMode::StaticPeer,
            false,
            vec![seed_peer_from_ticket(&ticket_b)],
            Vec::new(),
        )
        .await
        .expect("configure a");
    transport_b
        .configure_discovery(
            DiscoveryMode::StaticPeer,
            false,
            vec![seed_peer_from_ticket(&ticket_a)],
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
        "custom relay static peer with addr hints",
    )
    .await;

    // characterization: 実データ経路が direct(active IP)である限り、relay URL が
    // 構成されていても active_path は direct_p2p のままで fallback にならない。
    let snapshot_a =
        wait_for_topic_active_path(&transport_a, &ConnectionPath::DirectP2p, "transport a").await;
    assert_eq!(snapshot_a.active_path, ConnectionPath::DirectP2p);
    assert_eq!(snapshot_a.fallback_peer_count, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transport_custom_relay_static_peer_seed_peers_ignore_stale_addr_hints() {
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
    let topic = TopicId::new("kukuri:topic:relay-seed-stale-addr-hint-roundtrip");
    let peer_id_a = transport_a.endpoint.id().to_string();
    let peer_id_b = transport_b.endpoint.id().to_string();
    let (mut stream_a, mut stream_b) = tokio::try_join!(
        transport_a.subscribe_hints(&topic),
        transport_b.subscribe_hints(&topic)
    )
    .expect("subscribe hints");

    transport_a
        .configure_discovery(
            DiscoveryMode::StaticPeer,
            false,
            vec![SeedPeer {
                endpoint_id: peer_id_b.clone(),
                addr_hint: Some("127.0.0.1:9".into()),
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
                addr_hint: Some("127.0.0.1:9".into()),
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
        "custom relay static peer with stale addr hints",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn transport_custom_relay_three_clients_multiple_topics_with_stale_addr_hints() {
    let (_relay_map, relay_url, _guard) = iroh::test_utils::run_relay_server()
        .await
        .expect("relay server");
    let relay_config = TransportRelayConfig {
        iroh_relay_urls: vec![relay_url.to_string()],
    }
    .normalized();
    let config = TransportNetworkConfig::loopback();
    let (transport_a, transport_b, transport_c) = tokio::try_join!(
        IrohGossipTransport::bind_with_options(
            config.clone(),
            DhtDiscoveryOptions::disabled(),
            relay_config.clone(),
        ),
        IrohGossipTransport::bind_with_options(
            config.clone(),
            DhtDiscoveryOptions::disabled(),
            relay_config.clone(),
        ),
        IrohGossipTransport::bind_with_options(
            config,
            DhtDiscoveryOptions::disabled(),
            relay_config,
        )
    )
    .expect("transports");

    let topic_one = TopicId::new("kukuri:topic:relay-three-client-stale-one");
    let topic_two = TopicId::new("kukuri:topic:relay-three-client-stale-two");
    let peer_id_a = transport_a.endpoint.id().to_string();
    let peer_id_b = transport_b.endpoint.id().to_string();
    let peer_id_c = transport_c.endpoint.id().to_string();
    let (
        mut stream_a_one,
        mut stream_b_one,
        mut stream_c_one,
        _stream_a_two,
        mut stream_b_two,
        mut stream_c_two,
    ) = tokio::try_join!(
        transport_a.subscribe_hints(&topic_one),
        transport_b.subscribe_hints(&topic_one),
        transport_c.subscribe_hints(&topic_one),
        transport_a.subscribe_hints(&topic_two),
        transport_b.subscribe_hints(&topic_two),
        transport_c.subscribe_hints(&topic_two),
    )
    .expect("subscribe hints");

    tokio::try_join!(
        transport_a.configure_discovery(
            DiscoveryMode::StaticPeer,
            false,
            vec![
                SeedPeer {
                    endpoint_id: peer_id_b.clone(),
                    addr_hint: Some("127.0.0.1:9".into()),
                },
                SeedPeer {
                    endpoint_id: peer_id_c.clone(),
                    addr_hint: Some("127.0.0.1:9".into()),
                },
            ],
            Vec::new(),
        ),
        transport_b.configure_discovery(
            DiscoveryMode::StaticPeer,
            false,
            vec![
                SeedPeer {
                    endpoint_id: peer_id_a.clone(),
                    addr_hint: Some("127.0.0.1:9".into()),
                },
                SeedPeer {
                    endpoint_id: peer_id_c.clone(),
                    addr_hint: Some("127.0.0.1:9".into()),
                },
            ],
            Vec::new(),
        ),
        transport_c.configure_discovery(
            DiscoveryMode::StaticPeer,
            false,
            vec![
                SeedPeer {
                    endpoint_id: peer_id_a.clone(),
                    addr_hint: Some("127.0.0.1:9".into()),
                },
                SeedPeer {
                    endpoint_id: peer_id_b.clone(),
                    addr_hint: Some("127.0.0.1:9".into()),
                },
            ],
            Vec::new(),
        ),
    )
    .expect("configure stale seed peers");

    wait_for_hint_roundtrip(
        HintRoundtripParticipant {
            transport: &transport_a,
            stream: &mut stream_a_one,
            expected_source_peer: None,
        },
        HintRoundtripParticipant {
            transport: &transport_b,
            stream: &mut stream_b_one,
            expected_source_peer: None,
        },
        &topic_one,
        Duration::from_secs(30),
        "custom relay three clients stale topic one a-b",
    )
    .await;
    wait_for_hint_roundtrip(
        HintRoundtripParticipant {
            transport: &transport_b,
            stream: &mut stream_b_two,
            expected_source_peer: None,
        },
        HintRoundtripParticipant {
            transport: &transport_c,
            stream: &mut stream_c_two,
            expected_source_peer: None,
        },
        &topic_two,
        Duration::from_secs(30),
        "custom relay three clients stale topic two b-c",
    )
    .await;
    wait_for_hint_roundtrip(
        HintRoundtripParticipant {
            transport: &transport_a,
            stream: &mut stream_a_one,
            expected_source_peer: None,
        },
        HintRoundtripParticipant {
            transport: &transport_c,
            stream: &mut stream_c_one,
            expected_source_peer: None,
        },
        &topic_one,
        Duration::from_secs(30),
        "custom relay three clients stale topic one a-c",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn transport_unreachable_home_relay_does_not_starve_concurrent_direct_dials() {
    let relay_config = TransportRelayConfig {
        iroh_relay_urls: vec!["https://127.0.0.1:1".to_string()],
    }
    .normalized();
    let config = TransportNetworkConfig::loopback();
    let (transport_seed, transport_leaf_one, transport_leaf_two) = tokio::try_join!(
        IrohGossipTransport::bind_with_options(
            config.clone(),
            DhtDiscoveryOptions::disabled(),
            relay_config.clone(),
        ),
        IrohGossipTransport::bind_with_options(
            config.clone(),
            DhtDiscoveryOptions::disabled(),
            relay_config.clone(),
        ),
        IrohGossipTransport::bind_with_options(
            config,
            DhtDiscoveryOptions::disabled(),
            relay_config,
        )
    )
    .expect("transports");

    let topic = TopicId::new("kukuri:topic:unreachable-relay-concurrent-dial");
    let ticket_seed = transport_seed
        .export_ticket()
        .await
        .expect("ticket seed")
        .expect("ticket seed value");
    let ticket_leaf_one = transport_leaf_one
        .export_ticket()
        .await
        .expect("ticket leaf one")
        .expect("ticket leaf one value");
    let ticket_leaf_two = transport_leaf_two
        .export_ticket()
        .await
        .expect("ticket leaf two")
        .expect("ticket leaf two value");

    let (mut stream_seed, mut stream_leaf_one, mut stream_leaf_two) = tokio::try_join!(
        transport_seed.subscribe_hints(&topic),
        transport_leaf_one.subscribe_hints(&topic),
        transport_leaf_two.subscribe_hints(&topic),
    )
    .expect("subscribe hints");

    tokio::try_join!(
        transport_seed.configure_discovery(
            DiscoveryMode::StaticPeer,
            false,
            vec![
                seed_peer_from_ticket(&ticket_leaf_one),
                seed_peer_from_ticket(&ticket_leaf_two),
            ],
            Vec::new(),
        ),
        transport_leaf_one.configure_discovery(
            DiscoveryMode::StaticPeer,
            false,
            vec![seed_peer_from_ticket(&ticket_seed)],
            Vec::new(),
        ),
        transport_leaf_two.configure_discovery(
            DiscoveryMode::StaticPeer,
            false,
            vec![seed_peer_from_ticket(&ticket_seed)],
            Vec::new(),
        ),
    )
    .expect("configure unreachable-relay seed peers");

    let join_timeout = initial_topic_join_timeout();
    wait_for_hint_roundtrip(
        HintRoundtripParticipant {
            transport: &transport_seed,
            stream: &mut stream_seed,
            expected_source_peer: None,
        },
        HintRoundtripParticipant {
            transport: &transport_leaf_one,
            stream: &mut stream_leaf_one,
            expected_source_peer: None,
        },
        &topic,
        join_timeout,
        "unreachable relay concurrent dial seed-leaf-one",
    )
    .await;
    wait_for_hint_roundtrip(
        HintRoundtripParticipant {
            transport: &transport_seed,
            stream: &mut stream_seed,
            expected_source_peer: None,
        },
        HintRoundtripParticipant {
            transport: &transport_leaf_two,
            stream: &mut stream_leaf_two,
            expected_source_peer: None,
        },
        &topic,
        join_timeout,
        "unreachable relay concurrent dial seed-leaf-two",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transport_custom_relay_lookup_connects_unknown_peer_without_dht_publish() {
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

    let endpoint_b = transport_b.endpoint.id();
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
    .expect("custom relay lookup connect timeout");

    drop(connection);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transport_peer_snapshot_reports_seeded_dht_mode() {
    let testnet = Testnet::new(5).await.expect("testnet");
    let transport = IrohGossipTransport::bind_with_discovery(
        TransportNetworkConfig::loopback(),
        DhtDiscoveryOptions::with_bootstrap(&testnet.bootstrap),
    )
    .await
    .expect("transport");
    wait_for_endpoint_in_testnet(&transport.endpoint, &testnet).await;
    let local_endpoint_id = transport
        .discovery()
        .await
        .expect("discovery")
        .local_endpoint_id;
    transport
        .configure_discovery(
            DiscoveryMode::SeededDht,
            false,
            vec![SeedPeer {
                endpoint_id: local_endpoint_id.clone(),
                addr_hint: None,
            }],
            Vec::new(),
        )
        .await
        .expect("configure discovery");

    let snapshot = transport.discovery().await.expect("discovery snapshot");
    let peers = transport.peers().await.expect("peer snapshot");

    assert_eq!(snapshot.mode, DiscoveryMode::SeededDht);
    assert_eq!(snapshot.connect_mode, ConnectMode::DirectOnly);
    assert_eq!(snapshot.configured_seed_peer_count, 1);
    assert_eq!(snapshot.bootstrap_seed_peer_count, 0);
    assert_eq!(peers.configured_peer_count, 1);
    assert_eq!(
        transport
            .peer_page(ConnectivityPeerKind::ConfiguredSeed, None, None, 64)
            .await
            .expect("configured seed page")
            .peer_ids,
        vec![local_endpoint_id]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transport_empty_seed_list_stays_idle_without_error() {
    let testnet = Testnet::new(5).await.expect("testnet");
    let transport = IrohGossipTransport::bind_with_discovery(
        TransportNetworkConfig::loopback(),
        DhtDiscoveryOptions::with_bootstrap(&testnet.bootstrap),
    )
    .await
    .expect("transport");
    wait_for_endpoint_in_testnet(&transport.endpoint, &testnet).await;

    transport
        .configure_discovery(DiscoveryMode::SeededDht, false, Vec::new(), Vec::new())
        .await
        .expect("configure discovery");

    let discovery = transport.discovery().await.expect("discovery");
    let peers = transport.peers().await.expect("peers");

    assert_eq!(discovery.mode, DiscoveryMode::SeededDht);
    assert_eq!(discovery.configured_seed_peer_count, 0);
    assert_eq!(discovery.bootstrap_seed_peer_count, 0);
    assert!(discovery.last_discovery_error.is_none());
    assert_eq!(peers.peer_count, 0);
    assert!(peers.last_error.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn topic_hint_late_subscriber_eventually_clears_missing_peer_ids_over_relay() {
    let (_relay_map, relay_url, _guard) = iroh::test_utils::run_relay_server()
        .await
        .expect("relay server");
    let relay_config = TransportRelayConfig {
        iroh_relay_urls: vec![relay_url.to_string()],
    }
    .normalized();
    let network_config = TransportNetworkConfig::loopback();
    let transport_a = IrohGossipTransport::bind_with_options(
        network_config.clone(),
        DhtDiscoveryOptions::disabled(),
        relay_config.clone(),
    )
    .await
    .expect("transport a");
    let transport_b = IrohGossipTransport::bind_with_options(
        network_config.clone(),
        DhtDiscoveryOptions::disabled(),
        relay_config.clone(),
    )
    .await
    .expect("transport b");
    let transport_c = IrohGossipTransport::bind_with_options(
        network_config,
        DhtDiscoveryOptions::disabled(),
        relay_config,
    )
    .await
    .expect("transport c");

    let discovery_a = transport_a.discovery().await.expect("discovery a");
    let discovery_b = transport_b.discovery().await.expect("discovery b");
    let discovery_c = transport_c.discovery().await.expect("discovery c");

    transport_a
        .configure_discovery(
            DiscoveryMode::StaticPeer,
            false,
            Vec::new(),
            vec![
                SeedPeer {
                    endpoint_id: discovery_b.local_endpoint_id.clone(),
                    addr_hint: None,
                },
                SeedPeer {
                    endpoint_id: discovery_c.local_endpoint_id.clone(),
                    addr_hint: None,
                },
            ],
        )
        .await
        .expect("configure a");
    transport_b
        .configure_discovery(
            DiscoveryMode::StaticPeer,
            false,
            Vec::new(),
            vec![
                SeedPeer {
                    endpoint_id: discovery_a.local_endpoint_id.clone(),
                    addr_hint: None,
                },
                SeedPeer {
                    endpoint_id: discovery_c.local_endpoint_id.clone(),
                    addr_hint: None,
                },
            ],
        )
        .await
        .expect("configure b");
    transport_c
        .configure_discovery(
            DiscoveryMode::StaticPeer,
            false,
            Vec::new(),
            vec![
                SeedPeer {
                    endpoint_id: discovery_a.local_endpoint_id.clone(),
                    addr_hint: None,
                },
                SeedPeer {
                    endpoint_id: discovery_b.local_endpoint_id.clone(),
                    addr_hint: None,
                },
            ],
        )
        .await
        .expect("configure c");

    let topic = TopicId::new("kukuri:topic:late-peer");
    let join_timeout = initial_topic_join_timeout();
    let _stream_a = transport_a
        .subscribe_hints(&topic)
        .await
        .expect("subscribe a");
    let mut stream_c = transport_c
        .subscribe_hints(&topic)
        .await
        .expect("subscribe c");

    timeout(join_timeout, async {
        loop {
            let peers_c = transport_c.peers().await.expect("peers c before b");
            assert!(
                peers_c
                    .topic_diagnostics
                    .iter()
                    .any(|topic| topic.topic == "hint/kukuri:topic:late-peer"),
                "diag c before b"
            );
            if missing_peer_ids(&transport_c, "hint/kukuri:topic:late-peer")
                .await
                .contains(&discovery_b.local_endpoint_id)
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("initial partial join timeout");

    let mut stream_b = transport_b
        .subscribe_hints(&topic)
        .await
        .expect("subscribe b");
    wait_for_hint_roundtrip(
        HintRoundtripParticipant {
            transport: &transport_b,
            stream: &mut stream_b,
            expected_source_peer: None,
        },
        HintRoundtripParticipant {
            transport: &transport_c,
            stream: &mut stream_c,
            expected_source_peer: None,
        },
        &topic,
        join_timeout,
        "late-subscriber",
    )
    .await;

    match timeout(join_timeout, async {
        loop {
            let peers_c = transport_c.peers().await.expect("peers c after b");
            assert!(
                peers_c
                    .topic_diagnostics
                    .iter()
                    .any(|topic| topic.topic == "hint/kukuri:topic:late-peer"),
                "diag c after b"
            );
            if !missing_peer_ids(&transport_c, "hint/kukuri:topic:late-peer")
                .await
                .contains(&discovery_b.local_endpoint_id)
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
            let peers_b = transport_b.peers().await.expect("peers b after timeout");
            let peers_c = transport_c.peers().await.expect("peers c after timeout");
            panic!(
                "late subscriber should clear missing peer ids: b={} c={}",
                format_peer_snapshot(&peers_b),
                format_peer_snapshot(&peers_c)
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gossip_low_level_roundtrip_baseline() {
    let endpoint_a = EndpointBuilder::new(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .bind_addr(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
        .expect("bind addr a")
        .bind()
        .await
        .expect("endpoint a");
    let gossip_a = Gossip::builder().spawn(endpoint_a.clone());
    let _router_a = Router::builder(endpoint_a.clone())
        .accept(GOSSIP_ALPN, gossip_a.clone())
        .spawn();

    let endpoint_b = EndpointBuilder::new(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .bind_addr(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
        .expect("bind addr b")
        .bind()
        .await
        .expect("endpoint b");
    let gossip_b = Gossip::builder().spawn(endpoint_b.clone());
    let _router_b = Router::builder(endpoint_b.clone())
        .accept(GOSSIP_ALPN, gossip_b.clone())
        .spawn();

    let discovery = MemoryLookup::new();
    discovery.add_endpoint_info(endpoint_a.addr());
    discovery.add_endpoint_info(endpoint_b.addr());
    endpoint_a
        .address_lookup()
        .expect("address lookup a")
        .add(discovery.clone());
    endpoint_b
        .address_lookup()
        .expect("address lookup b")
        .add(discovery);

    let topic = topic_to_gossip_id(&TopicId::new("kukuri:topic:baseline"));
    let peer_a = endpoint_a.id();
    let peer_b = endpoint_b.id();
    let topic_a = gossip_a
        .subscribe(topic, vec![peer_b])
        .await
        .expect("subscribe a");
    let (sender_a, mut receiver_a) = topic_a.split();
    let topic_b = gossip_b
        .subscribe(topic, vec![peer_a])
        .await
        .expect("subscribe b");
    let (_sender_b, mut receiver_b) = topic_b.split();

    timeout(Duration::from_secs(10), receiver_a.joined())
        .await
        .expect("join a timeout")
        .expect("join a");
    timeout(Duration::from_secs(10), receiver_b.joined())
        .await
        .expect("join b timeout")
        .expect("join b");

    let event = build_post_envelope(
        &generate_keys(),
        &TopicId::new("kukuri:topic:baseline"),
        "hello baseline",
        None,
    )
    .expect("event");
    sender_a
        .broadcast(serde_json::to_vec(&event).expect("serialize").into())
        .await
        .expect("broadcast");

    let received = timeout(Duration::from_secs(10), async {
        while let Some(message) = receiver_b.next().await {
            match message.expect("gossip event") {
                GossipEvent::Received(message) => {
                    let parsed: KukuriEnvelope =
                        serde_json::from_slice(&message.content).expect("parse event");
                    return parsed;
                }
                GossipEvent::Lagged => continue,
                _ => {}
            }
        }
        panic!("receiver b closed");
    })
    .await
    .expect("receive timeout");

    assert_eq!(received.id, event.id);
    assert_eq!(
        received
            .post_content()
            .expect("post content")
            .expect("post content")
            .payload_ref,
        kukuri_core::PayloadRef::InlineText {
            text: "hello baseline".into(),
        }
    );
    endpoint_a.close().await;
    endpoint_b.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transport_static_peer_can_connect_endpoint() {
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

    transport_a
        .import_ticket(&ticket_b)
        .await
        .expect("import b");
    let addr_b = parse_endpoint_ticket(&ticket_b).expect("parse ticket b");
    timeout(
        Duration::from_secs(5),
        transport_a.endpoint.connect(addr_b, GOSSIP_ALPN),
    )
    .await
    .expect("connect timeout")
    .expect("connect");
}

async fn missing_peer_ids(transport: &IrohGossipTransport, topic: &str) -> Vec<String> {
    transport
        .peer_page(ConnectivityPeerKind::Missing, Some(topic), None, 64)
        .await
        .expect("missing page")
        .peer_ids
}
