//! WP-S6 T8: sqlite/memory 差分ハーネス(backend parity)。
//!
//! production は SqliteStore、テストダブルは MemoryStore という非対称に対し、
//! `run_scenario` 形式(`async fn xxx_scenario<S: Store + ProjectionStore>(store: &S)`)で
//! 同一操作列 → 同一クエリ列の結果を両実装から回収し、`assert_eq!` で完全一致を固定する。
//! WP-H1(ProjectionStore の sub-trait 分割 + sqlite/memory 委譲ボイラープレート解消)の
//! 安全網であり、ハーネス自体が trait 境界互換(`Store + ProjectionStore` 合成で両実装が
//! 呼び出せること)の検証を兼ねる。golden/fixture ファイルは持たない。
//!
//! - 期待値の生リテラル(sanity assert)は sqlite 実装の観測挙動から採取した具体値
//!   (両実装が「空同士で一致」する tautology を防ぐため、parity assert とは別に置く)。
//! - トレイトメソッド呼び出しは UFCS 徹底(Store と ProjectionStore の同名メソッド回避 +
//!   H1 の trait 分割後に機械置換で追従するため)。
//!
//! ハーネスから意図的に除外した既知 divergence(fix が T7 の範囲外のため。いずれも実測済み):
//! - filtered 系(list_topic_timeline_filtered)の limit == 0 または allowed_channels 空 +
//!   Some(cursor): sqlite は cursor をそのまま next_cursor にエコーするが、memory は None を
//!   返す(memory/projections.rs 未修正のため limit=0 / 空集合はハーネス対象外)。
//! - 同一 envelope の重複 put_envelope: memory は topic_objects の Vec に id が重複蓄積し
//!   timeline に同一 envelope が 2 回現れる(sqlite は ON CONFLICT upsert で 1 行)。
//!   本ハーネスでは envelope の再 put を行わない(memory/envelopes.rs 未修正)。
//! - sqlite(sqlx-sqlite)の NULL decode quirk(NULL TEXT → `Ok("")`、NULL INTEGER →
//!   `Ok(0)`)は WP-B16 で解消済み: row_mapping の Option 列は `try_get::<Option<T>>` で
//!   読むため put(None) → get は両実装とも None になり、fixture は実 NULL を投入できる。
//!   残る既知 divergence は空文字入力のみ: sqlite 側の trim + filter 列
//!   (object_index_cache / bookmarked_posts の root/reply、notifications の参照列など)は
//!   put(Some("")) → get で None になるが memory は Some("") のまま返す。
//!   実書き込み経路は Option バインドで '' を書かないため、本ハーネスの fixture も
//!   これらの列に空文字を投入しない。

use super::*;

use kukuri_core::{
    AssetRef, AssetRole, CustomReactionAssetSnapshotV1, DomeCustomizationV1, DomeInstanceStatusV1,
    DomePresetRefV1, GameRoomKind, GameRoomStatus, GameScoreEntry, KukuriEnvelope,
    LiveSessionStatus, MetaverseAssetKind, MetaverseAssetRef, MetaverseDomeV1, MetaversePrimitive,
    MetaverseRoomChatMessageV1, MetaverseRoomSpawnV1, MetaverseRoomStateV1, RepostSourceSnapshotV1,
    SpatialContextV1, TopicId,
};

mod lists;
mod live_game;
mod pagination;

/// cursor 走査の暴走(divergence による無限ループ)を止める上限ページ数。
const MAX_PAGES: usize = 8;

/// topic タグ + 任意の root/reply_to タグを持つ手組み envelope。
/// kind は post_content() の JSON パース経路に入らない値にする(tags のみで完結)。
fn parity_envelope(
    topic: &str,
    id: &str,
    created_at: i64,
    root: Option<&str>,
    reply_to: Option<&str>,
) -> KukuriEnvelope {
    let mut tags = vec![vec!["topic".to_string(), topic.to_string()]];
    if let Some(root) = root {
        tags.push(vec!["root".to_string(), root.to_string()]);
    }
    if let Some(reply_to) = reply_to {
        tags.push(vec!["reply_to".to_string(), reply_to.to_string()]);
    }
    KukuriEnvelope {
        id: EnvelopeId::from(id),
        pubkey: "a".repeat(64).into(),
        created_at,
        kind: "parity-post".to_string(),
        tags,
        content: format!("body:{id}"),
        sig: "f".repeat(128),
    }
}

fn parity_projection_row(
    topic_id: &str,
    channel_id: &str,
    object_id: &str,
    created_at: i64,
) -> ObjectProjectionRow {
    let hash = BlobHash::new(format!("{created_at:064x}"));
    ObjectProjectionRow {
        object_id: EnvelopeId::from(object_id),
        topic_id: topic_id.to_string(),
        channel_id: channel_id.to_string(),
        author_pubkey: "a".repeat(64),
        created_at,
        object_kind: "post".into(),
        root_object_id: None,
        reply_to_object_id: None,
        payload_ref: PayloadRef::BlobText {
            hash: hash.clone(),
            mime: "text/plain".into(),
            bytes: 8,
        },
        content: Some(format!("content:{object_id}")),
        attachments: Vec::new(),
        repost_of: None,
        content_labels: Vec::new(),
        source_replica_id: ReplicaId::new(format!("topic::{topic_id}")),
        source_key: format!("objects/{object_id}/header"),
        source_envelope_id: EnvelopeId::from(object_id),
        source_blob_hash: Some(hash),
        source_docs_author: None,
        derived_at: created_at,
        projection_version: 2,
    }
}

fn parity_dm_message(dm_id: &str, message_id: &str, created_at: i64) -> DirectMessageMessageRow {
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

fn parity_notification(
    notification_id: &str,
    received_at: i64,
    kind: NotificationKind,
) -> NotificationRow {
    NotificationRow {
        notification_id: notification_id.into(),
        recipient_pubkey: "0".repeat(64),
        kind,
        actor_pubkey: "1".repeat(64),
        source_envelope_id: None,
        source_replica_id: None,
        topic_id: None,
        channel_id: None,
        object_id: None,
        dm_id: None,
        message_id: None,
        preview_text: None,
        content_labels: None,
        created_at: received_at - 5,
        received_at,
        read_at: None,
    }
}

fn parity_live_session(
    session_id: &str,
    topic_id: &str,
    channel_id: &str,
    status: LiveSessionStatus,
    started_at: i64,
) -> LiveSessionProjectionRow {
    LiveSessionProjectionRow {
        session_id: session_id.into(),
        topic_id: topic_id.into(),
        channel_id: channel_id.into(),
        host_pubkey: "c".repeat(64),
        title: format!("title:{session_id}"),
        description: format!("desc:{session_id}"),
        status,
        started_at,
        ended_at: None,
        updated_at: started_at + 1,
        source_replica_id: ReplicaId::new(format!("topic::{topic_id}")),
        source_key: format!("live/{session_id}/manifest"),
        manifest_blob_hash: BlobHash::new("9".repeat(64)),
        derived_at: started_at + 2,
        projection_version: 1,
        viewer_count: 0,
    }
}

fn parity_game_room(
    room_id: &str,
    topic_id: &str,
    status: GameRoomStatus,
    updated_at: i64,
) -> GameRoomProjectionRow {
    GameRoomProjectionRow {
        room_id: room_id.into(),
        topic_id: topic_id.into(),
        channel_id: "ch-game".into(),
        host_pubkey: "d".repeat(64),
        title: format!("room:{room_id}"),
        description: format!("desc:{room_id}"),
        status,
        phase_label: None,
        scores: Vec::new(),
        room_kind: GameRoomKind::ScoreGame,
        metaverse: None,
        updated_at,
        source_replica_id: ReplicaId::new(format!("topic::{topic_id}")),
        source_key: format!("games/{room_id}/manifest"),
        manifest_blob_hash: BlobHash::new("8".repeat(64)),
        derived_at: updated_at + 1,
        projection_version: 1,
    }
}

fn parity_game_scores() -> Vec<GameScoreEntry> {
    vec![
        GameScoreEntry {
            participant_id: "player-1".into(),
            label: "Player One".into(),
            score: 120,
        },
        GameScoreEntry {
            participant_id: "player-2".into(),
            label: "Player Two".into(),
            score: -5,
        },
    ]
}

fn parity_metaverse_state() -> MetaverseRoomStateV1 {
    let mut customization = DomeCustomizationV1::default();
    customization.persistent_props[0].prop_id = "shared-cube".into();
    customization.persistent_props[0].asset_ref = Some(MetaverseAssetRef {
        kind: MetaverseAssetKind::Glb,
        blob_hash: "7".repeat(64),
        mime_type: Some("model/gltf-binary".into()),
        size_bytes: Some(2048),
        name: Some("cube".into()),
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
        instance_id: "room-meta".into(),
        spatial_context: SpatialContextV1::Topic {
            topic_id: TopicId::new("kukuri:topic:parity"),
        },
        instance_generation: 1,
        instance_status: DomeInstanceStatusV1::Active,
        relationship_detach: None,
        replacement_instance_id: None,
        preset_ref: DomePresetRefV1 {
            preset_id: "preset-meta".into(),
            owner_pubkey: "a".repeat(64).into(),
            revision: 1,
            manifest_blob_hash: "9".repeat(64),
            manifest_mime: "application/vnd.kukuri.dome-preset+json".into(),
            manifest_bytes: 1024,
        },
        session_id: "room-meta".into(),
        max_peers: Some(8),
        dome: MetaverseDomeV1 {
            spec_id: "fixed_dome_v1".into(),
            customization,
        },
        default_spawn: MetaverseRoomSpawnV1 {
            position: [0, 0, 0],
            rotation: [0, 0, 0],
        },
        asset_refs: Vec::new(),
        chat_history: vec![MetaverseRoomChatMessageV1 {
            room_id: "room-meta".into(),
            message_id: "chat-1".into(),
            author_peer_id: "peer-1".into(),
            display_name: None,
            body: "hello".into(),
            created_at: 80,
        }],
    }
}

fn parity_reaction(
    replica: &str,
    target_object_id: &str,
    reaction_id: &str,
    author_pubkey: &str,
    normalized_key: &str,
    emoji: &str,
    updated_at: i64,
) -> ReactionProjectionRow {
    ReactionProjectionRow {
        source_replica_id: ReplicaId::new(replica),
        target_object_id: EnvelopeId::from(target_object_id),
        reaction_id: EnvelopeId::from(reaction_id),
        author_pubkey: author_pubkey.to_string(),
        created_at: updated_at - 1,
        updated_at,
        reaction_key_kind: ReactionKeyKind::Emoji,
        normalized_reaction_key: normalized_key.to_string(),
        emoji: Some(emoji.to_string()),
        custom_asset_id: None,
        custom_asset_snapshot: None,
        status: ObjectStatus::Active,
        source_key: format!("reactions/{reaction_id}"),
        source_envelope_id: EnvelopeId::from(reaction_id),
        derived_at: updated_at,
        projection_version: 1,
    }
}

/// custom_asset 種のリアクション(snapshot JSON 列の往復も差分対象に含める)。
fn parity_reaction_custom(
    replica: &str,
    target_object_id: &str,
    reaction_id: &str,
    author_pubkey: &str,
    updated_at: i64,
) -> ReactionProjectionRow {
    let mut row = parity_reaction(
        replica,
        target_object_id,
        reaction_id,
        author_pubkey,
        "custom:asset-1",
        "",
        updated_at,
    );
    row.reaction_key_kind = ReactionKeyKind::CustomAsset;
    row.emoji = None;
    row.custom_asset_id = Some("asset-1".into());
    row.custom_asset_snapshot = Some(CustomReactionAssetSnapshotV1 {
        asset_id: "asset-1".into(),
        owner_pubkey: "e".repeat(64).into(),
        blob_hash: BlobHash::new("4".repeat(64)),
        search_key: "flame".into(),
        mime: "image/png".into(),
        bytes: 777,
        width: 32,
        height: 32,
    });
    row
}

fn parity_bookmarked_post(source_object_id: &str, bookmarked_at: i64) -> BookmarkedPostRow {
    BookmarkedPostRow {
        source_object_id: EnvelopeId::from(source_object_id),
        source_envelope_id: EnvelopeId::from(source_object_id),
        source_replica_id: ReplicaId::new("topic::parity-bookmarks"),
        topic_id: "kukuri:topic:parity-bookmarks".into(),
        channel_id: "public".into(),
        author_pubkey: "e".repeat(64),
        created_at: bookmarked_at - 10,
        object_kind: "post".into(),
        payload_ref: PayloadRef::BlobText {
            hash: BlobHash::new("6".repeat(64)),
            mime: "text/plain".into(),
            bytes: 4,
        },
        content: None,
        attachments: Vec::new(),
        reply_to_object_id: None,
        root_object_id: None,
        repost_of: None,
        bookmarked_at,
    }
}

/// 全 Option = Some / Vec 非空のブックマーク投稿(JSON 列・Option 列の往復も差分対象)。
fn parity_bookmarked_post_max(source_object_id: &str, bookmarked_at: i64) -> BookmarkedPostRow {
    let mut row = parity_bookmarked_post(source_object_id, bookmarked_at);
    row.content = Some(format!("content:{source_object_id}"));
    row.attachments = vec![AssetRef {
        hash: BlobHash::new("3".repeat(64)),
        mime: "image/png".into(),
        bytes: 42,
        role: AssetRole::ImageOriginal,
    }];
    row.reply_to_object_id = Some(EnvelopeId::from("bp-parent"));
    row.root_object_id = Some(EnvelopeId::from("bp-thread-root"));
    row.repost_of = Some(RepostSourceSnapshotV1 {
        source_object_id: EnvelopeId::from("bp-origin"),
        source_topic_id: TopicId::new("kukuri:topic:origin"),
        source_author_pubkey: "b".repeat(64).into(),
        source_object_kind: "post".into(),
        content: "original body".into(),
        attachments: Vec::new(),
        reply_to_object_id: None,
        root_id: Some(EnvelopeId::from("bp-origin-root")),
        content_labels: Vec::new(),
    });
    row
}

fn parity_custom_reaction(
    asset_id: &str,
    search_key: &str,
    bookmarked_at: i64,
) -> BookmarkedCustomReactionRow {
    BookmarkedCustomReactionRow {
        asset_id: asset_id.into(),
        owner_pubkey: "e".repeat(64),
        blob_hash: BlobHash::new("5".repeat(64)),
        search_key: search_key.into(),
        mime: "image/png".into(),
        bytes: 321,
        width: 64,
        height: 48,
        bookmarked_at,
    }
}

fn parity_outbox(dm_id: &str, message_id: &str, created_at: i64) -> DirectMessageOutboxRow {
    DirectMessageOutboxRow {
        dm_id: dm_id.into(),
        message_id: message_id.into(),
        peer_pubkey: "b".repeat(64),
        frame_blob_hash: BlobHash::new("2".repeat(64)),
        created_at,
        last_attempt_at: None,
    }
}

fn parity_conversation(
    dm_id: &str,
    peer_pubkey: &str,
    updated_at: i64,
) -> DirectMessageConversationRow {
    DirectMessageConversationRow {
        dm_id: dm_id.into(),
        peer_pubkey: peer_pubkey.into(),
        updated_at,
        last_message_at: Some(updated_at - 1),
        last_message_id: Some(format!("last:{dm_id}")),
        last_message_preview: Some(format!("preview:{dm_id}")),
    }
}

fn parity_follow_edge(
    subject_pubkey: &str,
    target_pubkey: &str,
    status: FollowEdgeStatus,
    updated_at: i64,
    envelope_id: &str,
) -> kukuri_core::FollowEdge {
    kukuri_core::FollowEdge {
        subject_pubkey: subject_pubkey.to_string().into(),
        target_pubkey: target_pubkey.to_string().into(),
        status,
        updated_at,
        envelope_id: EnvelopeId::from(envelope_id),
    }
}
