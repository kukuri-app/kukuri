//! #1239(AC-4、TR-6): 遡った範囲の本体が手元に無い(提供する peer がいない)とき、取得はその数を返して画面に示させ、
//! 操作を止めない。反映できない entry が続いてページが尽きても、読み進めた位置を続きの位置として返し、その先へ進める。

use super::range_reconcile::{cursor_at, project, put_post_at, range_fixture};
use super::range_reconcile_faults::put_dangling_index_entry;
use super::*;

// 遡ったページの範囲にある、本体が手元に無い投稿の数を返す。台帳の間隔の内で照合しなかった取得も、前回の数を返す。
#[tokio::test]
async fn an_older_page_reports_the_posts_whose_body_is_not_here() {
    let posts = 20usize;
    let fixture = range_fixture("unavailable-count", posts, |index| index >= posts - 5).await;
    let replica = topic_replica_id(fixture.topic.as_str());
    let oldest_projected = &fixture.posts[posts - 5];
    for index in 0..7usize {
        put_dangling_index_entry(
            fixture.docs_sync.as_ref(),
            &replica,
            oldest_projected.created_at,
            format!("{index:064x}").as_str(),
        )
        .await;
    }

    let mut counts = Vec::new();
    for _ in 0..2 {
        let page = fixture
            .app
            .list_timeline(
                fixture.topic.as_str(),
                Some(cursor_at(oldest_projected)),
                10,
            )
            .await
            .expect("older page");
        assert_eq!(page.items.len(), 10, "the reachable posts are listed");
        counts.push(page.unavailable_count);
    }
    fixture.app.shutdown().await;
    assert_eq!(
        counts,
        vec![7, 7],
        "the second listing is within the interval"
    );
}

// 反映できない entry が 1 回の照合の上限(200 件)を超えて続いても、取得は読み進めた位置を続きの位置として返す。
// 画面は続きの位置をたどるだけで、その先の投稿へ進める(以前は、続きの位置が無く、画面から先へ進めなかった)。
#[tokio::test]
async fn following_the_next_cursor_passes_over_a_long_run_of_unavailable_posts() {
    let posts = 12usize;
    let fixture = range_fixture("unavailable-long-run", posts, |index| index >= posts - 2).await;
    let replica = topic_replica_id(fixture.topic.as_str());
    let oldest_projected = &fixture.posts[posts - 2];
    for index in 0..450usize {
        put_dangling_index_entry(
            fixture.docs_sync.as_ref(),
            &replica,
            oldest_projected.created_at,
            format!("{index:064x}").as_str(),
        )
        .await;
    }

    let first = fixture
        .app
        .list_timeline(
            fixture.topic.as_str(),
            Some(cursor_at(oldest_projected)),
            10,
        )
        .await
        .expect("first older page");
    assert!(first.items.is_empty(), "the first check reads only the run");
    assert!(first.unavailable_count > 0);
    assert!(
        first.next_cursor.is_some(),
        "the listing lets the user continue past the run"
    );

    let mut listed = std::collections::BTreeSet::new();
    let mut cursor = first.next_cursor;
    let mut pages = 1;
    while let Some(next) = cursor {
        pages += 1;
        assert!(pages <= 10, "every page advances");
        let page = fixture
            .app
            .list_timeline(fixture.topic.as_str(), Some(next), 10)
            .await
            .expect("following page");
        listed.extend(page.items.iter().map(|item| item.content.clone()));
        cursor = page.next_cursor;
    }
    fixture.app.shutdown().await;
    assert_eq!(
        listed,
        (0..(posts - 2))
            .map(|index| format!("post {index}"))
            .collect::<std::collections::BTreeSet<_>>(),
        "the older posts beyond the run are reached"
    );
}

// thread のページも、本体が手元に無い返信の数を返す。
#[tokio::test]
async fn a_thread_page_reports_the_replies_whose_body_is_not_here() {
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
    let topic = TopicId::new("kukuri:topic:unavailable-thread");
    let replica = topic_replica_id(topic.as_str());
    let keys = generate_keys();
    let root = put_post_at(
        docs_sync.as_ref(),
        &replica,
        &keys,
        &topic,
        1_700_000_000,
        "the root",
        None,
    )
    .await;
    let reply = put_post_at(
        docs_sync.as_ref(),
        &replica,
        &keys,
        &topic,
        1_700_000_010,
        "a reply",
        Some(&root.envelope),
    )
    .await;
    project(store.as_ref(), &root, &replica).await;
    project(store.as_ref(), &reply, &replica).await;
    for index in 0..3usize {
        let id = format!("{index:064x}");
        docs_sync
            .apply_doc_op(
                &replica,
                DocOp::SetJson {
                    key: stable_key(
                        "indexes/thread",
                        &format!(
                            "{}/{:020}-{id}/{id}",
                            root.object_id.as_str(),
                            1_700_000_020_i64 + index as i64
                        ),
                    ),
                    value: serde_json::json!({}),
                },
            )
            .await
            .expect("write a thread index entry");
    }

    let page = app
        .list_thread(topic.as_str(), root.object_id.as_str(), None, 20)
        .await
        .expect("thread page");
    app.shutdown().await;
    assert!(
        page.items
            .iter()
            .any(|item| item.object_id == reply.object_id.as_str())
    );
    assert_eq!(page.unavailable_count, 3);
}

/// 反映できない entry を `run` 件置いた範囲を、続きの位置をたどって最後まで読み、並んだ本文を返す。
async fn list_past_a_run(name: &str, run: usize) -> Vec<String> {
    let fixture = range_fixture(name, 5, |index| index == 0 || index == 4).await;
    let replica = topic_replica_id(fixture.topic.as_str());
    for index in 0..run {
        put_dangling_index_entry(
            fixture.docs_sync.as_ref(),
            &replica,
            fixture.posts[2].created_at,
            format!("{index:064x}").as_str(),
        )
        .await;
    }
    let mut listed = Vec::new();
    let mut cursor = Some(cursor_at(&fixture.posts[4]));
    let mut pages = 0;
    while let Some(next) = cursor {
        pages += 1;
        assert!(pages <= 10, "every page advances");
        let page = fixture
            .app
            .list_timeline(fixture.topic.as_str(), Some(next), 10)
            .await
            .expect("page");
        listed.extend(page.items.iter().map(|item| item.content.clone()));
        cursor = page.next_cursor;
    }
    fixture.app.shutdown().await;
    listed
}

// 読み進めた位置を続きの位置にするとき、その位置より先(古い側)の行はこのページに入れない(独立監査 B1)。
// 入れると、次のページがその行をもう一度返し、そのあいだに届いた投稿が後ろに並んで、画面の並びが崩れる。
// 続きの位置の行そのもの(照合が最後に読んだ entry が本体のある投稿のとき)は、このページに残す。次のページは
// その位置を含まないので、外すと欠ける(delta 監査 N1。197 件のときは、照合の 200 件目が post 1 になる)。
#[tokio::test]
async fn continuing_past_unavailable_posts_keeps_the_pages_in_order_without_overlap() {
    for run in [300usize, 197] {
        assert_eq!(
            list_past_a_run(format!("unavailable-order-{run}").as_str(), run).await,
            vec!["post 3", "post 2", "post 1", "post 0"],
            "run={run}: the pages are in order, without overlap or gap"
        );
    }
}

// thread(古い順)でも、読み進めた位置より先(新しい側)の行はこのページに入れず、並びが崩れない(独立監査 B1)。
// 続きの位置の行そのものは残す(delta 監査 N1)。
#[tokio::test]
async fn thread_pages_past_unavailable_replies_stay_in_order_without_overlap() {
    for run in [300usize, 197] {
        assert_eq!(
            list_thread_past_a_run(format!("unavailable-thread-order-{run}").as_str(), run).await,
            vec!["reply 1", "reply 2", "reply 3", "reply 4"],
            "run={run}: the thread pages are in order, without overlap or gap"
        );
    }
}

/// thread の返信のあいだに反映できない entry を `run` 件置き、続きの位置をたどって最後まで読んだ本文を返す。
async fn list_thread_past_a_run(name: &str, run: usize) -> Vec<String> {
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
    let topic = TopicId::new(format!("kukuri:topic:{name}").as_str());
    let replica = topic_replica_id(topic.as_str());
    let keys = generate_keys();
    let root = put_post_at(
        docs_sync.as_ref(),
        &replica,
        &keys,
        &topic,
        1_700_000_000,
        "the root",
        None,
    )
    .await;
    let mut replies = Vec::new();
    for index in 0..5i64 {
        replies.push(
            put_post_at(
                docs_sync.as_ref(),
                &replica,
                &keys,
                &topic,
                1_700_000_010 + index * 10,
                format!("reply {index}").as_str(),
                Some(&root.envelope),
            )
            .await,
        );
    }
    for projected in [&root, &replies[0], &replies[4]] {
        project(store.as_ref(), projected, &replica).await;
    }
    for index in 0..run {
        let id = format!("{index:064x}");
        docs_sync
            .apply_doc_op(
                &replica,
                DocOp::SetJson {
                    key: stable_key(
                        "indexes/thread",
                        &format!(
                            "{}/{:020}-{id}/{id}",
                            root.object_id.as_str(),
                            replies[2].created_at
                        ),
                    ),
                    value: serde_json::json!({}),
                },
            )
            .await
            .expect("write a thread index entry");
    }

    let mut listed = Vec::new();
    let mut cursor = Some(cursor_at(&replies[0]));
    let mut pages = 0;
    while let Some(next) = cursor {
        pages += 1;
        assert!(pages <= 10, "every page advances");
        let page = app
            .list_thread(topic.as_str(), root.object_id.as_str(), Some(next), 10)
            .await
            .expect("thread page");
        listed.extend(page.items.iter().map(|item| item.content.clone()));
        cursor = page.next_cursor;
    }
    app.shutdown().await;
    listed
}

// thread の root 行は、続きの位置より先にあってもページから外さない(delta 監査の non-blocker)。時計のずれで root より古い
// 時刻の返信の entry が続くと、照合の読み進めた位置が root より前になる。
#[tokio::test]
async fn the_thread_root_stays_on_the_first_page_past_unavailable_replies() {
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
    let topic = TopicId::new("kukuri:topic:unavailable-thread-root");
    let replica = topic_replica_id(topic.as_str());
    let root = put_post_at(
        docs_sync.as_ref(),
        &replica,
        &generate_keys(),
        &topic,
        1_700_000_000,
        "the root",
        None,
    )
    .await;
    project(store.as_ref(), &root, &replica).await;
    for index in 0..300usize {
        let id = format!("{index:064x}");
        docs_sync
            .apply_doc_op(
                &replica,
                DocOp::SetJson {
                    key: stable_key(
                        "indexes/thread",
                        &format!(
                            "{}/{:020}-{id}/{id}",
                            root.object_id.as_str(),
                            1_699_999_000_i64
                        ),
                    ),
                    value: serde_json::json!({}),
                },
            )
            .await
            .expect("write a thread index entry");
    }

    let page = app
        .list_thread(topic.as_str(), root.object_id.as_str(), None, 10)
        .await
        .expect("thread page");
    app.shutdown().await;
    assert!(
        page.items
            .iter()
            .any(|item| item.object_id == root.object_id.as_str()),
        "the root is listed"
    );
    assert!(
        page.next_cursor.is_some(),
        "the listing continues past the run"
    );
}
