use super::sync::{CountingDocsSync, ShadowingDocsSync};
use super::*;
use kukuri_core::{BlobHash, DomePresetRefV1, SpatialContextV1};

#[derive(Default)]
struct DelayedPresetBlob {
    inner: MemoryBlobService,
    held_hash: TokioMutex<Option<BlobHash>>,
    failing_hash: TokioMutex<Option<BlobHash>>,
}

#[async_trait]
impl BlobService for DelayedPresetBlob {
    async fn fetch_local_blob(&self, hash: &kukuri_core::BlobHash) -> Result<Option<Vec<u8>>> {
        self.fetch_blob(hash).await
    }
    async fn put_blob(&self, bytes: Vec<u8>, mime: &str) -> Result<StoredBlob> {
        self.inner.put_blob(bytes, mime).await
    }
    async fn fetch_blob(&self, hash: &BlobHash) -> Result<Option<Vec<u8>>> {
        if self.failing_hash.lock().await.as_ref() == Some(hash) {
            anyhow::bail!("simulated blob read failure");
        }
        if self.held_hash.lock().await.as_ref() == Some(hash) {
            return Ok(None);
        }
        self.inner.fetch_blob(hash).await
    }
    async fn pin_blob(&self, hash: &BlobHash) -> Result<()> {
        self.inner.pin_blob(hash).await
    }
    async fn blob_status(&self, hash: &BlobHash) -> Result<BlobStatus> {
        self.inner.blob_status(hash).await
    }
    async fn local_blob_status(&self, hash: &BlobHash) -> Result<BlobStatus> {
        self.inner.local_blob_status(hash).await
    }
    async fn import_peer_ticket(&self, ticket: &str) -> Result<()> {
        self.inner.import_peer_ticket(ticket).await
    }
}

const TOPIC: &str = "kukuri:topic:pending-dome-preset";

#[tokio::test]
async fn owner_can_manage_and_delete_without_preset_bytes() {
    let f = fixture().await;
    *f.blobs.held_hash.lock().await = Some(BlobHash::new(f.preset.manifest_blob_hash.clone()));
    let rooms = f.app.list_game_rooms(TOPIC).await.unwrap();
    assert!(
        rooms.iter().any(|r| r.room_id == f.dome_id),
        "owner management must not depend on Preset delivery"
    );
    f.app
        .delete_dome(crate::DeleteDomeInput {
            spatial_context: kukuri_core::SpatialContextV1::Topic {
                topic_id: TopicId::new(TOPIC),
            },
            instance_id: f.dome_id.clone(),
            expected_generation: 1,
            operation_id: "missing-preset".into(),
        })
        .await
        .unwrap();
    assert!(
        f.app
            .list_game_rooms(TOPIC)
            .await
            .unwrap()
            .iter()
            .all(|r| r.room_id != f.dome_id)
    );
}

#[tokio::test]
async fn unreadable_unrelated_instance_does_not_break_game_room_listing() {
    let f = fixture().await;
    f.docs
        .apply_doc_op(
            &topic_replica_id(TOPIC),
            DocOp::SetBytes {
                key: stable_key(
                    "metaverse/dome-instances",
                    &format!("{}/state", "0".repeat(64)),
                ),
                value: b"not-json".to_vec(),
            },
        )
        .await
        .expect("write unrelated unreadable Instance");

    let rooms = f
        .app
        .list_game_rooms(TOPIC)
        .await
        .expect("an unrelated invalid Instance must be skipped");
    assert!(rooms.iter().any(|room| room.room_id == f.dome_id));
    assert!(rooms.iter().any(|room| room.room_id == f.game_id));
}

#[tokio::test]
async fn invalid_first_record_for_owner_slot_does_not_shadow_valid_instance() {
    let f = fixture().await;
    let key = instance_state_key(&f);
    f.docs
        .shadow(&key, serde_json::json!("not an Instance state"))
        .await;

    let rooms = f
        .app
        .list_game_rooms(TOPIC)
        .await
        .expect("a valid bounded candidate must remain readable");
    assert!(rooms.iter().any(|room| room.room_id == f.dome_id));
}

#[tokio::test]
async fn invalid_first_envelope_record_does_not_shadow_valid_instance() {
    let f = fixture().await;
    let state_key = instance_state_key(&f);
    let state_record = f
        .docs
        .query_replica(&owner_replica(&f), DocQuery::Exact(state_key))
        .await
        .expect("read Instance state")
        .pop()
        .expect("Instance state");
    let state: DomeInstanceStateDocV1 =
        serde_json::from_slice(&state_record.value).expect("decode Instance state");
    f.docs
        .shadow(
            &stable_key("envelopes", state.last_envelope_id.as_str()),
            serde_json::json!("not a signed envelope"),
        )
        .await;

    let rooms = f
        .app
        .list_game_rooms(TOPIC)
        .await
        .expect("a valid signed candidate must remain readable");
    assert!(rooms.iter().any(|room| room.room_id == f.dome_id));
}

#[tokio::test]
async fn instance_blob_io_failure_is_returned() {
    let f = fixture().await;
    let state_key = instance_state_key(&f);
    let state_record = f
        .docs
        .query_replica(&owner_replica(&f), DocQuery::Exact(state_key))
        .await
        .expect("read Instance state")
        .pop()
        .expect("Instance state");
    let state: DomeInstanceStateDocV1 =
        serde_json::from_slice(&state_record.value).expect("decode Instance state");
    *f.blobs.failing_hash.lock().await = Some(state.current_manifest.hash);

    let error = f
        .app
        .list_game_rooms(TOPIC)
        .await
        .expect_err("blob I/O failure must remain an error");
    assert!(error.to_string().contains("simulated blob read failure"));
}

#[tokio::test]
async fn instance_docs_io_failure_is_returned() {
    let f = fixture().await;
    let state_key = instance_state_key(&f);
    f.docs.fail_on_key(&state_key).await;

    let error = f
        .app
        .list_game_rooms(TOPIC)
        .await
        .expect_err("docs I/O failure must remain an error");
    assert!(error.to_string().contains("simulated docs read failure"));
}

#[tokio::test]
async fn instance_lookup_reads_a_constant_number_of_docs_records() {
    let store = Arc::new(MemoryStore::default());
    let docs = Arc::new(CountingDocsSync::default());
    let blobs = Arc::new(MemoryBlobService::default());
    let transport = Arc::new(FakeTransport::new(
        "bounded-instance",
        FakeNetwork::default(),
    ));
    let app = app_service_from_dependencies(
        store.clone(),
        store,
        transport.clone(),
        transport,
        docs.clone(),
        blobs,
        generate_keys(),
    );
    let instance_id = app
        .create_metaverse_room(
            TOPIC,
            CreateMetaverseRoomInput {
                title: "bounded Dome".into(),
                description: String::new(),
                max_peers: Some(4),
            },
        )
        .await
        .expect("create Dome");
    // #1221 R5-H: Instance は owner の制御領域の、instance の id の key に置く。
    let replica = author_replica_id(app.keys().public_key().as_str());
    let owner_key = stable_key("metaverse/dome-instances", &format!("{instance_id}/state"));
    let state_record = docs
        .query_replica(&replica, DocQuery::Exact(owner_key))
        .await
        .expect("read Instance state")
        .pop()
        .expect("Instance state");
    let mut state: serde_json::Value =
        serde_json::from_slice(&state_record.value).expect("decode Instance state");

    // このtestが数えるのは同期的なInstance lookupだけ。createで起動した購読taskを止め、
    // 並行するsession catch-upの読み出しを同じcounterへ混ぜない。
    app.shutdown().await;

    docs.reset_records_returned();
    docs.clear_queries().await;
    let context = SpatialContextV1::Topic {
        topic_id: TopicId::new(TOPIC),
    };
    app.hosting_instance(&replica, &context, &instance_id)
        .await
        .expect("baseline lookup")
        .expect("baseline Instance");
    let baseline_records = docs.records_returned();

    for index in 0..24 {
        state["instance_id"] = serde_json::json!(format!("unrelated-{index}"));
        docs.apply_doc_op(
            &replica,
            DocOp::SetJson {
                key: stable_key(
                    "metaverse/dome-instances",
                    &format!("unrelated-{index}/state"),
                ),
                value: state.clone(),
            },
        )
        .await
        .expect("write unrelated Instance state");
    }

    docs.reset_records_returned();
    docs.clear_queries().await;
    app.hosting_instance(&replica, &context, &instance_id)
        .await
        .expect("bounded lookup")
        .expect("bounded Instance");
    assert_eq!(docs.records_returned(), baseline_records);
    assert!(docs.queries().await.into_iter().all(|(_, query)| {
        query != DocQuery::Prefix(stable_key("metaverse/dome-instances", ""))
    }));
}

#[tokio::test]
async fn owner_can_delete_without_derived_game_manifest() {
    let f = fixture().await;
    let row = f
        .store
        .get_game_room(TOPIC, f.dome_id.as_str())
        .await
        .unwrap()
        .unwrap();
    *f.blobs.held_hash.lock().await = Some(row.manifest_blob_hash);
    let mut handles = f.app.services.clone();
    let fresh_store = Arc::new(MemoryStore::default());
    handles.store = fresh_store.clone();
    handles.projection_store = fresh_store;
    let reader = AppService::from_handles(handles);
    let rooms = reader.list_game_rooms(TOPIC).await.unwrap();
    assert!(
        rooms.iter().any(|r| r.room_id == f.dome_id),
        "canonical owner metadata survives a missing derived game blob/cache"
    );

    f.app
        .delete_dome(crate::DeleteDomeInput {
            spatial_context: kukuri_core::SpatialContextV1::Topic {
                topic_id: TopicId::new(TOPIC),
            },
            instance_id: f.dome_id,
            expected_generation: 1,
            operation_id: "missing-projection".into(),
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn missing_other_instance_keeps_owner_management_topology_available() {
    let f = fixture().await;
    let instance = f
        .app
        .fetch_dome_instance_manifest(
            &kukuri_core::SpatialContextV1::Topic {
                topic_id: TopicId::new(TOPIC),
            },
            &f.app.keys().public_key(),
        )
        .await
        .unwrap()
        .unwrap();
    let mut other_handles = f.app.services.clone();
    other_handles.keys = Arc::new(generate_keys());
    let other = AppService::from_handles(other_handles);
    let owned = other
        .create_metaverse_room(
            TOPIC,
            CreateMetaverseRoomInput {
                title: "Available owner".into(),
                description: String::new(),
                max_peers: Some(8),
            },
        )
        .await
        .unwrap();
    *f.blobs.held_hash.lock().await = Some(instance.0.current_manifest.hash);
    let context = kukuri_core::SpatialContextV1::Topic {
        topic_id: TopicId::new(TOPIC),
    };
    let topology = other
        .list_dome_connection_topology(context)
        .await
        .expect("missing remote manifest must not block owner management");
    assert!(
        topology
            .resolution
            .topology
            .components
            .iter()
            .any(|component| component.instance_ids.contains(&owned))
    );
}

// #1221 R5-H(2026-09-27 ユーザー決定): 訪問者は、owner の端末の hosting の heartbeat から owner と context を知り、
// owner の制御領域(provider)の Instance と hosting の記録を exact に読んで一覧に出す。Dome の session は読まない。
// heartbeat が無ければ(owner が offline で hint も無い)一覧に出ない。
#[tokio::test]
async fn a_hosted_dome_is_listed_from_the_heartbeat_and_the_owner_control_area() {
    let owner_docs = Arc::new(CountingDocsSync::default());
    let blobs = Arc::new(MemoryBlobService::default());
    let app = |docs: Arc<CountingDocsSync>| {
        let store = Arc::new(MemoryStore::default());
        let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
        app_service_from_dependencies(
            store.clone(),
            store,
            transport,
            Arc::new(NoopHintTransport),
            docs,
            blobs.clone(),
            generate_keys(),
        )
    };
    let owner = app(owner_docs.clone());
    let visitor = app(Arc::new(CountingDocsSync::reading_from(owner_docs)));
    let context = SpatialContextV1::Topic {
        topic_id: TopicId::new(TOPIC),
    };
    let dome_id = owner
        .create_metaverse_room(
            TOPIC,
            CreateMetaverseRoomInput {
                title: "hosted Dome".into(),
                description: String::new(),
                max_peers: Some(4),
            },
        )
        .await
        .expect("create Dome");
    owner
        .start_owner_dome_hosting(crate::StartOwnerDomeHostingInput {
            expected_generation: None,
            spatial_context: context,
            instance_id: dome_id.clone(),
            endpoint_id: "owner-endpoint".into(),
            lease_duration_millis: 60_000,
        })
        .await
        .expect("start owner hosting");
    assert!(
        visitor
            .list_game_rooms(TOPIC)
            .await
            .unwrap()
            .iter()
            .all(|room| room.room_id != dome_id),
        "without a hint the Dome is not listed"
    );

    // topic の hint で受け取る heartbeat(scope の task が台帳へ置く)。
    let heartbeat = owner
        .dome_host_sessions
        .lock()
        .await
        .get(&dome_id)
        .expect("owner session")
        .signed_heartbeat(Utc::now().timestamp_millis())
        .expect("signed heartbeat");
    visitor
        .dome_host_heartbeats
        .lock()
        .await
        .insert(dome_id.clone(), heartbeat);
    let rooms = visitor.list_game_rooms(TOPIC).await.unwrap();
    let room = rooms
        .iter()
        .find(|room| room.room_id == dome_id)
        .expect("the hosted Dome is listed");
    assert_eq!(room.host_pubkey, owner.current_author_pubkey());
    assert_eq!(
        room.dome_hosting.as_ref().map(|state| state.kind),
        Some(kukuri_core::DomeHostingStateKindV1::OwnerHosted)
    );
    owner.shutdown().await;
}

/// #1221 R5-H: Instance は owner の制御領域の、instance の id の key に置く。
fn instance_state_key(f: &Fixture) -> String {
    stable_key("metaverse/dome-instances", &format!("{}/state", f.dome_id))
}

fn owner_replica(f: &Fixture) -> ReplicaId {
    author_replica_id(f.app.keys().public_key().as_str())
}

struct Fixture {
    app: AppService,
    docs: Arc<ShadowingDocsSync>,
    blobs: Arc<DelayedPresetBlob>,
    store: Arc<MemoryStore>,
    dome_id: String,
    game_id: String,
    preset: DomePresetRefV1,
}

async fn fixture() -> Fixture {
    let store = Arc::new(MemoryStore::default());
    let docs = Arc::new(ShadowingDocsSync::default());
    let blobs = Arc::new(DelayedPresetBlob::default());
    let transport = Arc::new(FakeTransport::new("listing", FakeNetwork::default()));
    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        transport.clone(),
        transport,
        docs.clone(),
        blobs.clone(),
        generate_keys(),
    );
    let dome_id = app
        .create_metaverse_room(
            TOPIC,
            CreateMetaverseRoomInput {
                title: "Dome awaiting preset".into(),
                description: String::new(),
                max_peers: Some(4),
            },
        )
        .await
        .expect("create Dome");
    let game_id = app
        .create_game_room(
            TOPIC,
            CreateGameRoomInput {
                title: "available game".into(),
                description: String::new(),
                participants: vec!["Alice".into(), "Bob".into()],
            },
        )
        .await
        .expect("create game");
    let preset = app
        .list_game_rooms(TOPIC)
        .await
        .expect("initial rooms")
        .into_iter()
        .find(|room| room.room_id == dome_id)
        .expect("Dome row")
        .metaverse
        .expect("Dome state")
        .preset_ref;
    Fixture {
        app,
        docs,
        blobs,
        store,
        dome_id,
        game_id,
        preset,
    }
}

fn preset_key(preset: &DomePresetRefV1) -> String {
    format!(
        "metaverse/dome-presets/{}/revisions/{:020}",
        preset.preset_id, preset.revision
    )
}

async fn assert_pending_then_available(f: &Fixture) {
    let rooms = f
        .app
        .list_game_rooms(TOPIC)
        .await
        .expect("pending preset must not fail room listing");
    assert!(rooms.iter().any(|r| r.room_id == f.game_id));
    let owned = rooms
        .iter()
        .find(|r| r.room_id == f.dome_id)
        .expect("owner management remains available");
    assert_eq!(owned.phase_label.as_deref(), Some("management_only"));
    // Pending is a read result, not removal of the canonical/projection record.
    assert_eq!(
        f.store
            .list_channel_game_rooms(TOPIC, "public", 100)
            .await
            .expect("stored rooms")
            .len(),
        2
    );
}

#[tokio::test]
async fn missing_preset_state_keeps_other_rooms_and_recovers_after_delivery() {
    let f = fixture().await;
    let replica = author_replica_id(f.preset.owner_pubkey.as_str());
    let key = preset_key(&f.preset);
    let record = f
        .docs
        .query_replica(&replica, DocQuery::Exact(key.clone()))
        .await
        .expect("preset state")
        .pop()
        .expect("state record");
    f.docs
        .apply_doc_op(
            &replica,
            DocOp::DeletePrefix {
                prefix: key.clone(),
            },
        )
        .await
        .expect("delay state");
    assert_pending_then_available(&f).await;
    f.docs
        .apply_doc_op(
            &replica,
            DocOp::SetBytes {
                key,
                value: record.value,
            },
        )
        .await
        .expect("deliver state");
    let rooms = f.app.list_game_rooms(TOPIC).await.expect("delivered rooms");
    assert_eq!(rooms.len(), 2);
    assert!(
        rooms
            .iter()
            .any(|room| room.room_id == f.dome_id && room.dome_hosting.is_some())
    );
}

#[tokio::test]
async fn missing_preset_blob_keeps_other_rooms_and_recovers_after_delivery() {
    let f = fixture().await;
    *f.blobs.held_hash.lock().await = Some(BlobHash::new(f.preset.manifest_blob_hash.clone()));
    assert_pending_then_available(&f).await;
    *f.blobs.held_hash.lock().await = None;
    let rooms = f.app.list_game_rooms(TOPIC).await.expect("delivered rooms");
    assert_eq!(rooms.len(), 2);
    assert!(
        rooms
            .iter()
            .any(|room| room.room_id == f.dome_id && room.dome_hosting.is_some())
    );
}

#[tokio::test]
async fn mismatched_preset_reference_is_an_error_not_pending() {
    let f = fixture().await;
    let replica = author_replica_id(f.preset.owner_pubkey.as_str());
    let key = preset_key(&f.preset);
    let record = f
        .docs
        .query_replica(&replica, DocQuery::Exact(key.clone()))
        .await
        .expect("preset state")
        .pop()
        .expect("state record");
    let mut state: serde_json::Value = serde_json::from_slice(&record.value).expect("decode");
    state["revision"] = serde_json::json!(f.preset.revision + 1);
    f.docs
        .apply_doc_op(&replica, DocOp::SetJson { key, value: state })
        .await
        .expect("mismatch state");
    let error = f
        .app
        .list_game_rooms(TOPIC)
        .await
        .expect_err("invalid reference must fail");
    assert!(error.to_string().contains("does not match its reference"));
}

#[tokio::test]
async fn invalid_preset_signature_is_an_error_not_pending() {
    let f = fixture().await;
    let replica = author_replica_id(f.preset.owner_pubkey.as_str());
    let state_record = f
        .docs
        .query_replica(&replica, DocQuery::Exact(preset_key(&f.preset)))
        .await
        .expect("preset state")
        .pop()
        .expect("state record");
    let state: DomePresetStateDocV1 =
        serde_json::from_slice(&state_record.value).expect("decode state");
    let key = format!("envelopes/{}", state.last_envelope_id.as_str());
    let record = f
        .docs
        .query_replica(&replica, DocQuery::Exact(key.clone()))
        .await
        .expect("envelope")
        .pop()
        .expect("signed envelope");
    let mut envelope: KukuriEnvelope =
        serde_json::from_slice(&record.value).expect("decode envelope");
    envelope.content = "tampered preset".into();
    f.docs
        .apply_doc_op(
            &replica,
            DocOp::SetJson {
                key,
                value: serde_json::to_value(envelope).expect("encode"),
            },
        )
        .await
        .expect("tamper");
    f.app
        .list_game_rooms(TOPIC)
        .await
        .expect_err("invalid signature must fail, not be skipped");
}

#[tokio::test]
async fn missing_preset_envelope_keeps_other_rooms_and_recovers_after_delivery() {
    let f = fixture().await;
    let replica = author_replica_id(f.preset.owner_pubkey.as_str());
    let record = f
        .docs
        .query_replica(&replica, DocQuery::Exact(preset_key(&f.preset)))
        .await
        .expect("state")
        .pop()
        .expect("state record");
    let state: DomePresetStateDocV1 = serde_json::from_slice(&record.value).expect("decode state");
    let key = format!("envelopes/{}", state.last_envelope_id.as_str());
    let signed = f
        .docs
        .query_replica(&replica, DocQuery::Exact(key.clone()))
        .await
        .expect("signed envelope")
        .pop()
        .expect("envelope");
    f.docs
        .apply_doc_op(
            &replica,
            DocOp::DeletePrefix {
                prefix: key.clone(),
            },
        )
        .await
        .expect("delay envelope");
    assert_pending_then_available(&f).await;
    f.docs
        .apply_doc_op(
            &replica,
            DocOp::SetBytes {
                key,
                value: signed.value,
            },
        )
        .await
        .expect("deliver envelope");
    assert_eq!(
        f.app
            .list_game_rooms(TOPIC)
            .await
            .expect("complete rooms")
            .len(),
        2
    );
}

#[tokio::test]
async fn current_instance_preset_controls_readiness_even_with_an_old_cache_row() {
    let f = fixture().await;
    let old_row = f
        .store
        .get_game_room(TOPIC, f.dome_id.as_str())
        .await
        .expect("rows")
        .expect("Dome row");
    let mut customization = old_row
        .metaverse
        .as_ref()
        .expect("state")
        .dome
        .customization
        .clone();
    customization.environment.fog_density_micros += 1;
    f.app
        .update_metaverse_room(
            TOPIC,
            &f.dome_id,
            UpdateMetaverseRoomInput {
                status: old_row.status.clone(),
                customization,
            },
        )
        .await
        .expect("new revision");
    let current = f
        .app
        .list_game_rooms(TOPIC)
        .await
        .expect("new rooms")
        .into_iter()
        .find(|row| row.room_id == f.dome_id)
        .expect("new Dome")
        .metaverse
        .expect("new state");
    assert_eq!(current.preset_ref.revision, f.preset.revision + 1);
    f.store
        .upsert_game_room_cache(old_row)
        .await
        .expect("lagging derived cache");
    *f.blobs.held_hash.lock().await = Some(BlobHash::new(current.preset_ref.manifest_blob_hash));
    assert_pending_then_available(&f).await;
    *f.blobs.held_hash.lock().await = None;
    assert_eq!(
        f.app
            .list_game_rooms(TOPIC)
            .await
            .expect("complete rooms")
            .len(),
        2
    );
}
