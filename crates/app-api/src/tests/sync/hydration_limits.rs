//! #1225: 取得できない本文 blob や、変化の無い replica があっても、定期取得・recovery tick・hint 受信が
//! replica の全件走査と購読再起動・再 sync を繰り返さないことを固定する。

use super::*;

fn missing_text_payload() -> PayloadRef {
    PayloadRef::BlobText {
        hash: BlobHash::new("d".repeat(64)),
        mime: "text/plain".into(),
        bytes: 12,
    }
}

fn counting_app(
    store: Arc<MemoryStore>,
    transport: Arc<StaticTransport>,
    docs_sync: Arc<CountingDocsSync>,
    keys: KukuriKeys,
) -> AppService {
    app_service_from_dependencies(
        store.clone(),
        store,
        transport.clone(),
        transport,
        docs_sync,
        Arc::new(MemoryBlobService::default()),
        keys,
    )
}

fn assisted_peer_snapshot(topic: &TopicId) -> PeerSnapshot {
    PeerSnapshot {
        connected: true,
        peer_count: 1,
        connected_peers: vec!["peer-a".into()],
        configured_peers: vec!["peer-a".into()],
        subscribed_topics: vec![topic.as_str().to_string()],
        active_path: Default::default(),
        fallback_peer_ids: Vec::new(),
        pending_events: 0,
        status_detail: "live peer connected".into(),
        last_error: None,
        topic_diagnostics: vec![TopicPeerSnapshot {
            topic: topic.as_str().to_string(),
            joined: true,
            peer_count: 1,
            connected_peers: vec!["peer-a".into()],
            configured_peer_ids: vec!["peer-a".into()],
            missing_peer_ids: Vec::new(),
            active_path: Default::default(),
            rendezvous_peer_ids: Vec::new(),
            fallback_peer_ids: Vec::new(),
            last_received_at: None,
            status_detail: "live peer connected".into(),
            last_error: None,
        }],
    }
}

// TR-2 / AC-1 / AC-2: 欠損行が残っていても、2 回目以降の定期取得は全件走査も再 sync も起動しない。
#[tokio::test]
async fn timeline_refresh_with_a_missing_body_does_not_rescan_or_restart() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let docs_sync = Arc::new(CountingDocsSync::default());
    let keys = generate_keys();
    let topic = TopicId::new("kukuri:topic:missing-body-refresh");
    persist_test_post(
        docs_sync.as_ref(),
        None,
        &keys,
        &topic,
        missing_text_payload(),
        Vec::new(),
        None,
    )
    .await;
    let app = counting_app(store, transport, docs_sync.clone(), keys);

    let first = app
        .list_timeline(topic.as_str(), None, 20)
        .await
        .expect("first timeline");
    assert_eq!(first.items.len(), 1, "the post is listed without its body");
    sleep(Duration::from_millis(100)).await;
    docs_sync.clear_queries().await;
    let restarts_before = docs_sync.restarts().await;

    for _ in 0..3 {
        let view = app
            .list_timeline(topic.as_str(), None, 20)
            .await
            .expect("refreshed timeline");
        assert_eq!(view.items.len(), 1);
    }

    assert_eq!(
        docs_sync.object_scans().await,
        0,
        "refreshing a page with a missing body must not rescan the replica"
    );
    assert_eq!(
        docs_sync.restarts().await,
        restarts_before,
        "refreshing a page with a missing body must not restart replica sync"
    );
}

// TR-2: thread の定期取得も同じ。
#[tokio::test]
async fn thread_refresh_with_a_missing_body_does_not_rescan_or_restart() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let docs_sync = Arc::new(CountingDocsSync::default());
    let keys = generate_keys();
    let topic = TopicId::new("kukuri:topic:missing-body-thread");
    let root = persist_test_post(
        docs_sync.as_ref(),
        None,
        &keys,
        &topic,
        missing_text_payload(),
        Vec::new(),
        None,
    )
    .await;
    let app = counting_app(store, transport, docs_sync.clone(), keys);

    let first = app
        .list_thread(topic.as_str(), root.id.as_str(), None, 20)
        .await
        .expect("first thread");
    assert_eq!(first.items.len(), 1);
    sleep(Duration::from_millis(100)).await;
    docs_sync.clear_queries().await;
    let restarts_before = docs_sync.restarts().await;

    for _ in 0..3 {
        app.list_thread(topic.as_str(), root.id.as_str(), None, 20)
            .await
            .expect("refreshed thread");
    }

    assert_eq!(docs_sync.object_scans().await, 0);
    assert_eq!(docs_sync.restarts().await, restarts_before);
}

// AC-2 / TR-4: 空のタイムラインからの復旧でも、1 回の取得で行う全件走査は 1 回まで。
#[tokio::test]
async fn one_timeline_call_scans_the_replica_at_most_once() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let docs_sync = Arc::new(CountingDocsSync::default());
    let keys = generate_keys();
    let topic = TopicId::new("kukuri:topic:single-scan");
    let app = counting_app(store, transport, docs_sync.clone(), keys);

    app.list_timeline(topic.as_str(), None, 20)
        .await
        .expect("initial timeline");
    sleep(Duration::from_millis(100)).await;

    for _ in 0..3 {
        docs_sync.clear_queries().await;
        app.list_timeline(topic.as_str(), None, 20)
            .await
            .expect("empty timeline");
        assert!(
            docs_sync.object_scans().await <= 1,
            "one call must scan the replica at most once"
        );
    }
}

// TR-8 / AC-4: docs の支援 peer がいて replica に変化が無いとき、recovery tick の全件走査は backoff に従う。
#[tokio::test]
async fn recovery_tick_backs_off_when_the_replica_does_not_change() {
    let store = Arc::new(MemoryStore::default());
    let topic = TopicId::new("kukuri:topic:steady-recovery-tick");
    let transport = Arc::new(StaticTransport::new(assisted_peer_snapshot(&topic)));
    let docs_sync = Arc::new(CountingDocsSync::with_assist_peer_ids(vec!["peer-a"]));
    let keys = generate_keys();
    persist_test_post(
        docs_sync.as_ref(),
        None,
        &keys,
        &topic,
        PayloadRef::InlineText {
            text: "steady post".into(),
        },
        Vec::new(),
        None,
    )
    .await;
    let app = counting_app(store, transport, docs_sync.clone(), keys);

    app.list_timeline(topic.as_str(), None, 20)
        .await
        .expect("initial timeline");
    sleep(Duration::from_millis(100)).await;
    docs_sync.clear_queries().await;

    // 変化の無い replica を 14 秒観測する。3 秒ごとの走査なら 4 回、backoff(3 / 10 / 30 秒)なら 2 回まで。
    sleep(Duration::from_secs(14)).await;
    let scans = docs_sync.object_scans().await;
    assert!(
        scans <= 2,
        "an unchanged replica must not be rescanned every grace period, got {scans} scans"
    );
}

// TR-6 / AC-4: 個別反映が 0 件の hint を連続で受けても、hint ごとに全件走査しない。
#[tokio::test]
async fn repeated_unresolved_hints_do_not_rescan_per_hint() {
    let store = Arc::new(MemoryStore::default());
    let topic = TopicId::new("kukuri:topic:hint-storm");
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let docs_sync = Arc::new(CountingDocsSync::default());
    let keys = generate_keys();
    let app = counting_app(store, transport.clone(), docs_sync.clone(), keys);

    app.list_timeline(topic.as_str(), None, 20)
        .await
        .expect("initial timeline");
    sleep(Duration::from_millis(100)).await;
    docs_sync.clear_queries().await;

    for index in 0..10 {
        transport
            .publish_hint(
                &channel_hint_topic_for(topic.as_str(), None),
                GossipHint::TopicObjectsChanged {
                    topic_id: topic.clone(),
                    objects: vec![HintObjectRef {
                        object_id: format!("not-synced-yet-{index}"),
                        object_kind: "post".into(),
                    }],
                },
            )
            .await
            .expect("publish hint");
    }
    sleep(Duration::from_millis(500)).await;

    let scans = docs_sync.object_scans().await;
    assert!(
        scans <= 1,
        "unresolved hints must share one recovery scan per interval, got {scans} scans"
    );
}

/// 開閉できる blob service。閉じている間は本文を返さず、remote 取得の試行回数だけを数える。
#[derive(Default)]
struct GatedBlobService {
    inner: MemoryBlobService,
    open: std::sync::atomic::AtomicBool,
    fetches: std::sync::atomic::AtomicUsize,
}

#[async_trait]
impl BlobService for GatedBlobService {
    async fn put_blob(&self, data: Vec<u8>, mime: &str) -> Result<StoredBlob> {
        self.inner.put_blob(data, mime).await
    }

    async fn fetch_blob(&self, hash: &BlobHash) -> Result<Option<Vec<u8>>> {
        self.fetches
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if !self.open.load(std::sync::atomic::Ordering::SeqCst) {
            return Ok(None);
        }
        self.inner.fetch_blob(hash).await
    }

    async fn pin_blob(&self, hash: &BlobHash) -> Result<()> {
        self.inner.pin_blob(hash).await
    }

    async fn blob_status(&self, hash: &BlobHash) -> Result<BlobStatus> {
        self.local_blob_status(hash).await
    }

    async fn local_blob_status(&self, hash: &BlobHash) -> Result<BlobStatus> {
        if !self.open.load(std::sync::atomic::Ordering::SeqCst) {
            return Ok(BlobStatus::Missing);
        }
        self.inner.local_blob_status(hash).await
    }

    async fn import_peer_ticket(&self, ticket: &str) -> Result<()> {
        self.inner.import_peer_ticket(ticket).await
    }
}

// TR-3 / AC-3: 欠損した本文は台帳の間隔でだけ取りに行き、提供元が戻った後の試行で行へ反映される。
#[tokio::test]
async fn missing_body_is_retried_on_the_ledger_schedule_and_recovers() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let docs_sync = Arc::new(CountingDocsSync::default());
    let blob_service = Arc::new(GatedBlobService::default());
    let keys = generate_keys();
    let topic = TopicId::new("kukuri:topic:missing-body-recovery");
    let stored = blob_service
        .put_blob(b"recovered body".to_vec(), "text/plain")
        .await
        .expect("store body");
    let envelope = persist_test_post(
        docs_sync.as_ref(),
        None,
        &keys,
        &topic,
        PayloadRef::BlobText {
            hash: stored.hash.clone(),
            mime: "text/plain".into(),
            bytes: stored.bytes,
        },
        Vec::new(),
        None,
    )
    .await;
    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        transport.clone(),
        transport,
        docs_sync.clone(),
        blob_service.clone(),
        keys,
    );

    for _ in 0..5 {
        let view = app
            .list_timeline(topic.as_str(), None, 20)
            .await
            .expect("timeline without body");
        assert_eq!(view.items.len(), 1);
    }
    sleep(Duration::from_millis(200)).await;
    assert_eq!(
        blob_service
            .fetches
            .load(std::sync::atomic::Ordering::SeqCst),
        1,
        "repeated listing must not refetch a missing body before the next scheduled attempt"
    );
    let row = ObjectProjectionStore::get_object_projection(store.as_ref(), &envelope.id)
        .await
        .expect("projection")
        .expect("row");
    assert!(row.content.is_none());

    blob_service
        .open
        .store(true, std::sync::atomic::Ordering::SeqCst);
    timeout(Duration::from_secs(15), async {
        loop {
            app.list_timeline(topic.as_str(), None, 20)
                .await
                .expect("timeline during recovery");
            let row = ObjectProjectionStore::get_object_projection(store.as_ref(), &envelope.id)
                .await
                .expect("projection")
                .expect("row");
            if row.content.as_deref() == Some("recovered body") {
                break;
            }
            sleep(Duration::from_millis(250)).await;
        }
    })
    .await
    .expect("the body is recovered on the next scheduled attempt");
    assert_eq!(docs_sync.restarts().await, 0);
}

// AC-5 の計測用。`cargo test -p kukuri-app-api --lib measure_full_scan_cost -- --ignored --nocapture` で実行する。
// 1 replica に N 件の投稿を置き、(a) 反映を伴う最初の全件走査と (b) 変化が無いときの全件走査の所要時間を出す。
#[tokio::test]
#[ignore = "measurement only"]
async fn measure_full_scan_cost() {
    for count in [100usize, 1_000, 10_000] {
        let store = Arc::new(
            kukuri_store::SqliteStore::connect_memory()
                .await
                .expect("sqlite"),
        );
        let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
        let docs_sync = Arc::new(MemoryDocsSync::default());
        let keys = generate_keys();
        let topic = TopicId::new(format!("kukuri:topic:scan-cost-{count}").as_str());
        for index in 0..count {
            persist_test_post(
                docs_sync.as_ref(),
                None,
                &keys,
                &topic,
                PayloadRef::InlineText {
                    text: format!("post {index}"),
                },
                Vec::new(),
                None,
            )
            .await;
        }
        let services = ServiceHandles::new(
            store.clone(),
            store,
            transport.clone(),
            transport,
            docs_sync,
            Arc::new(MemoryBlobService::default()),
            keys,
        );
        let replica = topic_replica_id(topic.as_str());
        let started = std::time::Instant::now();
        let first = hydrate_subscription_state(
            &services,
            topic.as_str(),
            &replica,
            DocFetchPolicy::LocalOnly,
        )
        .await
        .expect("first scan");
        let first_elapsed = started.elapsed();
        let started = std::time::Instant::now();
        let second = hydrate_subscription_state(
            &services,
            topic.as_str(),
            &replica,
            DocFetchPolicy::LocalOnly,
        )
        .await
        .expect("second scan");
        let second_elapsed = started.elapsed();
        println!(
            "[scan-cost] posts={count} first: reflected={first} {first_elapsed:?} / unchanged: reflected={second} {second_elapsed:?}"
        );
        assert_eq!(first, count);
        assert_eq!(second, 0);
    }
}

/// 本文の取得が返ってこない blob service(応答しない peer を模す)。
#[derive(Default)]
struct HangingBlobService {
    inner: MemoryBlobService,
}

#[async_trait]
impl BlobService for HangingBlobService {
    async fn put_blob(&self, data: Vec<u8>, mime: &str) -> Result<StoredBlob> {
        self.inner.put_blob(data, mime).await
    }

    async fn fetch_blob(&self, _hash: &BlobHash) -> Result<Option<Vec<u8>>> {
        std::future::pending::<()>().await;
        Ok(None)
    }

    async fn pin_blob(&self, hash: &BlobHash) -> Result<()> {
        self.inner.pin_blob(hash).await
    }

    async fn blob_status(&self, _hash: &BlobHash) -> Result<BlobStatus> {
        Ok(BlobStatus::Missing)
    }

    async fn local_blob_status(&self, _hash: &BlobHash) -> Result<BlobStatus> {
        Ok(BlobStatus::Missing)
    }

    async fn import_peer_ticket(&self, ticket: &str) -> Result<()> {
        self.inner.import_peer_ticket(ticket).await
    }
}

// TR-3: 本文の取得を待っている走査が abort されても(購読の再起動など)、その hash は「取得中」のまま残らない。
#[tokio::test]
async fn aborting_a_scan_during_a_body_fetch_does_not_block_later_attempts() {
    let blob_service = Arc::new(HangingBlobService::default());
    let ledger = Arc::new(crate::service::hydration_limits::MissingBodyLedger::default());
    let hash = BlobHash::new("e".repeat(64));

    let scan = tokio::spawn({
        let blob_service = blob_service.clone();
        let ledger = ledger.clone();
        let hash = hash.clone();
        async move {
            crate::service::hydration_limits::fetch_projection_blob_text_bounded(
                blob_service.as_ref(),
                ledger.as_ref(),
                &hash,
            )
            .await
        }
    });
    timeout(Duration::from_secs(5), async {
        while ledger.attempts(&hash) == 0 {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the scan starts fetching the body");
    assert!(
        ledger.try_begin(&hash, i64::MAX).is_none(),
        "the fetch is in flight"
    );

    scan.abort();
    let _ = scan.await;

    assert!(
        ledger.try_begin(&hash, i64::MAX).is_some(),
        "an aborted fetch must not leave the hash in flight forever"
    );
}
