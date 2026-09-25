//! #1239: ページの範囲の照合が、新しく反映した投稿の reaction を上限つきで反映する。
//!
//! 独立監査(PR #1247 の delta)の再現 test を恒久化したものを含む。

use super::range_reconcile::{BASE_TIME, put_post_at};
use super::range_reconcile_access::{RecordingDocsSync, recording_app_with_blobs};
use super::*;
use crate::service::reaction_hydration::REACTION_KEYS_PER_LEAD;
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
    // reaction id は無作為なので、先頭の 1 文字ごとの一覧が上限まで埋まるかは回ごとに違う(240 件でも、16 文字の
    // どれかが 4 件未満の回がある)。返る数どうしではなく、reaction の数に依存しない上限と比べる。
    for reactions in [240usize, 480] {
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
    // 先頭の一覧(上限ぶんの reaction の `state` と `envelope`)、先頭の 1 文字ごとの 16 回の一覧、反映する reaction の
    // envelope、投稿の索引の entry と envelope。reaction の総数ぶんの key を一覧すれば、240 件でもこれを超える。
    let bound = RANGE_CHECK_REACTIONS_PER_OBJECT * 3 + 16 * REACTION_KEYS_PER_LEAD + 2;
    assert!(
        returned.iter().all(|count| *count <= bound),
        "the keys and records returned by docs must not grow with the number of reactions: {returned:?} > {bound}"
    );
}

// `reactions/<target>/` の下に、正しい reaction より先に並ぶ key を一覧の上限ぶん置かれても、正しい reaction は
// 反映される(独立監査 PR #1247 の再現 test を恒久化)。public topic の replica は誰もが書ける。
#[tokio::test]
async fn junk_keys_before_the_real_reactions_do_not_hide_them_from_the_reconcile() {
    let topic = "kukuri:topic:range-reactions-junk";
    let replica = topic_replica_id(topic);
    let docs_sync = Arc::new(RecordingDocsSync::default());
    let blob_service = Arc::new(MemoryBlobService::default());
    let (author, _author_store) = recording_app_with_blobs(docs_sync.clone(), blob_service.clone());
    let target = post_with_reactions(&author, topic, 3).await;
    author.shutdown().await;
    // `!` は 16 進の文字より前に並ぶ。reaction id の形(`/` を含まない 1 区画)で、envelope の無い key。
    for index in 0..64usize {
        docs_sync
            .apply_doc_op(
                &replica,
                DocOp::SetJson {
                    key: stable_key("reactions", &format!("{target}/!junk-{index:03}/state")),
                    value: serde_json::json!({}),
                },
            )
            .await
            .expect("write a junk key");
    }

    let (viewer, viewer_store) = recording_app_with_blobs(docs_sync.clone(), blob_service);
    let hydrated = viewer
        .reconcile_timeline_range(topic, &TimelineScope::Public, None, 20)
        .await
        .expect("the reconcile itself must not fail");
    assert_eq!(hydrated, 1, "the post is still reflected");
    let projection_store: &dyn ProjectionStore = viewer_store.as_ref();
    let rows = projection_store
        .list_reaction_cache_for_target(&replica, &EnvelopeId::from(target.as_str()))
        .await
        .expect("reaction rows");
    viewer.shutdown().await;
    assert_eq!(
        rows.len(),
        3,
        "the three honest reactions are reflected even when junk keys sort first"
    );
}

/// reaction が 1 件ずつ付いた投稿を `posts` 件、docs に置く(`toggle_reaction` を通さない軽い fixture)。
async fn put_posts_with_one_reaction(
    docs_sync: &dyn DocsSync,
    topic: &str,
    posts: usize,
) -> Vec<String> {
    let replica = topic_replica_id(topic);
    let keys = generate_keys();
    let topic_id = TopicId::new(topic);
    let mut targets = Vec::new();
    for index in 0..posts {
        let post = put_post_at(
            docs_sync,
            &replica,
            &keys,
            &topic_id,
            BASE_TIME + index as i64,
            format!("post {index}").as_str(),
            None,
        )
        .await;
        let reaction_key = ReactionKeyV1::Emoji {
            emoji: "👍".into()
        };
        let reaction_id = deterministic_reaction_id(
            &replica,
            &post.object_id,
            &keys.public_key(),
            reaction_key
                .normalized_key()
                .expect("normalized reaction key")
                .as_str(),
        );
        let envelope = build_reaction_envelope(
            &keys,
            &topic_id,
            None,
            &post.object_id,
            reaction_key,
            &reaction_id,
            ObjectStatus::Active,
        )
        .expect("reaction envelope");
        let reaction = parse_reaction(&envelope)
            .expect("parse reaction")
            .expect("reaction doc");
        persist_reaction_doc(docs_sync, &replica, &reaction, &envelope)
            .await
            .expect("persist reaction");
        targets.push(post.object_id.as_str().to_string());
    }
    targets
}

// 1 回の照合が reaction を読む投稿の数には上限がある(1 回の読み出しの最悪の量を抑える)。超えた投稿の
// reaction は、その reaction の docs の event で入る(best effort)。
#[tokio::test]
async fn one_reconcile_reads_the_reactions_of_a_bounded_number_of_posts() {
    use crate::service::replica_window::RANGE_CHECK_REACTION_TARGETS;

    let topic = "kukuri:topic:range-reactions-targets";
    let replica = topic_replica_id(topic);
    let docs_sync = Arc::new(RecordingDocsSync::default());
    let blob_service = Arc::new(MemoryBlobService::default());
    let posts = RANGE_CHECK_REACTION_TARGETS + 10;
    let targets = put_posts_with_one_reaction(docs_sync.as_ref(), topic, posts).await;

    let (viewer, viewer_store) = recording_app_with_blobs(docs_sync.clone(), blob_service);
    let hydrated = viewer
        .reconcile_timeline_range(topic, &TimelineScope::Public, None, posts)
        .await
        .expect("reconcile");
    assert_eq!(hydrated, posts);
    let projection_store: &dyn ProjectionStore = viewer_store.as_ref();
    let mut with_reactions = 0usize;
    for target in &targets {
        with_reactions += usize::from(
            !projection_store
                .list_reaction_cache_for_target(&replica, &EnvelopeId::from(target.as_str()))
                .await
                .expect("reaction rows")
                .is_empty(),
        );
    }
    viewer.shutdown().await;
    assert_eq!(with_reactions, RANGE_CHECK_REACTION_TARGETS);
}

// 購読タスクの追いつきも同じ。新しく反映した投稿と、読み直しの合計で、reaction を読む投稿の数に上限がある。
#[tokio::test]
async fn one_catch_up_reads_the_reactions_of_a_bounded_number_of_posts() {
    use crate::service::replica_window::RANGE_CHECK_REACTION_TARGETS;

    let topic = "kukuri:topic:catch-up-reactions-targets";
    let replica = topic_replica_id(topic);
    let docs_sync = Arc::new(RecordingDocsSync::default());
    let posts = RANGE_CHECK_REACTION_TARGETS + 10;
    let targets = put_posts_with_one_reaction(docs_sync.as_ref(), topic, posts).await;
    let (viewer, viewer_store) =
        recording_app_with_blobs(docs_sync.clone(), Arc::new(MemoryBlobService::default()));
    let hydrated = catch_up_replica_window(
        &viewer.services,
        topic,
        &replica,
        DocFetchPolicy::LocalOnly,
        true,
    )
    .await
    .expect("catch up");
    assert_eq!(hydrated, posts);
    let projection_store: &dyn ProjectionStore = viewer_store.as_ref();
    let mut with_reactions = 0usize;
    for target in &targets {
        with_reactions += usize::from(
            !projection_store
                .list_reaction_cache_for_target(&replica, &EnvelopeId::from(target.as_str()))
                .await
                .expect("reaction rows")
                .is_empty(),
        );
    }
    viewer.shutdown().await;
    assert_eq!(with_reactions, RANGE_CHECK_REACTION_TARGETS);
}
