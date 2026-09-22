//! #1239(ADR 0052 §5): 1 回の取得が読む projection のページ数に上限があり、`next_cursor` が行を飛ばさない。

use super::range_reconcile::{BASE_TIME, project, put_post_at};
use super::*;
use crate::service::projection_support::{
    HIDDEN_AUTHOR_SKIP_PAGES, filtered_thread_page, filtered_timeline_page,
};

struct Rows {
    store: Arc<MemoryStore>,
    topic: TopicId,
    /// 古い順。
    ids: Vec<String>,
}

/// `authors[index % authors.len()]` が書いた投稿を `count` 件、projection へ入れる。
async fn rows(name: &str, count: usize, authors: &[KukuriKeys]) -> Rows {
    let store = Arc::new(MemoryStore::default());
    let docs_sync = MemoryDocsSync::default();
    let topic = TopicId::new(format!("kukuri:topic:page-bounds-{name}"));
    let replica = topic_replica_id(topic.as_str());
    let mut ids = Vec::new();
    for index in 0..count {
        let post = put_post_at(
            &docs_sync,
            &replica,
            &authors[index % authors.len()],
            &topic,
            BASE_TIME + index as i64,
            format!("post {index}").as_str(),
            None,
        )
        .await;
        project(store.as_ref(), &post, &replica).await;
        ids.push(post.object_id.as_str().to_string());
    }
    Rows { store, topic, ids }
}

// `limit` が内部のページの大きさ(20)より小さいとき、以前は `next_cursor` がページの末尾を指していて、
// 同じページの残りの行を次の取得が飛ばしていた。ページを継いで、全行を 1 回ずつ読めること。
#[tokio::test]
async fn next_cursor_points_at_the_last_returned_row() {
    let author = generate_keys();
    let fixture = rows("cursor", 47, std::slice::from_ref(&author)).await;
    let mut listed = Vec::new();
    let mut cursor = None;
    loop {
        let page = filtered_timeline_page(
            fixture.store.as_ref(),
            fixture.topic.as_str(),
            cursor,
            5,
            PUBLIC_CHANNEL_ID,
            &BTreeSet::new(),
        )
        .await
        .expect("page");
        assert!(page.items.len() <= 5);
        listed.extend(
            page.items
                .iter()
                .map(|row| row.object_id.as_str().to_string()),
        );
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    let mut expected = fixture.ids.clone();
    expected.reverse();
    assert_eq!(listed, expected, "every row is listed once, newest first");
}

// 非表示の著者の投稿が続く範囲では、1 回の取得が読むページ数に上限がある。届かなかったときは、集まった分と
// 読み進めた位置を返し、続きの取得で先の投稿へ届く。
#[tokio::test]
async fn hidden_author_rows_are_skipped_with_a_bounded_number_of_pages() {
    let visible = generate_keys();
    let hidden = generate_keys();
    // 古い側の 3 件だけが表示できる著者。新しい側の 300 件は非表示の著者。
    let store = Arc::new(MemoryStore::default());
    let docs_sync = MemoryDocsSync::default();
    let topic = TopicId::new("kukuri:topic:page-bounds-hidden");
    let replica = topic_replica_id(topic.as_str());
    let mut visible_ids = Vec::new();
    for index in 0..303usize {
        let keys = if index < 3 { &visible } else { &hidden };
        let post = put_post_at(
            &docs_sync,
            &replica,
            keys,
            &topic,
            BASE_TIME + index as i64,
            format!("post {index}").as_str(),
            None,
        )
        .await;
        project(store.as_ref(), &post, &replica).await;
        if index < 3 {
            visible_ids.push(post.object_id.as_str().to_string());
        }
    }
    let hidden_authors = BTreeSet::from([hidden.public_key_hex()]);

    let first = filtered_timeline_page(
        store.as_ref(),
        topic.as_str(),
        None,
        10,
        PUBLIC_CHANNEL_ID,
        &hidden_authors,
    )
    .await
    .expect("first page");
    assert!(first.items.is_empty(), "the newest side is all hidden");
    let position = first
        .next_cursor
        .clone()
        .expect("the page reports where it stopped");
    // 1 回の取得が読むのは、上限のページ数 × 20 行まで。
    assert_eq!(
        position.created_at,
        BASE_TIME + 302 - (HIDDEN_AUTHOR_SKIP_PAGES as i64 * 20 - 1),
        "the first call stops after the page cap"
    );

    let mut listed = Vec::new();
    let mut cursor = first.next_cursor;
    let mut calls = 1usize;
    while let Some(current) = cursor {
        calls += 1;
        assert!(calls <= 10, "the listing must reach the end");
        let page = filtered_timeline_page(
            store.as_ref(),
            topic.as_str(),
            Some(current),
            10,
            PUBLIC_CHANNEL_ID,
            &hidden_authors,
        )
        .await
        .expect("page");
        listed.extend(
            page.items
                .iter()
                .map(|row| row.object_id.as_str().to_string()),
        );
        cursor = page.next_cursor;
    }
    visible_ids.reverse();
    assert_eq!(listed, visible_ids);
}

// thread も同じ。非表示の著者の返信が続く範囲では、1 回の取得が読むページ数に上限がある。
#[tokio::test]
async fn hidden_author_replies_are_skipped_with_a_bounded_number_of_pages() {
    let visible = generate_keys();
    let hidden = generate_keys();
    let store = Arc::new(MemoryStore::default());
    let docs_sync = MemoryDocsSync::default();
    let topic = TopicId::new("kukuri:topic:page-bounds-hidden-thread");
    let replica = topic_replica_id(topic.as_str());
    let root = put_post_at(
        &docs_sync, &replica, &visible, &topic, BASE_TIME, "root", None,
    )
    .await;
    project(store.as_ref(), &root, &replica).await;
    // 古い側の 200 件は非表示の著者の返信、最後の 1 件だけが表示できる返信。
    let mut last_visible = None;
    for index in 0..201usize {
        let keys = if index < 200 { &hidden } else { &visible };
        let reply = put_post_at(
            &docs_sync,
            &replica,
            keys,
            &topic,
            BASE_TIME + 1 + index as i64,
            format!("reply {index}").as_str(),
            Some(&root.envelope),
        )
        .await;
        project(store.as_ref(), &reply, &replica).await;
        last_visible = Some(reply.object_id.as_str().to_string());
    }
    let hidden_authors = BTreeSet::from([hidden.public_key_hex()]);

    let first = filtered_thread_page(
        store.as_ref(),
        topic.as_str(),
        &root.object_id,
        None,
        10,
        None,
        &hidden_authors,
    )
    .await
    .expect("first page");
    assert_eq!(first.items.len(), 1, "only the root is visible so far");
    let position = first
        .next_cursor
        .clone()
        .expect("the page reports where it stopped");
    assert_eq!(
        position.created_at,
        BASE_TIME + (HIDDEN_AUTHOR_SKIP_PAGES as i64 * 20 - 1),
        "the first call stops after the page cap"
    );

    let mut listed = Vec::new();
    let mut cursor = first.next_cursor;
    let mut calls = 1usize;
    while let Some(current) = cursor {
        calls += 1;
        assert!(calls <= 10, "the listing must reach the end");
        let page = filtered_thread_page(
            store.as_ref(),
            topic.as_str(),
            &root.object_id,
            Some(current),
            10,
            None,
            &hidden_authors,
        )
        .await
        .expect("page");
        listed.extend(
            page.items
                .iter()
                .map(|row| row.object_id.as_str().to_string()),
        );
        cursor = page.next_cursor;
    }
    assert_eq!(listed, vec![last_visible.expect("visible reply")]);
}
