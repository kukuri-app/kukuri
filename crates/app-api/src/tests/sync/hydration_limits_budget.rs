use super::*;

// #1390 AC-1: 取得先の最初の peer が応答せず接続待ち(5 秒)を使い切っても、node の取得の予算の内で
// 次の peer から届いた本文は行へ反映する。旧い projection の締切(Windows 5 秒、他 2 秒)で打ち切らない。
#[tokio::test(start_paused = true)]
async fn a_body_delivered_after_an_unresponsive_first_peer_is_recovered() {
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let blobs = Arc::new(HangingBlobService {
        gate: Some(gate.clone()),
        ..Default::default()
    });
    let stored = blobs
        .put_blob(b"body behind a silent peer".to_vec(), "text/plain")
        .await
        .unwrap();
    let docs = Arc::new(CountingDocsSync::default());
    let keys = generate_keys();
    let topic = TopicId::new("kukuri:topic:silent-first-peer");
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
    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        transport.clone(),
        transport,
        docs,
        blobs.clone(),
        keys,
    );

    let missing = app.list_timeline(topic.as_str(), None, 20).await.unwrap();
    assert_eq!(missing.items[0].content_status, BlobViewStatus::Missing);
    assert_eq!(blobs.fetches.load(std::sync::atomic::Ordering::SeqCst), 1);
    sleep(Duration::from_secs(6)).await;
    gate.add_permits(1);
    timeout(Duration::from_secs(1), async {
        loop {
            let row = store.get_object_projection(&envelope.id).await.unwrap();
            if row.and_then(|row| row.content).as_deref() == Some("body behind a silent peer") {
                break;
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the body delivered within the node fetch budget is stored");
}
