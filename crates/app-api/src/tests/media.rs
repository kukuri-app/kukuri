use super::*;

#[tokio::test]
async fn create_post_with_image_attachment_surfaces_attachment_metadata() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(FakeTransport::new("app", FakeNetwork::default()));
    let app = AppService::new(store, transport);

    let object_id = app
        .create_post_with_attachments(
            "kukuri:topic:image-write",
            "caption",
            None,
            vec![PendingAttachment {
                mime: "image/png".into(),
                bytes: b"fake-image".to_vec(),
                role: AssetRole::ImageOriginal,
            }],
        )
        .await
        .expect("create image post");
    let timeline = app
        .list_timeline("kukuri:topic:image-write", None, 10)
        .await
        .expect("timeline");

    let post = timeline
        .items
        .iter()
        .find(|post| post.object_id == object_id)
        .expect("image post");
    assert_eq!(post.content, "caption");
    assert_eq!(post.attachments.len(), 1);
    assert_eq!(post.attachments[0].mime, "image/png");
    assert_eq!(post.attachments[0].role, "image_original");
    assert_eq!(post.attachments[0].status, BlobViewStatus::Available);
}

#[tokio::test]
async fn create_post_with_image_only_succeeds() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(FakeTransport::new("app", FakeNetwork::default()));
    let app = AppService::new(store, transport);

    let object_id = app
        .create_post_with_attachments(
            "kukuri:topic:image-only",
            "",
            None,
            vec![PendingAttachment {
                mime: "image/jpeg".into(),
                bytes: b"fake-jpeg".to_vec(),
                role: AssetRole::ImageOriginal,
            }],
        )
        .await
        .expect("create image-only post");
    let timeline = app
        .list_timeline("kukuri:topic:image-only", None, 10)
        .await
        .expect("timeline");

    let post = timeline
        .items
        .iter()
        .find(|post| post.object_id == object_id)
        .expect("image-only post");
    assert_eq!(post.attachments.len(), 1);
    assert_eq!(post.attachments[0].mime, "image/jpeg");
}

#[tokio::test]
async fn create_post_with_video_attachments_surfaces_video_metadata() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(FakeTransport::new("app", FakeNetwork::default()));
    let app = AppService::new(store, transport);

    let object_id = app
        .create_post_with_attachments(
            "kukuri:topic:video-write",
            "video caption",
            None,
            vec![
                pending_video_attachment(
                    AssetRole::VideoManifest,
                    "video/mp4",
                    b"fake-video-manifest",
                ),
                pending_video_attachment(
                    AssetRole::VideoPoster,
                    "image/jpeg",
                    b"fake-video-poster",
                ),
            ],
        )
        .await
        .expect("create video post");
    let timeline = app
        .list_timeline("kukuri:topic:video-write", None, 10)
        .await
        .expect("timeline");

    let post = timeline
        .items
        .iter()
        .find(|post| post.object_id == object_id)
        .expect("video post");
    assert_eq!(post.attachments.len(), 2);
    assert!(
        post.attachments
            .iter()
            .any(|attachment| attachment.role == "video_manifest")
    );
    assert!(
        post.attachments
            .iter()
            .any(|attachment| attachment.role == "video_poster")
    );
}

#[tokio::test]
async fn list_timeline_rehydrates_placeholder_from_blob_store() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let docs_sync = Arc::new(MemoryDocsSync::default());
    let blob_service = Arc::new(MemoryBlobService::default());
    let keys = generate_keys();
    let topic = TopicId::new("kukuri:topic:hydrate");
    let stored_blob = blob_service
        .put_blob(b"hello after blob fetch".to_vec(), "text/plain")
        .await
        .expect("put blob");
    persist_test_post(
        docs_sync.as_ref(),
        Some(store.as_ref()),
        &keys,
        &topic,
        PayloadRef::BlobText {
            hash: stored_blob.hash.clone(),
            mime: stored_blob.mime.clone(),
            bytes: stored_blob.bytes,
        },
        Vec::new(),
        None,
    )
    .await;

    let app = app_service_from_dependencies(
        store.clone(),
        store,
        transport.clone(),
        transport,
        docs_sync,
        blob_service,
        keys,
    );

    let timeline = app
        .list_timeline(topic.as_str(), None, 20)
        .await
        .expect("timeline");

    assert_eq!(timeline.items.len(), 1);
    assert_eq!(timeline.items[0].content, "hello after blob fetch");
}

#[tokio::test]
async fn on_demand_hydration_updates_last_sync_ts() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let docs_sync = Arc::new(MemoryDocsSync::default());
    let blob_service = Arc::new(MemoryBlobService::default());
    let keys = generate_keys();
    let topic = TopicId::new("kukuri:topic:on-demand-sync-ts");
    let stored_blob = blob_service
        .put_blob(b"hydrate updates sync ts".to_vec(), "text/plain")
        .await
        .expect("put blob");
    persist_test_post(
        docs_sync.as_ref(),
        None,
        &keys,
        &topic,
        PayloadRef::BlobText {
            hash: stored_blob.hash,
            mime: stored_blob.mime,
            bytes: stored_blob.bytes,
        },
        Vec::new(),
        None,
    )
    .await;

    let app = app_service_from_dependencies(
        store.clone(),
        store,
        transport.clone(),
        transport,
        docs_sync,
        blob_service,
        keys,
    );

    assert!(
        app.get_sync_status()
            .await
            .expect("status")
            .last_sync_ts
            .is_none()
    );

    let timeline = app
        .list_timeline(topic.as_str(), None, 20)
        .await
        .expect("timeline");
    assert_eq!(timeline.items.len(), 1);

    assert!(
        app.get_sync_status()
            .await
            .expect("status")
            .last_sync_ts
            .is_some()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[cfg(feature = "iroh-integration-tests")]
async fn missing_gossip_but_docs_sync_recovers_post() {
    let _guard = iroh_integration_test_lock().lock_owned().await;
    assert_docs_sync_recovers_post_without_hints("kukuri:topic:missing-gossip", "docs recover")
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[cfg(feature = "iroh-integration-tests")]
async fn gossip_loss_does_not_lose_durable_post() {
    let _guard = iroh_integration_test_lock().lock_owned().await;
    assert_docs_sync_recovers_post_without_hints(
        "kukuri:topic:gossip-loss",
        "durable docs payload",
    )
    .await;
}

#[tokio::test]
async fn thread_open_triggers_lazy_blob_fetch() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let docs_sync = Arc::new(MemoryDocsSync::default());
    let blob_service = Arc::new(MemoryBlobService::default());
    let keys = generate_keys();
    let topic = TopicId::new("kukuri:topic:thread-lazy");
    let root_blob = blob_service
        .put_blob(b"root body".to_vec(), "text/plain")
        .await
        .expect("put root blob");
    let root = persist_test_post(
        docs_sync.as_ref(),
        Some(store.as_ref()),
        &keys,
        &topic,
        PayloadRef::BlobText {
            hash: root_blob.hash,
            mime: root_blob.mime,
            bytes: root_blob.bytes,
        },
        Vec::new(),
        None,
    )
    .await;
    let reply_blob = blob_service
        .put_blob(b"reply body".to_vec(), "text/plain")
        .await
        .expect("put reply blob");
    let _reply = persist_test_post(
        docs_sync.as_ref(),
        Some(store.as_ref()),
        &keys,
        &topic,
        PayloadRef::BlobText {
            hash: reply_blob.hash,
            mime: reply_blob.mime,
            bytes: reply_blob.bytes,
        },
        Vec::new(),
        Some(&root),
    )
    .await;

    let app = app_service_from_dependencies(
        store.clone(),
        store,
        transport.clone(),
        transport,
        docs_sync,
        blob_service,
        generate_keys(),
    );

    let thread = app
        .list_thread(topic.as_str(), root.id.as_str(), None, 20)
        .await
        .expect("thread");

    assert_eq!(
        thread.items.len(),
        2,
        "thread items: {:?}",
        thread
            .items
            .iter()
            .map(|post| format!(
                "{}|reply={:?}|root={:?}",
                post.object_id, post.reply_to, post.root_id
            ))
            .collect::<Vec<_>>()
    );
    assert!(thread.items.iter().any(|post| post.content == "root body"));
    assert!(thread.items.iter().any(|post| post.content == "reply body"));
}

#[tokio::test]
async fn image_post_visible_before_full_blob_download() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let docs_sync = Arc::new(MemoryDocsSync::default());
    let blob_service = Arc::new(MemoryBlobService::default());
    let keys = generate_keys();
    let topic = TopicId::new("kukuri:topic:image");
    let image_bytes = b"fake image bytes".to_vec();
    let image_hash = kukuri_core::blob_hash(&image_bytes);
    persist_test_post(
        docs_sync.as_ref(),
        None,
        &keys,
        &topic,
        PayloadRef::BlobText {
            hash: kukuri_core::BlobHash::new("f".repeat(64)),
            mime: "text/plain".into(),
            bytes: 0,
        },
        vec![kukuri_core::AssetRef {
            hash: image_hash.clone(),
            mime: "image/png".into(),
            bytes: image_bytes.len() as u64,
            role: kukuri_core::AssetRole::ImageOriginal,
        }],
        None,
    )
    .await;

    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        transport.clone(),
        transport,
        docs_sync,
        blob_service.clone(),
        generate_keys(),
    );

    let timeline = app
        .list_timeline(topic.as_str(), None, 20)
        .await
        .expect("timeline");
    assert_eq!(timeline.items.len(), 1);
    assert_eq!(timeline.items[0].content, "[blob pending]");
    assert_eq!(timeline.items[0].content_status, BlobViewStatus::Missing);
    assert_eq!(timeline.items[0].attachments.len(), 1);
    assert_eq!(
        timeline.items[0].attachments[0].status,
        BlobViewStatus::Missing
    );
    assert_eq!(timeline.items[0].attachments[0].role, "image_original");

    blob_service
        .put_blob(image_bytes, "image/png")
        .await
        .expect("put image blob");

    let refreshed = app
        .list_timeline(topic.as_str(), None, 20)
        .await
        .expect("timeline after image fetch");
    assert_eq!(refreshed.items.len(), 1);
    assert_eq!(
        refreshed.items[0].attachments[0].status,
        BlobViewStatus::Available
    );
    assert_eq!(refreshed.items[0].attachments[0].mime, "image/png");
}

#[tokio::test]
async fn video_post_visible_before_full_blob_download() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let docs_sync = Arc::new(MemoryDocsSync::default());
    let blob_service = Arc::new(MemoryBlobService::default());
    let keys = generate_keys();
    let topic = TopicId::new("kukuri:topic:video");
    let poster_hash = kukuri_core::blob_hash(b"poster-bytes");
    persist_test_post(
        docs_sync.as_ref(),
        None,
        &keys,
        &topic,
        PayloadRef::BlobText {
            hash: kukuri_core::BlobHash::new("f".repeat(64)),
            mime: "text/plain".into(),
            bytes: 13,
        },
        vec![
            kukuri_core::AssetRef {
                hash: kukuri_core::blob_hash(b"video-bytes"),
                mime: "video/mp4".into(),
                bytes: 8192,
                role: kukuri_core::AssetRole::VideoManifest,
            },
            kukuri_core::AssetRef {
                hash: poster_hash.clone(),
                mime: "image/jpeg".into(),
                bytes: 1024,
                role: kukuri_core::AssetRole::VideoPoster,
            },
        ],
        None,
    )
    .await;

    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        transport.clone(),
        transport,
        docs_sync,
        blob_service.clone(),
        generate_keys(),
    );

    let timeline = app
        .list_timeline(topic.as_str(), None, 20)
        .await
        .expect("timeline");
    let post = &timeline.items[0];
    assert!(
        post.attachments
            .iter()
            .any(|attachment| attachment.role == "video_manifest")
    );
    assert!(
        post.attachments
            .iter()
            .find(|attachment| attachment.role == "video_poster")
            .is_some_and(|attachment| attachment.status == BlobViewStatus::Missing)
    );

    blob_service
        .put_blob(b"poster-bytes".to_vec(), "image/jpeg")
        .await
        .expect("put poster blob");
    let refreshed = app
        .list_timeline(topic.as_str(), None, 20)
        .await
        .expect("timeline");
    assert!(
        refreshed.items[0]
            .attachments
            .iter()
            .find(|attachment| attachment.role == "video_poster")
            .is_some_and(|attachment| attachment.status == BlobViewStatus::Available)
    );
}

#[tokio::test]
async fn new_writes_use_blob_text_payload_refs() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(FakeTransport::new("app", FakeNetwork::default()));
    let app = AppService::new(store.clone(), transport);
    let topic = "kukuri:topic:blobtext";

    let object_id = app
        .create_post(topic, "blob text only", None)
        .await
        .expect("create post");
    let projection =
        ObjectProjectionStore::get_object_projection(store.as_ref(), &EnvelopeId::from(object_id))
            .await
            .expect("projection")
            .expect("projection row");

    assert!(matches!(
        projection.payload_ref,
        PayloadRef::BlobText { .. }
    ));
    assert!(!matches!(
        projection.payload_ref,
        PayloadRef::InlineText { .. }
    ));
}

#[tokio::test]
async fn blob_media_payload_roundtrip() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(FakeTransport::new("app", FakeNetwork::default()));
    let blob_service = Arc::new(MemoryBlobService::default());
    let app = app_service_from_dependencies(
        store.clone(),
        store,
        transport.clone(),
        transport,
        Arc::new(MemoryDocsSync::default()),
        blob_service.clone(),
        generate_keys(),
    );

    let stored = blob_service
        .put_blob(b"fake-image".to_vec(), "image/png")
        .await
        .expect("put image");
    let payload = app
        .blob_media_payload(stored.hash.as_str(), "image/png")
        .await
        .expect("media payload")
        .expect("media payload present");

    assert_eq!(payload.bytes_base64, "ZmFrZS1pbWFnZQ==");
    assert_eq!(payload.mime, "image/png");
    assert!(
        app.blob_media_payload(&"f".repeat(64), "image/png")
            .await
            .expect("missing payload")
            .is_none()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[cfg(feature = "iroh-integration-tests")]
async fn iroh_transport_syncs_image_post_between_apps() {
    let _guard = iroh_integration_test_lock().lock_owned().await;
    let dir = tempdir().expect("tempdir");
    let stack_a = TestIrohStack::new(&dir.path().join("image-post-a")).await;
    let stack_b = TestIrohStack::new(&dir.path().join("image-post-b")).await;
    let store_a = Arc::new(MemoryStore::default());
    let store_b = Arc::new(MemoryStore::default());
    let app_a = app_with_iroh_services(store_a, &stack_a);
    let app_b = app_with_iroh_services(store_b.clone(), &stack_b);

    let ticket_a = app_a
        .peer_ticket()
        .await
        .expect("ticket a")
        .expect("ticket a value");
    let ticket_b = app_b
        .peer_ticket()
        .await
        .expect("ticket b")
        .expect("ticket b value");
    app_a
        .import_peer_ticket(&ticket_b)
        .await
        .expect("import b into a");
    app_b
        .import_peer_ticket(&ticket_a)
        .await
        .expect("import a into b");

    let topic = "kukuri:topic:image-sync";
    let _ = app_b
        .list_timeline(topic, None, 20)
        .await
        .expect("app b should subscribe to topic");

    let object_id = app_a
        .create_post_with_attachments(
            topic,
            "caption over iroh",
            None,
            vec![pending_image_attachment("image/png", b"fake-image-sync")],
        )
        .await
        .expect("create image post");

    let received = timeout(Duration::from_secs(30), async {
        loop {
            let timeline = app_b
                .list_timeline(topic, None, 20)
                .await
                .expect("timeline should load");
            if let Some(post) = timeline
                .items
                .iter()
                .find(|post| post.object_id == object_id)
            {
                return post.clone();
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("image sync timeout");

    assert_eq!(received.content, "caption over iroh");
    assert_eq!(received.attachments.len(), 1);
    assert_eq!(received.attachments[0].mime, "image/png");
    // view の状態は remote から取得しない（#1152）。表示要求が取得して local へ保存する。
    assert_eq!(received.attachments[0].status, BlobViewStatus::Missing);
    let hash = received.attachments[0].hash.as_str();
    assert!(
        app_b
            .blob_preview_data_url(hash, "image/png")
            .await
            .expect("preview data url")
            .is_some()
    );
    assert_eq!(
        timeline_attachment_status(&app_b, topic, &object_id, hash).await,
        BlobViewStatus::Available
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[cfg(feature = "iroh-integration-tests")]
async fn remote_video_manifest_payload_available_after_sync() {
    let _guard = iroh_integration_test_lock().lock_owned().await;
    let dir = tempdir().expect("tempdir");
    let stack_a = TestIrohStack::new(&dir.path().join("video-post-a")).await;
    let stack_b = TestIrohStack::new(&dir.path().join("video-post-b")).await;
    let store_a = Arc::new(MemoryStore::default());
    let store_b = Arc::new(MemoryStore::default());
    let app_a = app_with_iroh_services(store_a, &stack_a);
    let app_b = app_with_iroh_services(store_b, &stack_b);

    let ticket_a = app_a
        .peer_ticket()
        .await
        .expect("ticket a")
        .expect("ticket a value");
    let ticket_b = app_b
        .peer_ticket()
        .await
        .expect("ticket b")
        .expect("ticket b value");
    app_a
        .import_peer_ticket(&ticket_b)
        .await
        .expect("import b into a");
    app_b
        .import_peer_ticket(&ticket_a)
        .await
        .expect("import a into b");

    let topic = "kukuri:topic:video-sync";
    let _ = app_b
        .list_timeline(topic, None, 20)
        .await
        .expect("subscribe b timeline");

    let object_id = app_a
        .create_post_with_attachments(
            topic,
            "video caption",
            None,
            vec![
                pending_video_attachment(AssetRole::VideoManifest, "video/mp4", b"video-sync"),
                pending_video_attachment(AssetRole::VideoPoster, "image/jpeg", b"poster-sync"),
            ],
        )
        .await
        .expect("create video post");

    let received = timeout(Duration::from_secs(30), async {
        loop {
            let timeline = app_b
                .list_timeline(topic, None, 20)
                .await
                .expect("timeline");
            if let Some(post) = timeline
                .items
                .iter()
                .find(|post| post.object_id == object_id)
            {
                return post.clone();
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("video sync timeout");

    assert!(
        received
            .attachments
            .iter()
            .any(|attachment| attachment.role == "video_manifest")
    );
    let poster = received
        .attachments
        .iter()
        .find(|attachment| attachment.role == "video_poster")
        .expect("video poster");
    assert_eq!(poster.status, BlobViewStatus::Missing);
    let poster_payload = app_b
        .blob_media_payload(poster.hash.as_str(), "image/jpeg")
        .await
        .expect("poster media payload")
        .expect("poster payload present");
    assert_eq!(poster_payload.mime, "image/jpeg");
    assert_eq!(
        timeline_attachment_status(&app_b, topic, &object_id, poster.hash.as_str()).await,
        BlobViewStatus::Available
    );
    let manifest = received
        .attachments
        .iter()
        .find(|attachment| attachment.role == "video_manifest")
        .expect("video manifest");
    let manifest_payload = app_b
        .blob_media_payload(manifest.hash.as_str(), "video/mp4")
        .await
        .expect("video media payload")
        .expect("manifest payload present");
    assert_eq!(manifest_payload.mime, "video/mp4");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[cfg(feature = "iroh-integration-tests")]
async fn late_joiner_backfills_image_post_from_docs() {
    let _guard = iroh_integration_test_lock().lock_owned().await;
    let dir = tempdir().expect("tempdir");
    let stack_a = TestIrohStack::new(&dir.path().join("late-image-a")).await;
    let stack_b = TestIrohStack::new(&dir.path().join("late-image-b")).await;
    let store_a = Arc::new(MemoryStore::default());
    let store_b = Arc::new(MemoryStore::default());
    let app_a = app_with_iroh_services(store_a, &stack_a);
    let app_b = app_with_iroh_services(store_b, &stack_b);

    let topic = "kukuri:topic:late-image";
    let object_id = app_a
        .create_post_with_attachments(
            topic,
            "late image caption",
            None,
            vec![pending_image_attachment("image/png", b"late-image-bytes")],
        )
        .await
        .expect("create image post before join");
    let ticket_a = app_a
        .peer_ticket()
        .await
        .expect("ticket a")
        .expect("ticket a value");

    app_b
        .import_peer_ticket(&ticket_a)
        .await
        .expect("import a into b");

    let received = timeout(Duration::from_secs(60), async {
        loop {
            let timeline = app_b
                .list_timeline(topic, None, 20)
                .await
                .expect("timeline b");
            if let Some(post) = timeline
                .items
                .iter()
                .find(|post| post.object_id == object_id)
            {
                let post = post.clone();
                if post.attachments.len() == 1 {
                    let preview = app_b
                        .blob_preview_data_url(post.attachments[0].hash.as_str(), "image/png")
                        .await
                        .expect("preview data url");
                    if preview.is_some() {
                        let refreshed_timeline = app_b
                            .list_timeline(topic, None, 20)
                            .await
                            .expect("timeline b refreshed");
                        if let Some(refreshed_post) = refreshed_timeline
                            .items
                            .iter()
                            .find(|candidate| candidate.object_id == object_id)
                            .cloned()
                            && refreshed_post.attachments.len() == 1
                            && refreshed_post.attachments[0].status == BlobViewStatus::Available
                        {
                            return refreshed_post;
                        }
                    }
                }
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("late image join timeout");

    assert_eq!(received.attachments.len(), 1);
    assert_eq!(received.attachments[0].status, BlobViewStatus::Available);
    assert!(
        app_b
            .blob_preview_data_url(received.attachments[0].hash.as_str(), "image/png")
            .await
            .expect("preview data url")
            .is_some()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[cfg(feature = "iroh-integration-tests")]
async fn late_joiner_backfills_video_media_payload() {
    let _guard = iroh_integration_test_lock().lock_owned().await;
    let dir = tempdir().expect("tempdir");
    let stack_a = TestIrohStack::new(&dir.path().join("late-video-a")).await;
    let stack_b = TestIrohStack::new(&dir.path().join("late-video-b")).await;
    let store_a = Arc::new(MemoryStore::default());
    let store_b = Arc::new(MemoryStore::default());
    let app_a = app_with_iroh_services(store_a, &stack_a);
    let app_b = app_with_iroh_services(store_b, &stack_b);

    let topic = "kukuri:topic:late-video";
    let object_id = app_a
        .create_post_with_attachments(
            topic,
            "late video caption",
            None,
            vec![
                pending_video_attachment(AssetRole::VideoManifest, "video/mp4", b"late-video"),
                pending_video_attachment(AssetRole::VideoPoster, "image/jpeg", b"late-poster"),
            ],
        )
        .await
        .expect("create video post before join");
    let ticket_a = app_a
        .peer_ticket()
        .await
        .expect("ticket a")
        .expect("ticket a value");

    app_b
        .import_peer_ticket(&ticket_a)
        .await
        .expect("import a into b");

    let received = timeout(Duration::from_secs(10), async {
        loop {
            let timeline = app_b
                .list_timeline(topic, None, 20)
                .await
                .expect("timeline b");
            if let Some(post) = timeline
                .items
                .iter()
                .find(|post| post.object_id == object_id)
            {
                return post.clone();
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("late video join timeout");

    let poster = received
        .attachments
        .iter()
        .find(|attachment| attachment.role == "video_poster")
        .expect("video poster");
    assert_eq!(poster.status, BlobViewStatus::Missing);
    let poster_payload = app_b
        .blob_media_payload(poster.hash.as_str(), "image/jpeg")
        .await
        .expect("poster media payload")
        .expect("poster payload present");
    assert_eq!(poster_payload.mime, "image/jpeg");
    assert_eq!(
        timeline_attachment_status(&app_b, topic, &object_id, poster.hash.as_str()).await,
        BlobViewStatus::Available
    );
    let manifest = received
        .attachments
        .iter()
        .find(|attachment| attachment.role == "video_manifest")
        .expect("video manifest");
    let manifest_payload = app_b
        .blob_media_payload(manifest.hash.as_str(), "video/mp4")
        .await
        .expect("video media payload")
        .expect("manifest payload present");
    assert_eq!(manifest_payload.mime, "video/mp4");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[cfg(feature = "iroh-integration-tests")]
async fn image_reply_thread_syncs() {
    let _guard = iroh_integration_test_lock().lock_owned().await;
    let dir = tempdir().expect("tempdir");
    let stack_a = TestIrohStack::new(&dir.path().join("image-thread-a")).await;
    let stack_b = TestIrohStack::new(&dir.path().join("image-thread-b")).await;
    let store_a = Arc::new(MemoryStore::default());
    let store_b = Arc::new(MemoryStore::default());
    let app_a = app_with_iroh_services(store_a.clone(), &stack_a);
    let app_b = app_with_iroh_services(store_b.clone(), &stack_b);
    let topic = "kukuri:topic:image-thread";

    let ticket_a = app_a
        .peer_ticket()
        .await
        .expect("ticket a")
        .expect("ticket a value");
    let ticket_b = app_b
        .peer_ticket()
        .await
        .expect("ticket b")
        .expect("ticket b value");
    app_a
        .import_peer_ticket(&ticket_b)
        .await
        .expect("import b into a");
    app_b
        .import_peer_ticket(&ticket_a)
        .await
        .expect("import a into b");
    let _ = app_a
        .list_timeline(topic, None, 20)
        .await
        .expect("subscribe a timeline");
    let _ = app_b
        .list_timeline(topic, None, 20)
        .await
        .expect("subscribe b timeline");
    wait_for_topic_delivery(&app_a, topic, 1).await;
    wait_for_topic_delivery(&app_b, topic, 1).await;

    let root_id = app_a
        .create_post_with_attachments(
            topic,
            "root image",
            None,
            vec![pending_image_attachment("image/png", b"root-image")],
        )
        .await
        .expect("create root image");

    timeout(p2p_replication_timeout(), async {
        loop {
            let timeline = app_b
                .list_timeline(topic, None, 20)
                .await
                .expect("timeline b");
            if timeline.items.iter().any(|post| post.object_id == root_id) {
                return;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("root image propagation timeout");

    let reply_id = app_b
        .create_post_with_attachments(
            topic,
            "reply image",
            Some(root_id.as_str()),
            vec![pending_image_attachment("image/jpeg", b"reply-image")],
        )
        .await
        .expect("create reply image");
    let thread = timeout(p2p_replication_timeout(), async {
        loop {
            let thread = app_b
                .list_thread(topic, root_id.as_str(), None, 20)
                .await
                .expect("thread b");
            if thread.items.iter().any(|post| post.object_id == reply_id) {
                return thread;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("image reply propagation timeout");

    let root = thread
        .items
        .iter()
        .find(|post| post.object_id == root_id)
        .expect("root in thread");
    let reply = thread
        .items
        .iter()
        .find(|post| post.object_id == reply_id)
        .expect("reply in thread");
    assert_eq!(root.attachments[0].mime, "image/png");
    assert_eq!(reply.attachments[0].mime, "image/jpeg");
    assert_eq!(reply.reply_to.as_deref(), Some(root_id.as_str()));
}
