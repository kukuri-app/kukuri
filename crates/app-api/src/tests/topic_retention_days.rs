//! #1909 AC-3: 前日より前の日は、保持の期間の内を新しい日から古い日へ 1 日ずつ最後まで読む。3 ページを超える日の取り込みを
//! 1 回で止めて端末を作り直しても、続きの位置から再開して全件がそろう。期間の端と bucket の境界の投稿を取り違えない。期間を
//! 延ばすと、新しく期間に入った日も読む。無関係な履歴・公開スレッド・topic を 10 倍にしても、1 回の読み取りの数と取り込みの
//! 件数は変わらない。

use super::public_channels::device;
use super::sync::{CountingDocsSync, put_post_at};
use super::*;
use crate::topic_retention::RetentionState;
use kukuri_docs_sync::{BucketReplica, BucketScope, MemoryDocsSync, TimeBucket};
use kukuri_store::TopicRetentionStore;

const TOPIC: &str = "kukuri:topic:retention-days";
const OTHER_TOPIC: &str = "kukuri:topic:retention-days-other";

fn today() -> u64 {
    TimeBucket::from_unix_seconds(Utc::now().timestamp())
        .expect("today")
        .index()
}

/// 保持する topic の bucket の replica と、その日の最初の時刻(秒)。
fn day(bucket: u64) -> (ReplicaId, i64) {
    let replica = BucketReplica::new(
        BucketScope::Topic {
            topic_id: TOPIC.into(),
        },
        TimeBucket::from_index(bucket).expect("bucket"),
    )
    .expect("replica")
    .replica_id();
    (replica, i64::try_from(bucket * 86_400).expect("seconds"))
}

/// `store` を保存先にし、`peer` を provider として読んで `TOPIC` を保持する端末。取り込みの task は止め、手で回す。
async fn retaining(store: Arc<MemoryStore>, peer: Arc<dyn DocsSync>) -> AppService {
    let transport = Arc::new(FakeTransport::new("k", FakeNetwork::default()));
    let app = AppService::from_handles(
        ServiceHandles::new(
            store.clone(),
            store.clone(),
            transport.clone(),
            transport,
            Arc::new(CountingDocsSync::reading_with(
                MemoryDocsSync::default(),
                peer,
            )),
            Arc::new(MemoryBlobService::default()),
            generate_keys(),
        )
        .with_topic_retention(store, std::env::temp_dir()),
    );
    app.switch_writer(1);
    app.set_topic_retention(TOPIC, true)
        .await
        .expect("retain the topic");
    app.shutdown_topic_retention().await;
    app
}

/// 保持する topic の `bucket` の日の、topic のタイムラインの索引を読み終えた最後の entry の時刻(秒)。
async fn timeline_position(store: &MemoryStore, bucket: u64) -> Option<i64> {
    store
        .topic_retention_cursors(TOPIC, bucket)
        .await
        .expect("cursors")
        .into_iter()
        .find(|cursor| cursor.index_prefix == "indexes/timeline/" && cursor.bucket == bucket)
        .map(|cursor| cursor.created_at)
}

/// 続きが無くなるまで進める。
async fn catch_up(app: &AppService) {
    let mut state = RetentionState::default();
    let mut steps = 0;
    while app
        .step_topic_retention(TOPIC, &mut state)
        .await
        .expect("step")
    {
        steps += 1;
        assert!(steps < 64, "retention catches up");
    }
}

/// `posts` のうち、取り込んだ行がある数。
async fn retained(store: &MemoryStore, posts: &[EnvelopeId]) -> usize {
    let mut count = 0;
    for post in posts {
        if store
            .get_object_projection(post)
            .await
            .expect("projection")
            .is_some()
        {
            count += 1;
        }
    }
    count
}

#[tokio::test]
async fn older_days_are_read_newest_first_and_resume_after_a_restart() {
    let a_docs = MemoryDocsSync::with_docs_author("01".repeat(32));
    let keys = generate_keys();
    let topic = TopicId::new(TOPIC);
    let today = today();
    // 期間 5 日の内で前日より前の日は、2・3・4 日前。2 日前に 3 ページ(1 回 32 件)を超える 100 件、期間の端の 4 日前に
    // その日の最初と最後の秒の投稿、期間の外の 5 日前に最後の秒の投稿を置く。
    let (replica, newer_start) = day(today - 2);
    let mut newer = Vec::new();
    for index in 0..100 {
        let post = put_post_at(
            &a_docs,
            &replica,
            &keys,
            &topic,
            newer_start + index,
            &format!("新しい日 {index}"),
            None,
        )
        .await;
        newer.push(post.object_id.clone());
    }
    let (replica, start) = day(today - 4);
    let mut edge = Vec::new();
    for (at, text) in [
        (start, "期間の端の日の最初の秒"),
        (start + 86_399, "期間の端の日の最後の秒"),
    ] {
        let post = put_post_at(&a_docs, &replica, &keys, &topic, at, text, None).await;
        edge.push(post.object_id.clone());
    }
    let (replica, start) = day(today - 5);
    let outside = put_post_at(
        &a_docs,
        &replica,
        &keys,
        &topic,
        start + 86_399,
        "期間の外の日の最後の秒",
        None,
    )
    .await
    .object_id
    .clone();
    let store = Arc::new(MemoryStore::default());
    store
        .set_topic_retention_limits(5, 1 << 30)
        .await
        .expect("limits");

    // 1 回(32 件)で止める。新しい日の続きが残り、古い日はまだ読んでいない。
    let k = retaining(store.clone(), Arc::new(a_docs.clone())).await;
    assert!(
        k.step_topic_retention(TOPIC, &mut RetentionState::default())
            .await
            .expect("first step")
    );
    assert_eq!(retained(&store, &newer).await, 32);
    assert_eq!(
        timeline_position(&store, today - 2).await,
        Some(newer_start + 31)
    );
    assert_eq!(retained(&store, &edge).await, 0);
    k.shutdown().await;

    // 同じ保存先で作り直した端末は、最初から読み直さずに続きの位置から再開し、全件がそろう。期間の外の日は読まない。
    let restarted = retaining(store.clone(), Arc::new(a_docs)).await;
    assert!(
        restarted
            .step_topic_retention(TOPIC, &mut RetentionState::default())
            .await
            .expect("first step after the restart")
    );
    assert_eq!(retained(&store, &newer).await, 64);
    assert_eq!(
        timeline_position(&store, today - 2).await,
        Some(newer_start + 63)
    );
    catch_up(&restarted).await;
    assert_eq!(retained(&store, &newer).await, 100);
    assert_eq!(retained(&store, &edge).await, 2);
    assert_eq!(retained(&store, std::slice::from_ref(&outside)).await, 0);

    // 期間を延ばすと、新しく期間に入った日も読む。
    restarted
        .set_topic_retention_limits(7, 1 << 30)
        .await
        .expect("a longer period");
    catch_up(&restarted).await;
    assert_eq!(retained(&store, &[outside]).await, 1);
    restarted.shutdown().await;
}

/// 1 回の取り込みで provider に出した query の数と、返った record の数、key の一覧の数と、取り込んだ投稿の数。
async fn first_step(unrelated: usize) -> (usize, usize, usize, usize) {
    let a_docs = MemoryDocsSync::with_docs_author("01".repeat(32));
    let a = device(
        "a",
        &FakeNetwork::default(),
        a_docs.clone(),
        MemoryDocsSync::default(),
        Arc::new(MemoryBlobService::default()),
    );
    let mut posts = Vec::new();
    for index in 0..3 {
        let post = a
            .create_post(TOPIC, &format!("保持する投稿 {index}"), None)
            .await
            .expect("post");
        posts.push(EnvelopeId::from(post.as_str()));
    }
    // 無関係なもの: 別の topic の投稿と公開スレッド、期間(既定の 30 日)の外の日の、保持する topic の投稿。
    let keys = generate_keys();
    let (old, start) = day(today() - 40);
    for index in 0..unrelated {
        a.create_post(OTHER_TOPIC, &format!("別の topic の投稿 {index}"), None)
            .await
            .expect("other topic post");
        a.create_public_channel(
            OTHER_TOPIC,
            &format!("別の topic のスレッド {index}"),
            "",
            Vec::new(),
            Vec::new(),
        )
        .await
        .expect("other topic thread");
        put_post_at(
            &a_docs,
            &old,
            &keys,
            &TopicId::new(TOPIC),
            start + i64::try_from(index).expect("index"),
            &format!("期間の外の投稿 {index}"),
            None,
        )
        .await;
    }
    let provider = CountingDocsSync::reading_with(a_docs, Arc::new(MemoryDocsSync::default()));
    let store = Arc::new(MemoryStore::default());
    let k = retaining(store.clone(), Arc::new(provider.clone())).await;
    provider.clear_queries().await;
    provider.reset_records_returned();
    let key_queries = provider.key_queries();
    k.step_topic_retention(TOPIC, &mut RetentionState::default())
        .await
        .expect("step");
    let reads = (
        provider.queries().await.len(),
        provider.records_returned(),
        provider.key_queries() - key_queries,
    );
    let retained = retained(&store, &posts).await;
    k.shutdown().await;
    (reads.0, reads.1, reads.2, retained)
}

#[tokio::test]
async fn unrelated_history_threads_and_topics_do_not_change_the_reads_of_a_step() {
    let base = first_step(1).await;
    assert_eq!(base.3, 3, "the step retains the topic's posts");
    assert_eq!(first_step(10).await, base);
}
