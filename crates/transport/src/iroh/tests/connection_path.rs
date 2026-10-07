use super::*;

use iroh::endpoint::transports::{PathSelection, PathSelectionContext, PathSelector};

#[derive(Debug, Default)]
struct SwitchPath(AtomicBool);

impl PathSelector for SwitchPath {
    fn select(&self, context: &PathSelectionContext<'_>) -> PathSelection {
        let ip = self.0.load(Ordering::Acquire);
        let mut selection = PathSelection::none();
        if let Some(path) = context
            .paths()
            .find(|path| path.network_path().is_ip() == ip)
        {
            selection.set(&path);
        }
        selection
    }
}

/// #1595 AC-1: 同じgossip接続・neighborを保ち、hintやstatusのpollなしで経路の変化を通知する。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn path_only_switch_marks_the_topic_without_hints() {
    let (relay_map, _, _relay) = iroh::test_utils::run_relay_server().await.expect("relay");
    let selector = Arc::new(SwitchPath::default());
    let connection_paths = GossipConnectionPaths::default();
    let bind = async || {
        EndpointBuilder::new(presets::Minimal)
            .relay_mode(RelayMode::Custom(relay_map.clone()))
            .ca_tls_config(CaTlsConfig::insecure_skip_verify())
            .alpns(vec![GOSSIP_ALPN.to_vec()])
            .clear_ip_transports()
            .bind_addr(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
            .expect("bind address")
            .transport_config(
                QuicTransportConfig::builder()
                    .default_path_max_idle_timeout(Duration::from_millis(500))
                    .default_path_keep_alive_interval(Duration::from_secs(1))
                    .send_observed_address_reports(false)
                    .receive_observed_address_reports(false)
                    .build(),
            )
            .path_selector(selector.clone())
            .hooks(connection_paths.clone())
            .bind()
            .await
            .expect("endpoint")
    };
    let (a, b) = tokio::join!(bind(), bind());
    tokio::join!(a.online(), b.online());
    // 既存接続に別の実IP経路を開き、selectorを再評価させるためのUDP転送。gossip接続は作り直さない。
    let mut proxies = Vec::new();
    for _ in 0..3 {
        let proxy = tokio::net::UdpSocket::bind("127.0.0.1:0")
            .await
            .expect("UDP proxy");
        let proxy_addr = proxy.local_addr().expect("proxy address");
        let destination = b.bound_sockets()[0];
        let proxy_task = n0_future::task::spawn(async move {
            let mut buf = vec![0; 65_536];
            let mut client = None;
            while let Ok((len, source)) = proxy.recv_from(&mut buf).await {
                let target = if source == destination {
                    let Some(client) = client else { continue };
                    client
                } else {
                    client = Some(source);
                    destination
                };
                proxy
                    .send_to(&buf[..len], target)
                    .await
                    .expect("proxy send");
            }
        });
        proxies.push((proxy_addr, proxy_task));
    }
    let gossip_a = Gossip::builder().spawn(a.clone());
    let gossip_b = Gossip::builder().spawn(b.clone());
    let changes = StatusChanges::default();
    let make = |endpoint: Endpoint, gossip: Gossip| {
        IrohGossipTransport::from_shared_parts(
            endpoint,
            gossip,
            Arc::new(MemoryLookup::new()),
            TransportNetworkConfig::loopback(),
            TransportRelayConfig::default(),
        )
        .expect("transport")
    };
    let transport_a = Arc::new(
        make(a.clone(), gossip_a.clone())
            .with_connection_paths(connection_paths.clone())
            .with_status_changes(changes.clone()),
    );
    let transport_b = make(b.clone(), gossip_b.clone());
    let topic = TopicId::new("kukuri:topic:path-only-switch");
    let hint_topic = kukuri_core::wire::hint_topic_id(&topic);
    let key = StatusKey::Topic(hint_topic.as_str().to_string());
    // 実observerと同じく、印の付いた時だけ診断を読み、1秒にまとめる。定期・手動読取りは行わない。
    let (pushed, mut statuses) = watch::channel(None::<(u64, PeerSnapshot)>);
    let observer = n0_future::task::spawn({
        let transport = transport_a.clone();
        async move {
            let mut sequence = 0;
            loop {
                assert_eq!(changes.changed().await, BTreeSet::from([key.clone()]));
                sequence += 1;
                pushed
                    .send(Some((
                        sequence,
                        transport.peers().await.expect("dirty status"),
                    )))
                    .expect("push");
                sleep(Duration::from_secs(1)).await;
            }
        }
    });
    let (_stream_a, _stream_b) = tokio::try_join!(
        transport_a.subscribe_hints(&topic),
        transport_b.subscribe_hints(&topic)
    )
    .expect("subscribe");
    let mut relay_addr = b.addr();
    relay_addr.addrs.retain(|addr| addr.is_relay());
    let (connection, accepted) = tokio::join!(a.connect(relay_addr, GOSSIP_ALPN), async {
        b.accept().await.expect("incoming").await.expect("accept")
    });
    let connection = connection.expect("connect");
    // hintを送らず、選択中の経路だけを維持する。backupのIPはidle timeoutで閉じる。
    let keep_alive = n0_future::task::spawn({
        let connection = connection.clone();
        async move {
            loop {
                connection
                    .send_datagram(vec![0].into())
                    .expect("QUIC keep alive");
                sleep(Duration::from_millis(100)).await;
            }
        }
    });
    gossip_a
        .handle_connection(connection.clone())
        .await
        .expect("gossip a");
    gossip_b
        .handle_connection(accepted.clone())
        .await
        .expect("gossip b");
    transport_a
        .topic_states
        .lock()
        .await
        .get(hint_topic.as_str())
        .expect("topic")
        .sender
        .lock()
        .await
        .join_peers(vec![b.id()])
        .await
        .expect("join");
    let neighbors = transport_a
        .topic_states
        .lock()
        .await
        .get(hint_topic.as_str())
        .expect("topic")
        .neighbors
        .clone();
    timeout(Duration::from_secs(10), async {
        while neighbors.read().await.is_empty() {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("neighbor");
    let original_neighbors = neighbors.read().await.clone();
    let mut paths = connection.paths_stream();
    let mut missed = Vec::new();
    for (step, ip) in [false, true, false].into_iter().enumerate() {
        let previous_sequence = statuses
            .borrow()
            .as_ref()
            .map_or(0, |(sequence, _)| *sequence);
        selector.0.store(ip, Ordering::Release);
        // 新しい到達候補でselectorを再評価させる。QUICの全経路keep-aliveは有効にしない。
        b.add_external_addr(proxies[step].0).await;
        tokio::join!(a.network_change(), b.network_change());
        timeout(Duration::from_secs(20), async {
            while let Some(paths) = paths.next().await {
                if paths.iter().any(|path| path.is_ip()) == ip
                    && paths.iter().any(|path| path.is_relay())
                {
                    return;
                }
            }
            panic!("connection closed");
        })
        .await
        .unwrap_or_else(|_| {
            panic!(
                "observable route at step {step}, ip={ip}, paths={:?}",
                connection.paths()
            )
        });
        let expected = if ip {
            ConnectionPath::DirectP2p
        } else {
            ConnectionPath::RelayFallback
        };
        match timeout(
            Duration::from_secs(3),
            statuses.wait_for(|status| {
                status.as_ref().is_some_and(|(sequence, status)| {
                    *sequence > previous_sequence
                        && status.peer_count == 1
                        && status.active_path == expected
                        && status.fallback_peer_count == usize::from(!ip)
                })
            }),
        )
        .await
        .map(|result| result.map(|status| status.clone()))
        {
            Ok(Ok(status)) => {
                let topic_status = &status.as_ref().expect("status").1.topic_diagnostics[0];
                assert_eq!(topic_status.active_path, expected);
                assert_eq!(topic_status.fallback_peer_count, usize::from(!ip));
                assert_eq!(topic_status.last_received_at, None, "no hints");
            }
            _ => {
                let last_pushed = statuses.borrow().clone();
                eprintln!(
                    "step {step}: pushed={:?}; remote={:?}; paths={:?}",
                    last_pushed,
                    a.remote_info(b.id()).await,
                    connection.paths()
                );
                missed.push(if step == 0 {
                    "baseline"
                } else if ip {
                    "relay→IP"
                } else {
                    "IP→relay"
                });
            }
        }
        assert_eq!(*neighbors.read().await, original_neighbors, "same neighbor");
        assert!(
            connection.close_reason().is_none() && accepted.close_reason().is_none(),
            "same QUIC connection"
        );
    }
    assert_eq!(connection_paths.watcher_count(), 1);
    transport_a
        .unsubscribe_hints(&topic)
        .await
        .expect("unsubscribe");
    assert_eq!(
        connection_paths.watcher_count(),
        0,
        "unsubscribe releases the path observer"
    );
    timeout(
        Duration::from_secs(3),
        statuses.wait_for(|status| {
            status
                .as_ref()
                .is_some_and(|(_, status)| status.topic_diagnostics.is_empty())
        }),
    )
    .await
    .expect("removed topic push")
    .expect("status channel");
    transport_a.shutdown().await;
    observer.abort();
    keep_alive.abort();
    gossip_a.shutdown().await.expect("shutdown a");
    gossip_b.shutdown().await.expect("shutdown b");
    tokio::join!(a.close(), b.close());
    for (_, task) in proxies {
        task.abort();
    }
    assert!(
        missed.is_empty(),
        "missing path-only status push: {missed:?}"
    );
}

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
