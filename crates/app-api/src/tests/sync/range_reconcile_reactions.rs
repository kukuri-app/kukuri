//! #1239: ページの範囲の照合が、新しく反映した投稿の reaction を上限つきで反映する。
//!
//! 独立監査(PR #1247 の delta)の再現 test を恒久化したものを含む。

use super::range_reconcile::{BASE_TIME, put_post_at};
use super::range_reconcile_access::{RecordingDocsSync, recording_app_with_blobs};
use super::*;
use crate::service::replica_window::RANGE_CHECK_REACTIONS_PER_OBJECT;

/// 投稿 1 件に reaction を `reactions` 個付けた docs を作る。戻り値は投稿の object id。
/// 同じ著者でも、key が違えば別の reaction になる。
async fn post_with_reactions(author: &AppService, topic: &str, reactions: usize) -> String {
    let target = author
        .create_post(topic, "a post with reactions", None)
        .await
        .expect("create post");
    for index in 0..reactions {
        author
            .toggle_reaction(
                topic,
                target.as_str(),
                ReactionKeyV1::Emoji {
                    emoji: format!("key-{index:03}"),
                },
                Some(ChannelRef::Public),
            )
            .await
            .expect("toggle reaction");
    }
    target
}

// 照合が新しく反映した投稿は、reaction も上限つきで一緒に反映する。docs の event が届かない古い reaction は、
// ここでしか入らない(以前は、空ページの全件走査が reaction も反映していた)。読む量は reaction の総数に依存しない。
#[tokio::test]
async fn newly_reconciled_post_brings_a_bounded_number_of_its_reactions() {
    let (few_rows, few_reads) = reconcile_a_post_with_reactions(
        "kukuri:topic:range-reactions-few",
        RANGE_CHECK_REACTIONS_PER_OBJECT + 8,
    )
    .await;
    let (many_rows, many_reads) = reconcile_a_post_with_reactions(
        "kukuri:topic:range-reactions-many",
        RANGE_CHECK_REACTIONS_PER_OBJECT * 4,
    )
    .await;
    assert_eq!(
        few_rows, RANGE_CHECK_REACTIONS_PER_OBJECT,
        "the reconcile reflects reactions up to the per-object limit"
    );
    assert_eq!(many_rows, RANGE_CHECK_REACTIONS_PER_OBJECT);
    assert_eq!(
        few_reads, many_reads,
        "the number of docs reads does not grow with the number of reactions"
    );
}

/// 投稿 1 件に `reactions` 個の reaction を付け、反映が空の利用者が照合する。
/// 戻り値は、反映された reaction の行数と、照合が発行した docs の読み出しの回数。
async fn reconcile_a_post_with_reactions(topic: &str, reactions: usize) -> (usize, usize) {
    let docs_sync = Arc::new(RecordingDocsSync::default());
    let blob_service = Arc::new(MemoryBlobService::default());
    let (author, _author_store) = recording_app_with_blobs(docs_sync.clone(), blob_service.clone());
    let replica = topic_replica_id(topic);
    let target = post_with_reactions(&author, topic, reactions).await;
    author.shutdown().await;

    // 反映が空の利用者。購読タスクを起動せず、照合だけを呼ぶ。
    let (viewer, viewer_store) = recording_app_with_blobs(docs_sync.clone(), blob_service);
    docs_sync.reads.lock().await.clear();
    let hydrated = viewer
        .reconcile_timeline_range(topic, &TimelineScope::Public, None, 20)
        .await
        .expect("reconcile");
    let reads = docs_sync.reads.lock().await.len();
    assert_eq!(hydrated, 1);
    let projection_store: &dyn ProjectionStore = viewer_store.as_ref();
    let rows = projection_store
        .list_reaction_cache_for_target(&replica, &EnvelopeId::from(target.as_str()))
        .await
        .expect("reaction rows");
    viewer.shutdown().await;
    (rows.len(), reads)
}

// reaction を読むのは、投稿が projection に入るときの 1 回だけ。projection に既にある投稿では読まない
// (ADR 0052 §2)。「既にある」側でも reaction を読む形にすると、表示のたびに投稿の数ぶんの読み出しが走る。
#[tokio::test]
async fn reactions_are_read_only_when_the_post_enters_the_projection() {
    let topic = "kukuri:topic:range-reactions-once";
    let docs_sync = Arc::new(RecordingDocsSync::default());
    let blob_service = Arc::new(MemoryBlobService::default());
    let (author, _author_store) = recording_app_with_blobs(docs_sync.clone(), blob_service.clone());
    post_with_reactions(&author, topic, 5).await;
    author.shutdown().await;

    let (viewer, _viewer_store) = recording_app_with_blobs(docs_sync.clone(), blob_service);
    docs_sync.reads.lock().await.clear();
    let first = viewer
        .reconcile_timeline_range(topic, &TimelineScope::Public, None, 20)
        .await
        .expect("first reconcile");
    let first_reads = docs_sync.reads.lock().await.len();
    assert_eq!(first, 1);

    // 同じ範囲の次の照合(台帳の間隔が過ぎた後)。投稿は projection に既にある。
    viewer.services.range_checks.expire_all_for_test().await;
    docs_sync.reads.lock().await.clear();
    let second = viewer
        .reconcile_timeline_range(topic, &TimelineScope::Public, None, 20)
        .await
        .expect("second reconcile");
    let second_reads = docs_sync.reads.lock().await.len();
    viewer.shutdown().await;
    assert_eq!(second, 0);
    // 2 回目が読むのは、索引の key の一覧 1 回と、既にある投稿の取り下げの確認(key 指定 1 回)だけ。
    assert!(
        second_reads <= 2,
        "a post that is already projected must not have its reactions read again: first={first_reads}, second={second_reads}"
    );
}

// 照合の読み出しは `LocalOnly`(ADR 0052 §2)。reaction の envelope の読み出しも、remote 取得を待たない。
#[tokio::test]
async fn reconcile_does_not_wait_for_a_remote_reaction_envelope() {
    let docs_sync = Arc::new(HangingRemoteOnMissDocsSync::default());
    let store = Arc::new(MemoryStore::default());
    let app = app_with_hanging_remote_docs(
        store.clone(),
        docs_sync.clone(),
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    );
    let topic = TopicId::new("kukuri:topic:range-reaction-local-only");
    let replica = topic_replica_id(topic.as_str());
    let post = put_post_at(
        docs_sync.as_ref(),
        &replica,
        &generate_keys(),
        &topic,
        BASE_TIME,
        "a post whose reaction envelope has not arrived",
        None,
    )
    .await;
    // reaction の `state` の key だけがあり、envelope はまだ手元に無い(同期の途中と同じ状態)。
    docs_sync
        .apply_doc_op(
            &replica,
            DocOp::SetJson {
                key: stable_key(
                    "reactions",
                    &format!("{}/{}/state", post.object_id.as_str(), "a".repeat(64)),
                ),
                value: serde_json::json!({}),
            },
        )
        .await
        .expect("write the reaction state key");

    let hydrated = timeout(
        Duration::from_secs(5),
        app.reconcile_timeline_range(topic.as_str(), &TimelineScope::Public, None, 20),
    )
    .await
    .expect("the reconcile must not wait for a remote fetch of a reaction envelope")
    .expect("reconcile");
    app.shutdown().await;
    assert_eq!(hydrated, 1);
}

// reaction の key の一覧は上限つき。1 回の一覧が返す key の数が、その投稿の reaction の総数に比例しない
// (読み出しの回数だけでなく、docs が返した record と key の総数で比べる)。
#[tokio::test]
async fn reaction_key_listing_returns_a_bounded_number_of_keys() {
    let mut returned = Vec::new();
    for reactions in [40usize, 128] {
        let topic = format!("kukuri:topic:range-reaction-keys-{reactions}");
        let docs_sync = Arc::new(CountingDocsSync::default());
        let blob_service = Arc::new(MemoryBlobService::default());
        let app_over_counting_docs = || {
            let store = Arc::new(MemoryStore::default());
            app_service_from_dependencies(
                store.clone(),
                store,
                Arc::new(StaticTransport::new(PeerSnapshot::default())),
                Arc::new(NoopHintTransport),
                docs_sync.clone(),
                blob_service.clone(),
                generate_keys(),
            )
        };
        let author = app_over_counting_docs();
        post_with_reactions(&author, topic.as_str(), reactions).await;
        author.shutdown().await;

        let viewer = app_over_counting_docs();
        docs_sync.reset_records_returned();
        let hydrated = viewer
            .reconcile_timeline_range(topic.as_str(), &TimelineScope::Public, None, 20)
            .await
            .expect("reconcile");
        viewer.shutdown().await;
        assert_eq!(hydrated, 1);
        returned.push(docs_sync.records_returned());
    }
    assert_eq!(
        returned[0], returned[1],
        "the keys and records returned by docs must not grow with the number of reactions: {returned:?}"
    );
}
