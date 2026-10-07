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

/// #1632 AC-6: Web の検索の client。検索を提供する node（bootstrap の `public_blob_search`）だけへ、session の token と
/// 取得に残る時間（上限 8 秒。残りが無ければ送らない）を付けて hash を送り、候補の relay URL・直接の address を取得の
/// 候補にする（読めない候補は捨てる）。node の失敗は候補なしとする。候補の stream を落とすと（表示の取消・取得の期限）、
/// node への要求も取り消す。
#[tokio::test]
async fn public_blob_search_asks_only_a_node_that_offers_it() {
    use axum::response::IntoResponse;
    use kukuri_cn_protocol::{
        BLOB_PROVIDER_SEARCH_PATH, BlobProviderCandidate, BlobProviderSearchRequest,
        BlobProviderSearchResponse,
    };
    use tokio::sync::Notify;

    fn resolved_urls(base_url: &str, offers: bool) -> CommunityNodeResolvedUrls {
        CommunityNodeResolvedUrls {
            public_blob_search: offers,
            ..CommunityNodeResolvedUrls::new(base_url, Vec::new(), Vec::new())
                .expect("resolved urls")
        }
    }

    /// 応答を待つ要求が取り消されたことを知らせる。
    struct Cancelled(Arc<Notify>);
    impl Drop for Cancelled {
        fn drop(&mut self) {
            self.0.notify_one();
        }
    }

    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("public-blob-search.db");
    let runtime = Arc::new(
        DesktopRuntime::new_with_config_and_identity(
            &db_path,
            TransportNetworkConfig::loopback(),
            IdentityStorageMode::FileOnly,
        )
        .await
        .expect("runtime"),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let base_url = format!("http://{}", listener.local_addr().expect("local addr"));
    let holder = iroh::SecretKey::generate().public();
    let [answered, failing, waiting] = ["answered", "failing", "waiting"]
        .map(|name| iroh_blobs::Hash::new(format!("kukuri-1632-ac6-{name}")));
    let offered = Arc::new(AtomicBool::new(false));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let (arrived, cancelled) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    // 背景の session は、期限の来た登録（bootstrap の heartbeat・node の一覧・topic の rendezvous）を先に済ませる。node の
    // 一覧は、今の提供の有無を返す（設定はその応答で更新される）。
    let state = Arc::new(MockCommunityNodeState {
        base_url: base_url.clone(),
        seed_peers: Arc::new(Mutex::new(Vec::new())),
        heartbeat_seed_peers: Arc::new(Mutex::new(None)),
        heartbeat_hits: Arc::new(AtomicUsize::new(0)),
        bootstrap_hits: Arc::new(AtomicUsize::new(0)),
    });
    let app = Router::new()
        .route("/v1/bootstrap/heartbeat", post(mock_bootstrap_heartbeat))
        .route(
            "/v1/bootstrap/nodes",
            get({
                let (base_url, offered) = (base_url.clone(), offered.clone());
                move || {
                    let node = kukuri_cn_protocol::CommunityNodeBootstrapNode {
                        base_url: base_url.clone(),
                        resolved_urls: resolved_urls(&base_url, offered.load(Ordering::SeqCst)),
                    };
                    async move { Json(BootstrapNodesResponse { nodes: vec![node] }) }
                }
            }),
        )
        .route("/v1/rendezvous/topics/heartbeat", post(mock_rendezvous))
        .route(
            BLOB_PROVIDER_SEARCH_PATH,
            post({
                let (requests, arrived, cancelled) =
                    (requests.clone(), arrived.clone(), cancelled.clone());
                move |headers: HeaderMap, Json(request): Json<BlobProviderSearchRequest>| {
                    let (requests, arrived, cancelled) =
                        (requests.clone(), arrived.clone(), cancelled.clone());
                    async move {
                        let bearer = headers
                            .get(AUTHORIZATION)
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default()
                            .to_string();
                        requests.lock().await.push((bearer, request.clone()));
                        if request.hash == failing.to_string() {
                            return StatusCode::SERVICE_UNAVAILABLE.into_response();
                        }
                        if request.hash == waiting.to_string() {
                            let _cancelled = Cancelled(cancelled);
                            arrived.notify_one();
                            std::future::pending::<()>().await;
                        }
                        let candidate = |endpoint_id: String| BlobProviderCandidate {
                            endpoint_id,
                            relay_urls: vec!["https://relay.kukuri.test/".into()],
                            direct_addrs: vec!["127.0.0.1:4433".into()],
                        };
                        Json(BlobProviderSearchResponse {
                            candidates: vec![
                                candidate(holder.to_string()),
                                candidate("broken".into()),
                            ],
                            partial: false,
                        })
                        .into_response()
                    }
                }
            }),
        );
    let server = tokio::spawn(async move { axum::serve(listener, app.with_state(state)).await });
    persist_community_node_token(
        &db_path,
        IdentityStorageMode::FileOnly,
        base_url.as_str(),
        &StoredCommunityNodeToken {
            access_token: "search-token".into(),
            expires_at: Utc::now().timestamp() + 3600,
        },
    )
    .await
    .expect("persist token");
    seed_local_community_node_consents(&runtime, base_url.as_str(), 1).await;
    for offers in [false, true] {
        offered.store(offers, Ordering::SeqCst);
        *runtime.community_node_config.lock().await = CommunityNodeConfig {
            trust_node_priority: Vec::new(),
            nodes: vec![CommunityNodeNodeConfig {
                content_advisory_enabled: false,
                base_url: base_url.clone(),
                resolved_urls: Some(resolved_urls(&base_url, offers)),
            }],
        };
        mark_community_node_session_ready_for_test(&runtime, base_url.as_str()).await;
        let found = runtime
            .search_public_blob_providers(answered, Duration::from_secs(30))
            .await;
        let expected = offers.then(|| {
            iroh::EndpointAddr::from_parts(
                holder,
                [
                    iroh::TransportAddr::Relay("https://relay.kukuri.test/".parse().unwrap()),
                    iroh::TransportAddr::Ip("127.0.0.1:4433".parse().unwrap()),
                ],
            )
        });
        assert_eq!(found, expected.into_iter().collect::<Vec<_>>());
    }
    for (hash, budget) in [
        (answered, Duration::ZERO),
        (failing, Duration::from_secs(2)),
    ] {
        assert!(
            runtime
                .search_public_blob_providers(hash, budget)
                .await
                .is_empty()
        );
    }
    let candidates = runtime
        .community_node_public_blob_search()
        .providers(waiting, Duration::from_secs(30))
        .expect("search");
    timeout(Duration::from_secs(10), arrived.notified())
        .await
        .expect("the search arrives");
    drop(candidates);
    timeout(Duration::from_secs(10), cancelled.notified())
        .await
        .expect("dropping the candidates cancels the search");

    let request = |hash: iroh_blobs::Hash, budget_ms| {
        (
            "Bearer search-token".to_string(),
            BlobProviderSearchRequest {
                hash: hash.to_string(),
                budget_ms,
            },
        )
    };
    assert_eq!(
        *requests.lock().await,
        [
            request(answered, 8_000),
            request(failing, 2_000),
            request(waiting, 8_000)
        ],
        "only the node that offers the search gets the hash, while the fetch still has time"
    );
    server.abort();
    runtime.shutdown().await;
}
