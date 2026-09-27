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
        configured_peer_count: 1,
        subscribed_topics: vec![topic.as_str().to_string()],
        active_path: Default::default(),
        fallback_peer_count: 0,
        pending_events: 0,
        status_detail: "live peer connected".into(),
        last_error: None,
        topic_diagnostics: vec![TopicPeerSnapshot {
            topic: topic.as_str().to_string(),
            joined: true,
            peer_count: 1,
            configured_peer_count: 1,
            missing_peer_count: 0,
            active_path: Default::default(),
            rendezvous_peer_count: 0,
            fallback_peer_count: 0,
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
                        docs_author: None,
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
    reject_admission: std::sync::atomic::AtomicBool,
}

#[tokio::test]
async fn deferred_body_admission_does_not_spend_a_network_attempt() {
    let blobs = GatedBlobService::default();
    let ledger = crate::service::hydration_limits::MissingBodyLedger::default();
    let hash = BlobHash::new("f".repeat(64));
    blobs
        .reject_admission
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(
        crate::service::hydration_limits::fetch_projection_blob_text_bounded(
            &blobs, &ledger, &hash
        )
        .await
        .is_none()
    );
    assert_eq!(ledger.attempts(&hash), 0);
    blobs
        .reject_admission
        .store(false, std::sync::atomic::Ordering::SeqCst);
    assert!(
        crate::service::hydration_limits::fetch_projection_blob_text_bounded(
            &blobs, &ledger, &hash
        )
        .await
        .is_none()
    );
    assert_eq!(ledger.attempts(&hash), 1);
}

// #1284 AC-2 / AC-5: 利用者の明示再試行は自動 retry の cooldown 中でも対象本文を
// 1 回だけ取り直し、更新したカード view を返す。
#[tokio::test]
async fn manual_post_body_retry_bypasses_the_automatic_cooldown_once() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let docs_sync = Arc::new(CountingDocsSync::default());
    let blobs = Arc::new(GatedBlobService::default());
    let keys = generate_keys();
    let topic = TopicId::new("kukuri:topic:manual-body-retry");
    let stored = blobs
        .put_blob(b"manual retry body".to_vec(), "text/plain")
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
        docs_sync,
        blobs.clone(),
        keys,
    );

    let missing = app
        .list_timeline(topic.as_str(), None, 20)
        .await
        .expect("timeline with the missing body");
    assert_eq!(missing.items[0].content, "[blob pending]");
    assert_eq!(app.services.missing_body_ledger.attempts(&stored.hash), 1);

    let fetches_before_unrelated = blobs.fetches.load(std::sync::atomic::Ordering::SeqCst);
    app.retry_post_elements(envelope.id.as_str(), Some(envelope.id.as_str()), false)
        .await
        .expect("automatic visible retry")
        .expect("post view");
    assert_eq!(
        blobs.fetches.load(std::sync::atomic::Ordering::SeqCst),
        fetches_before_unrelated,
        "automatic viewport recovery must keep the finite ledger cooldown"
    );
    let unrelated = app
        .retry_post_elements(envelope.id.as_str(), Some("unrelated-object"), true)
        .await
        .expect("unrelated retry is rejected");
    assert!(unrelated.is_none());
    assert_eq!(
        blobs.fetches.load(std::sync::atomic::Ordering::SeqCst),
        fetches_before_unrelated,
        "an unrelated body must not start blob I/O"
    );

    let mut private_leftover = store
        .get_object_projection(&envelope.id)
        .await
        .expect("projection")
        .expect("projected post");
    private_leftover.channel_id = "private-not-joined".into();
    store
        .put_object_projection(private_leftover.clone())
        .await
        .expect("store private leftover");
    assert!(
        app.retry_post_elements(envelope.id.as_str(), Some(envelope.id.as_str()), true)
            .await
            .is_err(),
        "a leftover private projection requires current membership"
    );
    assert_eq!(
        blobs.fetches.load(std::sync::atomic::Ordering::SeqCst),
        fetches_before_unrelated,
        "membership rejection must happen before blob I/O"
    );
    private_leftover.channel_id = PUBLIC_CHANNEL_ID.into();
    private_leftover.content_labels = vec!["adult".into()];
    store
        .put_object_projection(private_leftover)
        .await
        .expect("restore public projection");
    let gated = app
        .retry_post_elements(envelope.id.as_str(), Some(envelope.id.as_str()), true)
        .await
        .expect("adult-gated retry")
        .expect("post view");
    assert_eq!(gated.content, "[blob pending]");
    assert_eq!(
        blobs.fetches.load(std::sync::atomic::Ordering::SeqCst),
        fetches_before_unrelated,
        "adult gate must reject manual body retry before blob I/O"
    );

    app.set_adult_content_display_enabled(true);
    blobs.open.store(true, std::sync::atomic::Ordering::SeqCst);
    let recovered = app
        .retry_post_elements(envelope.id.as_str(), Some(envelope.id.as_str()), true)
        .await
        .expect("manual retry")
        .expect("post view");
    assert_eq!(recovered.content, "manual retry body");
    assert_eq!(recovered.content_status, BlobViewStatus::Available);
    assert_eq!(app.services.missing_body_ledger.attempts(&stored.hash), 0);
}

#[async_trait]
impl BlobService for GatedBlobService {
    async fn prepare_retry_fetch<'a>(
        &'a self,
        hash: &BlobHash,
    ) -> Result<kukuri_blob_service::PreparedRetryFetch<'a>> {
        if self
            .reject_admission
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            anyhow::bail!("shared network capacity is full");
        }
        let hash = hash.clone();
        Ok(Box::pin(async move { self.fetch_blob(&hash).await }))
    }
    async fn fetch_local_blob(
        &self,
        hash: &kukuri_core::BlobHash,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        if !self.open.load(std::sync::atomic::Ordering::SeqCst) {
            return Ok(None);
        }
        self.inner.fetch_local_blob(hash).await
    }

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

    blob_service
        .reject_admission
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let deferred = app
        .list_timeline(topic.as_str(), None, 20)
        .await
        .expect("timeline while shared network admission is full");
    sleep(Duration::from_millis(100)).await;
    assert_eq!(app.services.missing_body_ledger.attempts(&stored.hash), 0);
    assert_eq!(
        blob_service
            .fetches
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    assert!(
        app.post_display_retry_at(&deferred.items[0])
            .await
            .expect("retry deadline")
            .is_some()
    );
    blob_service
        .reject_admission
        .store(false, std::sync::atomic::Ordering::SeqCst);

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
    let recovered = app.list_timeline(topic.as_str(), None, 20).await.unwrap();
    assert_eq!(
        app.post_display_retry_at(&recovered.items[0])
            .await
            .unwrap(),
        None
    );
    assert_eq!(docs_sync.restarts().await, 0);
}

/// 本文の取得が返ってこない blob service(応答しない peer を模す)。
#[derive(Default)]
struct HangingBlobService {
    inner: MemoryBlobService,
    gate: Option<Arc<tokio::sync::Semaphore>>,
    fetches: Arc<std::sync::atomic::AtomicUsize>,
    completed: Arc<std::sync::atomic::AtomicUsize>,
}

#[async_trait]
impl BlobService for HangingBlobService {
    async fn prepare_retry_fetch<'a>(
        &'a self,
        hash: &BlobHash,
    ) -> Result<kukuri_blob_service::PreparedRetryFetch<'a>> {
        let hash = hash.clone();
        Ok(Box::pin(async move { self.fetch_blob(&hash).await }))
    }

    async fn prepare_display_fetch(
        &self,
        hash: &BlobHash,
    ) -> Result<kukuri_blob_service::DisplayBlobFetch> {
        let Some(gate) = &self.gate else {
            anyhow::bail!("display fetch unavailable");
        };
        let gate = gate.clone();
        let inner = self.inner.clone();
        let hash = hash.clone();
        let fetches = self.fetches.clone();
        let completed = self.completed.clone();
        Ok(Box::pin(async move {
            fetches.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            gate.acquire().await?.forget();
            let bytes = inner.fetch_blob(&hash).await;
            completed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            bytes
        }))
    }
    async fn fetch_local_blob(
        &self,
        _hash: &kukuri_core::BlobHash,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        Ok(None)
    }

    async fn put_blob(&self, data: Vec<u8>, mime: &str) -> Result<StoredBlob> {
        self.inner.put_blob(data, mime).await
    }

    async fn fetch_blob(&self, hash: &BlobHash) -> Result<Option<Vec<u8>>> {
        self.fetches
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if let Some(gate) = &self.gate {
            gate.acquire().await?.forget();
            let bytes = self.inner.fetch_blob(hash).await;
            self.completed
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            return bytes;
        }
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

#[path = "hydration_limits_cancel.rs"]
mod hydration_limits_cancel;

#[tokio::test]
async fn shutdown_rejects_a_body_that_arrives_after_the_account_closes() {
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let blobs = Arc::new(HangingBlobService {
        gate: Some(gate.clone()),
        ..Default::default()
    });
    let stored = blobs
        .put_blob(b"late body".to_vec(), "text/plain")
        .await
        .unwrap();
    let docs = Arc::new(CountingDocsSync::default());
    let keys = generate_keys();
    let topic = TopicId::new("kukuri:topic:late-body-after-shutdown");
    let envelope = persist_test_post(
        docs.as_ref(),
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
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        transport.clone(),
        transport,
        docs,
        blobs.clone(),
        keys,
    );
    app.list_timeline(topic.as_str(), None, 20).await.unwrap();
    assert_eq!(blobs.fetches.load(std::sync::atomic::Ordering::SeqCst), 1);
    app.shutdown().await;
    gate.add_permits(1);
    sleep(Duration::from_millis(100)).await;
    let row = store
        .get_object_projection(&envelope.id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        row.content.is_none(),
        "closed account must reject late body bytes"
    );
    assert_eq!(blobs.completed.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[tokio::test]
async fn closing_the_account_after_fetch_but_before_save_discards_the_body() {
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let blobs = Arc::new(HangingBlobService {
        gate: Some(gate.clone()),
        ..Default::default()
    });
    let stored = blobs
        .put_blob(b"completed body".to_vec(), "text/plain")
        .await
        .unwrap();
    let docs = Arc::new(CountingDocsSync::default());
    let keys = generate_keys();
    let topic = TopicId::new("kukuri:topic:late-save-after-fetch");
    let envelope = persist_test_post(
        docs.as_ref(),
        None,
        &keys,
        &topic,
        PayloadRef::BlobText {
            hash: stored.hash,
            mime: "text/plain".into(),
            bytes: stored.bytes,
        },
        Vec::new(),
        None,
    )
    .await;
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let app = Arc::new(app_service_from_dependencies(
        store.clone(),
        store.clone(),
        transport.clone(),
        transport,
        docs,
        blobs.clone(),
        keys,
    ));
    let listing = {
        let app = app.clone();
        let topic = topic.clone();
        tokio::spawn(async move { app.list_timeline(topic.as_str(), None, 20).await })
    };
    timeout(Duration::from_secs(2), async {
        while blobs.fetches.load(std::sync::atomic::Ordering::SeqCst) == 0 {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("body fetch starts");
    let save_access = app.services.content_save_access.lock().await;
    gate.add_permits(1);
    timeout(Duration::from_secs(2), async {
        while blobs.completed.load(std::sync::atomic::Ordering::SeqCst) == 0 {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("bytes arrive before account closure");
    app.services.content_closed.send_replace(true);
    drop(save_access);
    let _ = listing.await;
    let row = store
        .get_object_projection(&envelope.id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        row.content.is_none(),
        "completed bytes must not cross a closed save boundary"
    );
    app.shutdown().await;
}

#[tokio::test]
async fn leaving_a_private_channel_rejects_its_late_body() {
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let blobs = Arc::new(HangingBlobService {
        gate: Some(gate.clone()),
        ..Default::default()
    });
    let docs = Arc::new(CountingDocsSync::default());
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let app = Arc::new(app_service_from_dependencies(
        store.clone(),
        store.clone(),
        transport.clone(),
        transport,
        docs.clone(),
        blobs.clone(),
        generate_keys(),
    ));
    let topic = "kukuri:topic:late-private-body";
    let channel = app
        .create_private_channel(CreatePrivateChannelInput {
            topic_id: TopicId::new(topic),
            label: "late body".into(),
            audience_kind: ChannelAudienceKind::InviteOnly,
        })
        .await
        .unwrap();
    let channel_id = ChannelId::new(channel.channel_id.clone());
    let state = app
        .joined_private_channel_state(topic, channel_id.as_str())
        .await
        .unwrap();
    app.register_joined_private_channel(state.clone())
        .await
        .unwrap();
    assert_eq!(
        app.joined_private_channel_state(topic, channel_id.as_str())
            .await
            .unwrap()
            .generation,
        state.generation,
        "the same membership must not invalidate an active scope"
    );
    let replica = current_private_channel_replica_id(&state);
    let stored = blobs
        .put_blob(b"private late body".to_vec(), "text/plain")
        .await
        .unwrap();
    let media = blobs
        .put_blob(b"private image".to_vec(), "image/png")
        .await
        .unwrap();
    let envelope = build_post_envelope_with_payload_in_channel(
        app.keys(),
        &TopicId::new(topic),
        PayloadRef::BlobText {
            hash: stored.hash.clone(),
            mime: "text/plain".into(),
            bytes: stored.bytes,
        },
        vec![kukuri_core::AssetRef {
            hash: media.hash.clone(),
            mime: "image/png".into(),
            bytes: media.bytes,
            role: AssetRole::ImageOriginal,
        }],
        Vec::new(),
        None,
        ObjectVisibility::Private,
        Some(&channel_id),
        Vec::new(),
    )
    .unwrap();
    let post = envelope.to_post_object().unwrap().unwrap();
    persist_post_object(docs.as_ref(), &replica, post, envelope.clone())
        .await
        .unwrap();
    store
        .put_object_projection(super::super::support::verified_projection_row(
            &envelope, &replica, None,
        ))
        .await
        .unwrap();
    assert_eq!(blobs.fetches.load(std::sync::atomic::Ordering::SeqCst), 0);
    let listing = {
        let app = app.clone();
        let channel_id = channel_id.clone();
        tokio::spawn(async move {
            app.list_timeline_scoped(topic, TimelineScope::Channel { channel_id }, None, 20)
                .await
        })
    };
    let media_fetch = {
        let app = app.clone();
        let hash = media.hash.as_str().to_owned();
        let object_id = envelope.id.as_str().to_owned();
        tokio::spawn(async move {
            app.blob_media_payload_for_post(&hash, "image/png", Some(&object_id))
                .await
        })
    };
    timeout(Duration::from_secs(2), async {
        while blobs.fetches.load(std::sync::atomic::Ordering::SeqCst) < 2 {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("private body and attachment fetches start");
    app.remove_joined_private_channel(topic, channel_id.as_str())
        .await
        .unwrap();
    app.register_joined_private_channel(state.clone())
        .await
        .unwrap();
    assert_ne!(
        app.joined_private_channel_state(topic, channel_id.as_str())
            .await
            .unwrap()
            .generation,
        state.generation,
        "rejoining the same epoch must not revive an old fetch"
    );
    gate.add_permits(2);
    let _ = listing.await;
    assert!(media_fetch.await.unwrap().unwrap().is_none());
    sleep(Duration::from_millis(100)).await;
    let row = store
        .get_object_projection(&envelope.id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        row.content.is_none(),
        "revoked scope must reject late body bytes"
    );
    assert_eq!(blobs.completed.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert_eq!(
        gate.available_permits(),
        2,
        "revoked fetches must drop their waiters"
    );
    app.shutdown().await;
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
