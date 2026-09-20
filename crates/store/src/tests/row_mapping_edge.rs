//! WP-S6 T5: row_mapping エッジ/legacy 層の characterization テスト。
//!
//! put 経由では到達できない行(legacy 別名・空文字・未知 enum 値)を
//! `store.pool()` への生 SQL INSERT で作り、trait の読み出しメソッドが
//! 既存 DB に残っている行をどう解釈するかを「観測した現挙動」の生リテラルで固定する。
//! 後続 WP-H1(ProjectionStore の sub-trait 分割 + 委譲解消)の安全網であり、
//! テストが落ちた場合はテストではなく変更側を疑うこと。
//!
//! 生 INSERT の列リストは migrations/*.up.sql と後続 ALTER を実読して全列を明示している:
//! - game_room_cache: 20260315(14 列)+ channel_id(20260319010000)
//!   + room_kind / metaverse_json(20260527)= 17 列
//! - bookmarked_custom_reactions: 20260328010000(8 列)+ search_key(20260330)= 9 列
//! - object_index_cache: 20260312(14 列)+ channel_id(20260319010000)
//!   + object_kind / repost_of_json(20260328)+ attachments_json(20260411)= 18 列
//! - bookmarked_posts: 20260330020000(15 列)
//!
//! live_session_cache は viewer_count が SELECT 計算列のため生 SQL 読みの対象にしない(付録 C)。

use super::*;
use kukuri_core::{GameRoomKind, GameRoomStatus};

/// game_room_cache へ全 17 列を明示した生 INSERT を行う。
/// status / room_kind は put(upsert_game_room_cache)では現行表記しか書けないため、
/// legacy 別名・空文字・未知値はこの経路でのみ作れる。
async fn insert_raw_game_room(
    store: &SqliteStore,
    room_id: &str,
    topic_id: &str,
    status: &str,
    room_kind: &str,
    updated_at: i64,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        INSERT INTO game_room_cache (
          room_id, topic_id, host_pubkey, title, description, status, phase_label,
          scores_json, updated_at, source_replica_id, source_key, manifest_blob_hash,
          derived_at, projection_version, channel_id, room_kind, metaverse_json
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
        "#,
    )
    .bind(room_id)
    .bind(topic_id)
    .bind("f".repeat(64))
    .bind("レガシー部屋")
    .bind("edge-case room")
    .bind(status)
    .bind(Option::<String>::None) // phase_label
    .bind("[]") // scores_json
    .bind(updated_at)
    .bind("replica-game-edge")
    .bind(format!("games/{room_id}/manifest"))
    .bind(format!("blob-{room_id}"))
    .bind(updated_at) // derived_at
    .bind(1_i64) // projection_version
    .bind("public") // channel_id
    .bind(room_kind)
    .bind(Option::<String>::None) // metaverse_json
    .execute(store.pool())
    .await?;
    Ok(())
}

/// bookmarked_custom_reactions へ全 9 列を明示した生 INSERT を行う。
/// search_key の空白のみ値は put では作れる余地があるが、既存 DB 行の読み出し互換を
/// put 実装から独立に固定するため生 SQL で書く。
async fn insert_raw_bookmarked_custom_reaction(
    store: &SqliteStore,
    asset_id: &str,
    search_key: &str,
    bookmarked_at: i64,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        INSERT INTO bookmarked_custom_reactions (
          asset_id, owner_pubkey, blob_hash, mime, bytes, width, height, bookmarked_at,
          search_key
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
        "#,
    )
    .bind(asset_id)
    .bind("a".repeat(64))
    .bind(format!("blob-{asset_id}"))
    .bind("image/png")
    .bind(2048_i64)
    .bind(64_i64)
    .bind(32_i64)
    .bind(bookmarked_at)
    .bind(search_key)
    .execute(store.pool())
    .await?;
    Ok(())
}

/// object_index_cache へ全 18 列を明示した生 INSERT を行う。
/// root/reply の空文字は put(バインドが Option 経由で NULL になる)では作れない。
async fn insert_raw_object_index(
    store: &SqliteStore,
    object_id: &str,
    root_object_id: Option<&str>,
    reply_to_object_id: Option<&str>,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        INSERT INTO object_index_cache (
          object_id, topic_id, author_pubkey, created_at, root_object_id, reply_to_object_id,
          payload_ref_json, content, source_replica_id, source_key, source_envelope_id,
          source_blob_hash, derived_at, projection_version, channel_id, object_kind,
          repost_of_json, attachments_json
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)
        "#,
    )
    .bind(object_id)
    .bind("topic-object-edge")
    .bind("b".repeat(64))
    .bind(1000_i64)
    .bind(root_object_id)
    .bind(reply_to_object_id)
    .bind(r#"{"InlineText":{"text":"hello"}}"#)
    .bind(Some("hello"))
    .bind("replica-object-edge")
    .bind(format!("objects/{object_id}/header"))
    .bind(format!("env-{object_id}"))
    .bind(Option::<String>::None) // source_blob_hash
    .bind(1001_i64) // derived_at
    .bind(2_i64) // projection_version
    .bind("public") // channel_id
    .bind("post") // object_kind
    .bind(Option::<String>::None) // repost_of_json
    .bind("[]") // attachments_json
    .execute(store.pool())
    .await?;
    Ok(())
}

/// bookmarked_posts へ全 15 列を明示した生 INSERT を行う。
async fn insert_raw_bookmarked_post(
    store: &SqliteStore,
    source_object_id: &str,
    root_object_id: Option<&str>,
    reply_to_object_id: Option<&str>,
    bookmarked_at: i64,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        INSERT INTO bookmarked_posts (
          source_object_id, source_envelope_id, source_replica_id, topic_id, channel_id,
          author_pubkey, created_at, object_kind, payload_ref_json, content, attachments_json,
          reply_to_object_id, root_object_id, repost_of_json, bookmarked_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
        "#,
    )
    .bind(source_object_id)
    .bind(format!("env-{source_object_id}"))
    .bind("replica-bookmark-edge")
    .bind("topic-bookmark-edge")
    .bind("public")
    .bind("c".repeat(64))
    .bind(500_i64)
    .bind("post")
    .bind(r#"{"InlineText":{"text":"ブックマーク本文"}}"#)
    .bind(Some("ブックマーク本文"))
    .bind("[]")
    .bind(reply_to_object_id)
    .bind(root_object_id)
    .bind(Option::<String>::None) // repost_of_json
    .bind(bookmarked_at)
    .execute(store.pool())
    .await?;
    Ok(())
}

/// ケース 1: game_room_cache.status の legacy 別名 'open' / 'in_progress' / 'finished' が
/// Waiting / Running / Ended として読めること(row_mapping.rs parse_game_status)。
#[tokio::test]
async fn game_room_status_legacy_aliases_map_to_current_variants() {
    let store = SqliteStore::connect_memory().await.expect("sqlite store");
    let topic_id = "topic-game-legacy";
    insert_raw_game_room(&store, "room-open", topic_id, "open", "score_game", 30)
        .await
        .expect("insert legacy open");
    insert_raw_game_room(
        &store,
        "room-progress",
        topic_id,
        "in_progress",
        "score_game",
        20,
    )
    .await
    .expect("insert legacy in_progress");
    insert_raw_game_room(
        &store,
        "room-finished",
        topic_id,
        "finished",
        "score_game",
        10,
    )
    .await
    .expect("insert legacy finished");

    let rooms = LiveGameProjectionStore::list_topic_game_rooms(&store, topic_id)
        .await
        .expect("list game rooms");

    // updated_at DESC, room_id DESC の順で返る。
    let statuses: Vec<(&str, GameRoomStatus)> = rooms
        .iter()
        .map(|room| (room.room_id.as_str(), room.status.clone()))
        .collect();
    assert_eq!(
        statuses,
        vec![
            ("room-open", GameRoomStatus::Waiting),
            ("room-progress", GameRoomStatus::Running),
            ("room-finished", GameRoomStatus::Ended),
        ]
    );

    // 先頭行は全列を生リテラルで固定(列名対応・キャストの回帰検出)。
    assert_eq!(
        rooms[0],
        GameRoomProjectionRow {
            room_id: "room-open".into(),
            topic_id: "topic-game-legacy".into(),
            channel_id: "public".into(),
            host_pubkey: "f".repeat(64),
            title: "レガシー部屋".into(),
            description: "edge-case room".into(),
            status: GameRoomStatus::Waiting,
            // NULL の phase_label は None(WP-B16 で Option decode を適正化。
            // sqlx-sqlite の try_get::<String> は NULL でも Ok("") を返すため、
            // Option 列は try_get::<Option<String>> で読む)。
            phase_label: None,
            scores: vec![],
            room_kind: GameRoomKind::ScoreGame,
            metaverse: None,
            updated_at: 30,
            source_replica_id: ReplicaId::new("replica-game-edge"),
            source_key: "games/room-open/manifest".into(),
            manifest_blob_hash: BlobHash::new("blob-room-open"),
            derived_at: 30,
            projection_version: 1,
        }
    );
}

/// ケース 2: game_room_cache.room_kind = ''(空文字)は ScoreGame として読める
/// (row_mapping.rs parse_game_room_kind の "" | "score_game" 分岐)。
#[tokio::test]
async fn game_room_kind_empty_string_maps_to_score_game() {
    let store = SqliteStore::connect_memory().await.expect("sqlite store");
    let topic_id = "topic-game-kind-empty";
    insert_raw_game_room(&store, "room-kind-empty", topic_id, "waiting", "", 10)
        .await
        .expect("insert empty room_kind");

    let rooms = LiveGameProjectionStore::list_topic_game_rooms(&store, topic_id)
        .await
        .expect("list game rooms");

    assert_eq!(rooms.len(), 1);
    assert_eq!(rooms[0].room_id, "room-kind-empty");
    assert_eq!(rooms[0].room_kind, GameRoomKind::ScoreGame);
    assert_eq!(rooms[0].status, GameRoomStatus::Waiting);
}

/// ケース 3: bookmarked_custom_reactions.search_key = ''(および空白のみ)は
/// 読み出し時に asset_id へフォールバックする
/// (row_mapping.rs row_to_bookmarked_custom_reaction の search_key フォールバック分岐)。
#[tokio::test]
async fn bookmarked_custom_reaction_blank_search_key_falls_back_to_asset_id() {
    let store = SqliteStore::connect_memory().await.expect("sqlite store");
    insert_raw_bookmarked_custom_reaction(&store, "asset-empty-key", "", 200)
        .await
        .expect("insert empty search_key");
    insert_raw_bookmarked_custom_reaction(&store, "asset-blank-key", "   ", 100)
        .await
        .expect("insert whitespace search_key");

    let rows = ReactionBookmarkStore::list_bookmarked_custom_reactions(&store)
        .await
        .expect("list bookmarked custom reactions");

    // bookmarked_at DESC, asset_id DESC の順。search_key は DB 上 '' / '   ' のままだが
    // 読み出し結果では asset_id に置き換わる。
    assert_eq!(
        rows,
        vec![
            BookmarkedCustomReactionRow {
                asset_id: "asset-empty-key".into(),
                owner_pubkey: "a".repeat(64),
                blob_hash: BlobHash::new("blob-asset-empty-key"),
                search_key: "asset-empty-key".into(),
                mime: "image/png".into(),
                bytes: 2048,
                width: 64,
                height: 32,
                bookmarked_at: 200,
            },
            BookmarkedCustomReactionRow {
                asset_id: "asset-blank-key".into(),
                owner_pubkey: "a".repeat(64),
                blob_hash: BlobHash::new("blob-asset-blank-key"),
                search_key: "asset-blank-key".into(),
                mime: "image/png".into(),
                bytes: 2048,
                width: 64,
                height: 32,
                bookmarked_at: 100,
            },
        ]
    );
}

/// ケース 4: object_index_cache の root_object_id / reply_to_object_id = ''(および空白のみ)は
/// None として読める(row_mapping.rs row_to_object_projection の
/// root_object_id / reply_to_object_id trim+filter 分岐)。
#[tokio::test]
async fn object_projection_blank_root_and_reply_map_to_none() {
    let store = SqliteStore::connect_memory().await.expect("sqlite store");
    insert_raw_object_index(&store, "obj-blank-refs", Some(""), Some(""))
        .await
        .expect("insert empty root/reply");
    insert_raw_object_index(&store, "obj-whitespace-refs", Some("   "), Some("  "))
        .await
        .expect("insert whitespace root/reply");

    let blank =
        ObjectProjectionStore::get_object_projection(&store, &EnvelopeId::from("obj-blank-refs"))
            .await
            .expect("get blank refs")
            .expect("row exists");
    assert_eq!(
        blank,
        ObjectProjectionRow {
            object_id: EnvelopeId::from("obj-blank-refs"),
            topic_id: "topic-object-edge".into(),
            channel_id: "public".into(),
            author_pubkey: "b".repeat(64),
            created_at: 1000,
            object_kind: "post".into(),
            root_object_id: None,
            reply_to_object_id: None,
            payload_ref: PayloadRef::InlineText {
                text: "hello".into(),
            },
            content: Some("hello".into()),
            attachments: vec![],
            repost_of: None,
            content_labels: Vec::new(),
            source_replica_id: ReplicaId::new("replica-object-edge"),
            source_key: "objects/obj-blank-refs/header".into(),
            source_envelope_id: EnvelopeId::from("env-obj-blank-refs"),
            // NULL の source_blob_hash は None(WP-B16 で Option decode を適正化)。
            source_blob_hash: None,
            source_docs_author: None,
            derived_at: 1001,
            projection_version: 2,
        }
    );

    let whitespace = ObjectProjectionStore::get_object_projection(
        &store,
        &EnvelopeId::from("obj-whitespace-refs"),
    )
    .await
    .expect("get whitespace refs")
    .expect("row exists");
    assert_eq!(whitespace.root_object_id, None);
    assert_eq!(whitespace.reply_to_object_id, None);
}

/// ケース 5: bookmarked_posts の root_object_id / reply_to_object_id は ''(空文字)も
/// NULL も None として読める(row_mapping.rs row_to_bookmarked_post — WP-B16 で
/// object_index_cache 側(ケース 4)と同じ trim + filter に統一し、非対称を解消)。
#[tokio::test]
async fn bookmarked_post_blank_root_and_reply_map_to_none() {
    let store = SqliteStore::connect_memory().await.expect("sqlite store");
    insert_raw_bookmarked_post(&store, "bp-empty-refs", Some(""), Some(""), 300)
        .await
        .expect("insert empty root/reply");
    insert_raw_bookmarked_post(&store, "bp-null-refs", None, None, 200)
        .await
        .expect("insert null root/reply");

    let rows = ReactionBookmarkStore::list_bookmarked_posts(&store)
        .await
        .expect("list bookmarked posts");
    assert_eq!(rows.len(), 2);

    // bookmarked_at DESC 順の先頭 = 空文字行。'' は None に落ちる(ケース 4 と対称)。
    assert_eq!(
        rows[0],
        BookmarkedPostRow {
            source_object_id: EnvelopeId::from("bp-empty-refs"),
            source_envelope_id: EnvelopeId::from("env-bp-empty-refs"),
            source_replica_id: ReplicaId::new("replica-bookmark-edge"),
            topic_id: "topic-bookmark-edge".into(),
            channel_id: "public".into(),
            author_pubkey: "c".repeat(64),
            created_at: 500,
            object_kind: "post".into(),
            payload_ref: PayloadRef::InlineText {
                text: "ブックマーク本文".into(),
            },
            content: Some("ブックマーク本文".into()),
            attachments: vec![],
            reply_to_object_id: None,
            root_object_id: None,
            repost_of: None,
            bookmarked_at: 300,
        }
    );

    // NULL も None(WP-B16 の Option decode 適正化。'' と同じ結果に正規化される)。
    assert_eq!(rows[1].source_object_id, EnvelopeId::from("bp-null-refs"));
    assert_eq!(rows[1].root_object_id, None);
    assert_eq!(rows[1].reply_to_object_id, None);
}

/// ケース 6a: 未知の status 値('nonsense')が残った行は list_topic_game_rooms が Err になる
/// (row_mapping.rs parse_game_status の bail)。
#[tokio::test]
async fn game_room_unknown_status_fails_listing() {
    let store = SqliteStore::connect_memory().await.expect("sqlite store");
    let topic_id = "topic-game-unknown-status";
    insert_raw_game_room(
        &store,
        "room-bad-status",
        topic_id,
        "nonsense",
        "score_game",
        10,
    )
    .await
    .expect("insert unknown status");

    let err = LiveGameProjectionStore::list_topic_game_rooms(&store, topic_id)
        .await
        .expect_err("unknown status must fail");
    assert_eq!(err.to_string(), "unknown game room status: nonsense");
}

/// ケース 6b: 未知の room_kind 値('vr_world')が残った行も list_topic_game_rooms が Err になる
/// (row_mapping.rs parse_game_room_kind の bail)。
#[tokio::test]
async fn game_room_unknown_room_kind_fails_listing() {
    let store = SqliteStore::connect_memory().await.expect("sqlite store");
    let topic_id = "topic-game-unknown-kind";
    insert_raw_game_room(&store, "room-bad-kind", topic_id, "waiting", "vr_world", 10)
        .await
        .expect("insert unknown room_kind");

    let err = LiveGameProjectionStore::list_topic_game_rooms(&store, topic_id)
        .await
        .expect_err("unknown room_kind must fail");
    assert_eq!(err.to_string(), "unknown game room kind: vr_world");
}
