//! #1221 R2-D: 通常の状態は件数と稼働中の topic だけを持ち、読取りの仕事は休止した履歴に依らない。
//! peer の一覧は詳細のページで読み、変わった topic だけに印を付ける。
use super::*;
use std::sync::atomic::AtomicU64;

fn endpoint_id(index: u64) -> String {
    let mut secret = [0; 32];
    secret[..8].copy_from_slice(&index.to_be_bytes());
    SecretKey::from_bytes(&secret).public().to_string()
}

fn seeds(range: std::ops::Range<u64>) -> Vec<SeedPeer> {
    range
        .map(|index| SeedPeer {
            endpoint_id: endpoint_id(index),
            addr_hint: None,
        })
        .collect()
}

async fn put_imported(store: &kukuri_store::SqliteStore, range: std::ops::Range<u64>) {
    for index in range {
        let id = endpoint_id(index);
        let addr = EndpointAddr::new(id.parse::<EndpointId>().expect("id"));
        store
            .put_peer_candidate(
                "gossip",
                "imported",
                &id,
                &serde_json::to_vec(&addr).expect("addr"),
                Utc::now().timestamp_millis(),
            )
            .await
            .expect("imported row");
    }
}

async fn wait_for_topic_neighbor(transport: &IrohGossipTransport, topic: &str) {
    timeout(Duration::from_secs(30), async {
        while !transport
            .peers()
            .await
            .expect("peers")
            .topic_diagnostics
            .iter()
            .any(|diag| diag.topic == topic && diag.peer_count > 0)
        {
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("topic neighbor");
}

/// AC-1・AC-6: 状態の 1 回の読取り(`peers` と `discovery`)は、SQLite を読まず、見る topic・peer・
/// `remote_info` の数は休止した topic・取り込んだ ticket・seed を 10 倍にしても変わらない。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn status_read_work_does_not_grow_with_dormant_history() {
    let vm_steps = Arc::new(AtomicU64::new(0));
    let store = Arc::new(
        kukuri_store::SqliteStore::connect_memory_counting_vm_steps(vm_steps.clone())
            .await
            .expect("store"),
    );
    let transport_a = IrohGossipTransport::bind_local()
        .await
        .expect("a")
        .with_account_store(store.clone());
    let transport_b = IrohGossipTransport::bind_local().await.expect("b");
    let ticket_b = transport_b
        .export_ticket()
        .await
        .expect("ticket b")
        .expect("ticket b value");
    transport_a
        .import_ticket(&ticket_b)
        .await
        .expect("import b");
    let leased = TopicId::new("kukuri:topic:status-leased");
    let _leased_b = transport_b.subscribe_hints(&leased).await.expect("b");
    let _leased_a = transport_a.subscribe_hints(&leased).await.expect("a");
    wait_for_topic_neighbor(&transport_a, "hint/kukuri:topic:status-leased").await;

    let mut history = 0_u64;
    let mut add_history = async |count: u64| {
        for _ in 0..count {
            let dormant = TopicId::new(format!("kukuri:topic:status-dormant-{history}"));
            let _stream = transport_a
                .subscribe_hints(&dormant)
                .await
                .expect("dormant");
            transport_a
                .unsubscribe_hints(&dormant)
                .await
                .expect("leave dormant");
            put_imported(&store, 1_000 + history * 10..1_000 + (history + 1) * 10).await;
            history += 1;
        }
        transport_a
            .configure_discovery(
                DiscoveryMode::StaticPeer,
                false,
                seeds(0..history * 10),
                seeds(10_000..10_000 + history),
            )
            .await
            .expect("seed history");
    };
    let measure = async || {
        let (steps, vm) = (
            transport_a.status_read_steps(),
            vm_steps.load(Ordering::Relaxed),
        );
        let peers = transport_a.peers().await.expect("peers");
        let discovery = transport_a.discovery().await.expect("discovery");
        assert_eq!(vm_steps.load(Ordering::Relaxed), vm, "no SQLite read");
        assert_eq!(peers.topic_diagnostics.len(), 1, "only the active topic");
        assert_eq!(peers.peer_count, 1);
        assert_eq!(discovery.connected_peer_count, 1);
        (
            transport_a.status_read_steps() - steps,
            peers.configured_peer_count,
        )
    };
    add_history(1).await;
    let once = measure().await;
    add_history(9).await;
    let tenfold = measure().await;
    assert_eq!(once.1, 11, "configured seeds and CN seeds are counted");
    assert_eq!(tenfold.1, 110);
    assert!(once.0 > 0);
    assert_eq!(once.0, tenfold.0, "read work does not grow with history");
}

/// AC-3・AC-6: 詳細のページは、重複も欠落も無く全件を返し、1 回は上限以内。取得の候補の cursor は進めない。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn peer_pages_return_every_peer_once_within_the_limit() {
    let store = Arc::new(
        kukuri_store::SqliteStore::connect_memory()
            .await
            .expect("store"),
    );
    let transport = IrohGossipTransport::bind_local()
        .await
        .expect("transport")
        .with_account_store(store.clone());
    transport
        .configure_discovery(
            DiscoveryMode::StaticPeer,
            false,
            seeds(0..150),
            seeds(500..503),
        )
        .await
        .expect("seeds");
    put_imported(&store, 1_000..1_130).await;
    for (kind, expected) in [
        (ConnectivityPeerKind::ConfiguredSeed, 0..150),
        (ConnectivityPeerKind::BootstrapSeed, 500..503),
        (ConnectivityPeerKind::ManualTicket, 1_000..1_130),
    ] {
        let mut cursor = None::<String>;
        let mut seen = Vec::new();
        loop {
            let page = transport
                .peer_page(kind, None, cursor.as_deref(), 64)
                .await
                .expect("page");
            assert!(page.peer_ids.len() <= 64, "{kind:?}: page within the limit");
            seen.extend(page.peer_ids);
            match page.next_cursor {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        let mut expected = expected.map(endpoint_id).collect::<Vec<_>>();
        expected.sort();
        assert_eq!(seen, expected, "{kind:?}: every peer once, in id order");
    }
    assert_eq!(
        *transport.imported_cursor.lock().await,
        None,
        "the bootstrap candidate cursor is not advanced"
    );
    assert_eq!(
        transport.bootstrap_cursor.lock().await.1,
        [None, None, None]
    );
}

/// AC-4: 購読・neighbor の成立・購読の終了は、その topic だけに印を付ける。変化の無い間は印が無い。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn topic_changes_mark_only_the_changed_topic() {
    let changes = StatusChanges::default();
    let transport_a = IrohGossipTransport::bind_local()
        .await
        .expect("a")
        .with_status_changes(changes.clone());
    let transport_b = IrohGossipTransport::bind_local().await.expect("b");
    let ticket_b = transport_b
        .export_ticket()
        .await
        .expect("ticket b")
        .expect("ticket b value");
    let quiet = TopicId::new("kukuri:topic:mark-quiet");
    let busy = TopicId::new("kukuri:topic:mark-busy");
    let busy_key = StatusKey::Topic("hint/kukuri:topic:mark-busy".into());
    // 印が止むまで(`quiet` の間、印が無いまで)溜まった印を集める。最初の呼出しから印を受け付ける。
    let drain = async |quiet: Duration| {
        let mut keys = BTreeSet::new();
        while let Ok(next) = timeout(quiet, changes.changed()).await {
            keys.extend(next);
        }
        keys
    };
    assert!(drain(Duration::from_millis(10)).await.is_empty());
    let _quiet_a = transport_a.subscribe_hints(&quiet).await.expect("quiet");
    let _busy_b = transport_b.subscribe_hints(&busy).await.expect("b busy");
    let _busy_a = transport_a.subscribe_hints(&busy).await.expect("a busy");
    assert!(drain(Duration::from_millis(500)).await.contains(&busy_key));
    transport_a
        .import_ticket(&ticket_b)
        .await
        .expect("import b");
    wait_for_topic_neighbor(&transport_a, "hint/kukuri:topic:mark-busy").await;
    let keys = drain(Duration::from_millis(1_500)).await;
    assert_eq!(
        keys,
        BTreeSet::from([busy_key.clone()]),
        "the neighbor marks only its topic"
    );
    // 変化の無い間は印が付かない。
    assert!(
        drain(Duration::from_millis(1_500)).await.is_empty(),
        "no marks while nothing changes"
    );
    transport_a.unsubscribe_hints(&busy).await.expect("leave");
    assert_eq!(
        timeout(Duration::from_secs(3), changes.changed())
            .await
            .expect("mark on leave"),
        BTreeSet::from([busy_key])
    );
}
