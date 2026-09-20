//! #1239: 照合が、query 数の上限で止まった索引の読み出しを読み継ぐことと、thread のページの範囲の照合。
//!
//! 独立監査(PR #1247、対象 `7a6c3bd1`)の non-blocker の再現を恒久化したもの。購読タスクの全件走査を外す前に
//! 直しておく必要がある(全件走査が無くなると、ここで届かない投稿へは二度と届かない)。

use super::range_reconcile::{BASE_TIME, NoScanDocsSync, cursor_at, is_projected, put_post_at};
use super::range_reconcile_access::{RecordingDocsSync, recording_app_with_blobs};
use super::*;

/// `index_prefix` の下の `second` の秒に、索引の形をしていない key を `count` 件置く。
/// `~` は 16 進の文字より後ろに並ぶので、その秒の有効な entry より新しい側に来る。
async fn put_malformed_index_keys(
    docs_sync: &dyn DocsSync,
    replica: &ReplicaId,
    index_prefix: &str,
    second: i64,
    count: usize,
) {
    for index in 0..count {
        docs_sync
            .apply_doc_op(
                replica,
                DocOp::SetJson {
                    key: format!("{index_prefix}{second:020}-~junk-{index:03}"),
                    value: serde_json::json!({}),
                },
            )
            .await
            .expect("write a malformed index key");
    }
}

// 形の違う key で埋まった秒が続くと、索引の 1 回の読み出しは query 数の上限で止まり、1 件も返せないことがある。
// 照合はそれを「索引は尽きた」とみなさず、読み進めた位置を台帳に残して、次の照合がそこから続ける。
// 「尽きた」とみなすと、その範囲は欠け無しとして 30 秒の間隔に入り、何度取得しても古い投稿へ届かない。
#[tokio::test]
async fn a_walk_stopped_at_the_query_cap_is_continued_by_the_next_reconcile() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let docs_sync = Arc::new(NoScanDocsSync::default());
    let topic = TopicId::new("kukuri:topic:range-walk-cap");
    let replica = topic_replica_id(topic.as_str());
    let author_keys = generate_keys();
    let newer_at = BASE_TIME + 100_000;
    let older = put_post_at(
        docs_sync.as_ref(),
        &replica,
        &author_keys,
        &topic,
        BASE_TIME,
        "older",
        None,
    )
    .await;
    let newer = put_post_at(
        docs_sync.as_ref(),
        &replica,
        &author_keys,
        &topic,
        newer_at,
        "newer",
        None,
    )
    .await;
    // 2 件のあいだの 40 個の「10 秒の範囲」それぞれの 1 秒に、形の違う key を読み出しの余裕を超えて置く。
    for window in 1..=40_i64 {
        put_malformed_index_keys(
            docs_sync.as_ref(),
            &replica,
            "indexes/timeline/",
            newer_at - (window * 10 + 3),
            60,
        )
        .await;
    }
    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        transport.clone(),
        transport,
        docs_sync.clone(),
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    );

    let cursor = cursor_at(&newer);
    let mut checks = 0usize;
    while !is_projected(store.as_ref(), &older).await {
        checks += 1;
        assert!(
            checks <= 12,
            "the reconcile must reach the older post within a bounded number of checks"
        );
        app.reconcile_timeline_range(topic.as_str(), &TimelineScope::Public, Some(&cursor), 20)
            .await
            .expect("reconcile");
        // 次の照合までの間隔が過ぎた状態にする(読み進めた位置は保つ)。
        app.services.range_checks.expire_all_for_test().await;
    }
    app.shutdown().await;
    assert!(
        checks >= 2,
        "the fixture must make the first walk stop at the query cap: {checks} checks"
    );
}

// 索引の新しい側が形の違う key で埋まっていると、先頭の範囲の読み出しは何十回もの query になる。
// 「索引から 1 件も読めなかった」だけを見て間隔を空けずにいると、取得のたびにそれを繰り返す。
// 間隔を空けないのは、読み出しが key の一覧 1 回で済んだとき(索引がまだ空の replica)だけ。
#[tokio::test]
async fn a_head_range_buried_under_malformed_keys_is_not_read_on_every_listing() {
    let topic = "kukuri:topic:range-walk-buried-head";
    let replica = topic_replica_id(topic);
    let docs_sync = Arc::new(RecordingDocsSync::default());
    let (app, _store) =
        recording_app_with_blobs(docs_sync.clone(), Arc::new(MemoryBlobService::default()));
    docs_sync
        .open_replica(&replica)
        .await
        .expect("open replica");
    for index in 0..100usize {
        docs_sync
            .apply_doc_op(
                &replica,
                DocOp::SetJson {
                    key: format!("indexes/timeline/~junk-{index:03}"),
                    value: serde_json::json!({}),
                },
            )
            .await
            .expect("write a malformed index key");
    }

    docs_sync.reads.lock().await.clear();
    let first = app
        .reconcile_timeline_range(topic, &TimelineScope::Public, None, 20)
        .await
        .expect("first reconcile");
    let first_reads = docs_sync.reads.lock().await.len();
    docs_sync.reads.lock().await.clear();
    let second = app
        .reconcile_timeline_range(topic, &TimelineScope::Public, None, 20)
        .await
        .expect("second reconcile");
    let second_reads = docs_sync.reads.lock().await.len();
    app.shutdown().await;
    assert_eq!((first, second), (0, 0));
    assert!(
        first_reads > 1,
        "the fixture must make the head read fall back to the walk: {first_reads} reads"
    );
    assert_eq!(
        second_reads, 0,
        "a head range that took many queries must wait for the interval"
    );
}

// thread も、取得するページの範囲だけを照合する。1 ページを超える thread は、利用者が先のページへ進んだときに、
// その範囲が照合される。読む量は thread の返信の総数に依存せず、replica も走査しない。
#[tokio::test]
async fn a_long_thread_is_reconciled_page_by_page() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let docs_sync = Arc::new(NoScanDocsSync::default());
    let topic = TopicId::new("kukuri:topic:range-walk-long-thread");
    let replica = topic_replica_id(topic.as_str());
    let author_keys = generate_keys();
    let root = put_post_at(
        docs_sync.as_ref(),
        &replica,
        &author_keys,
        &topic,
        BASE_TIME,
        "root",
        None,
    )
    .await;
    let mut expected = vec![root.object_id.as_str().to_string()];
    for index in 0..120_i64 {
        let reply = put_post_at(
            docs_sync.as_ref(),
            &replica,
            &author_keys,
            &topic,
            BASE_TIME + 1 + index,
            format!("reply {index}").as_str(),
            Some(&root.envelope),
        )
        .await;
        expected.push(reply.object_id.as_str().to_string());
    }
    // thread と関係の無い投稿(thread の索引には入らない)。
    put_post_at(
        docs_sync.as_ref(),
        &replica,
        &author_keys,
        &topic,
        BASE_TIME + 500,
        "noise",
        None,
    )
    .await;
    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        transport.clone(),
        transport,
        docs_sync.clone(),
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    );

    let mut listed = Vec::new();
    let mut cursor = None;
    let mut pages = 0usize;
    loop {
        pages += 1;
        assert!(pages <= 5, "the thread must end");
        let page = app
            .list_thread(topic.as_str(), root.object_id.as_str(), cursor, 50)
            .await
            .expect("thread page");
        assert!(page.items.len() <= 50);
        listed.extend(page.items.iter().map(|item| item.object_id.clone()));
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    app.shutdown().await;
    assert_eq!(pages, 3, "121 rows in pages of 50");
    assert_eq!(listed, expected, "every reply is listed once, oldest first");
}

// thread の索引の古い側に、形の違う key を大量に置かれても、返信の照合は止まらない。
// `!` は数字より前に並ぶので、古い順の読み出しでは必ず先頭に来る。
#[tokio::test]
async fn malformed_keys_at_the_head_of_a_thread_index_do_not_hide_the_replies() {
    let store = Arc::new(MemoryStore::default());
    let transport = Arc::new(StaticTransport::new(PeerSnapshot::default()));
    let docs_sync = Arc::new(NoScanDocsSync::default());
    let topic = TopicId::new("kukuri:topic:range-walk-thread-junk");
    let replica = topic_replica_id(topic.as_str());
    let author_keys = generate_keys();
    let root = put_post_at(
        docs_sync.as_ref(),
        &replica,
        &author_keys,
        &topic,
        BASE_TIME,
        "root",
        None,
    )
    .await;
    let mut replies = Vec::new();
    for index in 0..3_i64 {
        replies.push(
            put_post_at(
                docs_sync.as_ref(),
                &replica,
                &author_keys,
                &topic,
                BASE_TIME + 1 + index,
                format!("reply {index}").as_str(),
                Some(&root.envelope),
            )
            .await,
        );
    }
    let index_prefix = stable_key("indexes/thread", &format!("{}/", root.object_id.as_str()));
    for index in 0..600usize {
        docs_sync
            .apply_doc_op(
                &replica,
                DocOp::SetJson {
                    key: format!("{index_prefix}!junk-{index:03}"),
                    value: serde_json::json!({}),
                },
            )
            .await
            .expect("write a malformed thread index key");
    }
    // 返信と同じ秒にも、余裕を超える数の形の違う key を置く。
    put_malformed_index_keys(
        docs_sync.as_ref(),
        &replica,
        index_prefix.as_str(),
        BASE_TIME + 2,
        60,
    )
    .await;
    let app = app_service_from_dependencies(
        store.clone(),
        store.clone(),
        transport.clone(),
        transport,
        docs_sync.clone(),
        Arc::new(MemoryBlobService::default()),
        generate_keys(),
    );

    let hydrated = app
        .reconcile_thread(topic.as_str(), &root.object_id, None, 20)
        .await
        .expect("reconcile thread");
    app.shutdown().await;
    assert_eq!(hydrated, 4, "the root and the three replies");
    for reply in &replies {
        assert!(is_projected(store.as_ref(), reply).await);
    }
}

// thread の最後のページ(索引が尽きる範囲)で、符号つき 64 bit に収まらない時刻の範囲に形の違う key を
// 置かれても、照合はその範囲を読まずに「確かめ終えた」とする(独立監査 PR #1265 の再現 test を恒久化)。
// 続きの起点が `i64::MAX` に張り付くと、照合のたびに query 数の上限まで読み続ける。
#[tokio::test]
async fn malformed_keys_beyond_the_valid_time_range_do_not_keep_the_thread_tail_unfinished() {
    let topic = "kukuri:topic:audit-t4a-thread-tail";
    let replica = topic_replica_id(topic);
    let docs_sync = Arc::new(RecordingDocsSync::default());
    let (app, _store) =
        recording_app_with_blobs(docs_sync.clone(), Arc::new(MemoryBlobService::default()));
    let root = app
        .create_post(topic, "root", None)
        .await
        .expect("create root");
    let index_prefix = stable_key("indexes/thread", &format!("{root}/"));
    for time in [
        "09500000000000000001",
        "09700000000000000002",
        "09900000000000000003",
    ] {
        for index in 0..80usize {
            docs_sync
                .apply_doc_op(
                    &replica,
                    DocOp::SetJson {
                        key: format!("{index_prefix}{time}-~junk-{index:03}"),
                        value: serde_json::json!({}),
                    },
                )
                .await
                .expect("write junk");
        }
    }
    let root_id = EnvelopeId::from(root.as_str());
    let mut reads = Vec::new();
    for _ in 0..8usize {
        docs_sync.reads.lock().await.clear();
        app.reconcile_thread(topic, &root_id, None, 30)
            .await
            .expect("reconcile thread");
        reads.push(docs_sync.reads.lock().await.len());
        app.services.range_checks.expire_all_for_test().await;
    }
    app.shutdown().await;
    println!("audit_t4a: 照合ごとの docs の読み出しの回数 = {reads:?}");
    assert!(
        reads.iter().skip(2).any(|count| *count < 100),
        "the tail of the thread must settle instead of walking to the query cap every time: {reads:?}"
    );
}
