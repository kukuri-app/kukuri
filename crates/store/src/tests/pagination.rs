//! WP-S6 T6: pagination の characterization テスト。
//!
//! 後続 WP-H1(ProjectionStore の trait 分割 + keyset ページングのヘルパ化)の安全網として、
//! (1) 純関数 5 つ(apply_desc_cursor / apply_asc_cursor / apply_desc_projection_cursor /
//!     apply_asc_projection_cursor / apply_desc_direct_message_cursor)の keyset 比較・
//!     tie-break・next_cursor 生成規則を表テストで、
//! (2) SQL 側 keyset(Store::list_topic_timeline / Store::list_thread /
//!     DirectMessageStore::list_direct_message_messages)を複数ページ走査で、
//! それぞれ観測した現挙動の生リテラルで固定する。
//!
//! 固定している契約(観測した現挙動):
//! - desc: created_at < cursor.created_at、同 created_at(tie)は object_id < で通過
//!   (asc は両方とも逆向き)。
//! - next_cursor は items.len() == limit のときのみ Some(最終要素)。データがちょうど
//!   limit で尽きても Some を返す(「幻の次ページ」— 次ページが空で終端を検知する)。
//! - limit == 0 は常に空ページ + next_cursor None(len == limit == 0 だが last が無い)。
//! - 純関数はソートせず入力順を保持する(ソート済み入力は呼び出し側の契約。
//!   apply_desc_cursor で代表固定 — 5 実装は構造同一)。
//! - DM 版の tie-break は message_id の文字列比較で、cursor.object_id には
//!   EnvelopeId::from(message_id) が入る。
//! - thread は root を CASE 式で先頭に固定し、cursor.created_at が root の created_at を
//!   超えた後のページには root が現れない。
//!
//! filtered 系(list_topic_timeline_filtered / list_thread_filtered)は
//! tests/sqlite_projection.rs に既存テストがあるためここでは扱わない。

use super::*;

use crate::pagination::{
    apply_asc_cursor, apply_asc_projection_cursor, apply_desc_cursor,
    apply_desc_direct_message_cursor, apply_desc_projection_cursor,
};
use kukuri_core::{KukuriEnvelope, Pubkey};

fn cursor_at(created_at: i64, object_id: &str) -> TimelineCursor {
    TimelineCursor {
        created_at,
        object_id: EnvelopeId::from(object_id),
    }
}

fn envelope_row(id: &str, created_at: i64) -> KukuriEnvelope {
    KukuriEnvelope {
        id: EnvelopeId::from(id),
        pubkey: Pubkey::from("a".repeat(64)),
        created_at,
        kind: "post".into(),
        tags: Vec::new(),
        content: format!("content:{id}"),
        sig: format!("sig:{id}"),
    }
}

/// topic タグ付きの envelope(put_envelope で topic_objects / object_threads に載る)。
/// 署名検証は store 層では行われないため、created_at と envelope_id を自由に固定できる。
fn topic_envelope(topic: &str, id: &str, created_at: i64) -> KukuriEnvelope {
    let mut envelope = envelope_row(id, created_at);
    envelope.tags = vec![vec!["topic".to_string(), topic.to_string()]];
    envelope
}

fn thread_reply_envelope(topic: &str, root_id: &str, id: &str, created_at: i64) -> KukuriEnvelope {
    let mut envelope = topic_envelope(topic, id, created_at);
    envelope.kind = "comment".into();
    envelope
        .tags
        .push(vec!["root".to_string(), root_id.to_string()]);
    envelope
        .tags
        .push(vec!["reply_to".to_string(), root_id.to_string()]);
    envelope
}

fn projection_row(id: &str, created_at: i64) -> ObjectProjectionRow {
    ObjectProjectionRow {
        object_id: EnvelopeId::from(id),
        topic_id: "kukuri:topic:pagination".into(),
        channel_id: "public".into(),
        author_pubkey: "b".repeat(64),
        created_at,
        object_kind: "post".into(),
        root_object_id: None,
        reply_to_object_id: None,
        payload_ref: PayloadRef::InlineText {
            text: id.to_string(),
        },
        content: Some(id.to_string()),
        attachments: Vec::new(),
        repost_of: None,
        content_labels: Vec::new(),
        source_replica_id: ReplicaId::new("topic::pagination"),
        source_key: format!("objects/{id}/header"),
        source_envelope_id: EnvelopeId::from(id),
        source_blob_hash: None,
        source_docs_author: None,
        derived_at: created_at,
        projection_version: 2,
    }
}

fn dm_message_row(dm_id: &str, message_id: &str, created_at: i64) -> DirectMessageMessageRow {
    DirectMessageMessageRow {
        dm_id: dm_id.into(),
        message_id: message_id.into(),
        sender_pubkey: "a".repeat(64),
        recipient_pubkey: "b".repeat(64),
        created_at,
        text: Some(format!("text:{message_id}")),
        reply_to_message_id: None,
        attachment_manifest: None,
        outgoing: false,
        acked_at: None,
    }
}

fn envelope_ids(page: &Page<KukuriEnvelope>) -> Vec<&str> {
    page.items.iter().map(|item| item.id.as_str()).collect()
}

fn projection_ids(page: &Page<ObjectProjectionRow>) -> Vec<&str> {
    page.items
        .iter()
        .map(|item| item.object_id.as_str())
        .collect()
}

fn message_ids(page: &Page<DirectMessageMessageRow>) -> Vec<&str> {
    page.items
        .iter()
        .map(|item| item.message_id.as_str())
        .collect()
}

// ---------------------------------------------------------------------------
// 純関数: apply_desc_cursor(KukuriEnvelope、desc keyset)
// ---------------------------------------------------------------------------

#[test]
fn apply_desc_cursor_keyset_and_tie_break_table() {
    // 入力は DESC(created_at DESC, object_id DESC)ソート済み前提。
    // created_at=20 に同時刻 3 行を置き、tie-break(object_id <)を明示的に固定する。
    let input = || {
        vec![
            envelope_row("id-e", 30),
            envelope_row("id-d", 20),
            envelope_row("id-c", 20),
            envelope_row("id-b", 20),
            envelope_row("id-a", 10),
        ]
    };
    let cases: Vec<(Option<TimelineCursor>, Vec<&str>)> = vec![
        // cursor=None は先頭ページ(全件、入力順のまま)
        (None, vec!["id-e", "id-d", "id-c", "id-b", "id-a"]),
        // created_at < 30 のみ通過
        (
            Some(cursor_at(30, "id-e")),
            vec!["id-d", "id-c", "id-b", "id-a"],
        ),
        // 同時刻 20 は object_id < "id-d" のみ通過
        (Some(cursor_at(20, "id-d")), vec!["id-c", "id-b", "id-a"]),
        (Some(cursor_at(20, "id-c")), vec!["id-b", "id-a"]),
        // 同時刻の残りが cursor.object_id 以上 → created_at < 20 の行だけ残る
        (Some(cursor_at(20, "id-b")), vec!["id-a"]),
        // 末尾要素を cursor にすると空
        (Some(cursor_at(10, "id-a")), vec![]),
    ];
    for (cursor, expected) in cases {
        let page = apply_desc_cursor(input(), cursor.clone(), 10);
        assert_eq!(envelope_ids(&page), expected, "cursor={cursor:?}");
        // limit(10)未満なので next_cursor は常に None
        assert_eq!(page.next_cursor, None, "cursor={cursor:?}");
    }
}

#[test]
fn apply_desc_cursor_next_cursor_only_on_full_page() {
    let input = || {
        vec![
            envelope_row("id-c", 30),
            envelope_row("id-b", 20),
            envelope_row("id-a", 10),
        ]
    };

    // items.len() == limit → next_cursor = Some(最終要素)
    let page = apply_desc_cursor(input(), None, 2);
    assert_eq!(envelope_ids(&page), ["id-c", "id-b"]);
    assert_eq!(page.next_cursor, Some(cursor_at(20, "id-b")));

    // items.len() < limit → None
    let page = apply_desc_cursor(input(), None, 4);
    assert_eq!(envelope_ids(&page), ["id-c", "id-b", "id-a"]);
    assert_eq!(page.next_cursor, None);

    // データがちょうど limit で尽きても Some(幻の次ページ)。次ページは空で終端検知。
    let page = apply_desc_cursor(input(), None, 3);
    assert_eq!(envelope_ids(&page), ["id-c", "id-b", "id-a"]);
    assert_eq!(page.next_cursor, Some(cursor_at(10, "id-a")));
    let phantom = apply_desc_cursor(input(), page.next_cursor, 3);
    assert!(phantom.items.is_empty());
    assert_eq!(phantom.next_cursor, None);

    // limit == 0 → 空ページ + next_cursor None
    let page = apply_desc_cursor(input(), None, 0);
    assert!(page.items.is_empty());
    assert_eq!(page.next_cursor, None);
}

#[test]
fn apply_desc_cursor_preserves_input_order_without_sorting() {
    // 関数はソートしない(ソート済み入力は呼び出し側の契約)。未ソート入力でも
    // フィルタ + take のみで入力順が保持される現挙動を固定する。
    let input = vec![
        envelope_row("id-a", 10),
        envelope_row("id-c", 30),
        envelope_row("id-b", 20),
    ];
    let page = apply_desc_cursor(input, Some(cursor_at(30, "id-c")), 10);
    // ソートするなら ["id-b", "id-a"] になるはずだが、入力順のまま返る
    assert_eq!(envelope_ids(&page), ["id-a", "id-b"]);
}

// ---------------------------------------------------------------------------
// 純関数: apply_asc_cursor(KukuriEnvelope、asc keyset)
// ---------------------------------------------------------------------------

#[test]
fn apply_asc_cursor_keyset_and_tie_break_table() {
    // 入力は ASC(created_at ASC, object_id ASC)ソート済み前提。
    // created_at=20 に同時刻 3 行を置き、tie-break(object_id >)を明示的に固定する。
    let input = || {
        vec![
            envelope_row("id-a", 10),
            envelope_row("id-b", 20),
            envelope_row("id-c", 20),
            envelope_row("id-d", 20),
            envelope_row("id-e", 30),
        ]
    };
    let cases: Vec<(Option<TimelineCursor>, Vec<&str>)> = vec![
        (None, vec!["id-a", "id-b", "id-c", "id-d", "id-e"]),
        // created_at > 10 のみ通過
        (
            Some(cursor_at(10, "id-a")),
            vec!["id-b", "id-c", "id-d", "id-e"],
        ),
        // 同時刻 20 は object_id > "id-b" のみ通過
        (Some(cursor_at(20, "id-b")), vec!["id-c", "id-d", "id-e"]),
        (Some(cursor_at(20, "id-c")), vec!["id-d", "id-e"]),
        // 同時刻の残りが cursor.object_id 以下 → created_at > 20 の行だけ残る
        (Some(cursor_at(20, "id-d")), vec!["id-e"]),
        (Some(cursor_at(30, "id-e")), vec![]),
    ];
    for (cursor, expected) in cases {
        let page = apply_asc_cursor(input(), cursor.clone(), 10);
        assert_eq!(envelope_ids(&page), expected, "cursor={cursor:?}");
        assert_eq!(page.next_cursor, None, "cursor={cursor:?}");
    }
}

#[test]
fn apply_asc_cursor_next_cursor_only_on_full_page() {
    let input = || {
        vec![
            envelope_row("id-a", 10),
            envelope_row("id-b", 20),
            envelope_row("id-c", 30),
        ]
    };

    let page = apply_asc_cursor(input(), None, 2);
    assert_eq!(envelope_ids(&page), ["id-a", "id-b"]);
    assert_eq!(page.next_cursor, Some(cursor_at(20, "id-b")));

    let page = apply_asc_cursor(input(), None, 4);
    assert_eq!(envelope_ids(&page), ["id-a", "id-b", "id-c"]);
    assert_eq!(page.next_cursor, None);

    // ちょうど尽きても Some(幻の次ページ)
    let page = apply_asc_cursor(input(), None, 3);
    assert_eq!(envelope_ids(&page), ["id-a", "id-b", "id-c"]);
    assert_eq!(page.next_cursor, Some(cursor_at(30, "id-c")));
    let phantom = apply_asc_cursor(input(), page.next_cursor, 3);
    assert!(phantom.items.is_empty());
    assert_eq!(phantom.next_cursor, None);

    let page = apply_asc_cursor(input(), None, 0);
    assert!(page.items.is_empty());
    assert_eq!(page.next_cursor, None);
}

// ---------------------------------------------------------------------------
// 純関数: apply_desc_projection_cursor / apply_asc_projection_cursor
// ---------------------------------------------------------------------------

#[test]
fn apply_desc_projection_cursor_keyset_and_tie_break_table() {
    let input = || {
        vec![
            projection_row("pr-e", 30),
            projection_row("pr-d", 20),
            projection_row("pr-c", 20),
            projection_row("pr-b", 20),
            projection_row("pr-a", 10),
        ]
    };
    let cases: Vec<(Option<TimelineCursor>, Vec<&str>)> = vec![
        (None, vec!["pr-e", "pr-d", "pr-c", "pr-b", "pr-a"]),
        (
            Some(cursor_at(30, "pr-e")),
            vec!["pr-d", "pr-c", "pr-b", "pr-a"],
        ),
        // 同時刻 20 は object_id < "pr-d" のみ通過
        (Some(cursor_at(20, "pr-d")), vec!["pr-c", "pr-b", "pr-a"]),
        (Some(cursor_at(20, "pr-b")), vec!["pr-a"]),
        (Some(cursor_at(10, "pr-a")), vec![]),
    ];
    for (cursor, expected) in cases {
        let page = apply_desc_projection_cursor(input(), cursor.clone(), 10);
        assert_eq!(projection_ids(&page), expected, "cursor={cursor:?}");
        assert_eq!(page.next_cursor, None, "cursor={cursor:?}");
    }
}

#[test]
fn apply_desc_projection_cursor_next_cursor_only_on_full_page() {
    let input = || {
        vec![
            projection_row("pr-c", 30),
            projection_row("pr-b", 20),
            projection_row("pr-a", 10),
        ]
    };

    let page = apply_desc_projection_cursor(input(), None, 2);
    assert_eq!(projection_ids(&page), ["pr-c", "pr-b"]);
    assert_eq!(page.next_cursor, Some(cursor_at(20, "pr-b")));

    let page = apply_desc_projection_cursor(input(), None, 4);
    assert_eq!(projection_ids(&page), ["pr-c", "pr-b", "pr-a"]);
    assert_eq!(page.next_cursor, None);

    // ちょうど尽きても Some(幻の次ページ)
    let page = apply_desc_projection_cursor(input(), None, 3);
    assert_eq!(projection_ids(&page), ["pr-c", "pr-b", "pr-a"]);
    assert_eq!(page.next_cursor, Some(cursor_at(10, "pr-a")));
    let phantom = apply_desc_projection_cursor(input(), page.next_cursor, 3);
    assert!(phantom.items.is_empty());
    assert_eq!(phantom.next_cursor, None);

    let page = apply_desc_projection_cursor(input(), None, 0);
    assert!(page.items.is_empty());
    assert_eq!(page.next_cursor, None);
}

#[test]
fn apply_asc_projection_cursor_keyset_and_tie_break_table() {
    let input = || {
        vec![
            projection_row("pr-a", 10),
            projection_row("pr-b", 20),
            projection_row("pr-c", 20),
            projection_row("pr-d", 20),
            projection_row("pr-e", 30),
        ]
    };
    let cases: Vec<(Option<TimelineCursor>, Vec<&str>)> = vec![
        (None, vec!["pr-a", "pr-b", "pr-c", "pr-d", "pr-e"]),
        (
            Some(cursor_at(10, "pr-a")),
            vec!["pr-b", "pr-c", "pr-d", "pr-e"],
        ),
        // 同時刻 20 は object_id > "pr-b" のみ通過
        (Some(cursor_at(20, "pr-b")), vec!["pr-c", "pr-d", "pr-e"]),
        (Some(cursor_at(20, "pr-d")), vec!["pr-e"]),
        (Some(cursor_at(30, "pr-e")), vec![]),
    ];
    for (cursor, expected) in cases {
        let page = apply_asc_projection_cursor(input(), cursor.clone(), 10);
        assert_eq!(projection_ids(&page), expected, "cursor={cursor:?}");
        assert_eq!(page.next_cursor, None, "cursor={cursor:?}");
    }
}

#[test]
fn apply_asc_projection_cursor_next_cursor_only_on_full_page() {
    let input = || {
        vec![
            projection_row("pr-a", 10),
            projection_row("pr-b", 20),
            projection_row("pr-c", 30),
        ]
    };

    let page = apply_asc_projection_cursor(input(), None, 2);
    assert_eq!(projection_ids(&page), ["pr-a", "pr-b"]);
    assert_eq!(page.next_cursor, Some(cursor_at(20, "pr-b")));

    let page = apply_asc_projection_cursor(input(), None, 4);
    assert_eq!(projection_ids(&page), ["pr-a", "pr-b", "pr-c"]);
    assert_eq!(page.next_cursor, None);

    // ちょうど尽きても Some(幻の次ページ)
    let page = apply_asc_projection_cursor(input(), None, 3);
    assert_eq!(projection_ids(&page), ["pr-a", "pr-b", "pr-c"]);
    assert_eq!(page.next_cursor, Some(cursor_at(30, "pr-c")));
    let phantom = apply_asc_projection_cursor(input(), page.next_cursor, 3);
    assert!(phantom.items.is_empty());
    assert_eq!(phantom.next_cursor, None);

    let page = apply_asc_projection_cursor(input(), None, 0);
    assert!(page.items.is_empty());
    assert_eq!(page.next_cursor, None);
}

// ---------------------------------------------------------------------------
// 純関数: apply_desc_direct_message_cursor(tie は message_id 比較)
// ---------------------------------------------------------------------------

#[test]
fn apply_desc_direct_message_cursor_keyset_and_message_id_tie_break_table() {
    // 入力は DESC(created_at DESC, message_id DESC)ソート済み前提。
    // tie-break は message_id と cursor.object_id の文字列比較。
    let input = || {
        vec![
            dm_message_row("dm-1", "msg-e", 30),
            dm_message_row("dm-1", "msg-d", 20),
            dm_message_row("dm-1", "msg-c", 20),
            dm_message_row("dm-1", "msg-b", 20),
            dm_message_row("dm-1", "msg-a", 10),
        ]
    };
    let cases: Vec<(Option<TimelineCursor>, Vec<&str>)> = vec![
        (None, vec!["msg-e", "msg-d", "msg-c", "msg-b", "msg-a"]),
        (
            Some(cursor_at(30, "msg-e")),
            vec!["msg-d", "msg-c", "msg-b", "msg-a"],
        ),
        // 同時刻 20 は message_id < "msg-d" のみ通過
        (
            Some(cursor_at(20, "msg-d")),
            vec!["msg-c", "msg-b", "msg-a"],
        ),
        (Some(cursor_at(20, "msg-b")), vec!["msg-a"]),
        (Some(cursor_at(10, "msg-a")), vec![]),
    ];
    for (cursor, expected) in cases {
        let page = apply_desc_direct_message_cursor(input(), cursor.clone(), 10);
        assert_eq!(message_ids(&page), expected, "cursor={cursor:?}");
        assert_eq!(page.next_cursor, None, "cursor={cursor:?}");
    }
}

#[test]
fn apply_desc_direct_message_cursor_next_cursor_uses_message_id_as_envelope_id() {
    let input = || {
        vec![
            dm_message_row("dm-1", "msg-c", 30),
            dm_message_row("dm-1", "msg-b", 20),
            dm_message_row("dm-1", "msg-a", 10),
        ]
    };

    // next_cursor.object_id には最終要素の message_id が EnvelopeId として入る
    let page = apply_desc_direct_message_cursor(input(), None, 2);
    assert_eq!(message_ids(&page), ["msg-c", "msg-b"]);
    assert_eq!(
        page.next_cursor,
        Some(TimelineCursor {
            created_at: 20,
            object_id: EnvelopeId::from("msg-b"),
        })
    );

    let page = apply_desc_direct_message_cursor(input(), None, 4);
    assert_eq!(message_ids(&page), ["msg-c", "msg-b", "msg-a"]);
    assert_eq!(page.next_cursor, None);

    // ちょうど尽きても Some(幻の次ページ)
    let page = apply_desc_direct_message_cursor(input(), None, 3);
    assert_eq!(message_ids(&page), ["msg-c", "msg-b", "msg-a"]);
    assert_eq!(page.next_cursor, Some(cursor_at(10, "msg-a")));
    let phantom = apply_desc_direct_message_cursor(input(), page.next_cursor, 3);
    assert!(phantom.items.is_empty());
    assert_eq!(phantom.next_cursor, None);

    let page = apply_desc_direct_message_cursor(input(), None, 0);
    assert!(page.items.is_empty());
    assert_eq!(page.next_cursor, None);
}

// ---------------------------------------------------------------------------
// DB 結合: SQL keyset の間接固定(connect_memory + UFCS)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sqlite_topic_timeline_multi_page_scan_fixes_tie_break_and_phantom_next_page() {
    let store = SqliteStore::connect_memory().await.expect("sqlite store");
    let topic = "kukuri:topic:pagination-timeline";
    // created_at=300 に同時刻 3 行、100 に同時刻 2 行(tie-break は envelope_id DESC)。
    // 挿入順はソート順と無関係に混ぜる。
    for (id, created_at) in [
        ("tl-200-a", 200),
        ("tl-300-b", 300),
        ("tl-100-a", 100),
        ("tl-300-c", 300),
        ("tl-100-b", 100),
        ("tl-200-b", 200),
        ("tl-300-a", 300),
    ] {
        Store::put_envelope(&store, topic_envelope(topic, id, created_at))
            .await
            .expect("put envelope");
    }
    let expected_order = [
        "tl-300-c", "tl-300-b", "tl-300-a", "tl-200-b", "tl-200-a", "tl-100-b", "tl-100-a",
    ];

    // limit=2 で next_cursor を辿って全ページ走査(ページ境界が同時刻 300 の途中に落ち、
    // SQL 側 tie-break 分岐 created_at = ?2 AND envelope_id < ?3 を通す)
    let mut pages: Vec<Page<KukuriEnvelope>> = Vec::new();
    let mut cursor: Option<TimelineCursor> = None;
    loop {
        let page = Store::list_topic_timeline(&store, topic, cursor.clone(), 2)
            .await
            .expect("walk page");
        cursor = page.next_cursor.clone();
        pages.push(page);
        if cursor.is_none() {
            break;
        }
        assert!(pages.len() < 8, "next_cursor 走査が終端しない");
    }

    assert_eq!(pages.len(), 4);
    assert_eq!(envelope_ids(&pages[0]), ["tl-300-c", "tl-300-b"]);
    assert_eq!(pages[0].next_cursor, Some(cursor_at(300, "tl-300-b")));
    // 2 ページ目先頭の具体値: 同時刻 300 の tie-break を跨いだ "tl-300-a"
    assert_eq!(envelope_ids(&pages[1]), ["tl-300-a", "tl-200-b"]);
    assert_eq!(pages[1].next_cursor, Some(cursor_at(200, "tl-200-b")));
    assert_eq!(envelope_ids(&pages[2]), ["tl-200-a", "tl-100-b"]);
    assert_eq!(pages[2].next_cursor, Some(cursor_at(100, "tl-100-b")));
    assert_eq!(envelope_ids(&pages[3]), ["tl-100-a"]);
    assert_eq!(pages[3].next_cursor, None);

    // 全ページ連結 = 重複なし・欠落なし・期待順序どおり
    let walked: Vec<&str> = pages
        .iter()
        .flat_map(|page| page.items.iter().map(|item| item.id.as_str()))
        .collect();
    assert_eq!(walked, expected_order);

    // データ総数ちょうどの limit=7 では全 7 件 + 幻の next_cursor が返り、
    // 次ページは空で終端を検知する
    let full = Store::list_topic_timeline(&store, topic, None, 7)
        .await
        .expect("full page");
    assert_eq!(envelope_ids(&full), expected_order);
    assert_eq!(full.next_cursor, Some(cursor_at(100, "tl-100-a")));
    let phantom = Store::list_topic_timeline(&store, topic, full.next_cursor.clone(), 7)
        .await
        .expect("phantom page");
    assert!(phantom.items.is_empty());
    assert_eq!(phantom.next_cursor, None);
}

#[tokio::test]
async fn sqlite_thread_pins_root_first_and_root_absent_after_cursor() {
    let store = SqliteStore::connect_memory().await.expect("sqlite store");
    let topic = "kukuri:topic:pagination-thread";
    let root_id = "th-root";
    // root(created_at=100)と同時刻で envelope_id が root より小さい reply "th-aaaa" を置く。
    // 自然順(created_at ASC, envelope_id ASC)なら "th-aaaa" が先頭になるデータで、
    // CASE 式による root 先頭固定を観測する。
    Store::put_envelope(&store, topic_envelope(topic, root_id, 100))
        .await
        .expect("put root");
    Store::put_envelope(
        &store,
        thread_reply_envelope(topic, root_id, "th-aaaa", 100),
    )
    .await
    .expect("put reply th-aaaa");
    Store::put_envelope(
        &store,
        thread_reply_envelope(topic, root_id, "th-bbbb", 150),
    )
    .await
    .expect("put reply th-bbbb");
    Store::put_envelope(
        &store,
        thread_reply_envelope(topic, root_id, "th-cccc", 200),
    )
    .await
    .expect("put reply th-cccc");
    // 同 topic の別スレッド(root 無し投稿)はこのスレッドに現れない
    Store::put_envelope(&store, topic_envelope(topic, "th-other", 120))
        .await
        .expect("put other thread");

    let root = EnvelopeId::from(root_id);
    let page1 = Store::list_thread(&store, topic, &root, None, 3)
        .await
        .expect("page1");
    // root が先頭固定(自然順なら同時刻 tie で "th-aaaa" < "th-root" が先になる)
    assert_eq!(envelope_ids(&page1), ["th-root", "th-aaaa", "th-bbbb"]);
    assert_eq!(page1.next_cursor, Some(cursor_at(150, "th-bbbb")));

    // cursor(created_at=150 > root の 100)通過後のページに root は現れない
    let page2 = Store::list_thread(&store, topic, &root, page1.next_cursor.clone(), 3)
        .await
        .expect("page2");
    assert_eq!(envelope_ids(&page2), ["th-cccc"]);
    assert_eq!(page2.next_cursor, None);
}

#[tokio::test]
async fn sqlite_direct_message_messages_multi_page_scan_fixes_message_id_tie_break() {
    let store = SqliteStore::connect_memory().await.expect("sqlite store");
    let dm_id = "dm-pagination";
    DirectMessageStore::upsert_direct_message_conversation(
        &store,
        DirectMessageConversationRow {
            dm_id: dm_id.into(),
            peer_pubkey: "b".repeat(64),
            updated_at: 300,
            last_message_at: Some(300),
            last_message_id: Some("msg-b".into()),
            last_message_preview: Some("text:msg-b".into()),
        },
    )
    .await
    .expect("upsert conversation");
    // created_at=300 と 100 に同時刻 2 行ずつ(tie-break は message_id DESC)。
    // 挿入順はソート順と無関係に混ぜる。
    for (message_id, created_at) in [
        ("msg-d", 100),
        ("msg-b", 300),
        ("msg-c", 200),
        ("msg-a", 300),
        ("msg-e", 100),
    ] {
        DirectMessageStore::put_direct_message_message(
            &store,
            dm_message_row(dm_id, message_id, created_at),
        )
        .await
        .expect("put message");
    }
    // 別 dm_id の行は走査に現れない
    DirectMessageStore::put_direct_message_message(
        &store,
        dm_message_row("dm-other", "msg-x", 250),
    )
    .await
    .expect("put other dm message");

    let page1 = DirectMessageStore::list_direct_message_messages(&store, dm_id, None, 2)
        .await
        .expect("page1");
    assert_eq!(message_ids(&page1), ["msg-b", "msg-a"]);
    // cursor.object_id には message_id が EnvelopeId として入る
    assert_eq!(
        page1.next_cursor,
        Some(TimelineCursor {
            created_at: 300,
            object_id: EnvelopeId::from("msg-a"),
        })
    );

    // 2 ページ目先頭の具体値は "msg-c"。境界が同時刻 100 の途中に落ちる
    let page2 = DirectMessageStore::list_direct_message_messages(
        &store,
        dm_id,
        page1.next_cursor.clone(),
        2,
    )
    .await
    .expect("page2");
    assert_eq!(message_ids(&page2), ["msg-c", "msg-e"]);
    assert_eq!(page2.next_cursor, Some(cursor_at(100, "msg-e")));

    // 3 ページ目は SQL 側 tie-break 分岐 created_at = ?2 AND message_id < ?3 を通る
    let page3 = DirectMessageStore::list_direct_message_messages(
        &store,
        dm_id,
        page2.next_cursor.clone(),
        2,
    )
    .await
    .expect("page3");
    assert_eq!(message_ids(&page3), ["msg-d"]);
    assert_eq!(page3.next_cursor, None);

    // 全ページ連結 = 重複なし・欠落なし・DESC 順
    let walked: Vec<&str> = [&page1, &page2, &page3]
        .into_iter()
        .flat_map(|page| page.items.iter().map(|item| item.message_id.as_str()))
        .collect();
    assert_eq!(walked, ["msg-b", "msg-a", "msg-c", "msg-e", "msg-d"]);
}
