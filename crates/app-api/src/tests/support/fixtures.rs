use super::super::*;

pub(crate) fn app_service_from_dependencies(
    store: Arc<dyn Store>,
    projection_store: Arc<dyn ProjectionStore>,
    transport: Arc<dyn Transport>,
    hint_transport: Arc<dyn HintTransport>,
    docs_sync: Arc<dyn DocsSync>,
    blob_service: Arc<dyn BlobService>,
    keys: KukuriKeys,
) -> AppService {
    AppService::from_handles(ServiceHandles::new(
        store,
        projection_store,
        transport,
        hint_transport,
        docs_sync,
        blob_service,
        keys,
    ))
}

pub(crate) async fn persist_test_post(
    docs_sync: &dyn DocsSync,
    projection_store: Option<&dyn ProjectionStore>,
    keys: &KukuriKeys,
    topic: &TopicId,
    payload_ref: PayloadRef,
    attachments: Vec<kukuri_core::AssetRef>,
    reply_to: Option<&KukuriEnvelope>,
) -> KukuriEnvelope {
    persist_test_post_with_labels(
        docs_sync,
        projection_store,
        keys,
        topic,
        payload_ref,
        attachments,
        reply_to,
        Vec::new(),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn persist_test_post_with_labels(
    docs_sync: &dyn DocsSync,
    projection_store: Option<&dyn ProjectionStore>,
    keys: &KukuriKeys,
    topic: &TopicId,
    payload_ref: PayloadRef,
    attachments: Vec<kukuri_core::AssetRef>,
    reply_to: Option<&KukuriEnvelope>,
    content_labels: Vec<String>,
) -> KukuriEnvelope {
    let envelope = build_post_envelope_with_payload_in_channel(
        keys,
        topic,
        payload_ref,
        attachments,
        Vec::new(),
        reply_to,
        ObjectVisibility::Public,
        None,
        content_labels,
    )
    .expect("event");
    let object = envelope
        .to_post_object()
        .expect("post object")
        .expect("post object");
    let replica = topic_replica_id(topic.as_str());
    persist_post_object(docs_sync, &replica, object.clone(), envelope.clone())
        .await
        .expect("persist post object");
    if let Some(projection_store) = projection_store {
        ObjectProjectionStore::put_object_projection(
            projection_store,
            verified_projection_row(&envelope, &replica, None),
        )
        .await
        .expect("put placeholder projection");
    }
    envelope
}

/// 署名つき envelope から、反映と同じ検証を通して projection の行を作る(#1248)。
pub(crate) fn verified_projection_row(
    envelope: &KukuriEnvelope,
    replica: &ReplicaId,
    content: Option<String>,
) -> ObjectProjectionRow {
    let post = VerifiedPost::verify_local(envelope.clone(), replica)
        .expect("the test envelope must pass the post verification");
    projection_row_from_post(&post, content)
}
pub(crate) fn pending_image_attachment(mime: &str, bytes: &[u8]) -> PendingAttachment {
    PendingAttachment {
        mime: mime.to_string(),
        bytes: bytes.to_vec(),
        role: AssetRole::ImageOriginal,
    }
}

pub(crate) fn pending_video_attachment(
    role: AssetRole,
    mime: &str,
    bytes: &[u8],
) -> PendingAttachment {
    PendingAttachment {
        mime: mime.to_string(),
        bytes: bytes.to_vec(),
        role,
    }
}

pub(crate) fn tiny_png_bytes() -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO7ZPioAAAAASUVORK5CYII=")
        .expect("decode png")
}

pub(crate) fn reaction_snapshot_from_view(
    asset: &CustomReactionAssetView,
) -> CustomReactionAssetSnapshotV1 {
    CustomReactionAssetSnapshotV1 {
        asset_id: asset.asset_id.clone(),
        owner_pubkey: Pubkey::from(asset.owner_pubkey.as_str()),
        blob_hash: kukuri_core::BlobHash::new(asset.blob_hash.clone()),
        search_key: asset.search_key.clone(),
        mime: asset.mime.clone(),
        bytes: asset.bytes,
        width: asset.width,
        height: asset.height,
    }
}

pub(crate) fn local_app_with_memory_services() -> (
    AppService,
    Arc<MemoryStore>,
    Arc<MemoryDocsSync>,
    Arc<MemoryBlobService>,
) {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let docs_sync = Arc::new(MemoryDocsSync::default());
    let blob_service = Arc::new(MemoryBlobService::default());
    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        transport,
        Arc::new(NoopHintTransport),
        docs_sync.clone(),
        blob_service.clone(),
        generate_keys(),
    );
    (app, store, docs_sync, blob_service)
}

pub(crate) fn shared_apps_with_memory_services() -> (
    AppService,
    KukuriKeys,
    AppService,
    KukuriKeys,
    Arc<MemoryStore>,
    Arc<MemoryDocsSync>,
    Arc<MemoryBlobService>,
) {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let docs_sync = Arc::new(MemoryDocsSync::default());
    let blob_service = Arc::new(MemoryBlobService::default());
    let local_keys = generate_keys();
    let remote_keys = generate_keys();
    let local_app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        transport.clone(),
        Arc::new(NoopHintTransport),
        docs_sync.clone(),
        blob_service.clone(),
        local_keys.clone(),
    );
    let remote_app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        transport,
        Arc::new(NoopHintTransport),
        docs_sync.clone(),
        blob_service.clone(),
        remote_keys.clone(),
    );
    (
        local_app,
        local_keys,
        remote_app,
        remote_keys,
        store,
        docs_sync,
        blob_service,
    )
}

pub(crate) async fn author_profile_post_docs(
    docs_sync: &dyn DocsSync,
    author_pubkey: &str,
) -> Vec<AuthorProfilePostDocV1> {
    docs_sync
        .query_replica(
            &author_replica_id(author_pubkey),
            DocQuery::Prefix("profile/posts/".into()),
        )
        .await
        .expect("profile post docs")
        .into_iter()
        .map(|record| {
            serde_json::from_slice::<AuthorProfilePostDocV1>(record.value.as_slice())
                .expect("decode profile post doc")
        })
        .collect()
}

pub(crate) async fn author_profile_repost_docs(
    docs_sync: &dyn DocsSync,
    author_pubkey: &str,
) -> Vec<AuthorProfileRepostDocV1> {
    docs_sync
        .query_replica(
            &author_replica_id(author_pubkey),
            DocQuery::Prefix("profile/reposts/".into()),
        )
        .await
        .expect("profile repost docs")
        .into_iter()
        .map(|record| {
            serde_json::from_slice::<AuthorProfileRepostDocV1>(record.value.as_slice())
                .expect("decode profile repost doc")
        })
        .collect()
}

pub(crate) async fn author_profile_doc(
    docs_sync: &dyn DocsSync,
    author_pubkey: &str,
) -> Option<AuthorProfileDocV1> {
    docs_sync
        .query_replica(
            &author_replica_id(author_pubkey),
            DocQuery::Exact(stable_key("profile", "latest")),
        )
        .await
        .expect("profile doc")
        .into_iter()
        .next()
        .map(|record| {
            serde_json::from_slice::<AuthorProfileDocV1>(record.value.as_slice())
                .expect("decode profile doc")
        })
}

pub(crate) async fn remote_doc_event(
    docs_sync: &dyn DocsSync,
    replica_id: &ReplicaId,
    key: String,
) -> DocEvent {
    let record = docs_sync
        .query_replica(replica_id, DocQuery::Exact(key.clone()))
        .await
        .expect("doc record")
        .into_iter()
        .next()
        .expect("doc record exists");
    DocEvent {
        replica_id: replica_id.clone(),
        key,
        content_hash: record.content_hash,
        source_peer: Some("remote-peer".into()),
    }
}

pub(crate) async fn create_remote_object_notification(
    app: &AppService,
    projection_store: &dyn ProjectionStore,
    docs_sync: &dyn DocsSync,
    blob_service: &dyn BlobService,
    event: DocEvent,
) -> bool {
    create_remote_object_notification_with_baseline(
        app,
        projection_store,
        docs_sync,
        blob_service,
        &NotificationDocEventBaseline::default(),
        event,
    )
    .await
}

pub(crate) async fn create_remote_object_notification_with_baseline(
    app: &AppService,
    projection_store: &dyn ProjectionStore,
    docs_sync: &dyn DocsSync,
    blob_service: &dyn BlobService,
    baseline: &NotificationDocEventBaseline,
    event: DocEvent,
) -> bool {
    // public topic の replica は、replica id から購読の topic が決まる。
    let topic_id = match kukuri_docs_sync::post_replica_kind(&event.replica_id) {
        Some(kukuri_docs_sync::PostReplicaKind::PublicTopic { topic_id }) => topic_id,
        other => panic!("the notification fixture expects a public topic replica: {other:?}"),
    };
    AppService::maybe_create_notification_for_remote_object_event(
        projection_store,
        docs_sync,
        blob_service,
        app.current_author_pubkey().as_str(),
        topic_id.as_str(),
        baseline,
        &event,
    )
    .await
    .expect("create remote object notification")
}

pub(crate) async fn create_remote_follow_notification(
    app: &AppService,
    store: &dyn Store,
    projection_store: &dyn ProjectionStore,
    docs_sync: &dyn DocsSync,
    author_pubkey: &str,
    event: DocEvent,
) -> bool {
    create_remote_follow_notification_with_baseline(
        app,
        store,
        projection_store,
        docs_sync,
        author_pubkey,
        &NotificationDocEventBaseline::default(),
        event,
    )
    .await
}

pub(crate) async fn create_remote_follow_notification_with_baseline(
    app: &AppService,
    store: &dyn Store,
    projection_store: &dyn ProjectionStore,
    docs_sync: &dyn DocsSync,
    author_pubkey: &str,
    baseline: &NotificationDocEventBaseline,
    event: DocEvent,
) -> bool {
    AppService::maybe_create_notification_for_remote_follow_event(
        store,
        projection_store,
        docs_sync,
        app.current_author_pubkey().as_str(),
        author_pubkey,
        baseline,
        &event,
    )
    .await
    .expect("create remote follow notification")
}
