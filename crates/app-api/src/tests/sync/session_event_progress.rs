//! #1262: 拒否・欠損 session は一度だけ読み、後続の event を待たせない。
use super::*;

async fn rejected_session_reads_once(kind: &str) {
    let docs = Arc::new(CountingDocsSync::default());
    let store = Arc::new(MemoryStore::default());
    let services = ServiceHandles::new(
        store.clone(),
        store,
        Arc::new(StaticTransport::new(PeerSnapshot::default())),
        Arc::new(NoopHintTransport),
        docs.clone(),
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    );
    let topic = "kukuri:topic:session-event-progress";
    let replica = topic_replica_id(topic);
    let key = format!("sessions/{kind}/invalid/state");
    docs.apply_doc_op(
        &replica,
        DocOp::SetJson {
            key: key.clone(),
            value: serde_json::json!({"unreadable": true}),
        },
    )
    .await
    .unwrap();
    docs.clear_queries().await;
    let event = kukuri_docs_sync::DocEvent {
        replica_id: replica.clone(),
        key: key.clone(),
        content_hash: String::new(),
        source_peer: None,
        docs_author: None,
    };
    assert_eq!(
        crate::service::hydration_support::hydrate_session_key(
            &services, topic, &replica, &event.key
        )
        .await
        .unwrap(),
        0
    );
    let reads = docs
        .queries()
        .await
        .into_iter()
        .filter(|(_, query)| *query == DocQuery::Exact(key.clone()))
        .count();
    assert_eq!(reads, 1, "a rejected session must not reread its state");
}

#[tokio::test]
async fn rejected_live_session_reads_state_once() {
    rejected_session_reads_once("live").await;
}

#[tokio::test]
async fn rejected_game_session_reads_state_once() {
    rejected_session_reads_once("game").await;
}

type LocalReadGate = (BlobHash, Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>);

#[derive(Default)]
struct RemoteManifest {
    local: MemoryBlobService,
    remote: MemoryBlobService,
    publish_remote: std::sync::atomic::AtomicBool,
    fetches: Arc<std::sync::atomic::AtomicUsize>,
    fail: Arc<std::sync::atomic::AtomicBool>,
    reject_admission: std::sync::atomic::AtomicBool,
    gate: std::sync::Mutex<Option<Arc<tokio::sync::Semaphore>>>,
    admission_gate: std::sync::Mutex<Option<Arc<tokio::sync::Semaphore>>>,
    local_gate: std::sync::Mutex<Option<LocalReadGate>>,
}

#[async_trait]
impl BlobService for RemoteManifest {
    async fn put_blob(&self, bytes: Vec<u8>, mime: &str) -> Result<StoredBlob> {
        if self
            .publish_remote
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            self.remote.put_blob(bytes, mime).await
        } else {
            self.local.put_blob(bytes, mime).await
        }
    }
    async fn fetch_local_blob(&self, hash: &BlobHash) -> Result<Option<Vec<u8>>> {
        let gate = self
            .local_gate
            .lock()
            .unwrap()
            .take_if(|(target, _, _)| target == hash);
        if let Some((_, entered, release)) = gate {
            entered.notify_one();
            release.notified().await;
        }
        self.local.fetch_local_blob(hash).await
    }
    async fn prepare_display_fetch(
        &self,
        hash: &BlobHash,
    ) -> Result<kukuri_blob_service::DisplayBlobFetch> {
        if self
            .reject_admission
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            anyhow::bail!("shared network capacity is full");
        }
        let admission = self.admission_gate.lock().unwrap().clone();
        let permit = match admission {
            Some(gate) => Some(gate.acquire_owned().await?),
            None => None,
        };
        let fetches = self.fetches.clone();
        let gate = self.gate.lock().unwrap().clone();
        let fail = self.fail.clone();
        let remote = self.remote.clone();
        let hash = hash.clone();
        Ok(Box::pin(async move {
            let _permit = permit;
            fetches.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if let Some(gate) = gate {
                let _ = gate.acquire().await?;
            }
            if fail.load(std::sync::atomic::Ordering::SeqCst) {
                return Ok(None);
            }
            remote.fetch_blob(&hash).await
        }))
    }
    async fn fetch_blob(&self, hash: &BlobHash) -> Result<Option<Vec<u8>>> {
        let bytes = self.remote.fetch_blob(hash).await?;
        if let Some(bytes) = &bytes {
            self.local
                .put_blob(bytes.clone(), "application/json")
                .await?;
        }
        Ok(bytes)
    }
    async fn pin_blob(&self, _: &BlobHash) -> Result<()> {
        Ok(())
    }
    async fn blob_status(&self, hash: &BlobHash) -> Result<BlobStatus> {
        self.local.blob_status(hash).await
    }
    async fn local_blob_status(&self, hash: &BlobHash) -> Result<BlobStatus> {
        self.local.local_blob_status(hash).await
    }
    async fn import_peer_ticket(&self, _: &str) -> Result<()> {
        Ok(())
    }
}

struct SessionFixture {
    owner: AppService,
    app: AppService,
    docs: Arc<CountingDocsSync>,
    blobs: Arc<RemoteManifest>,
    topic: &'static str,
    replica: ReplicaId,
    key: String,
    id: String,
    kind: &'static str,
}

impl SessionFixture {
    async fn new(kind: &'static str) -> Self {
        let docs = Arc::new(CountingDocsSync::default());
        let blobs = Arc::new(RemoteManifest {
            publish_remote: std::sync::atomic::AtomicBool::new(true),
            ..Default::default()
        });
        let owner_store = Arc::new(MemoryStore::default());
        let owner = app_service_from_dependencies(
            owner_store.clone(),
            owner_store,
            Arc::new(StaticTransport::new(PeerSnapshot::default())),
            Arc::new(NoopHintTransport),
            docs.clone(),
            blobs.clone(),
            generate_keys(),
        );
        let topic = "kukuri:topic:session-display-contract";
        let id = if kind == "live" {
            owner
                .create_live_session(
                    topic,
                    CreateLiveSessionInput {
                        title: "live".into(),
                        description: String::new(),
                    },
                )
                .await
                .unwrap()
        } else {
            owner
                .create_game_room(
                    topic,
                    CreateGameRoomInput {
                        title: "game".into(),
                        description: String::new(),
                        participants: vec!["Alice".into(), "Bob".into()],
                    },
                )
                .await
                .unwrap()
        };
        owner.shutdown().await;
        blobs
            .publish_remote
            .store(false, std::sync::atomic::Ordering::SeqCst);
        let store = Arc::new(MemoryStore::default());
        let app = app_service_from_dependencies(
            store.clone(),
            store,
            Arc::new(StaticTransport::new(PeerSnapshot::default())),
            Arc::new(NoopHintTransport),
            docs.clone(),
            blobs.clone(),
            generate_keys(),
        );
        Self {
            owner,
            app,
            docs,
            blobs,
            topic,
            replica: topic_replica_id(topic),
            key: format!("sessions/{kind}/{id}/state"),
            id,
            kind,
        }
    }

    async fn event(&self, key: &str) -> usize {
        crate::service::hydration_support::hydrate_session_key(
            &self.app.services,
            self.topic,
            &self.replica,
            key,
        )
        .await
        .unwrap()
    }

    async fn display(&self, visible: bool, retry: bool) {
        self.app
            .set_session_display(crate::SessionDisplayRequest {
                topic: self.topic.into(),
                scope: TimelineScope::Public,
                replica_id: self.replica.as_str().into(),
                session_id: self.id.clone(),
                kind: self.kind.into(),
                observer: "visible-card".into(),
                visible,
                retry,
            })
            .await
            .unwrap();
        self.app.services.session_projections.wait_idle().await;
    }
    fn fetches(&self) -> usize {
        self.blobs.fetches.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[tokio::test]
async fn manifest_fetch_requires_display_and_success_projects_only_that_session() {
    for kind in ["live", "game"] {
        let f = SessionFixture::new(kind).await;
        assert_eq!(f.event(&f.key).await, 0);
        let pending_change = f.app.last_sync_ts.get().await;
        assert!(
            pending_change.is_some(),
            "candidate arrival signals the visible list"
        );
        for _ in 0..5 {
            f.event(&f.key).await;
        }
        assert_eq!(
            f.app.last_sync_ts.get().await,
            pending_change,
            "duplicate events do not generate refresh loops"
        );
        assert_eq!(f.fetches(), 0, "events never fetch an offscreen manifest");
        assert_eq!(
            f.app
                .list_session_candidates(f.topic, TimelineScope::Public)
                .await
                .unwrap()
                .len(),
            1
        );
        f.display(true, false).await;
        assert!(
            f.app.last_sync_ts.get().await > pending_change,
            "completed projection signals the visible list"
        );
        assert_eq!(f.fetches(), 1);
        assert!(
            f.app
                .list_session_candidates(f.topic, TimelineScope::Public)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(f.event(&f.key).await, 1);
        f.display(false, false).await;
        f.app.shutdown().await;
    }
}

#[tokio::test]
async fn missing_manifest_ignores_rerenders_and_each_explicit_retry_is_one_attempt() {
    let f = SessionFixture::new("live").await;
    f.blobs
        .fail
        .store(true, std::sync::atomic::Ordering::SeqCst);
    f.event(&f.key).await;
    f.display(true, false).await;
    for _ in 0..8 {
        f.event(&f.key).await;
        f.display(true, false).await;
    }
    assert_eq!(f.fetches(), 1);
    for _ in 0..8 {
        f.display(true, true).await;
    }
    assert_eq!(f.fetches(), 9);
    f.display(false, false).await;
    f.display(true, false).await;
    assert_eq!(
        f.fetches(),
        9,
        "hiding/reopening does not restart an exhausted automatic retry"
    );
    f.app.shutdown().await;
}

#[path = "session_display_retry.rs"]
mod session_display_retry;

#[tokio::test(start_paused = true)]
async fn displayed_manifest_finishing_after_five_seconds_projects_without_another_event() {
    let f = SessionFixture::new("live").await;
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    *f.blobs.gate.lock().unwrap() = Some(gate.clone());
    f.event(&f.key).await;
    let before = f.app.last_sync_ts.get().await;
    f.app
        .set_session_display(crate::SessionDisplayRequest {
            topic: f.topic.into(),
            scope: TimelineScope::Public,
            replica_id: f.replica.as_str().into(),
            session_id: f.id.clone(),
            kind: "live".into(),
            observer: "slow-visible".into(),
            visible: true,
            retry: false,
        })
        .await
        .unwrap();
    wait_for_fetches(&f.blobs, 1).await;
    tokio::time::advance(Duration::from_secs(6)).await;
    gate.add_permits(1);
    f.app.services.session_projections.wait_idle().await;
    assert!(
        f.app
            .services
            .projection_store
            .get_live_session(f.topic, &f.id)
            .await
            .unwrap()
            .is_some()
    );
    assert!(f.app.last_sync_ts.get().await > before);
    assert_eq!(f.fetches(), 1);
    f.app.shutdown().await;
}

async fn wait_for_fetches(blobs: &RemoteManifest, expected: usize) {
    timeout(Duration::from_secs(5), async {
        while blobs.fetches.load(std::sync::atomic::Ordering::SeqCst) < expected {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("controlled fetch started");
}

#[tokio::test]
async fn queued_cancellation_does_not_spend_budget_and_fetch_concurrency_is_two() {
    let f = SessionFixture::new("live").await;
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    *f.blobs.gate.lock().unwrap() = Some(gate.clone());
    f.blobs
        .fail
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let registry = &f.app.services.session_projections;
    for index in 0..3 {
        let key = format!("sessions/live/controlled-{index}/state");
        registry
            .observe_key(
                f.topic,
                &f.replica,
                &key,
                vec![BlobHash::new(format!("hash-{index}"))],
                None,
            )
            .await;
        registry
            .visibility(
                f.topic,
                &f.replica,
                &key,
                &format!("card-{index}"),
                true,
                false,
            )
            .await
            .unwrap();
    }
    registry.schedule(&f.app.services).await;
    wait_for_fetches(&f.blobs, 2).await;
    let third = "sessions/live/controlled-2/state";
    for _ in 0..5 {
        registry
            .visibility(f.topic, &f.replica, third, "card-2", false, false)
            .await
            .unwrap();
        registry
            .visibility(f.topic, &f.replica, third, "card-2", true, false)
            .await
            .unwrap();
        registry.schedule(&f.app.services).await;
        tokio::task::yield_now().await;
    }
    assert_eq!(f.fetches(), 2, "only two network calls may be in flight");
    gate.add_permits(3);
    registry.wait_idle().await;
    assert_eq!(
        f.fetches(),
        3,
        "the queued card still has its first attempt"
    );
    // 本文取得と共有する内側walk枠を待つ間にも、表示取消で予算を使わない。
    let admission = Arc::new(tokio::sync::Semaphore::new(0));
    *f.blobs.admission_gate.lock().unwrap() = Some(admission.clone());
    for _ in 0..5 {
        registry
            .visibility(f.topic, &f.replica, third, "card-2", false, false)
            .await
            .unwrap();
        registry
            .visibility(f.topic, &f.replica, third, "card-2", true, false)
            .await
            .unwrap();
        registry.schedule(&f.app.services).await;
        tokio::task::yield_now().await;
    }
    assert_eq!(f.fetches(), 3);
    admission.add_permits(1);
    registry.wait_idle().await;
    assert_eq!(
        f.fetches(),
        3,
        "a cancelled admission does not bypass the shared retry cooldown"
    );
    let key = crate::service::hydration_limits::display_retry_key(
        "session",
        &format!("{}:{}", f.topic, f.replica.as_str()),
        &format!("{}:{}", third, "hash-2"),
    );
    assert!(
        f.app.services.missing_body_ledger.ready_key(&key, i64::MAX),
        "cancelled admission leaves a later retry available"
    );
    f.app.shutdown().await;
    assert!(
        f.app
            .list_session_candidates(f.topic, TimelineScope::Public)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn duplicate_candidate_sets_keep_shared_retry_history_and_candidate_memory_bounded() {
    let f = SessionFixture::new("live").await;
    f.blobs
        .fail
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let registry = &f.app.services.session_projections;
    let hashes = vec![BlobHash::new("a"), BlobHash::new("b")];
    registry
        .visibility(f.topic, &f.replica, &f.key, "card", true, false)
        .await
        .unwrap();
    for _ in 0..10 {
        registry
            .observe_key(f.topic, &f.replica, &f.key, hashes.clone(), None)
            .await;
        registry.schedule(&f.app.services).await;
        registry.wait_idle().await;
    }
    assert_eq!(
        f.fetches(),
        2,
        "each candidate is attempted once, not once per record pass"
    );
    for _ in 0..8 {
        registry
            .visibility(f.topic, &f.replica, &f.key, "card", true, true)
            .await
            .unwrap();
        registry.schedule(&f.app.services).await;
        registry.wait_idle().await;
    }
    assert_eq!(f.fetches(), 18);
    for count in [1000, 10_000, 100_000] {
        for index in 0..count {
            registry
                .observe_key(
                    f.topic,
                    &f.replica,
                    &format!("sessions/live/filler-{index}/state"),
                    hashes.clone(),
                    None,
                )
                .await;
        }
        assert_eq!(
            registry
                .candidates(f.topic, |replica| replica == &f.replica)
                .await
                .len(),
            64
        );
        assert_eq!(f.fetches(), 18, "nonvisible candidates do not fetch");
    }
    f.app.shutdown().await;
}

#[tokio::test]
async fn stale_live_read_cannot_overwrite_a_later_ended_projection() {
    let f = SessionFixture::new("live").await;
    f.event(&f.key).await;
    f.display(true, false).await;
    let state = f
        .docs
        .query_replica(&f.replica, DocQuery::Exact(f.key.clone()))
        .await
        .unwrap()
        .remove(0);
    let state: LiveSessionStateDocV1 = serde_json::from_slice(&state.value).unwrap();
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    *f.blobs.local_gate.lock().unwrap() = Some((
        state.current_manifest.hash,
        entered.clone(),
        release.clone(),
    ));
    let services = f.app.services.clone();
    let key = f.key.clone();
    let replica = f.replica.clone();
    let old = tokio::spawn(async move {
        crate::service::hydration_support::hydrate_session_key(
            &services,
            "kukuri:topic:session-display-contract",
            &replica,
            &key,
        )
        .await
    });
    timeout(Duration::from_secs(5), entered.notified())
        .await
        .unwrap();
    f.owner.end_live_session(f.topic, &f.id).await.unwrap();
    let latest = f
        .docs
        .query_replica(&f.replica, DocQuery::Exact(f.key.clone()))
        .await
        .unwrap()
        .remove(0);
    let latest: LiveSessionStateDocV1 = serde_json::from_slice(&latest.value).unwrap();
    f.blobs
        .fetch_blob(&latest.current_manifest.hash)
        .await
        .unwrap();
    assert_eq!(f.event(&f.key).await, 1);
    release.notify_one();
    old.await.unwrap().unwrap();
    let rows = f
        .app
        .services
        .projection_store
        .list_channel_live_sessions(f.topic, "public", 1)
        .await
        .unwrap();
    assert_eq!(rows[0].status, LiveSessionStatus::Ended);
    f.app.shutdown().await;
}

#[tokio::test]
async fn rejected_private_display_does_not_fetch_or_register_a_candidate() {
    let f = SessionFixture::new("live").await;
    let result = f
        .app
        .set_session_display(crate::SessionDisplayRequest {
            topic: f.topic.into(),
            scope: TimelineScope::Channel {
                channel_id: ChannelId::new("not-joined"),
            },
            replica_id: f.replica.as_str().into(),
            session_id: f.id.clone(),
            kind: "live".into(),
            observer: "unavailable-private-card".into(),
            visible: true,
            retry: false,
        })
        .await;
    assert!(result.is_err());
    assert_eq!(f.fetches(), 0);
    assert!(
        f.app
            .list_session_candidates(f.topic, TimelineScope::Public)
            .await
            .unwrap()
            .is_empty()
    );
    f.app.shutdown().await;
}

#[tokio::test]
async fn private_channel_removal_cancels_displayed_fetch_and_rejects_stale_registration() {
    let f = SessionFixture::new("live").await;
    let channel = f
        .app
        .create_private_channel(CreatePrivateChannelInput {
            topic_id: TopicId::new(f.topic),
            label: "private".into(),
            audience_kind: ChannelAudienceKind::InviteOnly,
        })
        .await
        .unwrap();
    f.blobs
        .publish_remote
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let id = f
        .app
        .create_live_session_in_channel(
            f.topic,
            ChannelRef::PrivateChannel {
                channel_id: ChannelId::new(channel.channel_id.clone()),
            },
            CreateLiveSessionInput {
                title: "private live".into(),
                description: String::new(),
            },
        )
        .await
        .unwrap();
    f.blobs
        .publish_remote
        .store(false, std::sync::atomic::Ordering::SeqCst);
    let scope = TimelineScope::Channel {
        channel_id: ChannelId::new(channel.channel_id.clone()),
    };
    let replica = f
        .app
        .scope_replicas(f.topic, &scope)
        .await
        .unwrap()
        .remove(0);
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    *f.blobs.gate.lock().unwrap() = Some(gate.clone());
    let request = crate::SessionDisplayRequest {
        topic: f.topic.into(),
        scope: scope.clone(),
        replica_id: replica.as_str().into(),
        session_id: id,
        kind: "live".into(),
        observer: "private-card".into(),
        visible: true,
        retry: false,
    };
    f.app.set_session_display(request.clone()).await.unwrap();
    wait_for_fetches(&f.blobs, 1).await;
    f.app
        .remove_joined_private_channel(f.topic, channel.channel_id.as_str())
        .await
        .unwrap();
    f.app.services.session_projections.wait_idle().await;
    gate.add_permits(1);
    assert!(f.app.set_session_display(request).await.is_err());
    assert!(
        f.app
            .services
            .session_projections
            .candidates(f.topic, |candidate| candidate == &replica)
            .await
            .is_empty()
    );
    assert_eq!(f.fetches(), 1);
    f.app.shutdown().await;
}
