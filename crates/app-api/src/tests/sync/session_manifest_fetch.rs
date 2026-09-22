//! #1261: manifest の取得前に署名と hash の対応を検証する。
use super::*;
use crate::service::{verify_game_room_record, verify_live_session_record};
use kukuri_core::{GameRoomStateDocV1, LiveSessionStateDocV1};
use kukuri_docs_sync::{DocFetchPolicy, DocRecord};

#[derive(Default)]
struct ObservedBlobs {
    inner: MemoryBlobService,
    calls: TokioMutex<Vec<BlobHash>>,
    fail_fetch: AtomicBool,
}

#[async_trait]
impl BlobService for ObservedBlobs {
    async fn put_blob(&self, data: Vec<u8>, mime: &str) -> Result<StoredBlob> {
        self.inner.put_blob(data, mime).await
    }
    async fn fetch_blob(&self, hash: &BlobHash) -> Result<Option<Vec<u8>>> {
        self.calls.lock().await.push(hash.clone());
        if self.fail_fetch.load(Ordering::SeqCst) {
            anyhow::bail!("simulated remote failure");
        }
        self.inner.fetch_blob(hash).await
    }
    async fn pin_blob(&self, _: &BlobHash) -> Result<()> {
        panic!("unexpected pin")
    }
    async fn blob_status(&self, _: &BlobHash) -> Result<BlobStatus> {
        panic!("unexpected status")
    }
    async fn local_blob_status(&self, _: &BlobHash) -> Result<BlobStatus> {
        panic!("unexpected local status")
    }
    async fn import_peer_ticket(&self, _: &str) -> Result<()> {
        panic!("unexpected ticket")
    }
}

struct Fixture {
    owner: AppService,
    docs: Arc<kukuri_docs_sync::MemoryDocsSync>,
    source: Arc<MemoryBlobService>,
    topic: TopicId,
    replica: ReplicaId,
    record: DocRecord,
    live: bool,
}

impl Fixture {
    async fn new(kind: &str) -> Self {
        let (owner, _, docs, source) = local_app_with_memory_services();
        let topic = TopicId::new(format!("kukuri:topic:manifest-fetch-{kind}"));
        let live = kind == "live";
        let id = match kind {
            "live" => owner
                .create_live_session(
                    topic.as_str(),
                    CreateLiveSessionInput {
                        title: "live".into(),
                        description: String::new(),
                    },
                )
                .await
                .expect("create live"),
            "score" => owner
                .create_game_room(
                    topic.as_str(),
                    CreateGameRoomInput {
                        title: "score".into(),
                        description: String::new(),
                        participants: vec!["a".into(), "b".into()],
                    },
                )
                .await
                .expect("create score"),
            _ => owner
                .create_metaverse_room(
                    topic.as_str(),
                    CreateMetaverseRoomInput {
                        title: "dome".into(),
                        description: String::new(),
                        max_peers: None,
                    },
                )
                .await
                .expect("create dome"),
        };
        let replica = topic_replica_id(topic.as_str());
        let key = format!("sessions/{}/{id}/state", if live { "live" } else { "game" });
        let record = docs
            .query_replica(&replica, DocQuery::Exact(key))
            .await
            .unwrap()
            .remove(0);
        Self {
            owner,
            docs,
            source,
            topic,
            replica,
            record,
            live,
        }
    }

    async fn verify(&self, blobs: &ObservedBlobs, record: &DocRecord) -> bool {
        if self.live {
            verify_live_session_record(
                self.docs.as_ref(),
                blobs,
                &self.replica,
                self.topic.as_str(),
                record,
                DocFetchPolicy::LocalOnly,
            )
            .await
            .unwrap()
            .is_some()
        } else {
            verify_game_room_record(
                self.docs.as_ref(),
                blobs,
                &self.replica,
                self.topic.as_str(),
                record,
                DocFetchPolicy::LocalOnly,
            )
            .await
            .unwrap()
            .is_some()
        }
    }

    fn hash(&self) -> BlobHash {
        let value: serde_json::Value = serde_json::from_slice(&self.record.value).unwrap();
        serde_json::from_value(value["current_manifest"]["hash"].clone()).unwrap()
    }

    async fn finish(self) {
        self.owner.shutdown().await;
    }
}

async fn rejects_changed_hash(kind: &str) {
    let fixture = Fixture::new(kind).await;
    let blobs = ObservedBlobs::default();
    let mut record = fixture.record.clone();
    let mut state: serde_json::Value = serde_json::from_slice(&record.value).unwrap();
    state["current_manifest"]["hash"] =
        serde_json::to_value(kukuri_core::blob_hash(b"untrusted")).unwrap();
    record.value = serde_json::to_vec(&state).unwrap();
    assert!(!fixture.verify(&blobs, &record).await);
    assert!(
        blobs.calls.lock().await.is_empty(),
        "untrusted hash reached BlobService"
    );
    fixture.finish().await;
}

#[tokio::test]
async fn live_changed_hash_never_reaches_blob_service() {
    rejects_changed_hash("live").await;
}
#[tokio::test]
async fn score_changed_hash_never_reaches_blob_service() {
    rejects_changed_hash("score").await;
}
#[tokio::test]
async fn signed_dome_changed_hash_never_reaches_blob_service() {
    rejects_changed_hash("dome").await;
}

#[tokio::test]
async fn valid_sessions_accept_late_blobs_without_changing_the_requested_hash() {
    for kind in ["live", "score", "dome"] {
        let fixture = Fixture::new(kind).await;
        let blobs = ObservedBlobs::default();
        blobs.fail_fetch.store(true, Ordering::SeqCst);
        assert!(!fixture.verify(&blobs, &fixture.record).await);
        blobs.fail_fetch.store(false, Ordering::SeqCst);
        assert!(!fixture.verify(&blobs, &fixture.record).await);
        let bytes = fixture
            .source
            .fetch_blob(&fixture.hash())
            .await
            .unwrap()
            .unwrap();
        blobs.put_blob(bytes, "application/json").await.unwrap();
        assert!(fixture.verify(&blobs, &fixture.record).await);
        assert_eq!(
            *blobs.calls.lock().await,
            vec![fixture.hash(), fixture.hash(), fixture.hash()]
        );
        fixture.finish().await;
    }
}

#[tokio::test]
async fn legacy_dome_allows_late_fetch_only_for_matching_identity_and_scope() {
    let fixture = Fixture::new("dome").await;
    let mut state: GameRoomStateDocV1 = serde_json::from_slice(&fixture.record.value).unwrap();
    state.last_envelope_id = EnvelopeId::from("legacy-envelope-not-present");
    let mut legacy = fixture.record.clone();
    legacy.value = serde_json::to_vec(&state).unwrap();
    let blobs = ObservedBlobs::default();
    for mutation in ["id", "owner", "topic", "channel", "key"] {
        let mut changed = state.clone();
        let mut record = legacy.clone();
        match mutation {
            "id" => {
                changed.room_id = "dome-000000000000000000000000".into();
                record.key = format!("sessions/game/{}/state", changed.room_id);
            }
            "owner" => changed.owner_pubkey = generate_keys().public_key(),
            "topic" => changed.topic_id = TopicId::new("another-topic"),
            "channel" => changed.channel_id = Some(ChannelId::new("another-channel")),
            _ => record.key = "sessions/game/another/state".into(),
        }
        record.value = serde_json::to_vec(&changed).unwrap();
        assert!(!fixture.verify(&blobs, &record).await, "{mutation}");
        assert!(
            blobs.calls.lock().await.is_empty(),
            "{mutation} fetched a blob"
        );
    }
    assert!(!fixture.verify(&blobs, &legacy).await);
    let bytes = fixture
        .source
        .fetch_blob(&fixture.hash())
        .await
        .unwrap()
        .unwrap();
    blobs.put_blob(bytes, "application/json").await.unwrap();
    assert!(fixture.verify(&blobs, &legacy).await);
    assert_eq!(
        *blobs.calls.lock().await,
        vec![fixture.hash(), fixture.hash()]
    );
    fixture.finish().await;
}

fn viewer(docs: Arc<dyn DocsSync>, blobs: Arc<ObservedBlobs>) -> (AppService, Arc<MemoryStore>) {
    let store = Arc::new(MemoryStore::default());
    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        Arc::new(StaticTransport::new(PeerSnapshot::default())),
        Arc::new(NoopHintTransport),
        docs,
        blobs,
        generate_keys(),
    );
    (app, store)
}

#[tokio::test]
async fn rejected_hash_preserves_projection_and_operations_across_viewer_restart() {
    for kind in ["live", "score", "dome"] {
        let fixture = Fixture::new(kind).await;
        let blobs = Arc::new(ObservedBlobs {
            inner: fixture.source.as_ref().clone(),
            ..Default::default()
        });
        let (app, store) = viewer(fixture.docs.clone(), blobs.clone());
        let id = if fixture.live {
            let state: LiveSessionStateDocV1 =
                serde_json::from_slice(&fixture.record.value).unwrap();
            assert!(
                app.fetch_live_session_state_and_manifest(
                    fixture.topic.as_str(),
                    &state.session_id
                )
                .await
                .unwrap()
                .is_some()
            );
            state.session_id
        } else {
            let state: GameRoomStateDocV1 = serde_json::from_slice(&fixture.record.value).unwrap();
            assert!(
                app.fetch_game_room_state_and_manifest(fixture.topic.as_str(), &state.room_id)
                    .await
                    .unwrap()
                    .is_some()
            );
            state.room_id
        };
        assert_eq!(
            hydrate_subscription_event(
                &app.services,
                fixture.topic.as_str(),
                &fixture.replica,
                &fixture.record.key
            )
            .await
            .unwrap(),
            1
        );
        let live_before = store
            .get_live_session(fixture.topic.as_str(), &id)
            .await
            .unwrap();
        let game_before = store
            .get_game_room(fixture.topic.as_str(), &id)
            .await
            .unwrap();
        let mut state: serde_json::Value = serde_json::from_slice(&fixture.record.value).unwrap();
        state["current_manifest"]["hash"] =
            serde_json::to_value(kukuri_core::blob_hash(b"untrusted")).unwrap();
        fixture
            .docs
            .apply_doc_op(
                &fixture.replica,
                DocOp::SetJson {
                    key: fixture.record.key.clone(),
                    value: state,
                },
            )
            .await
            .unwrap();
        blobs.calls.lock().await.clear();
        assert_eq!(
            hydrate_subscription_event(
                &app.services,
                fixture.topic.as_str(),
                &fixture.replica,
                &fixture.record.key
            )
            .await
            .unwrap(),
            0
        );
        assert_eq!(
            store
                .get_live_session(fixture.topic.as_str(), &id)
                .await
                .unwrap(),
            live_before
        );
        assert_eq!(
            store
                .get_game_room(fixture.topic.as_str(), &id)
                .await
                .unwrap(),
            game_before
        );
        app.shutdown().await;
        let (restarted, _) = viewer(fixture.docs.clone(), blobs.clone());
        crate::service::catch_up_sessions(
            &restarted.services,
            fixture.topic.as_str(),
            &fixture.replica,
            DocFetchPolicy::LocalOnly,
        )
        .await
        .unwrap();
        assert_eq!(
            hydrate_subscription_hint(
                &restarted.services,
                fixture.topic.as_str(),
                &fixture.replica,
                &GossipHint::SessionChanged {
                    topic_id: fixture.topic.clone(),
                    session_id: id.clone(),
                    object_kind: if fixture.live {
                        "live-session"
                    } else {
                        "game-session"
                    }
                    .into(),
                }
            )
            .await
            .unwrap(),
            0
        );
        if fixture.live {
            assert!(
                restarted
                    .fetch_live_session_state_and_manifest(fixture.topic.as_str(), &id)
                    .await
                    .unwrap()
                    .is_none()
            );
        } else {
            assert!(
                restarted
                    .fetch_game_room_state_and_manifest(fixture.topic.as_str(), &id)
                    .await
                    .unwrap()
                    .is_none()
            );
        }
        assert!(blobs.calls.lock().await.is_empty());
        if kind == "dome" {
            assert!(
                restarted
                    .hosting_instance(
                        &fixture.replica,
                        &kukuri_core::SpatialContextV1::Topic {
                            topic_id: fixture.topic.clone(),
                        },
                        &id
                    )
                    .await
                    .unwrap()
                    .is_none()
            );
            assert!(blobs.calls.lock().await.is_empty());
        }
        restarted.shutdown().await;
        fixture.finish().await;
    }
}

#[tokio::test]
async fn signed_content_bytes_are_used_without_reserializing_the_manifest() {
    for kind in ["live", "score", "dome"] {
        let fixture = Fixture::new(kind).await;
        let bytes = fixture
            .source
            .fetch_blob(&fixture.hash())
            .await
            .unwrap()
            .unwrap();
        // Value uses a different field order from the manifest struct. Unknown fields also
        // must remain in the signed bytes used for the hash, even though typed parsing ignores them.
        let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        value["future_field"] = serde_json::json!("retained in signed content");
        let mut state: serde_json::Value = serde_json::from_slice(&fixture.record.value).unwrap();
        let id = state[if fixture.live {
            "session_id"
        } else {
            "room_id"
        }]
        .as_str()
        .unwrap();
        let envelope = if fixture.live {
            kukuri_core::build_live_session_envelope(
                fixture.owner.services.keys.as_ref(),
                &fixture.topic,
                id,
                &value,
            )
            .unwrap()
        } else {
            kukuri_core::build_game_session_envelope(
                fixture.owner.services.keys.as_ref(),
                &fixture.topic,
                id,
                &value,
            )
            .unwrap()
        };
        let blobs = ObservedBlobs::default();
        let stored = blobs
            .put_blob(envelope.content.as_bytes().to_vec(), "application/json")
            .await
            .unwrap();
        state["current_manifest"]["hash"] = serde_json::to_value(&stored.hash).unwrap();
        state["last_envelope_id"] = serde_json::to_value(&envelope.id).unwrap();
        persist_session_envelope(fixture.docs.as_ref(), &fixture.replica, &envelope)
            .await
            .unwrap();
        let mut record = fixture.record.clone();
        record.value = serde_json::to_vec(&state).unwrap();
        assert!(fixture.verify(&blobs, &record).await);
        assert_eq!(*blobs.calls.lock().await, vec![stored.hash]);
        fixture.finish().await;
    }
}

#[tokio::test]
async fn object_reads_and_fetches_do_not_grow_with_unrelated_records() {
    for kind in ["live", "score", "dome"] {
        let fixture = Fixture::new(kind).await;
        let docs = Arc::new(CountingDocsSync::default());
        docs.open_replica(&fixture.replica).await.unwrap();
        for record in fixture
            .docs
            .query_replica(&fixture.replica, DocQuery::Prefix(String::new()))
            .await
            .unwrap()
        {
            docs.apply_doc_op(
                &fixture.replica,
                DocOp::SetJson {
                    key: record.key,
                    value: serde_json::from_slice(&record.value).unwrap(),
                },
            )
            .await
            .unwrap();
        }
        let blobs = Arc::new(ObservedBlobs {
            inner: fixture.source.as_ref().clone(),
            ..Default::default()
        });
        let (app, _) = viewer(docs.clone(), blobs.clone());
        let state: serde_json::Value = serde_json::from_slice(&fixture.record.value).unwrap();
        let id = state[if fixture.live {
            "session_id"
        } else {
            "room_id"
        }]
        .as_str()
        .unwrap();
        let mut baseline = None;
        for count in [0, 1000] {
            for index in 0..count {
                docs.apply_doc_op(
                    &fixture.replica,
                    DocOp::SetJson {
                        key: format!("sessions/game/unrelated-{index}/state"),
                        value: serde_json::json!({}),
                    },
                )
                .await
                .unwrap();
            }
            docs.reset_records_returned();
            docs.clear_queries().await;
            blobs.calls.lock().await.clear();
            if fixture.live {
                assert!(
                    app.fetch_live_session_state_and_manifest(fixture.topic.as_str(), id)
                        .await
                        .unwrap()
                        .is_some()
                );
            } else {
                assert!(
                    app.fetch_game_room_state_and_manifest(fixture.topic.as_str(), id)
                        .await
                        .unwrap()
                        .is_some()
                );
            }
            let observed = (docs.records_returned(), blobs.calls.lock().await.len());
            assert_eq!(observed, *baseline.get_or_insert(observed));
            assert_eq!(observed, (2, 1));
            assert!(
                docs.queries()
                    .await
                    .iter()
                    .all(|(_, query)| matches!(query, DocQuery::Exact(_)))
            );
        }
        app.shutdown().await;
        fixture.finish().await;
    }
}
