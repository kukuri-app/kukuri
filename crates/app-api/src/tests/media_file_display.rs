//! 表示用ファイルで取得した添付の cache と、提供者が止まった後の再表示(#1419 AC-4 を含む)。

use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_media_file_is_cached_and_redisplays_after_the_provider_stops() -> anyhow::Result<()>
{
    redisplay_after_the_provider_stops(Vec::new()).await
}

/// #1419 AC-4: 表示設定 ON の間に表示した成人向けの添付も cache に置き、作者が止まった後も再表示できる。
/// OFF に戻したら cache から消す。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn adult_media_file_shown_while_enabled_is_cached_and_forgotten_when_disabled()
-> anyhow::Result<()> {
    redisplay_after_the_provider_stops(vec![kukuri_core::ADULT_CONTENT_LABEL.to_string()]).await
}

async fn redisplay_after_the_provider_stops(content_labels: Vec<String>) -> anyhow::Result<()> {
    let adult = !content_labels.is_empty();
    let _guard = iroh_integration_test_lock().lock_owned().await;
    let dir = tempdir().unwrap();
    let sender_stack = TestIrohStack::new(&dir.path().join("media-file-sender")).await;
    let receiver_stack = TestIrohStack::new(&dir.path().join("media-file-receiver")).await;
    let sender_keys = generate_keys();
    let sender_store = Arc::new(MemoryStore::default());
    let sender_app = app_service_from_dependencies(
        sender_store.clone(),
        sender_store,
        sender_stack.transport.clone(),
        sender_stack.transport.clone(),
        sender_stack.docs_sync.clone(),
        sender_stack.blob_service.clone(),
        sender_keys.clone(),
    );
    let receiver_store = Arc::new(
        SqliteStore::connect_file(dir.path().join("receiver.db"))
            .await
            .unwrap(),
    );
    receiver_stack
        ._node
        .install_remote_cache(receiver_store.clone())
        .unwrap();
    let receiver_blob = Arc::new(IrohBlobService::with_account_store(
        receiver_stack._node.clone(),
        receiver_store.clone(),
    ));
    let receiver_app = app_service_from_dependencies(
        receiver_store.clone(),
        receiver_store.clone(),
        receiver_stack.transport.clone(),
        receiver_stack.transport.clone(),
        receiver_stack.docs_sync.clone(),
        receiver_blob,
        generate_keys(),
    );
    let ticket = sender_app.peer_ticket().await.unwrap().unwrap();
    receiver_app.import_peer_ticket(&ticket).await.unwrap();
    let bytes = vec![42u8; 2 * 1024 * 1024 + 11];
    let stored = sender_stack
        .blob_service
        .put_blob(bytes.clone(), "video/mp4")
        .await
        .unwrap();
    let topic = TopicId::new("kukuri:topic:media-file-display");
    let envelope = build_post_envelope_with_docs_author(
        &sender_keys,
        &topic,
        PayloadRef::InlineText {
            text: "video".into(),
        },
        vec![kukuri_core::AssetRef {
            hash: stored.hash.clone(),
            mime: "video/mp4".into(),
            bytes: stored.bytes,
            role: AssetRole::VideoManifest,
        }],
        Vec::new(),
        None,
        ObjectVisibility::Public,
        None,
        content_labels,
        None,
    )
    .unwrap();
    receiver_store
        .put_remote_object_projection(verified_projection_row(
            &envelope,
            &topic_replica_id(topic.as_str()),
            None,
        ))
        .await
        .unwrap();
    receiver_app.set_adult_content_display_enabled(adult);
    let display = dir.path().join("display.mp4");
    assert_eq!(
        receiver_app
            .blob_media_file_for_post(stored.hash.as_str(), Some(envelope.id.as_str()), &display)
            .await
            .unwrap(),
        Some(stored.bytes)
    );
    assert_eq!(tokio::fs::read(&display).await.unwrap(), bytes);
    assert!(
        receiver_store
            .has_remote_content("blob", stored.hash.as_str())
            .await
            .unwrap()
    );
    assert!(
        !receiver_stack
            ._node
            .blobs()
            .blobs()
            .has(blake3::Hash::from_hex(stored.hash.as_str()).unwrap())
            .await
            .unwrap()
    );
    tokio::fs::remove_file(&display).await.unwrap();
    sender_stack._node.shutdown().await.unwrap();
    assert_eq!(
        receiver_app
            .blob_media_file_for_post(stored.hash.as_str(), Some(envelope.id.as_str()), &display)
            .await
            .unwrap(),
        Some(stored.bytes)
    );
    assert_eq!(tokio::fs::read(display).await.unwrap(), bytes);
    if adult {
        receiver_app.set_adult_content_display_enabled(false);
        tokio::time::timeout(Duration::from_secs(10), async {
            while receiver_store
                .has_remote_content("blob", stored.hash.as_str())
                .await
                .unwrap()
            {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("adult media stays cached after the display setting is disabled");
    }
    receiver_app.shutdown().await;
    Ok(())
}
