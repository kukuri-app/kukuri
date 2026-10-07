use super::*;

impl IrohGossipTransport {
    pub async fn bind(network_config: TransportNetworkConfig) -> Result<Self> {
        Self::bind_with_options(
            network_config,
            DhtDiscoveryOptions::disabled(),
            TransportRelayConfig::default(),
        )
        .await
    }

    pub async fn bind_with_options(
        network_config: TransportNetworkConfig,
        dht_options: DhtDiscoveryOptions,
        relay_config: TransportRelayConfig,
    ) -> Result<Self> {
        let relay_config = relay_config.normalized();
        let relay_urls = Arc::new(StdRwLock::new(relay_config.parsed_relay_urls()?));
        let (endpoint, discovery, connection_paths) = bind_endpoint_with_options(
            network_config.bind_addr,
            &dht_options,
            &relay_config,
            Arc::clone(&relay_urls),
            None,
        )
        .await?;
        Ok(Self::spawn_gossip_transport(
            endpoint,
            discovery,
            network_config,
            &relay_config,
            relay_urls,
            connection_paths,
        ))
    }

    /// relay-only endpoint(IP transport なし)で bind するテスト専用コンストラクタ。
    /// loopback では直接 addr が holepunch で有効化されるため、実データが relay 経由に
    /// なる構成はこの knob でしか安定して再現できない。
    #[cfg(test)]
    pub(crate) async fn bind_relay_only_for_tests(
        network_config: TransportNetworkConfig,
        relay_config: TransportRelayConfig,
    ) -> Result<Self> {
        let relay_config = relay_config.normalized();
        let relay_urls = Arc::new(StdRwLock::new(relay_config.parsed_relay_urls()?));
        let (endpoint, discovery, connection_paths) =
            bind_endpoint_relay_only(&relay_config, Arc::clone(&relay_urls)).await?;
        Ok(Self::spawn_gossip_transport(
            endpoint,
            discovery,
            network_config,
            &relay_config,
            relay_urls,
            connection_paths,
        ))
    }

    fn spawn_gossip_transport(
        endpoint: Endpoint,
        discovery: Arc<MemoryLookup>,
        network_config: TransportNetworkConfig,
        relay_config: &TransportRelayConfig,
        relay_urls: Arc<StdRwLock<Vec<RelayUrl>>>,
        connection_paths: GossipConnectionPaths,
    ) -> Self {
        let gossip = Gossip::builder().spawn(endpoint.clone());
        let router = Router::builder(endpoint.clone())
            .accept(GOSSIP_ALPN, gossip.clone())
            .spawn();

        Self {
            receive_offer_instance: NEXT_RECEIVE_OFFER_TRANSPORT_INSTANCE
                .fetch_add(1, Ordering::Relaxed),
            endpoint,
            gossip,
            _router: Some(router),
            discovery,
            network_config,
            configured_seed_peers: Arc::new(Mutex::new(BTreeMap::new())),
            bootstrap_seed_peers: Arc::new(Mutex::new(BTreeMap::new())),
            imported_peers: Arc::new(Mutex::new(BTreeMap::new())),
            account_store: None,
            imported_cursor: Arc::new(Mutex::new(None)),
            hot_peer_ids: Arc::new(Mutex::new(VecDeque::new())),
            gossip_health: Arc::new(crate::peers::BlobPeerHealth::default()),
            bootstrap_cursor: Arc::new(Mutex::new((0, [None, None, None]))),
            candidates_added: watch::channel(0).0,
            receive_destinations: Mutex::new(receive_destination::DestinationWindow::default()),
            receive_destination_probes: Semaphore::new(2),
            subscribed_topics: Arc::new(Mutex::new(BTreeSet::new())),
            topic_states: Arc::new(Mutex::new(HashMap::new())),
            short_term_topics: Mutex::new(VecDeque::new()),
            receive_offer_topic: Mutex::new(None),
            outbound_offer_holds: Mutex::new(VecDeque::new()),
            offer_closed: AtomicBool::new(false),
            hint_closed: AtomicBool::new(false),
            #[cfg(test)]
            hint_existing_snapshot_observed: Arc::new(Notify::new()),
            offer_shutdown_notify: Notify::new(),
            #[cfg(test)]
            offer_receiver_tasks: Arc::new(AtomicUsize::new(0)),
            #[cfg(test)]
            offer_hold_tasks: Arc::new(AtomicUsize::new(0)),
            #[cfg(test)]
            offer_publish_joined: Arc::new(Notify::new()),
            #[cfg(test)]
            offer_publish_join_started: Arc::new(Notify::new()),
            topic_warmups: Arc::new(TopicWarmupCoordinator::default()),
            last_error: Arc::new(Mutex::new(None)),
            discovery_mode: Arc::new(Mutex::new(DiscoveryMode::StaticPeer)),
            connect_mode: Arc::new(Mutex::new(relay_config.connect_mode())),
            relay_urls,
            env_locked: Arc::new(Mutex::new(false)),
            status_changes: StatusChanges::default(),
            connection_paths,
            #[cfg(any(test, feature = "test-support"))]
            status_read_steps: Arc::new(AtomicU64::new(0)),
        }
    }

    pub async fn bind_with_discovery(
        network_config: TransportNetworkConfig,
        dht_options: DhtDiscoveryOptions,
    ) -> Result<Self> {
        Self::bind_with_options(network_config, dht_options, TransportRelayConfig::default()).await
    }

    pub fn from_shared_parts(
        endpoint: Endpoint,
        gossip: Gossip,
        discovery: Arc<MemoryLookup>,
        network_config: TransportNetworkConfig,
        relay_config: TransportRelayConfig,
    ) -> Result<Self> {
        let relay_config = relay_config.normalized();
        let relay_urls = Arc::new(StdRwLock::new(relay_config.parsed_relay_urls()?));
        discovery.add_endpoint_info(endpoint.addr());
        Ok(Self {
            receive_offer_instance: NEXT_RECEIVE_OFFER_TRANSPORT_INSTANCE
                .fetch_add(1, Ordering::Relaxed),
            endpoint,
            gossip,
            _router: None,
            discovery,
            network_config,
            configured_seed_peers: Arc::new(Mutex::new(BTreeMap::new())),
            bootstrap_seed_peers: Arc::new(Mutex::new(BTreeMap::new())),
            imported_peers: Arc::new(Mutex::new(BTreeMap::new())),
            account_store: None,
            imported_cursor: Arc::new(Mutex::new(None)),
            hot_peer_ids: Arc::new(Mutex::new(VecDeque::new())),
            gossip_health: Arc::new(crate::peers::BlobPeerHealth::default()),
            bootstrap_cursor: Arc::new(Mutex::new((0, [None, None, None]))),
            candidates_added: watch::channel(0).0,
            receive_destinations: Mutex::new(receive_destination::DestinationWindow::default()),
            receive_destination_probes: Semaphore::new(2),
            subscribed_topics: Arc::new(Mutex::new(BTreeSet::new())),
            topic_states: Arc::new(Mutex::new(HashMap::new())),
            short_term_topics: Mutex::new(VecDeque::new()),
            receive_offer_topic: Mutex::new(None),
            outbound_offer_holds: Mutex::new(VecDeque::new()),
            offer_closed: AtomicBool::new(false),
            hint_closed: AtomicBool::new(false),
            #[cfg(test)]
            hint_existing_snapshot_observed: Arc::new(Notify::new()),
            offer_shutdown_notify: Notify::new(),
            #[cfg(test)]
            offer_receiver_tasks: Arc::new(AtomicUsize::new(0)),
            #[cfg(test)]
            offer_hold_tasks: Arc::new(AtomicUsize::new(0)),
            #[cfg(test)]
            offer_publish_joined: Arc::new(Notify::new()),
            #[cfg(test)]
            offer_publish_join_started: Arc::new(Notify::new()),
            topic_warmups: Arc::new(TopicWarmupCoordinator::default()),
            last_error: Arc::new(Mutex::new(None)),
            discovery_mode: Arc::new(Mutex::new(DiscoveryMode::StaticPeer)),
            connect_mode: Arc::new(Mutex::new(relay_config.connect_mode())),
            relay_urls,
            env_locked: Arc::new(Mutex::new(false)),
            status_changes: StatusChanges::default(),
            connection_paths: GossipConnectionPaths::default(),
            #[cfg(any(test, feature = "test-support"))]
            status_read_steps: Arc::new(AtomicU64::new(0)),
        })
    }

    pub async fn bind_local() -> Result<Self> {
        Self::bind(TransportNetworkConfig::loopback()).await
    }

    /// 同じ EndpointId で bind し直す試験用（stack の作り直しの模擬）。
    #[cfg(test)]
    pub(crate) async fn bind_local_with_secret(secret_key: SecretKey) -> Result<Self> {
        let network_config = TransportNetworkConfig::loopback();
        let relay_config = TransportRelayConfig::default().normalized();
        let relay_urls = Arc::new(StdRwLock::new(relay_config.parsed_relay_urls()?));
        let (endpoint, discovery, connection_paths) = bind_endpoint_with_options(
            network_config.bind_addr,
            &DhtDiscoveryOptions::disabled(),
            &relay_config,
            Arc::clone(&relay_urls),
            Some(secret_key),
        )
        .await?;
        Ok(Self::spawn_gossip_transport(
            endpoint,
            discovery,
            network_config,
            &relay_config,
            relay_urls,
            connection_paths,
        ))
    }

    /// 通信状態の変わった部分の印を、この印へ付ける(#1221 R2-D)。topic を購読する前に呼ぶ。
    pub fn with_status_changes(mut self, changes: StatusChanges) -> Self {
        self.status_changes = changes;
        self
    }

    /// 共有endpointへbind前に取り付けたhookを渡す。topicの購読前に呼ぶ。
    pub fn with_connection_paths(mut self, paths: GossipConnectionPaths) -> Self {
        self.connection_paths = paths;
        self
    }

    pub fn with_account_store(mut self, store: Arc<crate::PeerCandidateStore>) -> Self {
        self.account_store = Some(store);
        self
    }
}

pub(crate) async fn bind_endpoint_with_options(
    bind_addr: SocketAddr,
    dht_options: &DhtDiscoveryOptions,
    relay_config: &TransportRelayConfig,
    relay_urls: Arc<StdRwLock<Vec<RelayUrl>>>,
    secret_key: Option<SecretKey>,
) -> Result<(Endpoint, Arc<MemoryLookup>, GossipConnectionPaths)> {
    let discovery = Arc::new(MemoryLookup::new());
    let connection_paths = GossipConnectionPaths::default();
    let mut builder = build_endpoint_builder(
        EndpointBuilder::new(presets::Minimal).relay_mode(relay_config.relay_mode()?),
        &discovery,
        Some(dht_options),
        relay_urls,
    )?;
    builder = builder.hooks(connection_paths.clone());
    if let Some(secret_key) = secret_key {
        builder = builder.secret_key(secret_key);
    }
    #[cfg(test)]
    {
        builder = builder.ca_tls_config(CaTlsConfig::insecure_skip_verify());
    }
    // ブラウザには UDP の socket が無い（ADR 0056 §8）。
    #[cfg(not(target_family = "wasm"))]
    {
        builder = apply_bind(builder, bind_addr)?;
    }
    #[cfg(target_family = "wasm")]
    let _ = bind_addr;
    let endpoint = builder
        .bind()
        .await
        .context("failed to bind iroh endpoint")?;
    prepare_endpoint_for_discovery(&endpoint, &discovery, relay_config).await?;
    Ok((endpoint, discovery, connection_paths))
}
/// relay-only endpoint bind(テスト専用)。IP transport を除去し、実データを relay 経由に強制する。
/// iroh 本家の relay-only テストと同じ `relay_mode(Custom) + clear_ip_transports` パターン。
#[cfg(test)]
async fn bind_endpoint_relay_only(
    relay_config: &TransportRelayConfig,
    relay_urls: Arc<StdRwLock<Vec<RelayUrl>>>,
) -> Result<(Endpoint, Arc<MemoryLookup>, GossipConnectionPaths)> {
    let discovery = Arc::new(MemoryLookup::new());
    let connection_paths = GossipConnectionPaths::default();
    let mut builder = build_endpoint_builder(
        EndpointBuilder::new(presets::Minimal).relay_mode(relay_config.relay_mode()?),
        &discovery,
        Some(&DhtDiscoveryOptions::disabled()),
        relay_urls,
    )?;
    builder = builder.hooks(connection_paths.clone());
    builder = builder.ca_tls_config(CaTlsConfig::insecure_skip_verify());
    builder = builder.clear_ip_transports();
    let endpoint = builder
        .bind()
        .await
        .context("failed to bind relay-only iroh endpoint")?;
    prepare_endpoint_for_discovery(&endpoint, &discovery, relay_config).await?;
    Ok((endpoint, discovery, connection_paths))
}

#[cfg(not(target_family = "wasm"))]
fn apply_bind(builder: EndpointBuilder, bind_addr: SocketAddr) -> Result<EndpointBuilder> {
    match bind_addr {
        SocketAddr::V4(addr) => builder
            .bind_addr(addr)
            .map_err(|error| anyhow!("failed to bind IPv4 address: {error}")),
        SocketAddr::V6(addr) => builder
            .bind_addr(addr)
            .map_err(|error| anyhow!("failed to bind IPv6 address: {error}")),
    }
}
