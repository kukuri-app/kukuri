//! WP-S6 T8: pagination 5 系統(Store timeline / Store thread / ProjectionStore filtered ×2 /
//! DM messages)の複数ページ走査を sqlite/memory で突き合わせる。
//! cursor 走査は「重複なし・欠落なし・順序一致」を parity(assert_eq)で、
//! 具体値は sqlite 実測の生リテラルで固定する。

use super::*;

fn envelope_page_ids(pages: &[Page<KukuriEnvelope>]) -> Vec<Vec<String>> {
    pages
        .iter()
        .map(|page| {
            page.items
                .iter()
                .map(|envelope| envelope.id.as_str().to_string())
                .collect()
        })
        .collect()
}

fn projection_page_ids(pages: &[Page<ObjectProjectionRow>]) -> Vec<Vec<String>> {
    pages
        .iter()
        .map(|page| {
            page.items
                .iter()
                .map(|row| row.object_id.as_str().to_string())
                .collect()
        })
        .collect()
}

fn dm_page_ids(pages: &[Page<DirectMessageMessageRow>]) -> Vec<Vec<String>> {
    pages
        .iter()
        .map(|page| {
            page.items
                .iter()
                .map(|row| row.message_id.clone())
                .collect()
        })
        .collect()
}

async fn walk_store_timeline<S: Store + ProjectionStore>(
    store: &S,
    topic_id: &str,
    limit: usize,
) -> Vec<Page<KukuriEnvelope>> {
    let mut pages = Vec::new();
    let mut cursor: Option<TimelineCursor> = None;
    loop {
        let page = Store::list_topic_timeline(store, topic_id, cursor.clone(), limit)
            .await
            .expect("Store::list_topic_timeline");
        cursor = page.next_cursor.clone();
        pages.push(page);
        if cursor.is_none() {
            return pages;
        }
        assert!(pages.len() < MAX_PAGES, "store timeline walk did not end");
    }
}

async fn walk_store_thread<S: Store + ProjectionStore>(
    store: &S,
    topic_id: &str,
    root: &EnvelopeId,
    limit: usize,
) -> Vec<Page<KukuriEnvelope>> {
    let mut pages = Vec::new();
    let mut cursor: Option<TimelineCursor> = None;
    loop {
        let page = Store::list_thread(store, topic_id, root, cursor.clone(), limit)
            .await
            .expect("Store::list_thread");
        cursor = page.next_cursor.clone();
        pages.push(page);
        if cursor.is_none() {
            return pages;
        }
        assert!(pages.len() < MAX_PAGES, "store thread walk did not end");
    }
}

async fn walk_projection_timeline<S: Store + ProjectionStore>(
    store: &S,
    topic_id: &str,
    limit: usize,
) -> Vec<Page<ObjectProjectionRow>> {
    let mut pages = Vec::new();
    let mut cursor: Option<TimelineCursor> = None;
    loop {
        let page =
            ObjectProjectionStore::list_topic_timeline(store, topic_id, cursor.clone(), limit)
                .await
                .expect("ObjectProjectionStore::list_topic_timeline");
        cursor = page.next_cursor.clone();
        pages.push(page);
        if cursor.is_none() {
            return pages;
        }
        assert!(
            pages.len() < MAX_PAGES,
            "projection timeline walk did not end"
        );
    }
}

async fn walk_projection_timeline_filtered<S: Store + ProjectionStore>(
    store: &S,
    topic_id: &str,
    channel_id: &str,
    limit: usize,
) -> Vec<Page<ObjectProjectionRow>> {
    let mut pages = Vec::new();
    let mut cursor: Option<TimelineCursor> = None;
    loop {
        let page = ObjectProjectionStore::list_topic_timeline_in_channel(
            store,
            topic_id,
            channel_id,
            cursor.clone(),
            limit,
        )
        .await
        .expect("ObjectProjectionStore::list_topic_timeline_in_channel");
        cursor = page.next_cursor.clone();
        pages.push(page);
        if cursor.is_none() {
            return pages;
        }
        assert!(
            pages.len() < MAX_PAGES,
            "filtered timeline walk did not end"
        );
    }
}

async fn walk_projection_thread<S: Store + ProjectionStore>(
    store: &S,
    topic_id: &str,
    root: &EnvelopeId,
    limit: usize,
) -> Vec<Page<ObjectProjectionRow>> {
    let mut pages = Vec::new();
    let mut cursor: Option<TimelineCursor> = None;
    loop {
        let page = ObjectProjectionStore::list_thread(store, topic_id, root, cursor.clone(), limit)
            .await
            .expect("ObjectProjectionStore::list_thread");
        cursor = page.next_cursor.clone();
        pages.push(page);
        if cursor.is_none() {
            return pages;
        }
        assert!(
            pages.len() < MAX_PAGES,
            "projection thread walk did not end"
        );
    }
}

async fn walk_projection_thread_filtered<S: Store + ProjectionStore>(
    store: &S,
    topic_id: &str,
    root: &EnvelopeId,
    allowed_channel: Option<&str>,
    limit: usize,
) -> Vec<Page<ObjectProjectionRow>> {
    let mut pages = Vec::new();
    let mut cursor: Option<TimelineCursor> = None;
    loop {
        let page = ObjectProjectionStore::list_thread_filtered(
            store,
            topic_id,
            root,
            allowed_channel,
            cursor.clone(),
            limit,
        )
        .await
        .expect("ObjectProjectionStore::list_thread_filtered");
        cursor = page.next_cursor.clone();
        pages.push(page);
        if cursor.is_none() {
            return pages;
        }
        assert!(pages.len() < MAX_PAGES, "filtered thread walk did not end");
    }
}

async fn walk_dm_messages<S: Store + ProjectionStore>(
    store: &S,
    dm_id: &str,
    limit: usize,
) -> Vec<Page<DirectMessageMessageRow>> {
    let mut pages = Vec::new();
    let mut cursor: Option<TimelineCursor> = None;
    loop {
        let page =
            DirectMessageStore::list_direct_message_messages(store, dm_id, cursor.clone(), limit)
                .await
                .expect("DirectMessageStore::list_direct_message_messages");
        cursor = page.next_cursor.clone();
        pages.push(page);
        if cursor.is_none() {
            return pages;
        }
        assert!(pages.len() < MAX_PAGES, "dm message walk did not end");
    }
}

#[derive(Debug, PartialEq)]
struct StoreEnvelopeScenarioResult {
    timeline_pages: Vec<Page<KukuriEnvelope>>,
    thread_pages: Vec<Page<KukuriEnvelope>>,
    full_page: Page<KukuriEnvelope>,
    envelope_hit: Option<KukuriEnvelope>,
    envelope_miss: Option<KukuriEnvelope>,
}

async fn store_envelope_scenario<S: Store + ProjectionStore>(
    store: &S,
) -> StoreEnvelopeScenarioResult {
    let topic = "kukuri:topic:parity-envelope";
    for envelope in [
        parity_envelope(topic, "env-root", 50, None, None),
        parity_envelope(topic, "env-reply-1", 60, Some("env-root"), Some("env-root")),
        parity_envelope(
            topic,
            "env-reply-2",
            60,
            Some("env-root"),
            Some("env-reply-1"),
        ),
        parity_envelope(topic, "env-reply-3", 70, Some("env-root"), Some("env-root")),
        parity_envelope(topic, "env-solo", 65, None, None),
        parity_envelope(topic, "env-old", 40, None, None),
    ] {
        Store::put_envelope(store, envelope)
            .await
            .expect("Store::put_envelope");
    }

    StoreEnvelopeScenarioResult {
        timeline_pages: walk_store_timeline(store, topic, 2).await,
        thread_pages: walk_store_thread(store, topic, &EnvelopeId::from("env-root"), 2).await,
        full_page: Store::list_topic_timeline(store, topic, None, 10)
            .await
            .expect("Store::list_topic_timeline full"),
        envelope_hit: Store::get_envelope(store, &EnvelopeId::from("env-root"))
            .await
            .expect("Store::get_envelope hit"),
        envelope_miss: Store::get_envelope(store, &EnvelopeId::from("env-missing"))
            .await
            .expect("Store::get_envelope miss"),
    }
}

#[tokio::test]
async fn store_envelope_pagination_matches_between_backends() {
    let sqlite = SqliteStore::connect_memory().await.expect("sqlite store");
    let memory = MemoryStore::default();
    let from_sqlite = store_envelope_scenario(&sqlite).await;
    let from_memory = store_envelope_scenario(&memory).await;
    assert_eq!(from_sqlite, from_memory);

    // sanity: sqlite 実測(created_at DESC, envelope_id DESC。60 の tie は id 降順)
    assert_eq!(
        envelope_page_ids(&from_sqlite.timeline_pages),
        vec![
            vec!["env-reply-3".to_string(), "env-solo".to_string()],
            vec!["env-reply-2".to_string(), "env-reply-1".to_string()],
            vec!["env-root".to_string(), "env-old".to_string()],
            vec![],
        ],
    );
    // ちょうど尽きた 3 ページ目も next_cursor=Some(幻の次ページ)を返す現挙動
    assert!(from_sqlite.timeline_pages[2].next_cursor.is_some());
    // thread は root 先頭固定 + created_at ASC, id ASC(60 の tie は id 昇順)
    assert_eq!(
        envelope_page_ids(&from_sqlite.thread_pages),
        vec![
            vec!["env-root".to_string(), "env-reply-1".to_string()],
            vec!["env-reply-2".to_string(), "env-reply-3".to_string()],
            vec![],
        ],
    );
    assert_eq!(from_sqlite.full_page.items.len(), 6);
    assert!(from_sqlite.full_page.next_cursor.is_none());
    assert!(from_sqlite.envelope_hit.is_some());
    assert!(from_sqlite.envelope_miss.is_none());
}

#[derive(Debug, PartialEq)]
struct ProjectionTimelineScenarioResult {
    timeline_pages: Vec<Page<ObjectProjectionRow>>,
    filtered_pages: Vec<Page<ObjectProjectionRow>>,
    filtered_public: Page<ObjectProjectionRow>,
    projection_hit: Option<ObjectProjectionRow>,
    projection_miss: Option<ObjectProjectionRow>,
}

async fn projection_timeline_scenario<S: Store + ProjectionStore>(
    store: &S,
) -> ProjectionTimelineScenarioResult {
    let topic = "kukuri:topic:parity-proj";
    ObjectProjectionStore::put_object_projections(
        store,
        vec![
            parity_projection_row(topic, "public", "proj-a", 100),
            parity_projection_row(topic, "private:friends", "proj-b", 100),
            parity_projection_row(topic, "private:friends", "proj-c", 90),
            parity_projection_row(topic, "public", "proj-d", 80),
            parity_projection_row(topic, "private:friends", "proj-e", 80),
            parity_projection_row(topic, "public", "proj-f", 70),
        ],
    )
    .await
    .expect("ObjectProjectionStore::put_object_projections");

    // 同一 object_id の再 put(upsert 更新経路)
    let mut updated = parity_projection_row(topic, "public", "proj-a", 100);
    updated.content = Some("content:proj-a:updated".into());
    ObjectProjectionStore::put_object_projection(store, updated)
        .await
        .expect("ObjectProjectionStore::put_object_projection update");

    ProjectionTimelineScenarioResult {
        timeline_pages: walk_projection_timeline(store, topic, 2).await,
        filtered_pages: walk_projection_timeline_filtered(store, topic, "private:friends", 2).await,
        filtered_public: ObjectProjectionStore::list_topic_timeline_in_channel(
            store, topic, "public", None, 10,
        )
        .await
        .expect("ObjectProjectionStore::list_topic_timeline_in_channel public"),
        projection_hit: ObjectProjectionStore::get_object_projection(
            store,
            &EnvelopeId::from("proj-a"),
        )
        .await
        .expect("ObjectProjectionStore::get_object_projection hit"),
        projection_miss: ObjectProjectionStore::get_object_projection(
            store,
            &EnvelopeId::from("proj-missing"),
        )
        .await
        .expect("ObjectProjectionStore::get_object_projection miss"),
    }
}

#[tokio::test]
async fn projection_timeline_pagination_matches_between_backends() {
    let sqlite = SqliteStore::connect_memory().await.expect("sqlite store");
    let memory = MemoryStore::default();
    let from_sqlite = projection_timeline_scenario(&sqlite).await;
    let from_memory = projection_timeline_scenario(&memory).await;
    assert_eq!(from_sqlite, from_memory);

    // sanity: sqlite 実測(created_at DESC, object_id DESC。tie は id 降順)
    assert_eq!(
        projection_page_ids(&from_sqlite.timeline_pages),
        vec![
            vec!["proj-b".to_string(), "proj-a".to_string()],
            vec!["proj-c".to_string(), "proj-e".to_string()],
            vec!["proj-d".to_string(), "proj-f".to_string()],
            vec![],
        ],
    );
    assert_eq!(
        projection_page_ids(&from_sqlite.filtered_pages),
        vec![
            vec!["proj-b".to_string(), "proj-c".to_string()],
            vec!["proj-e".to_string()],
        ],
    );
    assert_eq!(from_sqlite.filtered_public.items.len(), 3);
    assert_eq!(
        from_sqlite
            .projection_hit
            .as_ref()
            .and_then(|row| row.content.as_deref()),
        Some("content:proj-a:updated"),
    );
    assert!(from_sqlite.projection_miss.is_none());
}

#[derive(Debug, PartialEq)]
struct ProjectionThreadScenarioResult {
    thread_pages: Vec<Page<ObjectProjectionRow>>,
    filtered_pages: Vec<Page<ObjectProjectionRow>>,
    filtered_none_channel: Page<ObjectProjectionRow>,
}

async fn projection_thread_scenario<S: Store + ProjectionStore>(
    store: &S,
) -> ProjectionThreadScenarioResult {
    let topic = "kukuri:topic:parity-thread";
    let root_id = EnvelopeId::from("th-root");
    let root = parity_projection_row(topic, "private:friends", "th-root", 10);
    let mut reply_public = parity_projection_row(topic, "public", "th-r1", 20);
    let mut reply_private_a = parity_projection_row(topic, "private:friends", "th-r2", 20);
    let mut reply_private_b = parity_projection_row(topic, "private:friends", "th-r3", 30);
    for reply in [
        &mut reply_public,
        &mut reply_private_a,
        &mut reply_private_b,
    ] {
        reply.root_object_id = Some(root_id.clone());
        reply.reply_to_object_id = Some(root_id.clone());
        reply.object_kind = "comment".into();
    }
    let unrelated = parity_projection_row(topic, "private:friends", "th-x", 25);
    ObjectProjectionStore::put_object_projections(
        store,
        vec![
            root,
            reply_public,
            reply_private_a,
            reply_private_b,
            unrelated,
        ],
    )
    .await
    .expect("ObjectProjectionStore::put_object_projections");

    ProjectionThreadScenarioResult {
        thread_pages: walk_projection_thread(store, topic, &root_id, 2).await,
        filtered_pages: walk_projection_thread_filtered(
            store,
            topic,
            &root_id,
            Some("private:friends"),
            2,
        )
        .await,
        filtered_none_channel: ObjectProjectionStore::list_thread_filtered(
            store, topic, &root_id, None, None, 10,
        )
        .await
        .expect("ObjectProjectionStore::list_thread_filtered none"),
    }
}

#[tokio::test]
async fn projection_thread_pagination_matches_between_backends() {
    let sqlite = SqliteStore::connect_memory().await.expect("sqlite store");
    let memory = MemoryStore::default();
    let from_sqlite = projection_thread_scenario(&sqlite).await;
    let from_memory = projection_thread_scenario(&memory).await;
    assert_eq!(from_sqlite, from_memory);

    // sanity: sqlite 実測(root 先頭固定 + created_at ASC, object_id ASC)
    assert_eq!(
        projection_page_ids(&from_sqlite.thread_pages),
        vec![
            vec!["th-root".to_string(), "th-r1".to_string()],
            vec!["th-r2".to_string(), "th-r3".to_string()],
            vec![],
        ],
    );
    assert_eq!(
        projection_page_ids(&from_sqlite.filtered_pages),
        vec![
            vec!["th-root".to_string(), "th-r2".to_string()],
            vec!["th-r3".to_string()],
        ],
    );
    // allowed_channel = None は無フィルタの thread と同じ items(th-x は thread 外)
    assert_eq!(
        from_sqlite
            .filtered_none_channel
            .items
            .iter()
            .map(|row| row.object_id.as_str().to_string())
            .collect::<Vec<_>>(),
        vec![
            "th-root".to_string(),
            "th-r1".to_string(),
            "th-r2".to_string(),
            "th-r3".to_string(),
        ],
    );
}

#[derive(Debug, PartialEq)]
struct DirectMessageScenarioResult {
    first_walk: Vec<Page<DirectMessageMessageRow>>,
    acked_message: Option<DirectMessageMessageRow>,
    blocked_message: Option<DirectMessageMessageRow>,
    has_tombstone_hit: bool,
    has_tombstone_miss: bool,
    tombstones: Vec<DirectMessageTombstoneRow>,
    second_walk: Vec<Page<DirectMessageMessageRow>>,
}

async fn direct_message_scenario<S: Store + ProjectionStore>(
    store: &S,
) -> DirectMessageScenarioResult {
    let dm_id = "dm-parity";
    for (message_id, created_at) in [
        ("msg-1", 10),
        ("msg-2", 20),
        ("msg-3", 20),
        ("msg-4", 30),
        ("msg-5", 40),
        ("msg-6", 40),
    ] {
        DirectMessageStore::put_direct_message_message(
            store,
            parity_dm_message(dm_id, message_id, created_at),
        )
        .await
        .expect("DirectMessageStore::put_direct_message_message");
    }
    DirectMessageStore::set_direct_message_acked_at(store, dm_id, "msg-2", 99)
        .await
        .expect("DirectMessageStore::set_direct_message_acked_at");

    let first_walk = walk_dm_messages(store, dm_id, 2).await;

    // tombstone 中の put は無視される(bool 系 + 読み出しで固定)
    DirectMessageStore::put_direct_message_tombstone(
        store,
        DirectMessageTombstoneRow {
            dm_id: dm_id.into(),
            message_id: "blocked".into(),
            deleted_at: 100,
        },
    )
    .await
    .expect("put tombstone blocked");
    DirectMessageStore::put_direct_message_message(store, parity_dm_message(dm_id, "blocked", 25))
        .await
        .expect("put blocked message");

    // 既存メッセージの tombstone → ローカル削除 → 再 put も無視される
    DirectMessageStore::put_direct_message_tombstone(
        store,
        DirectMessageTombstoneRow {
            dm_id: dm_id.into(),
            message_id: "msg-4".into(),
            deleted_at: 100,
        },
    )
    .await
    .expect("put tombstone msg-4");
    DirectMessageStore::delete_direct_message_message_local(store, dm_id, "msg-4")
        .await
        .expect("delete msg-4");
    DirectMessageStore::put_direct_message_message(store, parity_dm_message(dm_id, "msg-4", 30))
        .await
        .expect("re-put msg-4 (ignored)");
    DirectMessageStore::put_direct_message_tombstone(
        store,
        DirectMessageTombstoneRow {
            dm_id: dm_id.into(),
            message_id: "old-tomb".into(),
            deleted_at: 90,
        },
    )
    .await
    .expect("put tombstone old-tomb");

    DirectMessageScenarioResult {
        first_walk,
        acked_message: DirectMessageStore::get_direct_message_message(store, dm_id, "msg-2")
            .await
            .expect("get msg-2"),
        blocked_message: DirectMessageStore::get_direct_message_message(store, dm_id, "blocked")
            .await
            .expect("get blocked"),
        has_tombstone_hit: DirectMessageStore::has_direct_message_tombstone(
            store, dm_id, "blocked",
        )
        .await
        .expect("has tombstone blocked"),
        has_tombstone_miss: DirectMessageStore::has_direct_message_tombstone(store, dm_id, "msg-1")
            .await
            .expect("has tombstone msg-1"),
        tombstones: DirectMessageStore::list_direct_message_tombstones(store, dm_id)
            .await
            .expect("list tombstones"),
        second_walk: walk_dm_messages(store, dm_id, 2).await,
    }
}

#[tokio::test]
async fn direct_message_pagination_and_tombstones_match_between_backends() {
    let sqlite = SqliteStore::connect_memory().await.expect("sqlite store");
    let memory = MemoryStore::default();
    let from_sqlite = direct_message_scenario(&sqlite).await;
    let from_memory = direct_message_scenario(&memory).await;
    assert_eq!(from_sqlite, from_memory);

    // sanity: sqlite 実測(created_at DESC, message_id DESC。40/20 の tie は id 降順)
    assert_eq!(
        dm_page_ids(&from_sqlite.first_walk),
        vec![
            vec!["msg-6".to_string(), "msg-5".to_string()],
            vec!["msg-4".to_string(), "msg-3".to_string()],
            vec!["msg-2".to_string(), "msg-1".to_string()],
            vec![],
        ],
    );
    assert_eq!(
        from_sqlite
            .acked_message
            .as_ref()
            .and_then(|row| row.acked_at),
        Some(99),
    );
    assert!(from_sqlite.blocked_message.is_none());
    assert!(from_sqlite.has_tombstone_hit);
    assert!(!from_sqlite.has_tombstone_miss);
    assert_eq!(
        from_sqlite
            .tombstones
            .iter()
            .map(|row| row.message_id.clone())
            .collect::<Vec<_>>(),
        vec![
            "msg-4".to_string(),
            "blocked".to_string(),
            "old-tomb".to_string(),
        ],
    );
    assert_eq!(
        dm_page_ids(&from_sqlite.second_walk),
        vec![
            vec!["msg-6".to_string(), "msg-5".to_string()],
            vec!["msg-3".to_string(), "msg-2".to_string()],
            vec!["msg-1".to_string()],
        ],
    );
}
