use super::super::*;

pub(crate) struct TestIrohStack {
    pub(crate) _node: Arc<IrohDocsNode>,
    pub(crate) transport: Arc<IrohGossipTransport>,
    pub(crate) docs_sync: Arc<IrohDocsSync>,
    pub(crate) blob_service: Arc<IrohBlobService>,
}

impl TestIrohStack {
    pub(crate) async fn new(root: &std::path::Path) -> Self {
        Self::new_with_options(
            root,
            DhtDiscoveryOptions::disabled(),
            TransportRelayConfig::default(),
        )
        .await
    }

    pub(crate) async fn new_with_dht(root: &std::path::Path, testnet: &Testnet) -> Self {
        let stack = Self::new_with_options(
            root,
            DhtDiscoveryOptions::with_bootstrap(&testnet.bootstrap),
            TransportRelayConfig::default(),
        )
        .await;
        wait_for_endpoint_in_testnet(stack._node.endpoint(), testnet).await;
        stack
    }

    pub(crate) async fn new_with_options(
        root: &std::path::Path,
        dht_options: DhtDiscoveryOptions,
        relay_config: TransportRelayConfig,
    ) -> Self {
        Self::new_with_network_options(
            root,
            kukuri_transport::TransportNetworkConfig::loopback(),
            dht_options,
            relay_config,
        )
        .await
    }

    pub(crate) async fn new_with_network_options(
        root: &std::path::Path,
        network_config: kukuri_transport::TransportNetworkConfig,
        dht_options: DhtDiscoveryOptions,
        relay_config: TransportRelayConfig,
    ) -> Self {
        let relay_config = relay_config.normalized();
        let node = IrohDocsNode::persistent_with_discovery_config(
            root,
            network_config.clone(),
            dht_options,
            relay_config.clone(),
        )
        .await
        .expect("iroh docs node");
        let transport = Arc::new(
            IrohGossipTransport::from_shared_parts(
                node.endpoint().clone(),
                node.gossip().clone(),
                node.discovery(),
                network_config,
                relay_config.clone(),
            )
            .expect("transport"),
        );
        let docs_sync = Arc::new(IrohDocsSync::new(node.clone()));
        let blob_service = Arc::new(IrohBlobService::new(node.clone()));
        Self {
            _node: node,
            transport,
            docs_sync,
            blob_service,
        }
    }
}

impl TestIrohStack {
    /// desktop の起動と同じく、この端末で app の account の受信 binding に答える(#1221 R5-C。private の参加は
    /// token の発行者の検証済み宛先から制御 record を読む)。
    pub(crate) async fn bind_account(&self, app: &AppService) {
        self._node
            .install_receive_binding(app.services.keys.clone())
            .await
            .expect("install receive binding");
    }
}

pub(crate) async fn wait_for_endpoint_in_testnet(endpoint: &iroh::Endpoint, testnet: &Testnet) {
    let mut builder = DhtBuilder::default();
    builder.bootstrap(&testnet.bootstrap);
    let lookup = DhtAddressLookup::builder()
        .dht_builder(builder)
        .no_publish()
        .addr_filter(AddrFilter::unfiltered())
        .build()
        .expect("dht lookup");
    timeout(seeded_dht_publish_resolve_timeout(), async {
        loop {
            if let Some(mut resolved) = lookup.resolve(endpoint.id())
                && let Some(Ok(item)) = resolved.next().await
                && item.endpoint_info().endpoint_id == endpoint.id()
            {
                return;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("resolve published endpoint info");
}

pub(crate) async fn configure_seeded_dht(app: &AppService, remote_endpoint_id: String) {
    app.set_discovery_seeds(
        DiscoveryMode::SeededDht,
        false,
        vec![SeedPeer {
            endpoint_id: remote_endpoint_id,
            addr_hint: None,
        }],
        Vec::new(),
    )
    .await
    .expect("configure seeded dht");
}

pub(crate) fn app_with_iroh_services(store: Arc<MemoryStore>, stack: &TestIrohStack) -> AppService {
    app_service_from_dependencies(
        store.clone(),
        store,
        stack.transport.clone(),
        stack.transport.clone(),
        stack.docs_sync.clone(),
        stack.blob_service.clone(),
        generate_keys(),
    )
}
pub(crate) async fn assert_docs_sync_recovers_post_without_hints(topic: &str, content: &str) {
    let dir = tempdir().expect("tempdir");
    let stack_a = TestIrohStack::new(&dir.path().join("a")).await;
    let stack_b = TestIrohStack::new(&dir.path().join("b")).await;
    let store_a = Arc::new(MemoryStore::default());
    let store_b = Arc::new(MemoryStore::default());
    let app_a = app_service_from_dependencies(
        store_a.clone(),
        store_a,
        stack_a.transport.clone(),
        Arc::new(NoopHintTransport),
        stack_a.docs_sync.clone(),
        stack_a.blob_service.clone(),
        generate_keys(),
    );
    let app_b = app_service_from_dependencies(
        store_b.clone(),
        store_b,
        stack_b.transport.clone(),
        Arc::new(NoopHintTransport),
        stack_b.docs_sync.clone(),
        stack_b.blob_service.clone(),
        generate_keys(),
    );

    let ticket_a = app_a
        .peer_ticket()
        .await
        .expect("ticket a")
        .expect("ticket a value");
    let ticket_b = app_b
        .peer_ticket()
        .await
        .expect("ticket b")
        .expect("ticket b value");
    app_a.import_peer_ticket(&ticket_b).await.expect("import b");
    app_b.import_peer_ticket(&ticket_a).await.expect("import a");

    let object_id = app_a
        .create_post(topic, content, None)
        .await
        .expect("create post");

    let received = timeout(Duration::from_secs(20), async {
        loop {
            let timeline = app_b
                .list_timeline(topic, None, 20)
                .await
                .expect("timeline");
            if let Some(post) = timeline
                .items
                .iter()
                .find(|post| post.object_id == object_id)
            {
                return post.clone();
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("missing gossip timeout");

    assert_eq!(received.content, content);
}
