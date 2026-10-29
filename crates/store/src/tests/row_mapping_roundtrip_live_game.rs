//! WP-S6 T4【contract】row_mapping round-trip 層 — live / game 系
//! (後続 WP-H1 の安全網)。
//!
//! 対象: live_session_cache(row_to_live_session_projection —
//! viewer_count はテーブル列ではなく live_presence_cache の COUNT を返す
//! SELECT 計算列。ended は常に 0)/ game_room_cache
//! (row_to_game_room_projection — scores_json / metaverse_json)。
//! viewer_count は必ず trait メソッド経由で読む(生 SQL で
//! live_session_cache を読まない)。
//! 分割元の全体説明は row_mapping_roundtrip.rs の冒頭を参照。

use super::*;
use kukuri_core::{
    ActionSessionV1, DomeCustomizationV1, DomeInstanceStatusV1, DomePresetRefV1, GameRoomKind,
    GameRoomStatus, GameScoreEntry, LiveSessionStatus, MetaverseAssetKind, MetaverseAssetRef,
    MetaverseDomeV1, MetaversePrimitive, MetaverseRoomChatMessageV1, MetaverseRoomSpawnV1,
    MetaverseRoomStateV1, SpatialContextV1, TopicId,
};

// ---------------------------------------------------------------------------
// live_session_cache(row_to_live_session_projection + viewer_count 計算列)
// ---------------------------------------------------------------------------

const LIVE_TOPIC: &str = "kukuri:topic:live-rt";

/// max fixture: ended_at=Some。viewer_count=99 は永続化されず、読み出しは
/// live_presence_cache の COUNT に置き換わる(このテストでは 2)。
fn live_session_max() -> LiveSessionProjectionRow {
    LiveSessionProjectionRow {
        session_id: "session-max".into(),
        revision: 5,
        topic_id: LIVE_TOPIC.into(),
        channel_id: "ch:live".into(),
        host_pubkey: "a".repeat(64),
        title: "配信タイトル 🎥".into(),
        description: "説明 description".into(),
        status: LiveSessionStatus::Live,
        started_at: 3_000,
        ended_at: Some(4_000),
        updated_at: 3_500,
        source_replica_id: ReplicaId::new("topic::kukuri:topic:live-rt"),
        source_key: "live/session-max/manifest".into(),
        manifest_blob_hash: BlobHash::new("a".repeat(64)),
        derived_at: 3_600,
        projection_version: 5,
        viewer_count: 99,
        start_post_id: Some(EnvelopeId::from("post-start")),
    }
}

/// min fixture: ended_at=None・presence なし → viewer_count=0。
fn live_session_min() -> LiveSessionProjectionRow {
    LiveSessionProjectionRow {
        session_id: "session-min".into(),
        revision: 1,
        topic_id: LIVE_TOPIC.into(),
        channel_id: "public".into(),
        host_pubkey: "b".repeat(64),
        title: String::new(),
        description: String::new(),
        status: LiveSessionStatus::Scheduled,
        started_at: 2_000,
        ended_at: None,
        updated_at: 2_000,
        source_replica_id: ReplicaId::new("topic::kukuri:topic:live-rt"),
        source_key: "live/session-min/manifest".into(),
        manifest_blob_hash: BlobHash::new("b".repeat(64)),
        derived_at: 2_100,
        projection_version: 1,
        viewer_count: 0,
        start_post_id: None,
    }
}

// #1840 AC-4: 配信中の行だけを、新しく始まった順に返す(終えた行は外れ、別の channel の行は返さない)。本番の SQLite と
// 試験の MemoryStore で同じ。
#[tokio::test]
async fn live_now_sessions_are_the_live_rows_of_the_channel_newest_first() {
    let sqlite = SqliteStore::connect_memory().await.expect("sqlite store");
    assert_live_now_sessions(&sqlite).await;
    assert_live_now_sessions(&MemoryStore::default()).await;
}

async fn assert_live_now_sessions(store: &dyn LiveGameProjectionStore) {
    const THREAD: &str = "pc-1700000000000-0123456789abcdef";
    let row = |id: &str, revision: i64, started_at: i64, status: LiveSessionStatus| {
        LiveSessionProjectionRow {
            session_id: id.into(),
            revision,
            channel_id: THREAD.into(),
            status,
            started_at,
            ..live_session_min()
        }
    };
    for live in [
        row("old", 1, 1_000, LiveSessionStatus::Live),
        row("ended", 1, 2_000, LiveSessionStatus::Ended),
        row("new", 1, 3_000, LiveSessionStatus::Live),
        LiveSessionProjectionRow {
            session_id: "elsewhere".into(),
            started_at: 4_000,
            status: LiveSessionStatus::Live,
            ..live_session_min()
        },
    ] {
        store
            .upsert_live_session_cache(live)
            .await
            .expect("upsert live session");
    }
    let ids = |rows: Vec<LiveSessionProjectionRow>| {
        rows.into_iter()
            .map(|row| row.session_id)
            .collect::<Vec<_>>()
    };
    let live_now = |limit| store.list_channel_live_now_sessions(LIVE_TOPIC, THREAD, limit);
    assert_eq!(ids(live_now(8).await.expect("live now")), ["new", "old"]);
    assert_eq!(ids(live_now(1).await.expect("live now")), ["new"]);
    // presence の結び付けは、session の channel だけを読む(別の topic の session は無い扱い)。
    let channel = |topic, session| store.live_session_channel(topic, session);
    assert_eq!(
        channel(LIVE_TOPIC, "old")
            .await
            .expect("channel")
            .as_deref(),
        Some(THREAD)
    );
    assert_eq!(
        channel("kukuri:topic:other", "old").await.expect("channel"),
        None
    );
    store
        .upsert_live_session_cache(row("new", 2, 3_000, LiveSessionStatus::Ended))
        .await
        .expect("end the live session");
    assert_eq!(ids(live_now(8).await.expect("live now")), ["old"]);
}

/// ended fixture: status=Ended は presence が居ても viewer_count=0。
fn live_session_ended() -> LiveSessionProjectionRow {
    LiveSessionProjectionRow {
        session_id: "session-ended".into(),
        revision: 2,
        topic_id: LIVE_TOPIC.into(),
        channel_id: "ch:live".into(),
        host_pubkey: "c".repeat(64),
        title: "終了済み".into(),
        description: "ended".into(),
        status: LiveSessionStatus::Ended,
        started_at: 1_000,
        ended_at: Some(1_500),
        updated_at: 1_500,
        source_replica_id: ReplicaId::new("topic::kukuri:topic:live-rt"),
        source_key: "live/session-ended/manifest".into(),
        manifest_blob_hash: BlobHash::new("c".repeat(64)),
        derived_at: 1_600,
        projection_version: 2,
        viewer_count: 0,
        start_post_id: None,
    }
}

#[tokio::test]
async fn live_session_roundtrip_preserves_all_columns_and_computes_viewer_count() {
    let store = SqliteStore::connect_memory().await.expect("sqlite store");
    LiveGameProjectionStore::upsert_live_session_cache(&store, live_session_max())
        .await
        .expect("upsert max session");
    LiveGameProjectionStore::upsert_live_session_cache(&store, live_session_min())
        .await
        .expect("upsert min session");
    LiveGameProjectionStore::upsert_live_session_cache(&store, live_session_ended())
        .await
        .expect("upsert ended session");

    // viewer_count の分子は (topic_id, channel_id, session_id) 完全一致の
    // presence のみ。channel / topic 違いは数えない。
    LiveGameProjectionStore::upsert_live_presence(
        &store,
        LIVE_TOPIC,
        "ch:live",
        "session-max",
        "viewer-1",
        10_000,
        3_000,
    )
    .await
    .expect("presence viewer-1");
    LiveGameProjectionStore::upsert_live_presence(
        &store,
        LIVE_TOPIC,
        "ch:live",
        "session-max",
        "viewer-2",
        10_000,
        3_001,
    )
    .await
    .expect("presence viewer-2");
    // channel 不一致 → 数えない。
    LiveGameProjectionStore::upsert_live_presence(
        &store,
        LIVE_TOPIC,
        "ch:other",
        "session-max",
        "viewer-3",
        10_000,
        3_002,
    )
    .await
    .expect("presence viewer-3");
    // topic 不一致 → 数えない。
    LiveGameProjectionStore::upsert_live_presence(
        &store,
        "kukuri:topic:live-other",
        "ch:live",
        "session-max",
        "viewer-4",
        10_000,
        3_003,
    )
    .await
    .expect("presence viewer-4");
    // ended セッションは presence が居ても 0。
    LiveGameProjectionStore::upsert_live_presence(
        &store,
        LIVE_TOPIC,
        "ch:live",
        "session-ended",
        "viewer-5",
        10_000,
        3_004,
    )
    .await
    .expect("presence viewer-5");

    let listed =
        LiveGameProjectionStore::list_channel_live_sessions(&store, LIVE_TOPIC, "ch:live", 100)
            .await
            .expect("list live sessions");

    // put 時の viewer_count=99 は破棄され、COUNT(=2)が返る。
    let mut expected_max = live_session_max();
    expected_max.viewer_count = 2;
    // None で put した ended_at は None のまま読み出される
    // (WP-B16 で NULL decode quirk を解消)。
    // started_at DESC, session_id DESC の順序ごと全列固定。
    assert_eq!(listed, vec![expected_max, live_session_ended()]);
    assert_eq!(
        LiveGameProjectionStore::list_channel_live_sessions(&store, LIVE_TOPIC, "public", 100,)
            .await
            .expect("list public live sessions"),
        vec![live_session_min()]
    );
}

// ---------------------------------------------------------------------------
// game_room_cache(row_to_game_room_projection)
// ---------------------------------------------------------------------------

const GAME_TOPIC: &str = "kukuri:topic:game-rt";

/// metaverse_json に入る非自明な入れ子 fixture(全 Option=Some・全 Vec 非空)。
fn metaverse_state() -> MetaverseRoomStateV1 {
    let mut customization = DomeCustomizationV1::default();
    customization.persistent_props[0].prop_id = "shared-1".into();
    customization.persistent_props[0].asset_ref = Some(MetaverseAssetRef {
        kind: MetaverseAssetKind::Glb,
        blob_hash: "c".repeat(64),
        mime_type: Some("model/gltf-binary".into()),
        size_bytes: Some(2_048),
        name: Some("チェア".into()),
        budget_metadata: Some(kukuri_core::MetaverseAssetBudgetMetadataV1 {
            stored_bytes: 2_048,
            model_triangles: 12,
            model_primitives: 1,
            ..Default::default()
        }),
    });
    customization.persistent_props[0].primitive_fallback = MetaversePrimitive::Cube;
    customization.persistent_props[0].position = [1, 2, 3];
    customization.persistent_props[0].rotation = [0, 90, 0];
    MetaverseRoomStateV1 {
        world_version: 4,
        instance_id: "room-max".into(),
        spatial_context: SpatialContextV1::Channel {
            topic_id: TopicId::new(GAME_TOPIC),
            channel_id: kukuri_core::ChannelId::new("ch:game"),
        },
        instance_generation: 1,
        instance_status: DomeInstanceStatusV1::Active,
        relationship_detach: None,
        replacement_instance_id: None,
        preset_ref: DomePresetRefV1 {
            preset_id: "preset-max".into(),
            owner_pubkey: "b".repeat(64).into(),
            revision: 1,
            manifest_blob_hash: "f".repeat(64),
            manifest_mime: "application/vnd.kukuri.dome-preset+json".into(),
            manifest_bytes: 4096,
        },
        session_id: "room-max".into(),
        max_peers: Some(16),
        dome: MetaverseDomeV1 {
            spec_id: "fixed_dome_v1".into(),
            customization,
        },
        default_spawn: MetaverseRoomSpawnV1 {
            position: [0, 0, 5],
            rotation: [0, 180, 0],
        },
        asset_refs: vec![MetaverseAssetRef {
            kind: MetaverseAssetKind::Vrm,
            blob_hash: "e".repeat(64),
            mime_type: None,
            size_bytes: Some(4_096),
            name: None,
            budget_metadata: Some(kukuri_core::MetaverseAssetBudgetMetadataV1 {
                stored_bytes: 4_096,
                model_triangles: 24,
                model_primitives: 1,
                ..Default::default()
            }),
        }],
        chat_history: vec![MetaverseRoomChatMessageV1 {
            room_id: "room-max".into(),
            message_id: "chat-1".into(),
            author_peer_id: "peer-1".into(),
            display_name: Some("ゲスト".into()),
            body: "こんにちは 🎮".into(),
            created_at: 1_700_000_000_100,
        }],
    }
}

/// max fixture: phase_label=Some・scores 2 要素(負値含む)・
/// room_kind=MetaverseRoom・metaverse=Some。
fn game_room_max() -> GameRoomProjectionRow {
    GameRoomProjectionRow {
        room_id: "room-max".into(),
        score_revision: None,
        topic_id: GAME_TOPIC.into(),
        channel_id: "ch:game".into(),
        host_pubkey: "b".repeat(64),
        title: "ゲーム部屋 🎮".into(),
        description: "対戦募集".into(),
        status: GameRoomStatus::Running,
        phase_label: Some("第2ラウンド".into()),
        scores: vec![
            GameScoreEntry {
                participant_id: "p1".into(),
                label: "プレイヤー1".into(),
                score: 1_200,
            },
            GameScoreEntry {
                participant_id: "p2".into(),
                label: "プレイヤー2".into(),
                score: -50,
            },
        ],
        room_kind: GameRoomKind::MetaverseRoom,
        metaverse: Some(metaverse_state()),
        // 列の往復だけを見る(種類との組合せは app-api が検証する)。
        action: Some(ActionSessionV1 {
            application_id: "com.example.arena".into(),
            host_endpoint_id: "e".repeat(64),
            max_players: 4,
        }),
        updated_at: 6_000,
        source_replica_id: ReplicaId::new("topic::kukuri:topic:game-rt"),
        source_key: "game/room-max/manifest".into(),
        manifest_blob_hash: BlobHash::new("f".repeat(64)),
        derived_at: 6_100,
        projection_version: 6,
    }
}

/// min fixture: phase_label=None・scores 空 Vec・room_kind=ScoreGame・
/// metaverse=None。
fn game_room_min() -> GameRoomProjectionRow {
    GameRoomProjectionRow {
        room_id: "room-min".into(),
        score_revision: Some(1),
        topic_id: GAME_TOPIC.into(),
        channel_id: "public".into(),
        host_pubkey: "c".repeat(64),
        title: String::new(),
        description: String::new(),
        status: GameRoomStatus::Waiting,
        phase_label: None,
        scores: Vec::new(),
        room_kind: GameRoomKind::ScoreGame,
        metaverse: None,
        action: None,
        updated_at: 5_000,
        source_replica_id: ReplicaId::new("topic::kukuri:topic:game-rt"),
        source_key: "game/room-min/manifest".into(),
        manifest_blob_hash: BlobHash::new("0".repeat(64)),
        derived_at: 5_100,
        projection_version: 1,
    }
}

#[tokio::test]
async fn game_room_roundtrip_preserves_all_columns() {
    let store = SqliteStore::connect_memory().await.expect("sqlite store");
    let max = game_room_max();
    let min = game_room_min();
    LiveGameProjectionStore::upsert_game_room_cache(&store, min.clone())
        .await
        .expect("upsert min room");
    LiveGameProjectionStore::upsert_game_room_cache(&store, max.clone())
        .await
        .expect("upsert max room");

    // None で put した phase_label / metaverse は None のまま読み出される
    // (WP-B16 で NULL decode quirk を解消)。
    // 読み出しは list のみ。主キー updated_at DESC の順序ごと全列固定
    // (副キー room_id DESC の tie-break はここでは未行使 —
    // T6 の pagination テストと T8 の backend_parity が担保)。
    // action を持つ行は application ID の一覧からだけ読める(#1978)。
    assert_eq!(
        LiveGameProjectionStore::list_channel_action_rooms(
            &store,
            GAME_TOPIC,
            "ch:game",
            "com.example.arena",
            None,
            100,
        )
        .await
        .expect("list action rooms"),
        vec![max]
    );
    assert!(
        LiveGameProjectionStore::list_channel_game_rooms(&store, GAME_TOPIC, "ch:game", 100)
            .await
            .expect("list game rooms")
            .is_empty()
    );
    assert_eq!(
        LiveGameProjectionStore::list_channel_game_rooms(&store, GAME_TOPIC, "public", 100,)
            .await
            .expect("list public game rooms"),
        vec![min]
    );
}

/// #1978: action session の行は ScoreGame・Dome の一覧に混ざらず、application ID ごとに更新の新しい順で、前の頁の
/// 最後の行より後ろから読める。sqlite と memory で同じ結果になる(Web は action の行を保存しないので比べない)。
#[tokio::test]
async fn action_rooms_are_listed_per_application_after_the_previous_page() {
    async fn scenario<S: LiveGameProjectionStore>(store: &S) -> Vec<Vec<String>> {
        let mut rows = vec![game_room_min()];
        for (room_id, application_id, updated_at) in [
            ("room-arena-1", "com.example.arena", 110),
            ("room-arena-2", "com.example.arena", 120),
            ("room-arena-3", "com.example.arena", 120),
            ("room-puzzle", "com.example.puzzle", 130),
        ] {
            let mut row = game_room_min();
            row.room_id = room_id.into();
            row.room_kind = GameRoomKind::ActionSession;
            row.updated_at = updated_at;
            row.action = Some(ActionSessionV1 {
                application_id: application_id.into(),
                host_endpoint_id: "e".repeat(64),
                max_players: 4,
            });
            rows.push(row);
        }
        for row in rows {
            store
                .upsert_game_room_cache(row)
                .await
                .expect("upsert room");
        }
        let ids = |rows: Vec<GameRoomProjectionRow>| {
            rows.into_iter().map(|row| row.room_id).collect::<Vec<_>>()
        };
        let arena = |before| {
            store.list_channel_action_rooms(GAME_TOPIC, "public", "com.example.arena", before, 2)
        };
        vec![
            ids(store
                .list_channel_game_rooms(GAME_TOPIC, "public", 100)
                .await
                .expect("score games")),
            ids(arena(None).await.expect("first page")),
            // 同じ updated_at の行が頁の境界にまたがっても、重複も欠落もしない。
            ids(arena(Some((120, "room-arena-2".into())))
                .await
                .expect("next page")),
        ]
    }
    let sqlite = SqliteStore::connect_memory().await.expect("sqlite store");
    let from_sqlite = scenario(&sqlite).await;
    assert_eq!(from_sqlite, scenario(&MemoryStore::default()).await);
    assert_eq!(
        from_sqlite,
        [
            vec!["room-min".to_string()],
            vec!["room-arena-3".to_string(), "room-arena-2".to_string()],
            vec!["room-arena-1".to_string()],
        ]
    );
}
