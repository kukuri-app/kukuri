use super::*;
use kukuri_core::{
    DomeMovePhaseV1, MetaverseAssetKind, MetaverseRoomEventV1, SpatialContextV1, TopicId,
};

#[derive(Clone, Default)]
struct SelectivelyMissingBlobService {
    inner: MemoryBlobService,
    missing: Arc<TokioMutex<std::collections::HashSet<String>>>,
}

impl SelectivelyMissingBlobService {
    async fn mark_missing(&self, hash: &str) {
        self.missing.lock().await.insert(hash.to_string());
    }

    async fn mark_available(&self, hash: &str) {
        self.missing.lock().await.remove(hash);
    }
}

#[async_trait]
impl BlobService for SelectivelyMissingBlobService {
    async fn fetch_local_blob(&self, hash: &kukuri_core::BlobHash) -> Result<Option<Vec<u8>>> {
        self.fetch_blob(hash).await
    }
    async fn put_blob(&self, data: Vec<u8>, mime: &str) -> Result<StoredBlob> {
        self.inner.put_blob(data, mime).await
    }

    async fn fetch_blob(&self, hash: &kukuri_core::BlobHash) -> Result<Option<Vec<u8>>> {
        self.inner.fetch_blob(hash).await
    }

    async fn pin_blob(&self, hash: &kukuri_core::BlobHash) -> Result<()> {
        self.inner.pin_blob(hash).await
    }

    async fn blob_status(&self, hash: &kukuri_core::BlobHash) -> Result<BlobStatus> {
        if self.missing.lock().await.contains(hash.as_str()) {
            return Ok(BlobStatus::Missing);
        }
        self.inner.blob_status(hash).await
    }

    async fn local_blob_status(&self, hash: &kukuri_core::BlobHash) -> Result<BlobStatus> {
        if self.missing.lock().await.contains(hash.as_str()) {
            return Ok(BlobStatus::Missing);
        }
        self.inner.local_blob_status(hash).await
    }

    async fn import_peer_ticket(&self, ticket: &str) -> Result<()> {
        self.inner.import_peer_ticket(ticket).await
    }

    async fn learn_peer(&self, endpoint_id: &str) -> Result<()> {
        self.inner.learn_peer(endpoint_id).await
    }

    async fn set_seed_peers(&self, peers: Vec<SeedPeer>) -> Result<()> {
        self.inner.set_seed_peers(peers).await
    }

    async fn assist_peer_ids(&self) -> Result<Vec<String>> {
        self.inner.assist_peer_ids().await
    }
}

#[tokio::test]
async fn dome_is_unique_per_owner_context_and_moves_with_the_same_preset_blob() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(FakeTransport::new("self", FakeNetwork::default()));
    let docs_sync = Arc::new(MemoryDocsSync::default());
    let blob_service = Arc::new(MemoryBlobService::default());
    let keys = generate_keys();
    let app = app_service_from_dependencies(
        store.clone(),
        store,
        transport.clone(),
        transport.clone(),
        docs_sync.clone(),
        blob_service.clone(),
        keys.clone(),
    );
    let source_topic = "kukuri:topic:dome-move-source";
    let target_topic = "kukuri:topic:dome-move-target";
    let input = CreateMetaverseRoomInput {
        title: "movable Dome".into(),
        description: "owner asset".into(),
        max_peers: Some(8),
    };

    let source_instance_id = app
        .create_metaverse_room(source_topic, input.clone())
        .await
        .expect("create source Dome");
    let duplicate = app
        .create_metaverse_room(source_topic, input)
        .await
        .expect_err("duplicate owner Dome must fail");
    assert!(duplicate.to_string().contains("already has a Dome"));

    let first_asset = app
        .import_metaverse_room_asset(
            source_topic,
            ImportMetaverseRoomAssetInput {
                room_id: source_instance_id.clone(),
                kind: MetaverseAssetKind::Glb,
                mime_type: "model/gltf-binary".into(),
                name: Some("table.glb".into()),
                bytes: minimal_metaverse_glb_bytes(),
            },
        )
        .await
        .expect("import source asset");
    let duplicate_asset = app
        .import_metaverse_room_asset(
            source_topic,
            ImportMetaverseRoomAssetInput {
                room_id: source_instance_id.clone(),
                kind: MetaverseAssetKind::Glb,
                mime_type: "model/gltf-binary".into(),
                name: Some("table-copy.glb".into()),
                bytes: minimal_metaverse_glb_bytes(),
            },
        )
        .await
        .expect("reimport source asset");
    assert_eq!(first_asset.blob_hash, duplicate_asset.blob_hash);

    let source = app
        .list_game_rooms(source_topic)
        .await
        .expect("source list")
        .into_iter()
        .find(|room| room.room_id == source_instance_id)
        .expect("source Dome");
    let source_preset_hash = source
        .metaverse
        .as_ref()
        .expect("source state")
        .preset_ref
        .manifest_blob_hash
        .clone();

    let move_record = app
        .move_dome(
            source_topic,
            MoveDomeInput {
                move_id: "move-source-to-target".into(),
                source_instance_id: source_instance_id.clone(),
                target_context: SpatialContextV1::Topic {
                    topic_id: TopicId::new(target_topic),
                },
            },
        )
        .await
        .expect("move Dome");
    assert_eq!(move_record.phase, DomeMovePhaseV1::Completed);
    assert!(
        app.list_game_rooms(source_topic)
            .await
            .expect("source after move")
            .into_iter()
            .all(|room| room.room_id != source_instance_id)
    );
    let target = app
        .list_game_rooms(target_topic)
        .await
        .expect("target list")
        .into_iter()
        .find(|room| room.room_id == move_record.target_instance_id)
        .expect("target Dome");
    let target_state = target.metaverse.expect("target state");
    assert_eq!(
        target_state.preset_ref.manifest_blob_hash,
        source_preset_hash
    );
    assert_eq!(target_state.spatial_context, move_record.target_context);
    assert_eq!(target_state.asset_refs.len(), 1);
    assert_eq!(target_state.asset_refs[0].blob_hash, first_asset.blob_hash);

    let stale_event = app
        .publish_metaverse_room_event(
            source_topic,
            PublishMetaverseRoomEventInput {
                room_id: source_instance_id.clone(),
                peer_id: "peer-after-move".into(),
                seq: 1,
                event: MetaverseRoomEventV1::PresenceLeave {
                    room_id: source_instance_id.clone(),
                    peer_id: "peer-after-move".into(),
                    left_at: Utc::now().timestamp_millis(),
                },
            },
        )
        .await
        .expect_err("tombstoned source must reject events");
    assert!(stale_event.to_string().contains("non-active"));

    let retried = app
        .move_dome(
            source_topic,
            MoveDomeInput {
                move_id: "move-source-to-target".into(),
                source_instance_id: source_instance_id.clone(),
                target_context: SpatialContextV1::Topic {
                    topic_id: TopicId::new(target_topic),
                },
            },
        )
        .await
        .expect("completed move is idempotent");
    assert_eq!(retried.phase, DomeMovePhaseV1::Completed);

    let restarted_store = Arc::new(MemoryStore::default());
    let restarted = app_service_from_dependencies(
        restarted_store.clone(),
        restarted_store,
        transport.clone(),
        transport,
        docs_sync,
        blob_service,
        keys,
    );
    assert!(
        restarted
            .list_game_rooms(source_topic)
            .await
            .expect("source list after restart")
            .into_iter()
            .all(|room| room.room_id != source_instance_id)
    );
    assert!(
        restarted
            .list_game_rooms(target_topic)
            .await
            .expect("target list after restart")
            .into_iter()
            .any(|room| room.room_id == move_record.target_instance_id)
    );

    let recreated_source = restarted
        .create_metaverse_room(
            source_topic,
            CreateMetaverseRoomInput {
                title: "new source Dome".into(),
                description: "new generation".into(),
                max_peers: Some(4),
            },
        )
        .await
        .expect("reuse tombstoned owner slot with a new generation");
    assert_eq!(recreated_source, source_instance_id);
    let recreated = restarted
        .list_game_rooms(source_topic)
        .await
        .expect("recreated source list")
        .into_iter()
        .find(|room| room.room_id == recreated_source)
        .expect("recreated source Dome");
    assert_eq!(
        recreated
            .metaverse
            .expect("recreated state")
            .instance_generation,
        2
    );
}

#[tokio::test]
async fn dome_moves_from_a_public_topic_into_a_private_channel_context() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(FakeTransport::new("self", FakeNetwork::default()));
    let app = AppService::new(store, transport);
    let topic = "kukuri:topic:dome-private-target";
    let source_instance_id = app
        .create_metaverse_room(
            topic,
            CreateMetaverseRoomInput {
                title: "public Dome".into(),
                description: String::new(),
                max_peers: Some(8),
            },
        )
        .await
        .expect("create public source Dome");
    let channel = app
        .create_private_channel(CreatePrivateChannelInput {
            topic_id: TopicId::new(topic),
            label: "Dome destination".into(),
            audience_kind: ChannelAudienceKind::InviteOnly,
        })
        .await
        .expect("create destination channel");
    let target_context = SpatialContextV1::Channel {
        topic_id: TopicId::new(topic),
        channel_id: ChannelId::new(channel.channel_id.clone()),
    };

    let moved = app
        .move_dome(
            topic,
            MoveDomeInput {
                move_id: "move-public-private".into(),
                source_instance_id,
                target_context: target_context.clone(),
            },
        )
        .await
        .expect("move into private channel");
    assert_eq!(moved.phase, DomeMovePhaseV1::Completed);

    let rooms = app
        .list_game_rooms_scoped(
            topic,
            TimelineScope::Channel {
                channel_id: ChannelId::new(channel.channel_id.clone()),
            },
        )
        .await
        .expect("list private channel Domes");
    let target = rooms
        .into_iter()
        .find(|room| room.room_id == moved.target_instance_id)
        .expect("active target Dome");
    assert_eq!(
        target.metaverse.expect("target state").spatial_context,
        target_context
    );

    let configured = app
        .set_private_channel_entry_dome(
            topic,
            channel.channel_id.as_str(),
            Some(moved.target_instance_id.clone()),
        )
        .await
        .expect("set channel entry Dome");
    assert_eq!(
        configured.entry_dome_instance_id.as_deref(),
        Some(moved.target_instance_id.as_str())
    );
    app.rotate_private_channel(topic, channel.channel_id.as_str())
        .await
        .expect("rotate channel with entry Dome");
    let rotated = app
        .list_joined_private_channels(topic, None)
        .await
        .expect("list rotated channel")
        .items
        .into_iter()
        .find(|candidate| candidate.channel_id == channel.channel_id)
        .expect("rotated channel view");
    assert_eq!(
        rotated.entry_dome_instance_id.as_deref(),
        Some(moved.target_instance_id.as_str())
    );
    let cleared = app
        .set_private_channel_entry_dome(topic, channel.channel_id.as_str(), None)
        .await
        .expect("clear channel entry Dome");
    assert_eq!(cleared.entry_dome_instance_id, None);

    let invalid = app
        .set_private_channel_entry_dome(
            topic,
            channel.channel_id.as_str(),
            Some("dome-from-another-context".into()),
        )
        .await
        .expect_err("cross-context entry Dome must fail");
    assert!(invalid.to_string().contains("not in this Spatial Context"));
}

#[tokio::test]
async fn dome_move_asset_validation_failure_keeps_source_active_and_retries_safely() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let docs_sync = Arc::new(MemoryDocsSync::default());
    let blob_service = Arc::new(SelectivelyMissingBlobService::default());
    let app = app_service_from_dependencies(
        store.clone(),
        store,
        transport.clone(),
        transport,
        docs_sync,
        blob_service.clone(),
        generate_keys(),
    );
    let source_topic = "kukuri:topic:dome-move-failure-source";
    let target_topic = "kukuri:topic:dome-move-failure-target";
    let source_instance_id = app
        .create_metaverse_room(
            source_topic,
            CreateMetaverseRoomInput {
                title: "safe source Dome".into(),
                description: String::new(),
                max_peers: Some(8),
            },
        )
        .await
        .expect("create source Dome");
    let asset = app
        .import_metaverse_room_asset(
            source_topic,
            ImportMetaverseRoomAssetInput {
                room_id: source_instance_id.clone(),
                kind: MetaverseAssetKind::Texture,
                mime_type: "image/png".into(),
                name: Some("wall.png".into()),
                bytes: minimal_metaverse_png_bytes(),
            },
        )
        .await
        .expect("import asset");
    blob_service.mark_missing(asset.blob_hash.as_str()).await;
    let input = MoveDomeInput {
        move_id: "move-with-missing-asset".into(),
        source_instance_id: source_instance_id.clone(),
        target_context: SpatialContextV1::Topic {
            topic_id: TopicId::new(target_topic),
        },
    };

    let error = app
        .move_dome(source_topic, input.clone())
        .await
        .expect_err("missing target asset must stop before detach");
    assert!(error.to_string().contains("asset is unavailable"));
    assert!(
        app.list_game_rooms(source_topic)
            .await
            .expect("source rooms after failure")
            .iter()
            .any(|room| room.room_id == source_instance_id)
    );
    assert!(
        app.list_game_rooms(target_topic)
            .await
            .expect("target rooms after failure")
            .is_empty()
    );

    blob_service.mark_available(asset.blob_hash.as_str()).await;
    let retried = app
        .move_dome(source_topic, input)
        .await
        .expect("retry same operation after asset recovery");
    assert_eq!(retried.phase, DomeMovePhaseV1::Completed);
}
