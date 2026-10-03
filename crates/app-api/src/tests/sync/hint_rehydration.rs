use super::*;

#[tokio::test]
async fn topic_object_hints_do_not_rehydrate_whole_replica() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let docs_sync = Arc::new(CountingDocsSync::default());
    let blob_service = Arc::new(MemoryBlobService::default());
    let keys = generate_keys();
    let app = app_service_from_dependencies(
        store.clone(),
        store,
        transport.clone(),
        transport.clone(),
        docs_sync.clone(),
        blob_service,
        keys.clone(),
    );
    let topic = TopicId::new("kukuri:topic:incremental-hint-event");

    display_topic(&app, topic.as_str())
        .await
        .expect("open the topic column");
    let _ = app
        .list_timeline(topic.as_str(), None, 20)
        .await
        .expect("initial timeline");
    sleep(Duration::from_millis(100)).await;
    // R5-H: 表示の後に届いた新着だけが、hint の exact 読取りの対象になる。
    let envelope = persist_test_post(
        docs_sync.as_ref(),
        None,
        &keys,
        &topic,
        PayloadRef::InlineText {
            text: "remote incremental hint".into(),
        },
        Vec::new(),
        None,
    )
    .await;
    docs_sync.clear_queries().await;

    transport
        .publish_hint(
            &channel_hint_topic_for(topic.as_str(), None),
            GossipHint::TopicObjectsChanged {
                topic_id: topic.clone(),
                objects: vec![HintObjectRef {
                    object_id: envelope.id.as_str().to_string(),
                    object_kind: "post".into(),
                    docs_author: None,
                    sent_at: None,
                }],
            },
        )
        .await
        .expect("publish hint");

    timeout(Duration::from_secs(5), async {
        loop {
            let queries = docs_sync.queries().await;
            if !queries.is_empty() {
                break;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("hint handling timeout");

    // #1248: 行は署名つき envelope から作るので、key 指定で読むのは `envelope` の key。
    let queries = docs_sync.queries().await;
    assert!(
        queries.iter().any(|(_, query)| {
            *query
                == DocQuery::Exact(stable_key(
                    "objects",
                    &format!("{}/envelope", envelope.id.as_str()),
                ))
        }),
        "expected exact object query after hint, got {queries:?}"
    );
    assert!(
        queries.iter().all(|(_, query)| {
            !matches!(
                query,
                DocQuery::Prefix(prefix)
                    if prefix == "objects/"
                        || prefix == "reactions/"
                        || prefix == "sessions/live/"
                        || prefix == "sessions/game/"
            )
        }),
        "hint should not trigger whole-replica rehydrate, got {queries:?}"
    );
}
