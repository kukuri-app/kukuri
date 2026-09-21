//! #1239(AC-6、#1277): view の生成は docs を読まない。projection に無い返信先は、背景で key 指定で反映し、
//! この回の preview は出さない(反映できれば次の取得で出る)。背景の反映は、確認先ごとに間隔を空ける。

use super::hydration_integrity::{signed_post, write_object_entries};
use super::shadowing_docs::honest_header;
use super::*;

#[tokio::test]
async fn a_missing_reply_target_is_reflected_in_the_background_without_docs_reads_in_the_view() {
    let docs_sync = Arc::new(CountingDocsSync::default());
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        transport.clone(),
        transport,
        docs_sync.clone(),
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    );
    let topic = TopicId::new("kukuri:topic:reply-target-background");
    let replica = topic_replica_id(topic.as_str());
    let parent = signed_post(
        &generate_keys(),
        &topic,
        "the parent",
        ObjectVisibility::Public,
        None,
    );
    write_object_entries(
        docs_sync.as_ref(),
        &replica,
        Some(&parent),
        &honest_header(&parent),
    )
    .await;
    let profiles = HashMap::new();
    let source = Some((&replica, topic.as_str()));

    // 背景の反映が走り出す前に、view の生成が docs を読まないことを確かめる(permit を取らせない)。
    let permits = app.services.reply_target_checks.permits();
    let held = permits
        .acquire_many(crate::service::hydration_limits::BACKGROUND_CHECK_MAX_CONCURRENT as u32)
        .await
        .expect("hold the permits");
    docs_sync.clear_queries().await;
    docs_sync.reset_records_returned();
    let first = app
        .reply_preview_for_object_id(Some(&parent.id), source, &profiles)
        .await
        .expect("reply preview");
    assert!(first.is_none(), "the preview waits for the background");
    assert!(
        docs_sync.queries().await.is_empty(),
        "the view generation does not read docs"
    );
    assert_eq!(docs_sync.records_returned(), 0);
    drop(held);

    timeout(Duration::from_secs(10), async {
        while store
            .get_object_projection(&parent.id)
            .await
            .expect("projection")
            .is_none()
        {
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the background reflects the reply target");
    let second = app
        .reply_preview_for_object_id(Some(&parent.id), source, &profiles)
        .await
        .expect("reply preview")
        .expect("the preview after the background");
    assert_eq!(second.content, "the parent");
}

// 反映できない返信先(手元に本体が無い)を表示し続けても、背景の読み出しは確認先ごとに間隔を空けて 1 回。
#[tokio::test]
async fn background_reflection_of_a_reply_target_is_spaced_per_target() {
    let docs_sync = Arc::new(CountingDocsSync::default());
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let app = app_service_from_dependencies(
        store.clone(),
        store,
        transport.clone(),
        transport,
        docs_sync.clone(),
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    );
    let topic = TopicId::new("kukuri:topic:reply-target-spaced");
    let replica = topic_replica_id(topic.as_str());
    let missing = EnvelopeId::from("f".repeat(64).as_str());
    let profiles = HashMap::new();
    docs_sync.clear_queries().await;

    for _ in 0..5 {
        assert!(
            app.reply_preview_for_object_id(
                Some(&missing),
                Some((&replica, topic.as_str())),
                &profiles
            )
            .await
            .expect("reply preview")
            .is_none()
        );
    }
    sleep(Duration::from_millis(300)).await;

    let envelope_key = stable_key("objects", &format!("{}/envelope", missing.as_str()));
    let reads = docs_sync
        .queries()
        .await
        .into_iter()
        .filter(|(_, query)| *query == DocQuery::Exact(envelope_key.clone()))
        .count();
    assert_eq!(reads, 1, "one background read per target and interval");
    assert_eq!(app.services.reply_target_checks.len(), 1);
}

// 取得側の反映(#1277): 遡ったページの行でも、返信先が手元の docs にあれば、その取得で preview が出る
// (view の生成は docs を読まず、取得が view の生成の前に key 指定で反映する)。
#[tokio::test]
async fn an_older_page_reflects_the_reply_target_before_building_the_view() {
    let docs_sync = Arc::new(MemoryDocsSync::default());
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        transport.clone(),
        transport,
        docs_sync.clone(),
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    );
    let topic = TopicId::new("kukuri:topic:reply-target-older-page");
    let replica = topic_replica_id(topic.as_str());
    let keys = generate_keys();
    let parent = super::range_reconcile::put_post_at(
        docs_sync.as_ref(),
        &replica,
        &keys,
        &topic,
        1_700_000_000,
        "the parent",
        None,
    )
    .await;
    let reply = super::range_reconcile::put_post_at(
        docs_sync.as_ref(),
        &replica,
        &keys,
        &topic,
        1_700_000_010,
        "the reply",
        Some(&parent.envelope),
    )
    .await;
    // 返信だけが projection にある(返信先は、索引の照合の範囲の外)。
    super::range_reconcile::project(store.as_ref(), &reply, &replica).await;

    let page = app
        .list_timeline(
            topic.as_str(),
            Some(TimelineCursor {
                created_at: 1_700_000_011,
                object_id: EnvelopeId::from("f".repeat(64).as_str()),
            }),
            1,
        )
        .await
        .expect("older page");

    let item = page
        .items
        .iter()
        .find(|item| item.object_id == reply.object_id.as_str())
        .expect("the reply is listed");
    assert_eq!(
        item.reply_preview
            .as_ref()
            .map(|preview| preview.content.as_str()),
        Some("the parent"),
        "the preview is shown by the same listing"
    );
}
