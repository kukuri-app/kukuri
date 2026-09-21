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
