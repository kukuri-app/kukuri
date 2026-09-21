use super::*;
use crate::service::game_projection_support::hydrate_game_room_from_record;
use crate::service::{catch_up_sessions, hydrate_game_room_from_key};
use kukuri_docs_sync::{DocEventStream, DocFetchPolicy, DocOp, DocQuery, DocRecord};
use kukuri_store::GameRoomProjectionRow;
use std::sync::atomic::AtomicUsize;
use tokio::sync::Notify;

const TOPIC: &str = "kukuri:topic:game-projection-freshness";

#[derive(Default)]
struct FetchGate {
    entered: Notify,
    release: Notify,
}

#[derive(Default)]
struct GatedBlobService {
    inner: MemoryBlobService,
    gate: TokioMutex<Option<(kukuri_core::BlobHash, Arc<FetchGate>)>>,
    hidden: TokioMutex<HashSet<String>>,
    puts: AtomicUsize,
    pins: AtomicUsize,
    missing_seen: Notify,
}

impl GatedBlobService {
    async fn pause_next_fetch(&self, hash: kukuri_core::BlobHash) -> Arc<FetchGate> {
        let gate = Arc::new(FetchGate::default());
        *self.gate.lock().await = Some((hash, gate.clone()));
        gate
    }
}

#[async_trait]
impl BlobService for GatedBlobService {
    async fn put_blob(&self, data: Vec<u8>, mime: &str) -> Result<StoredBlob> {
        self.puts.fetch_add(1, Ordering::SeqCst);
        self.inner.put_blob(data, mime).await
    }

    async fn fetch_blob(&self, hash: &kukuri_core::BlobHash) -> Result<Option<Vec<u8>>> {
        let gate = self.gate.lock().await.take_if(|(target, _)| target == hash);
        if let Some((_, gate)) = gate {
            gate.entered.notify_one();
            gate.release.notified().await;
        }
        if self.hidden.lock().await.contains(hash.as_str()) {
            self.missing_seen.notify_one();
            return Ok(None);
        }
        self.inner.fetch_blob(hash).await
    }

    async fn pin_blob(&self, hash: &kukuri_core::BlobHash) -> Result<()> {
        self.pins.fetch_add(1, Ordering::SeqCst);
        self.inner.pin_blob(hash).await
    }

    async fn blob_status(&self, hash: &kukuri_core::BlobHash) -> Result<BlobStatus> {
        self.inner.blob_status(hash).await
    }

    async fn local_blob_status(&self, hash: &kukuri_core::BlobHash) -> Result<BlobStatus> {
        self.inner.local_blob_status(hash).await
    }

    async fn import_peer_ticket(&self, ticket: &str) -> Result<()> {
        self.inner.import_peer_ticket(ticket).await
    }
}

#[derive(Default)]
struct ControlledDocs {
    inner: MemoryDocsSync,
    writes: AtomicUsize,
    queries: AtomicUsize,
    fail_local_query: AtomicBool,
    local_gate: TokioMutex<Option<(String, Arc<FetchGate>)>>,
}

#[async_trait]
impl DocsSync for ControlledDocs {
    async fn open_replica(&self, replica: &ReplicaId) -> Result<()> {
        self.inner.open_replica(replica).await
    }

    async fn apply_doc_op(&self, replica: &ReplicaId, op: DocOp) -> Result<()> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        self.inner.apply_doc_op(replica, op).await
    }

    async fn query_replica_with_policy(
        &self,
        replica: &ReplicaId,
        query: DocQuery,
        policy: DocFetchPolicy,
    ) -> Result<Vec<DocRecord>> {
        self.queries.fetch_add(1, Ordering::SeqCst);
        if policy == DocFetchPolicy::LocalOnly && self.fail_local_query.load(Ordering::SeqCst) {
            anyhow::bail!("injected canonical read failure");
        }
        let rows = self
            .inner
            .query_replica_with_policy(replica, query.clone(), policy)
            .await?;
        if let DocQuery::Exact(key) = query
            && policy == DocFetchPolicy::LocalOnly
        {
            let gate = self
                .local_gate
                .lock()
                .await
                .take_if(|(target, _)| *target == key);
            if let Some((_, gate)) = gate {
                // The canonical snapshot is fixed; the caller must protect its commit.
                gate.entered.notify_one();
                gate.release.notified().await;
            }
        }
        Ok(rows)
    }

    // 上限つきの key の一覧の既定実装はエラーを返す。session の固定件数の反映が使うので転送する。
    async fn query_replica_keys(
        &self,
        replica: &ReplicaId,
        query: kukuri_docs_sync::DocKeyQuery,
    ) -> Result<kukuri_docs_sync::DocKeyPage> {
        self.inner.query_replica_keys(replica, query).await
    }

    async fn subscribe_replica(&self, replica: &ReplicaId) -> Result<DocEventStream> {
        self.inner.subscribe_replica(replica).await
    }

    async fn import_peer_ticket(&self, ticket: &str) -> Result<()> {
        self.inner.import_peer_ticket(ticket).await
    }
}

#[derive(Default)]
struct CountingHints(AtomicUsize);

#[async_trait]
impl HintTransport for CountingHints {
    async fn subscribe_hints(&self, _: &TopicId) -> Result<HintStream> {
        Ok(Box::pin(futures_util::stream::pending()))
    }

    async fn unsubscribe_hints(&self, _: &TopicId) -> Result<()> {
        Ok(())
    }

    async fn publish_hint(&self, _: &TopicId, _: GossipHint) -> Result<()> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

struct Fixture {
    app: AppService,
    blobs: Arc<GatedBlobService>,
    docs: Arc<ControlledDocs>,
    hints: Arc<CountingHints>,
    sqlite: Option<Arc<SqliteStore>>,
    _root: tempfile::TempDir,
}

impl Fixture {
    async fn new(sqlite: bool) -> Self {
        let root = tempdir().unwrap();
        let mut sqlite_store = None;
        let (store, projection_store): (Arc<dyn Store>, Arc<dyn ProjectionStore>) = if sqlite {
            let store = Arc::new(
                SqliteStore::connect_file(root.path().join("game.db"))
                    .await
                    .unwrap(),
            );
            sqlx::raw_sql(
                "CREATE TABLE game_projection_write_log (room_id TEXT, operation TEXT);
                 CREATE TRIGGER observe_game_insert AFTER INSERT ON game_room_cache BEGIN
                   INSERT INTO game_projection_write_log VALUES (NEW.room_id, 'insert'); END;
                 CREATE TRIGGER observe_game_update AFTER UPDATE ON game_room_cache BEGIN
                   INSERT INTO game_projection_write_log VALUES (NEW.room_id, 'update'); END;
                 CREATE TRIGGER observe_game_delete AFTER DELETE ON game_room_cache BEGIN
                   INSERT INTO game_projection_write_log VALUES (OLD.room_id, 'delete'); END;",
            )
            .execute(store.pool())
            .await
            .unwrap();
            sqlite_store = Some(store.clone());
            (store.clone(), store)
        } else {
            let store = Arc::new(MemoryStore::default());
            (store.clone(), store)
        };
        let blobs = Arc::new(GatedBlobService::default());
        let docs = Arc::new(ControlledDocs::default());
        let hints = Arc::new(CountingHints::default());
        let app = AppService::from_handles(ServiceHandles::new(
            store,
            projection_store,
            Arc::new(FakeTransport::new("game-fixture", FakeNetwork::default())),
            hints.clone(),
            docs.clone(),
            blobs.clone(),
            generate_keys(),
        ));
        // Drive bootstrap/event hydration explicitly so the test owns every writer's order.
        app.gossip_disabled_topics.lock().await.insert(TOPIC.into());
        Self {
            app,
            blobs,
            docs,
            hints,
            sqlite: sqlite_store,
            _root: root,
        }
    }

    async fn create(&self) -> String {
        self.app
            .create_game_room(
                TOPIC,
                CreateGameRoomInput {
                    title: "finals".into(),
                    description: "controlled hydration".into(),
                    participants: vec!["Alice".into(), "Bob".into()],
                },
            )
            .await
            .unwrap()
    }

    async fn row(&self, room_id: &str) -> GameRoomProjectionRow {
        self.app
            .services
            .projection_store
            .list_topic_game_rooms(TOPIC)
            .await
            .unwrap()
            .into_iter()
            .find(|row| row.room_id == room_id)
            .unwrap()
    }

    async fn update(&self, room_id: &str, score: i64) {
        self.app
            .update_game_room(TOPIC, room_id, update_input(score))
            .await
            .unwrap();
    }

    async fn record(&self, room_id: &str) -> DocRecord {
        self.docs
            .query_replica(
                &topic_replica_id(TOPIC),
                DocQuery::Exact(stable_key("sessions/game", &format!("{room_id}/state"))),
            )
            .await
            .unwrap()
            .into_iter()
            .next()
            .unwrap()
    }

    async fn hydrate(&self, room_id: &str) -> Result<bool> {
        hydrate_game_room_from_key(
            &self.app.services,
            TOPIC,
            &topic_replica_id(TOPIC),
            &stable_key("sessions/game", &format!("{room_id}/state")),
        )
        .await
    }

    async fn publish_state(&self, room_id: &str, score: i64, updated_at: i64) -> DocRecord {
        let mut state: GameRoomStateDocV1 =
            serde_json::from_slice(&self.record(room_id).await.value).unwrap();
        let mut manifest: GameRoomManifestBlobV1 = serde_json::from_slice(
            &self
                .blobs
                .inner
                .fetch_blob(&state.current_manifest.hash)
                .await
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        manifest.scores[0].score = score;
        manifest.status = GameRoomStatus::Running;
        manifest.phase_label = Some("round 1".into());
        manifest.updated_at = updated_at;
        let blob = store_manifest_blob(self.blobs.as_ref(), &manifest, GAME_MANIFEST_MIME)
            .await
            .unwrap();
        // #1252: 反映は owner が署名した manifest を要求する。owner の別端末からの書き込みと同じ形で置く。
        let envelope = kukuri_core::build_game_session_envelope(
            self.app.services.keys.as_ref(),
            &manifest.topic_id,
            manifest.room_id.as_str(),
            &manifest,
        )
        .unwrap();
        persist_session_envelope(self.docs.as_ref(), &topic_replica_id(TOPIC), &envelope)
            .await
            .unwrap();
        state.last_envelope_id = envelope.id;
        state.status = manifest.status;
        state.updated_at = updated_at;
        state.current_manifest = ManifestBlobRef {
            hash: blob.hash,
            mime: blob.mime,
            bytes: blob.bytes,
        };
        persist_game_room_state(self.docs.as_ref(), &topic_replica_id(TOPIC), &state)
            .await
            .unwrap();
        self.record(room_id).await
    }

    async fn mutation_counts(&self) -> (usize, usize, usize, usize, Option<i64>) {
        let db_writes = if let Some(store) = &self.sqlite {
            Some(
                sqlx::query_scalar("SELECT COUNT(*) FROM game_projection_write_log")
                    .fetch_one(store.pool())
                    .await
                    .unwrap(),
            )
        } else {
            None
        };
        (
            self.docs.writes.load(Ordering::SeqCst),
            self.blobs.puts.load(Ordering::SeqCst),
            self.blobs.pins.load(Ordering::SeqCst),
            self.hints.0.load(Ordering::SeqCst),
            db_writes,
        )
    }

    async fn another_author(&self) -> AppService {
        let mut services = self.app.services.clone();
        services.keys = Arc::new(generate_keys());
        let app = AppService::from_handles(services);
        app.gossip_disabled_topics.lock().await.insert(TOPIC.into());
        app
    }
}

fn update_input(score: i64) -> UpdateGameRoomInput {
    UpdateGameRoomInput {
        status: GameRoomStatus::Running,
        phase_label: Some("round 1".into()),
        scores: vec![
            GameScoreView {
                participant_id: "participant-1".into(),
                label: "Alice".into(),
                score,
            },
            GameScoreView {
                participant_id: "participant-2".into(),
                label: "Bob".into(),
                score: 0,
            },
        ],
    }
}

async fn late_hydration_keeps_valid_update(sqlite: bool, batch: bool) {
    let fixture = Fixture::new(sqlite).await;
    let room_id = fixture.create().await;
    let old = fixture.row(&room_id).await;
    let gate = fixture
        .blobs
        .pause_next_fetch(old.manifest_blob_hash.clone())
        .await;
    let services = fixture.app.services.clone();
    let key = old.source_key.clone();
    let hydration = tokio::spawn(async move {
        let replica = topic_replica_id(TOPIC);
        if batch {
            // #1239: replica の全件走査は削除した。session の固定件数の反映(購読タスクの追いつきと同じ)を通す。
            catch_up_sessions(&services, TOPIC, &replica, DocFetchPolicy::LocalOnly)
                .await
                .map(|()| 1)
        } else {
            hydrate_game_room_from_key(&services, TOPIC, &replica, &key)
                .await
                .map(usize::from)
        }
    });
    timeout(Duration::from_secs(5), gate.entered.notified())
        .await
        .expect("old record reached the blob fetch gate");
    fixture.update(&room_id, 7).await;
    let before = fixture.row(&room_id).await;
    assert_eq!(before.scores[0].score, 7);
    assert_ne!(old.manifest_blob_hash, before.manifest_blob_hash);
    eprintln!(
        "writer sequence: {} captured {}/{} at {}; update committed {} at {}; resume old hydration ({})",
        if batch { "replica" } else { "record" },
        old.source_replica_id.as_str(),
        old.source_key,
        old.updated_at,
        before.manifest_blob_hash.as_str(),
        before.updated_at,
        old.manifest_blob_hash.as_str(),
    );
    gate.release.notify_one();
    timeout(Duration::from_secs(5), hydration)
        .await
        .expect("old hydration completed")
        .unwrap()
        .unwrap();
    let after = fixture.row(&room_id).await;
    assert_eq!(
        before, after,
        "old hydration must not replace a confirmed valid update"
    );
}

#[tokio::test]
async fn late_record_hydration_keeps_valid_update_memory() {
    late_hydration_keeps_valid_update(false, false).await;
}

#[tokio::test]
async fn late_record_hydration_keeps_valid_update_sqlite() {
    late_hydration_keeps_valid_update(true, false).await;
}

#[tokio::test]
async fn late_replica_hydration_keeps_valid_update_memory() {
    late_hydration_keeps_valid_update(false, true).await;
}

#[tokio::test]
async fn late_replica_hydration_keeps_valid_update_sqlite() {
    late_hydration_keeps_valid_update(true, true).await;
}

async fn rejected_updates_do_not_mutate(sqlite: bool) {
    let fixture = Fixture::new(sqlite).await;
    let room_id = fixture.create().await;
    fixture.update(&room_id, 7).await;
    let before = fixture.row(&room_id).await;
    let record = fixture.record(&room_id).await;
    let manifest = fixture
        .blobs
        .inner
        .fetch_blob(&before.manifest_blob_hash)
        .await
        .unwrap();
    let writes = fixture.mutation_counts().await;
    assert!(writes.0 >= 2 && writes.1 >= 2 && writes.3 >= 2);
    if sqlite {
        assert_eq!(
            writes.4,
            Some(2),
            "the trigger observes both successful cache writes"
        );
    }

    let mut empty = update_input(8);
    empty.scores.clear();
    let mut unknown = update_input(8);
    unknown.scores[1].participant_id = "unknown".into();
    let mut duplicate = update_input(8);
    duplicate.scores[1] = duplicate.scores[0].clone();
    let mut renamed = update_input(8);
    renamed.scores[0].label = "renamed".into();
    for input in [empty, unknown, duplicate, renamed] {
        fixture
            .app
            .update_game_room(TOPIC, &room_id, input)
            .await
            .expect_err("invalid roster");
        assert_eq!(fixture.mutation_counts().await, writes);
        assert_eq!(fixture.record(&room_id).await, record);
        assert_eq!(fixture.row(&room_id).await, before);
    }
    let non_owner = fixture.another_author().await;
    let error = non_owner
        .update_game_room(TOPIC, &room_id, update_input(8))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("only the game room owner"));
    assert_eq!(fixture.mutation_counts().await, writes);
    assert_eq!(fixture.record(&room_id).await, record);
    assert_eq!(fixture.row(&room_id).await, before);
    assert_eq!(
        fixture
            .blobs
            .inner
            .fetch_blob(&before.manifest_blob_hash)
            .await
            .unwrap(),
        manifest
    );
}

#[tokio::test]
async fn rejected_updates_do_not_mutate_memory() {
    rejected_updates_do_not_mutate(false).await;
}

#[tokio::test]
async fn rejected_updates_do_not_mutate_sqlite() {
    rejected_updates_do_not_mutate(true).await;
}

async fn refresh_trigger(services: &ServiceHandles, room_id: &str, hint: bool) -> Result<usize> {
    let replica = topic_replica_id(TOPIC);
    if hint {
        hydrate_subscription_hint(
            services,
            TOPIC,
            &replica,
            &GossipHint::SessionChanged {
                topic_id: TopicId::new(TOPIC),
                session_id: room_id.into(),
                object_kind: "game-session".into(),
            },
        )
        .await
    } else {
        hydrate_subscription_event(
            services,
            TOPIC,
            &replica,
            &stable_key("sessions/game", &format!("{room_id}/state")),
        )
        .await
    }
}

async fn current_pointer_wins_at_same_timestamp(hint: bool) {
    for sqlite in [false, true] {
        let fixture = Fixture::new(sqlite).await;
        let room_id = fixture.create().await;
        let old_record = fixture.publish_state(&room_id, 1, 100).await;
        assert!(fixture.hydrate(&room_id).await.unwrap());
        let old_row = fixture.row(&room_id).await;
        let gate = fixture
            .blobs
            .pause_next_fetch(old_row.manifest_blob_hash.clone())
            .await;
        let services = fixture.app.services.clone();
        let room = room_id.clone();
        let refresh = tokio::spawn(async move { refresh_trigger(&services, &room, hint).await });
        timeout(Duration::from_secs(5), gate.entered.notified())
            .await
            .unwrap();
        fixture.publish_state(&room_id, 7, 100).await;
        // B exists only in canonical docs/blobs; stale A must resolve B, not merely skip.
        assert_eq!(fixture.row(&room_id).await, old_row);
        gate.release.notify_one();
        assert_eq!(
            timeout(Duration::from_secs(5), refresh)
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
            1
        );
        let latest = fixture.row(&room_id).await;
        assert_eq!(latest.updated_at, old_row.updated_at);
        assert_eq!(latest.scores[0].score, 7);
        assert_ne!(latest.manifest_blob_hash, old_row.manifest_blob_hash);

        let writes = fixture.mutation_counts().await;
        assert!(
            hydrate_game_room_from_record(
                &fixture.app.services,
                TOPIC,
                &topic_replica_id(TOPIC),
                old_record,
            )
            .await
            .unwrap()
        );
        assert_eq!(
            fixture.row(&room_id).await,
            latest,
            "replaying A keeps all of B's columns"
        );
        assert_eq!(
            fixture.mutation_counts().await,
            writes,
            "replay does not rewrite B"
        );
    }
}

#[tokio::test]
async fn docs_event_resolves_current_pointer_at_same_timestamp() {
    current_pointer_wins_at_same_timestamp(false).await;
}

#[tokio::test]
async fn hint_resolves_current_pointer_at_same_timestamp() {
    current_pointer_wins_at_same_timestamp(true).await;
}

async fn comparison_and_commit_exclude_same_room_writer(sqlite: bool) {
    let fixture = Fixture::new(sqlite).await;
    let room_id = fixture.create().await;
    let another = fixture.another_author().await;
    let other_room = another
        .create_game_room(
            TOPIC,
            CreateGameRoomInput {
                title: "other".into(),
                description: String::new(),
                participants: vec!["Alice".into(), "Bob".into()],
            },
        )
        .await
        .unwrap();
    assert_ne!(room_id, other_room);
    let gate = Arc::new(FetchGate::default());
    *fixture.docs.local_gate.lock().await =
        Some((fixture.row(&room_id).await.source_key, gate.clone()));
    let services = fixture.app.services.clone();
    let room = room_id.clone();
    let hydration = tokio::spawn(async move { refresh_trigger(&services, &room, false).await });
    timeout(Duration::from_secs(5), gate.entered.notified())
        .await
        .unwrap();

    let reads = fixture.docs.queries.load(Ordering::SeqCst);
    let mut update = Box::pin(
        fixture
            .app
            .update_game_room(TOPIC, &room_id, update_input(7)),
    );
    assert!(futures_util::poll!(update.as_mut()).is_pending());
    assert_eq!(
        fixture.docs.queries.load(Ordering::SeqCst),
        reads,
        "same-room update cannot even read its mutable base until hydration commits"
    );
    timeout(
        Duration::from_secs(5),
        another.update_game_room(TOPIC, &other_room, update_input(3)),
    )
    .await
    .unwrap()
    .unwrap();
    let other_before = fixture.row(&other_room).await;
    assert_eq!(other_before.scores[0].score, 3);
    gate.release.notify_one();
    timeout(Duration::from_secs(5), hydration)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    timeout(Duration::from_secs(5), update)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fixture.row(&room_id).await.scores[0].score, 7);
    assert_eq!(fixture.row(&other_room).await, other_before);
}

#[tokio::test]
async fn comparison_and_commit_exclude_same_room_writer_memory() {
    comparison_and_commit_exclude_same_room_writer(false).await;
}

#[tokio::test]
async fn comparison_and_commit_exclude_same_room_writer_sqlite() {
    comparison_and_commit_exclude_same_room_writer(true).await;
}

#[tokio::test]
async fn canonical_read_failure_and_cancel_release_the_room() {
    for sqlite in [false, true] {
        let fixture = Fixture::new(sqlite).await;
        let room_id = fixture.create().await;
        let before = fixture.row(&room_id).await;
        let writes = fixture.mutation_counts().await;
        fixture.docs.fail_local_query.store(true, Ordering::SeqCst);
        assert!(
            fixture
                .hydrate(&room_id)
                .await
                .unwrap_err()
                .to_string()
                .contains("canonical read failure")
        );
        assert_eq!(fixture.row(&room_id).await, before);
        assert_eq!(fixture.mutation_counts().await, writes);
        fixture.docs.fail_local_query.store(false, Ordering::SeqCst);

        let gate = Arc::new(FetchGate::default());
        *fixture.docs.local_gate.lock().await = Some((before.source_key.clone(), gate.clone()));
        let services = fixture.app.services.clone();
        let room = room_id.clone();
        let hydration = tokio::spawn(async move { refresh_trigger(&services, &room, false).await });
        timeout(Duration::from_secs(5), gate.entered.notified())
            .await
            .unwrap();
        hydration.abort();
        assert!(hydration.await.unwrap_err().is_cancelled());
        timeout(
            Duration::from_secs(5),
            fixture
                .app
                .update_game_room(TOPIC, &room_id, update_input(7)),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(fixture.row(&room_id).await.scores[0].score, 7);
    }
}

#[tokio::test]
async fn missing_blob_retry_refreshes_without_overwriting_the_cache() {
    for sqlite in [false, true] {
        let fixture = Fixture::new(sqlite).await;
        let room_id = fixture.create().await;
        fixture.update(&room_id, 7).await;
        let before = fixture.row(&room_id).await;
        let record = fixture.publish_state(&room_id, 8, before.updated_at).await;
        let state: GameRoomStateDocV1 = serde_json::from_slice(&record.value).unwrap();
        fixture
            .blobs
            .hidden
            .lock()
            .await
            .insert(state.current_manifest.hash.as_str().into());
        let writes = fixture.mutation_counts().await;
        let services = fixture.app.services.clone();
        let room = room_id.clone();
        let hydration = tokio::spawn(async move { refresh_trigger(&services, &room, false).await });
        timeout(
            Duration::from_secs(5),
            fixture.blobs.missing_seen.notified(),
        )
        .await
        .unwrap();
        assert_eq!(fixture.row(&room_id).await, before);
        assert_eq!(fixture.mutation_counts().await, writes);
        fixture.blobs.hidden.lock().await.clear();
        assert_eq!(
            timeout(Duration::from_secs(15), hydration)
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
            1
        );
        assert_eq!(fixture.row(&room_id).await.scores[0].score, 8);
    }
}

#[tokio::test]
async fn missing_and_corrupt_canonical_state_do_not_apply_the_candidate() {
    for sqlite in [false, true] {
        let fixture = Fixture::new(sqlite).await;
        let room_id = fixture.create().await;
        fixture.update(&room_id, 7).await;
        let before = fixture.row(&room_id).await;
        let record = fixture.record(&room_id).await;
        let replica = topic_replica_id(TOPIC);
        fixture
            .docs
            .apply_doc_op(
                &replica,
                DocOp::DeletePrefix {
                    prefix: record.key.clone(),
                },
            )
            .await
            .unwrap();
        assert!(
            !hydrate_game_room_from_record(&fixture.app.services, TOPIC, &replica, record.clone())
                .await
                .unwrap()
        );
        assert_eq!(fixture.row(&room_id).await, before);
        fixture
            .docs
            .apply_doc_op(
                &replica,
                DocOp::SetBytes {
                    key: record.key.clone(),
                    value: b"invalid json".to_vec(),
                },
            )
            .await
            .unwrap();
        // #1252: 読めない record は、その room だけを飛ばす(エラーにしない)。
        assert!(!fixture.hydrate(&room_id).await.unwrap());
        assert_eq!(fixture.row(&room_id).await, before);
        fixture
            .docs
            .apply_doc_op(
                &replica,
                DocOp::SetBytes {
                    key: record.key,
                    value: record.value,
                },
            )
            .await
            .unwrap();
        let writes = fixture.mutation_counts().await;
        assert!(fixture.hydrate(&room_id).await.unwrap());
        assert_eq!(fixture.row(&room_id).await, before);
        assert_eq!(fixture.mutation_counts().await, writes);
    }
}

#[tokio::test]
async fn cache_write_failure_is_recoverable_from_canonical_state() {
    let fixture = Fixture::new(true).await;
    let room_id = fixture.create().await;
    let before = fixture.row(&room_id).await;
    let store = fixture.sqlite.as_ref().unwrap();
    sqlx::raw_sql(
        "CREATE TRIGGER fail_game_cache BEFORE INSERT ON game_room_cache BEGIN
        SELECT RAISE(ABORT, 'injected cache write failure'); END;",
    )
    .execute(store.pool())
    .await
    .unwrap();
    assert!(
        fixture
            .app
            .update_game_room(TOPIC, &room_id, update_input(7))
            .await
            .unwrap_err()
            .to_string()
            .contains("injected cache write failure")
    );
    assert_eq!(fixture.row(&room_id).await, before);
    sqlx::raw_sql("DROP TRIGGER fail_game_cache")
        .execute(store.pool())
        .await
        .unwrap();
    assert!(
        timeout(Duration::from_secs(5), fixture.hydrate(&room_id))
            .await
            .unwrap()
            .unwrap()
    );
    assert_eq!(fixture.row(&room_id).await.scores[0].score, 7);
}

#[tokio::test]
async fn restart_and_missing_cache_resolve_docs_at_the_same_timestamp() {
    let fixture = Fixture::new(true).await;
    let room_id = fixture.create().await;
    fixture.publish_state(&room_id, 1, 100).await;
    assert!(fixture.hydrate(&room_id).await.unwrap());
    fixture.publish_state(&room_id, 7, 100).await;
    let path = fixture._root.path().join("game.db");
    let mut services = fixture.app.services.clone();
    fixture.app.shutdown().await;
    fixture.sqlite.as_ref().unwrap().close().await;
    let reopened = Arc::new(SqliteStore::connect_file(&path).await.unwrap());
    services.store = reopened.clone();
    services.projection_store = reopened.clone();
    services.game_room_projections = Arc::default();
    catch_up_sessions(
        &services,
        TOPIC,
        &topic_replica_id(TOPIC),
        DocFetchPolicy::LocalOnly,
    )
    .await
    .unwrap();
    let row = reopened
        .list_topic_game_rooms(TOPIC)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(row.updated_at, 100);
    assert_eq!(row.scores[0].score, 7);
    sqlx::query("DELETE FROM game_room_cache")
        .execute(reopened.pool())
        .await
        .unwrap();
    catch_up_sessions(
        &services,
        TOPIC,
        &topic_replica_id(TOPIC),
        DocFetchPolicy::LocalOnly,
    )
    .await
    .unwrap();
    let rebuilt = reopened
        .list_topic_game_rooms(TOPIC)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(rebuilt.manifest_blob_hash, row.manifest_blob_hash);
    assert_eq!(rebuilt.scores, row.scores);
    reopened.close().await;
}
