//! WP-S6 T8: 順序付き list 系 + bool 系の sqlite/memory 突き合わせ。
//! notifications / bookmarked_posts / bookmarked_custom_reactions(search_key '' の
//! フォールバック含む)/ dm_outbox / dm_conversations / muted_authors / follow_edges /
//! reaction 3 種(for_target / for_targets / recent_by_author)。

use super::*;

use std::collections::HashMap;

use kukuri_core::FollowEdge;

#[derive(Debug, PartialEq)]
struct NotificationScenarioResult {
    inserted: Vec<bool>,
    unread_initial: usize,
    list_initial: Vec<NotificationRow>,
    unread_after_single: usize,
    unread_after_all: usize,
    list_final: Vec<NotificationRow>,
}

async fn notification_scenario<S: Store + ProjectionStore>(
    store: &S,
) -> NotificationScenarioResult {
    let n1 = parity_notification("notif-1", 100, NotificationKind::Mention);
    let mut n2 = parity_notification("notif-2", 100, NotificationKind::Reply);
    n2.source_envelope_id = Some(EnvelopeId::from("env-src"));
    n2.source_replica_id = Some(ReplicaId::new("topic::parity-notifications"));
    n2.topic_id = Some("kukuri:topic:parity-notifications".into());
    n2.channel_id = Some("public".into());
    n2.object_id = Some(EnvelopeId::from("obj-1"));
    n2.preview_text = Some("reply preview".into());
    n2.content_labels = Some(vec![kukuri_core::ADULT_CONTENT_LABEL.to_string()]);
    let mut n3 = parity_notification("notif-3", 90, NotificationKind::DirectMessage);
    n3.dm_id = Some("dm-parity".into());
    n3.message_id = Some("msg-9".into());
    n3.read_at = Some(95);
    // 同一 notification_id の再 put は無視され、初回の内容が残る
    let mut duplicate = n1.clone();
    duplicate.preview_text = Some("should be ignored".into());

    let mut inserted = Vec::new();
    for row in [n1, n2, n3, duplicate] {
        inserted.push(
            NotificationStore::put_notification_if_absent(store, row)
                .await
                .expect("NotificationStore::put_notification_if_absent"),
        );
    }
    let unread_initial = NotificationStore::count_unread_notifications(store)
        .await
        .expect("count unread initial");
    // read_at が NULL のままの行を含む list も比較対象(WP-B16 で NULL decode quirk を
    // 解消し、sqlite も memory と同じく None を返す)。
    let list_initial = NotificationStore::list_notifications(store)
        .await
        .expect("list notifications initial");

    NotificationStore::mark_notification_read(store, "notif-1", 110)
        .await
        .expect("mark notif-1 read");
    NotificationStore::mark_notification_read(store, "notif-1", 120)
        .await
        .expect("mark notif-1 read again (kept)");
    NotificationStore::mark_notification_read(store, "notif-missing", 130)
        .await
        .expect("mark missing notification");
    let unread_after_single = NotificationStore::count_unread_notifications(store)
        .await
        .expect("count unread after single");
    NotificationStore::mark_all_notifications_read(store, 140)
        .await
        .expect("mark all read");
    let unread_after_all = NotificationStore::count_unread_notifications(store)
        .await
        .expect("count unread after all");
    let list_final = NotificationStore::list_notifications(store)
        .await
        .expect("list notifications final");

    NotificationScenarioResult {
        inserted,
        unread_initial,
        list_initial,
        unread_after_single,
        unread_after_all,
        list_final,
    }
}

#[tokio::test]
async fn notifications_match_between_backends() {
    let sqlite = SqliteStore::connect_memory().await.expect("sqlite store");
    let memory = MemoryStore::default();
    let from_sqlite = notification_scenario(&sqlite).await;
    let from_memory = notification_scenario(&memory).await;
    assert_eq!(from_sqlite, from_memory);

    // sanity: sqlite 実測(received_at DESC, notification_id DESC。100 の tie は id 降順)
    assert_eq!(from_sqlite.inserted, vec![true, true, true, false]);
    assert_eq!(from_sqlite.unread_initial, 2);
    // 未読(read_at NULL)の行が None として読めること(WP-B16)。
    assert_eq!(
        from_sqlite
            .list_initial
            .iter()
            .map(|row| row.read_at)
            .collect::<Vec<_>>(),
        vec![None, None, Some(95)],
    );
    assert_eq!(
        from_sqlite
            .list_final
            .iter()
            .map(|row| row.notification_id.clone())
            .collect::<Vec<_>>(),
        vec![
            "notif-2".to_string(),
            "notif-1".to_string(),
            "notif-3".to_string(),
        ],
    );
    assert_eq!(from_sqlite.unread_after_single, 1);
    assert_eq!(from_sqlite.unread_after_all, 0);
    assert_eq!(
        from_sqlite
            .list_final
            .iter()
            .map(|row| row.read_at)
            .collect::<Vec<_>>(),
        vec![Some(140), Some(110), Some(95)],
    );
    // 重複 put は初回の内容が残る(notif-1 の preview_text は None のまま)
    assert_eq!(from_sqlite.list_final[1].preview_text, None);
}

#[derive(Debug, PartialEq)]
struct BookmarkScenarioResult {
    posts_initial: Vec<BookmarkedPostRow>,
    posts_after_remove: Vec<BookmarkedPostRow>,
    reactions_initial: Vec<BookmarkedCustomReactionRow>,
    reactions_after_remove: Vec<BookmarkedCustomReactionRow>,
}

async fn bookmark_scenario<S: Store + ProjectionStore>(store: &S) -> BookmarkScenarioResult {
    for row in [
        parity_bookmarked_post_max("bp-max", 100),
        parity_bookmarked_post("bp-min", 100),
        parity_bookmarked_post("bp-old", 90),
    ] {
        ReactionBookmarkStore::put_bookmarked_post(store, row)
            .await
            .expect("ReactionBookmarkStore::put_bookmarked_post");
    }
    // 同一 source_object_id の再 put(upsert 更新経路)
    let mut updated = parity_bookmarked_post_max("bp-max", 100);
    updated.content = Some("content:bp-max:updated".into());
    ReactionBookmarkStore::put_bookmarked_post(store, updated)
        .await
        .expect("put bookmarked post update");

    for row in [
        parity_custom_reaction("asset-normal", "smile", 100),
        parity_custom_reaction("asset-empty", "", 100),
        parity_custom_reaction("asset-blank", "   ", 90),
    ] {
        ReactionBookmarkStore::put_bookmarked_custom_reaction(store, row)
            .await
            .expect("ReactionBookmarkStore::put_bookmarked_custom_reaction");
    }
    // 同一 asset_id の再 put(upsert 更新経路)
    ReactionBookmarkStore::put_bookmarked_custom_reaction(
        store,
        parity_custom_reaction("asset-normal", "grin", 100),
    )
    .await
    .expect("put custom reaction update");

    let posts_initial = ReactionBookmarkStore::list_bookmarked_posts_page(store, None, false)
        .await
        .expect("list bookmarked posts initial");
    let reactions_initial = ReactionBookmarkStore::list_bookmarked_custom_reactions(store)
        .await
        .expect("list custom reactions initial");

    ReactionBookmarkStore::remove_bookmarked_post(store, &EnvelopeId::from("bp-old"))
        .await
        .expect("remove bp-old");
    ReactionBookmarkStore::remove_bookmarked_post(store, &EnvelopeId::from("bp-missing"))
        .await
        .expect("remove missing bookmarked post");
    ReactionBookmarkStore::remove_bookmarked_custom_reaction(store, "asset-empty")
        .await
        .expect("remove asset-empty");

    BookmarkScenarioResult {
        posts_initial,
        posts_after_remove: ReactionBookmarkStore::list_bookmarked_posts_page(store, None, false)
            .await
            .expect("list bookmarked posts after remove"),
        reactions_initial,
        reactions_after_remove: ReactionBookmarkStore::list_bookmarked_custom_reactions(store)
            .await
            .expect("list custom reactions after remove"),
    }
}

#[tokio::test]
async fn bookmarks_match_between_backends() {
    let sqlite = SqliteStore::connect_memory().await.expect("sqlite store");
    let memory = MemoryStore::default();
    let from_sqlite = bookmark_scenario(&sqlite).await;
    let from_memory = bookmark_scenario(&memory).await;
    assert_eq!(from_sqlite, from_memory);

    // sanity: sqlite 実測(bookmarked_at DESC, source_object_id DESC)
    assert_eq!(
        from_sqlite
            .posts_initial
            .iter()
            .map(|row| row.source_object_id.as_str().to_string())
            .collect::<Vec<_>>(),
        vec![
            "bp-min".to_string(),
            "bp-max".to_string(),
            "bp-old".to_string(),
        ],
    );
    assert_eq!(
        from_sqlite.posts_initial[1].content.as_deref(),
        Some("content:bp-max:updated"),
    );
    assert_eq!(from_sqlite.posts_after_remove.len(), 2);
    // search_key '' / 空白のみ → asset_id フォールバック(sqlite 読み出しと同義)
    assert_eq!(
        from_sqlite
            .reactions_initial
            .iter()
            .map(|row| (row.asset_id.clone(), row.search_key.clone()))
            .collect::<Vec<_>>(),
        vec![
            ("asset-normal".to_string(), "grin".to_string()),
            ("asset-empty".to_string(), "asset-empty".to_string()),
            ("asset-blank".to_string(), "asset-blank".to_string()),
        ],
    );
    assert_eq!(
        from_sqlite
            .reactions_after_remove
            .iter()
            .map(|row| row.asset_id.clone())
            .collect::<Vec<_>>(),
        vec!["asset-normal".to_string(), "asset-blank".to_string()],
    );
}

async fn bookmark_cursor_scenario<S: Store + ProjectionStore>(
    store: &S,
) -> (Vec<String>, Vec<String>, Vec<String>, Vec<String>) {
    for index in 0..30 {
        ReactionBookmarkStore::put_bookmarked_post(
            store,
            parity_bookmarked_post(&format!("bp-{index:02}"), index),
        )
        .await
        .expect("put bookmark");
    }
    let first = ReactionBookmarkStore::list_bookmarked_posts_page(store, None, false)
        .await
        .expect("first bookmark page");
    let older = ReactionBookmarkStore::list_bookmarked_posts_page(
        store,
        Some(&BookmarkCursor::from(&first[19])),
        false,
    )
    .await
    .expect("older bookmark page");
    let newer = ReactionBookmarkStore::list_bookmarked_posts_page(
        store,
        Some(&BookmarkCursor::from(&older[0])),
        true,
    )
    .await
    .expect("newer bookmark page");
    let selected = ReactionBookmarkStore::bookmarked_post_ids(
        store,
        &[EnvelopeId::from("bp-00"), EnvelopeId::from("missing")],
    )
    .await
    .expect("bounded bookmark membership");
    let ids = |rows: &[BookmarkedPostRow]| {
        rows.iter()
            .map(|row| row.source_object_id.as_str().to_owned())
            .collect()
    };
    (
        ids(&first),
        ids(&older),
        ids(&newer),
        selected.iter().map(|id| id.as_str().to_owned()).collect(),
    )
}

#[tokio::test]
async fn bookmarked_posts_seek_both_directions_without_loading_the_history() {
    let sqlite = SqliteStore::connect_memory().await.expect("sqlite store");
    let memory = MemoryStore::default();
    let expected = bookmark_cursor_scenario(&sqlite).await;
    assert_eq!(bookmark_cursor_scenario(&memory).await, expected);
    assert_eq!(expected.0.len(), 21);
    assert_eq!(expected.0[0], "bp-29");
    assert_eq!(expected.0[19], "bp-10");
    assert_eq!(
        expected.1,
        (0..10)
            .rev()
            .map(|index| format!("bp-{index:02}"))
            .collect::<Vec<_>>()
    );
    assert_eq!(expected.2.len(), 20);
    assert_eq!(expected.2[0], "bp-10");
    assert_eq!(expected.3, vec!["bp-00"]);
}

#[derive(Debug, PartialEq)]
struct MutedScenarioResult {
    list_initial: Vec<MutedAuthorRow>,
    muted_hit: Option<MutedAuthorRow>,
    muted_miss: Option<MutedAuthorRow>,
    list_after_remove: Vec<MutedAuthorRow>,
}

async fn muted_scenario<S: Store + ProjectionStore>(store: &S) -> MutedScenarioResult {
    for (author, muted_at) in [("m-a", 100), ("m-b", 100), ("m-c", 90)] {
        SocialProjectionStore::put_muted_author(
            store,
            MutedAuthorRow {
                author_pubkey: author.into(),
                muted_at,
            },
        )
        .await
        .expect("SocialProjectionStore::put_muted_author");
    }
    // 再 put で muted_at 更新(upsert 更新経路)
    SocialProjectionStore::put_muted_author(
        store,
        MutedAuthorRow {
            author_pubkey: "m-c".into(),
            muted_at: 120,
        },
    )
    .await
    .expect("put muted author update");

    let list_initial = SocialProjectionStore::list_muted_authors(store)
        .await
        .expect("list muted authors initial");
    let muted_hit = SocialProjectionStore::get_muted_author(store, "m-a")
        .await
        .expect("get muted author hit");
    let muted_miss = SocialProjectionStore::get_muted_author(store, "m-missing")
        .await
        .expect("get muted author miss");
    SocialProjectionStore::remove_muted_author(store, "m-b")
        .await
        .expect("remove muted author");

    MutedScenarioResult {
        list_initial,
        muted_hit,
        muted_miss,
        list_after_remove: SocialProjectionStore::list_muted_authors(store)
            .await
            .expect("list muted authors after remove"),
    }
}

#[tokio::test]
async fn muted_authors_match_between_backends() {
    let sqlite = SqliteStore::connect_memory().await.expect("sqlite store");
    let memory = MemoryStore::default();
    let from_sqlite = muted_scenario(&sqlite).await;
    let from_memory = muted_scenario(&memory).await;
    assert_eq!(from_sqlite, from_memory);

    // sanity: sqlite 実測(muted_at DESC, author_pubkey ASC。100 の tie は author 昇順)
    assert_eq!(
        from_sqlite
            .list_initial
            .iter()
            .map(|row| (row.author_pubkey.clone(), row.muted_at))
            .collect::<Vec<_>>(),
        vec![
            ("m-c".to_string(), 120),
            ("m-a".to_string(), 100),
            ("m-b".to_string(), 100),
        ],
    );
    assert_eq!(
        from_sqlite.muted_hit.as_ref().map(|row| row.muted_at),
        Some(100)
    );
    assert!(from_sqlite.muted_miss.is_none());
    assert_eq!(
        from_sqlite
            .list_after_remove
            .iter()
            .map(|row| row.author_pubkey.clone())
            .collect::<Vec<_>>(),
        vec!["m-c".to_string(), "m-a".to_string()],
    );
}

#[derive(Debug, PartialEq)]
struct FollowEdgeScenarioResult {
    by_subject: Vec<FollowEdge>,
    by_target: Vec<FollowEdge>,
}

async fn follow_edge_scenario<S: Store + ProjectionStore>(store: &S) -> FollowEdgeScenarioResult {
    let subject_1 = "1".repeat(64);
    let subject_2 = "2".repeat(64);
    let target_a = "a".repeat(64);
    let target_b = "b".repeat(64);
    let target_c = "c".repeat(64);

    for edge in [
        parity_follow_edge(
            &subject_1,
            &target_a,
            FollowEdgeStatus::Active,
            100,
            "edge-1",
        ),
        parity_follow_edge(
            &subject_1,
            &target_b,
            FollowEdgeStatus::Revoked,
            100,
            "edge-2",
        ),
        parity_follow_edge(
            &subject_1,
            &target_c,
            FollowEdgeStatus::Active,
            90,
            "edge-3",
        ),
        // 古い更新は無視される(updated_at 50 < 100)
        parity_follow_edge(
            &subject_1,
            &target_a,
            FollowEdgeStatus::Revoked,
            50,
            "edge-4",
        ),
        // 新しい更新は勝つ(95 > 90)
        parity_follow_edge(
            &subject_1,
            &target_c,
            FollowEdgeStatus::Revoked,
            95,
            "edge-5",
        ),
        // 同時刻(100 == 100)は上書きされる
        parity_follow_edge(
            &subject_1,
            &target_b,
            FollowEdgeStatus::Active,
            100,
            "edge-6",
        ),
        parity_follow_edge(
            &subject_2,
            &target_a,
            FollowEdgeStatus::Active,
            120,
            "edge-7",
        ),
    ] {
        Store::upsert_follow_edge(store, edge)
            .await
            .expect("Store::upsert_follow_edge");
    }

    FollowEdgeScenarioResult {
        by_subject: Store::list_follow_edges_by_subject(store, subject_1.as_str())
            .await
            .expect("Store::list_follow_edges_by_subject"),
        by_target: Store::list_follow_edges_by_target(store, target_a.as_str())
            .await
            .expect("Store::list_follow_edges_by_target"),
    }
}

#[tokio::test]
async fn follow_edges_match_between_backends() {
    let sqlite = SqliteStore::connect_memory().await.expect("sqlite store");
    let memory = MemoryStore::default();
    let from_sqlite = follow_edge_scenario(&sqlite).await;
    let from_memory = follow_edge_scenario(&memory).await;
    assert_eq!(from_sqlite, from_memory);

    // sanity: sqlite 実測(updated_at DESC, target ASC。stale 無視・同時刻上書き)
    assert_eq!(
        from_sqlite
            .by_subject
            .iter()
            .map(|edge| {
                (
                    edge.target_pubkey.as_str().to_string(),
                    edge.status.clone(),
                    edge.updated_at,
                    edge.envelope_id.as_str().to_string(),
                )
            })
            .collect::<Vec<_>>(),
        vec![
            (
                "a".repeat(64),
                FollowEdgeStatus::Active,
                100,
                "edge-1".to_string()
            ),
            (
                "b".repeat(64),
                FollowEdgeStatus::Active,
                100,
                "edge-6".to_string()
            ),
            (
                "c".repeat(64),
                FollowEdgeStatus::Revoked,
                95,
                "edge-5".to_string()
            ),
        ],
    );
    assert_eq!(
        from_sqlite
            .by_target
            .iter()
            .map(|edge| (edge.subject_pubkey.as_str().to_string(), edge.updated_at))
            .collect::<Vec<_>>(),
        vec![("2".repeat(64), 120), ("1".repeat(64), 100)],
    );
}

#[derive(Debug, PartialEq)]
struct ReactionScenarioResult {
    reaction_hit: Option<ReactionProjectionRow>,
    reaction_miss: Option<ReactionProjectionRow>,
    for_target: Vec<ReactionProjectionRow>,
    for_targets: HashMap<String, Vec<ReactionProjectionRow>>,
    for_targets_empty: HashMap<String, Vec<ReactionProjectionRow>>,
    recent_by_author: Vec<ReactionProjectionRow>,
}

async fn reaction_scenario<S: Store + ProjectionStore>(store: &S) -> ReactionScenarioResult {
    let replica = "topic::parity-reactions";
    let other_replica = "topic::parity-other";
    let alice = "a".repeat(64);
    let bob = "b".repeat(64);
    let carol = "c".repeat(64);
    let dave = "d".repeat(64);

    for row in [
        parity_reaction(replica, "obj-1", "react-1", &alice, "emoji:🔥", "🔥", 10),
        parity_reaction(replica, "obj-1", "react-2", &bob, "emoji:😀", "😀", 30),
        parity_reaction_custom(replica, "obj-1", "react-3", &alice, 20),
        // react-1 と同一 normalized_reaction_key(tie は reaction_id ASC)
        parity_reaction(replica, "obj-1", "react-0", &carol, "emoji:🔥", "🔥", 25),
        parity_reaction(replica, "obj-2", "react-9", &alice, "emoji:🎉", "🎉", 40),
        // 別 replica は R1 のクエリから除外される
        parity_reaction(
            other_replica,
            "obj-1",
            "react-8",
            &dave,
            "emoji:🔥",
            "🔥",
            50,
        ),
    ] {
        ReactionBookmarkStore::upsert_reaction_cache(store, row)
            .await
            .expect("ReactionBookmarkStore::upsert_reaction_cache");
    }
    // 同一キーの再 upsert(status / updated_at 更新経路)
    let mut updated = parity_reaction(replica, "obj-1", "react-1", &alice, "emoji:🔥", "🔥", 11);
    updated.status = ObjectStatus::Deleted;
    ReactionBookmarkStore::upsert_reaction_cache(store, updated)
        .await
        .expect("upsert reaction update");

    let replica_id = ReplicaId::new(replica);
    ReactionScenarioResult {
        reaction_hit: ReactionBookmarkStore::get_reaction_cache(
            store,
            &replica_id,
            &EnvelopeId::from("obj-1"),
            &EnvelopeId::from("react-1"),
        )
        .await
        .expect("get reaction hit"),
        reaction_miss: ReactionBookmarkStore::get_reaction_cache(
            store,
            &replica_id,
            &EnvelopeId::from("obj-1"),
            &EnvelopeId::from("react-missing"),
        )
        .await
        .expect("get reaction miss"),
        for_target: ReactionBookmarkStore::list_reaction_cache_for_target(
            store,
            &replica_id,
            &EnvelopeId::from("obj-1"),
        )
        .await
        .expect("list reactions for target"),
        for_targets: ReactionBookmarkStore::list_reaction_cache_for_targets(
            store,
            &replica_id,
            &[
                EnvelopeId::from("obj-1"),
                EnvelopeId::from("obj-2"),
                EnvelopeId::from("obj-3"),
            ],
        )
        .await
        .expect("list reactions for targets"),
        for_targets_empty: ReactionBookmarkStore::list_reaction_cache_for_targets(
            store,
            &replica_id,
            &[],
        )
        .await
        .expect("list reactions for empty targets"),
        recent_by_author: ReactionBookmarkStore::list_recent_reaction_cache_by_author(
            store,
            alice.as_str(),
        )
        .await
        .expect("list recent reactions by author"),
    }
}

#[tokio::test]
async fn reactions_match_between_backends() {
    let sqlite = SqliteStore::connect_memory().await.expect("sqlite store");
    let memory = MemoryStore::default();
    let from_sqlite = reaction_scenario(&sqlite).await;
    let from_memory = reaction_scenario(&memory).await;
    assert_eq!(from_sqlite, from_memory);

    // sanity: sqlite 実測
    assert_eq!(
        from_sqlite
            .reaction_hit
            .as_ref()
            .map(|row| row.status.clone()),
        Some(ObjectStatus::Deleted),
    );
    assert!(from_sqlite.reaction_miss.is_none());
    // normalized_reaction_key ASC, reaction_id ASC(custom:… < emoji:…、🔥 < 😀 は UTF-8 バイト順)
    assert_eq!(
        from_sqlite
            .for_target
            .iter()
            .map(|row| row.reaction_id.as_str().to_string())
            .collect::<Vec<_>>(),
        vec![
            "react-3".to_string(),
            "react-0".to_string(),
            "react-1".to_string(),
            "react-2".to_string(),
        ],
    );
    // 行のない obj-3 はキー自体が存在しない
    assert_eq!(from_sqlite.for_targets.len(), 2);
    assert_eq!(
        from_sqlite.for_targets.get("obj-2").map(|rows| rows.len()),
        Some(1),
    );
    assert!(from_sqlite.for_targets_empty.is_empty());
    // updated_at DESC, reaction_id DESC(react-1 は再 upsert で 11)
    assert_eq!(
        from_sqlite
            .recent_by_author
            .iter()
            .map(|row| row.reaction_id.as_str().to_string())
            .collect::<Vec<_>>(),
        vec![
            "react-9".to_string(),
            "react-3".to_string(),
            "react-1".to_string(),
        ],
    );
}

#[derive(Debug, PartialEq)]
struct OutboxScenarioResult {
    list_initial: Vec<DirectMessageOutboxRow>,
    list_after_update: Vec<DirectMessageOutboxRow>,
    outbox_hit: Option<DirectMessageOutboxRow>,
    outbox_miss: Option<DirectMessageOutboxRow>,
    list_after_remove: Vec<DirectMessageOutboxRow>,
}

async fn outbox_scenario<S: Store + ProjectionStore>(store: &S) -> OutboxScenarioResult {
    for row in [
        parity_outbox("dm-out", "om-1", 10),
        parity_outbox("dm-out", "om-2", 10),
        parity_outbox("dm-out2", "om-0", 5),
    ] {
        DirectMessageStore::put_direct_message_outbox(store, row)
            .await
            .expect("DirectMessageStore::put_direct_message_outbox");
    }
    let list_initial = DirectMessageStore::list_direct_message_outbox(store)
        .await
        .expect("list outbox initial");

    DirectMessageStore::touch_direct_message_outbox_attempt(store, "dm-out", "om-2", 99)
        .await
        .expect("touch outbox attempt");
    DirectMessageStore::touch_direct_message_outbox_attempt(store, "dm-out", "om-missing", 99)
        .await
        .expect("touch missing outbox attempt");
    // 再 put で created_at 更新(upsert 更新経路。順序も変わる)
    DirectMessageStore::put_direct_message_outbox(store, parity_outbox("dm-out", "om-1", 8))
        .await
        .expect("put outbox update");
    let list_after_update = DirectMessageStore::list_direct_message_outbox(store)
        .await
        .expect("list outbox after update");

    let outbox_hit = DirectMessageStore::get_direct_message_outbox(store, "dm-out", "om-2")
        .await
        .expect("get outbox hit");
    let outbox_miss = DirectMessageStore::get_direct_message_outbox(store, "dm-out", "om-missing")
        .await
        .expect("get outbox miss");
    DirectMessageStore::remove_direct_message_outbox(store, "dm-out2", "om-0")
        .await
        .expect("remove outbox");

    OutboxScenarioResult {
        list_initial,
        list_after_update,
        outbox_hit,
        outbox_miss,
        list_after_remove: DirectMessageStore::list_direct_message_outbox(store)
            .await
            .expect("list outbox after remove"),
    }
}

#[tokio::test]
async fn direct_message_outbox_matches_between_backends() {
    let sqlite = SqliteStore::connect_memory().await.expect("sqlite store");
    let memory = MemoryStore::default();
    let from_sqlite = outbox_scenario(&sqlite).await;
    let from_memory = outbox_scenario(&memory).await;
    assert_eq!(from_sqlite, from_memory);

    // sanity: sqlite 実測(created_at ASC, message_id ASC。10 の tie は id 昇順)
    assert_eq!(
        from_sqlite
            .list_initial
            .iter()
            .map(|row| row.message_id.clone())
            .collect::<Vec<_>>(),
        vec!["om-0".to_string(), "om-1".to_string(), "om-2".to_string()],
    );
    assert_eq!(
        from_sqlite
            .list_after_update
            .iter()
            .map(|row| (row.message_id.clone(), row.created_at, row.last_attempt_at))
            .collect::<Vec<_>>(),
        vec![
            ("om-0".to_string(), 5, None),
            ("om-1".to_string(), 8, None),
            ("om-2".to_string(), 10, Some(99)),
        ],
    );
    assert_eq!(
        from_sqlite
            .outbox_hit
            .as_ref()
            .and_then(|row| row.last_attempt_at),
        Some(99),
    );
    assert!(from_sqlite.outbox_miss.is_none());
    assert_eq!(from_sqlite.list_after_remove.len(), 2);
}

#[derive(Debug, PartialEq)]
struct ConversationScenarioResult {
    list_initial: Vec<DirectMessageConversationRow>,
    peer_hit: Option<DirectMessageConversationRow>,
    peer_miss: Option<DirectMessageConversationRow>,
    dm_id_hit: Option<DirectMessageConversationRow>,
    dm_id_miss: Option<DirectMessageConversationRow>,
    list_after_update: Vec<DirectMessageConversationRow>,
    list_after_clear: Vec<DirectMessageConversationRow>,
}

async fn conversation_scenario<S: Store + ProjectionStore>(
    store: &S,
) -> ConversationScenarioResult {
    let peer_1 = "3".repeat(64);
    let peer_2 = "4".repeat(64);
    let peer_3 = "5".repeat(64);
    for row in [
        parity_conversation("dm-a", &peer_1, 100),
        parity_conversation("dm-b", &peer_2, 100),
        parity_conversation("dm-c", &peer_3, 90),
    ] {
        DirectMessageStore::upsert_direct_message_conversation(store, row)
            .await
            .expect("DirectMessageStore::upsert_direct_message_conversation");
    }
    let list_initial = DirectMessageStore::list_direct_message_conversations(store)
        .await
        .expect("list conversations initial");
    let peer_hit =
        DirectMessageStore::get_direct_message_conversation_by_peer(store, peer_2.as_str())
            .await
            .expect("get conversation by peer hit");
    let peer_miss =
        DirectMessageStore::get_direct_message_conversation_by_peer(store, &"9".repeat(64))
            .await
            .expect("get conversation by peer miss");
    let dm_id_hit = DirectMessageStore::get_direct_message_conversation_by_dm_id(store, "dm-a")
        .await
        .expect("get conversation by dm_id hit");
    let dm_id_miss =
        DirectMessageStore::get_direct_message_conversation_by_dm_id(store, "dm-missing")
            .await
            .expect("get conversation by dm_id miss");

    // 再 upsert で updated_at 更新 → 順序が変わる
    DirectMessageStore::upsert_direct_message_conversation(
        store,
        parity_conversation("dm-c", &peer_3, 120),
    )
    .await
    .expect("upsert conversation update");
    let list_after_update = DirectMessageStore::list_direct_message_conversations(store)
        .await
        .expect("list conversations after update");

    DirectMessageStore::clear_direct_message_local(store, "dm-a")
        .await
        .expect("clear dm-a");

    ConversationScenarioResult {
        list_initial,
        peer_hit,
        peer_miss,
        dm_id_hit,
        dm_id_miss,
        list_after_update,
        list_after_clear: DirectMessageStore::list_direct_message_conversations(store)
            .await
            .expect("list conversations after clear"),
    }
}

#[tokio::test]
async fn direct_message_conversations_match_between_backends() {
    let sqlite = SqliteStore::connect_memory().await.expect("sqlite store");
    let memory = MemoryStore::default();
    let from_sqlite = conversation_scenario(&sqlite).await;
    let from_memory = conversation_scenario(&memory).await;
    assert_eq!(from_sqlite, from_memory);

    // sanity: sqlite 実測(updated_at DESC, dm_id DESC。100 の tie は dm_id 降順)
    assert_eq!(
        from_sqlite
            .list_initial
            .iter()
            .map(|row| row.dm_id.clone())
            .collect::<Vec<_>>(),
        vec!["dm-b".to_string(), "dm-a".to_string(), "dm-c".to_string()],
    );
    assert_eq!(
        from_sqlite.peer_hit.as_ref().map(|row| row.dm_id.clone()),
        Some("dm-b".to_string()),
    );
    assert!(from_sqlite.peer_miss.is_none());
    assert!(from_sqlite.dm_id_hit.is_some());
    assert!(from_sqlite.dm_id_miss.is_none());
    assert_eq!(
        from_sqlite
            .list_after_update
            .iter()
            .map(|row| row.dm_id.clone())
            .collect::<Vec<_>>(),
        vec!["dm-c".to_string(), "dm-b".to_string(), "dm-a".to_string()],
    );
    assert_eq!(
        from_sqlite
            .list_after_clear
            .iter()
            .map(|row| row.dm_id.clone())
            .collect::<Vec<_>>(),
        vec!["dm-c".to_string(), "dm-b".to_string()],
    );
}

fn parity_repost_row(
    topic_id: &str,
    author_pubkey: &str,
    object_id: &str,
    source_object_id: &str,
    created_at: i64,
) -> ObjectProjectionRow {
    ObjectProjectionRow {
        object_kind: "repost".into(),
        author_pubkey: author_pubkey.to_string(),
        payload_ref: PayloadRef::InlineText {
            text: String::new(),
        },
        content: Some(String::new()),
        source_blob_hash: None,
        source_docs_author: None,
        repost_of: Some(RepostSourceSnapshotV1 {
            source_object_id: EnvelopeId::from(source_object_id),
            source_topic_id: TopicId::new("kukuri:topic:parity-repost-source"),
            source_author_pubkey: kukuri_core::Pubkey::from("c".repeat(64).as_str()),
            source_object_kind: "post".into(),
            content: "source body".into(),
            attachments: Vec::new(),
            reply_to_object_id: None,
            root_id: None,
            content_labels: Vec::new(),
        }),
        ..parity_projection_row(topic_id, "public", object_id, created_at)
    }
}

async fn author_reposts_scenario<S: Store + ProjectionStore>(store: &S) -> Vec<Vec<String>> {
    let topic = "kukuri:topic:parity-repost";
    let me = "a".repeat(64);
    let other = "b".repeat(64);
    for row in [
        parity_repost_row(topic, &me, "repost-old", "source-1", 10),
        parity_repost_row(topic, &me, "repost-new", "source-1", 20),
        parity_repost_row(topic, &me, "repost-other-source", "source-2", 30),
        parity_repost_row(topic, &other, "repost-other-author", "source-1", 40),
        parity_repost_row(
            "kukuri:topic:parity-repost-elsewhere",
            &me,
            "repost-other-topic",
            "source-1",
            50,
        ),
        // repost ではない行は、同じ著者・topic でも対象にならない。
        parity_projection_row(topic, "public", "plain-post", 60),
    ] {
        ObjectProjectionStore::put_object_projection(store, row)
            .await
            .expect("ObjectProjectionStore::put_object_projection");
    }
    let mut results = Vec::new();
    for (source, limit) in [
        ("source-1", 10usize),
        ("source-1", 1),
        ("source-2", 10),
        ("missing", 10),
        ("source-1", 0),
    ] {
        let rows = ObjectProjectionStore::find_author_reposts_of(
            store,
            topic,
            me.as_str(),
            &EnvelopeId::from(source),
            limit,
        )
        .await
        .expect("ObjectProjectionStore::find_author_reposts_of");
        results.push(
            rows.into_iter()
                .map(|row| row.object_id.as_str().to_string())
                .collect(),
        );
    }
    results
}

// #1239: 自分の既存の repost の検索は、両実装で同じ結果を返す(新しい順、著者・topic・repost 元で絞る)。
#[tokio::test]
async fn author_reposts_lookup_matches_between_backends() {
    let sqlite = SqliteStore::connect_memory().await.expect("sqlite store");
    let memory = MemoryStore::default();
    let from_sqlite = author_reposts_scenario(&sqlite).await;
    let from_memory = author_reposts_scenario(&memory).await;
    assert_eq!(from_sqlite, from_memory);
    assert_eq!(
        from_sqlite,
        vec![
            vec!["repost-new".to_string(), "repost-old".to_string()],
            vec!["repost-new".to_string()],
            vec!["repost-other-source".to_string()],
            vec![],
            vec![],
        ],
    );
}

// #1239: SQLite の検索が、repost 元の式の索引を使う(全行の scan にならない)。
#[tokio::test]
async fn author_reposts_lookup_uses_the_repost_source_index() {
    let sqlite = SqliteStore::connect_memory().await.expect("sqlite store");
    let plan = sqlx::query(
        r#"
        EXPLAIN QUERY PLAN
        SELECT object_id
        FROM object_index_cache
        WHERE object_kind = 'repost'
          AND topic_id = ?1
          AND author_pubkey = ?2
          AND json_extract(repost_of_json, '$.source_object_id') = ?3
        ORDER BY created_at DESC
        LIMIT ?4
        "#,
    )
    .bind("kukuri:topic:parity-repost")
    .bind("a".repeat(64))
    .bind("source-1")
    .bind(10_i64)
    .fetch_all(sqlite.pool())
    .await
    .expect("explain query plan");
    let detail = plan
        .iter()
        .map(|row| sqlx::Row::get::<String, _>(row, "detail"))
        .collect::<Vec<_>>()
        .join(" / ");
    assert!(
        detail.contains("idx_object_index_cache_repost_source"),
        "the lookup must use the repost source index, got: {detail}"
    );
}
