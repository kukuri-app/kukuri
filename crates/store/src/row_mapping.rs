use anyhow::Result;
use kukuri_core::{
    BlobHash, BlockEdge, BlockEdgeStatus, EnvelopeId, FollowEdge, FollowEdgeStatus, GameRoomKind,
    GameRoomStatus, KukuriEnvelope, LiveSessionStatus, ObjectStatus, ReactionKeyKind, ReplicaId,
};
use sqlx::Row;

use crate::models::{
    AuthorRelationshipProjectionRow, BookmarkedCustomReactionRow, BookmarkedPostRow,
    DirectMessageConversationRow, DirectMessageMessageRow, DirectMessageOutboxRow,
    DirectMessageTombstoneRow, GameRoomProjectionRow, LiveSessionProjectionRow, MutedAuthorRow,
    NotificationKind, NotificationRow, ObjectProjectionRow, ReactionProjectionRow,
};

/// NULL 許容列の読み出し。sqlx-sqlite は NULL を `String` なら `Ok("")`、`i64` なら
/// `Ok(0)` にデコードするため、`try_get::<T>().ok()` では NULL が Some に化ける
/// (WP-B16 で解消した decode quirk)。Option 列は必ずこのヘルパで読むこと。
/// `.ok()` は列欠損を None に落とす従来挙動の維持。
fn opt_col<'r, T>(row: &'r sqlx::sqlite::SqliteRow, column: &str) -> Option<T>
where
    T: sqlx::Decode<'r, sqlx::Sqlite> + sqlx::Type<sqlx::Sqlite>,
{
    row.try_get::<Option<T>, _>(column).ok().flatten()
}

pub(crate) fn row_to_envelope(row: sqlx::sqlite::SqliteRow) -> Result<KukuriEnvelope> {
    Ok(KukuriEnvelope {
        id: row.get::<String, _>("envelope_id").into(),
        pubkey: row.get::<String, _>("pubkey").into(),
        created_at: row.get("created_at"),
        kind: row.get("kind"),
        content: row.get("content"),
        tags: serde_json::from_str(row.get::<String, _>("tags_json").as_str())?,
        sig: row.get("sig"),
    })
}

pub(crate) fn row_to_object_projection(
    row: sqlx::sqlite::SqliteRow,
) -> Result<ObjectProjectionRow> {
    Ok(ObjectProjectionRow {
        object_id: row.get::<String, _>("object_id").into(),
        topic_id: row.get("topic_id"),
        channel_id: row.get("channel_id"),
        author_pubkey: row.get("author_pubkey"),
        created_at: row.get("created_at"),
        object_kind: row.get("object_kind"),
        root_object_id: row
            .try_get::<String, _>("root_object_id")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(EnvelopeId::from),
        reply_to_object_id: row
            .try_get::<String, _>("reply_to_object_id")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(EnvelopeId::from),
        payload_ref: serde_json::from_str(row.get::<String, _>("payload_ref_json").as_str())?,
        content: opt_col(&row, "content"),
        attachments: row
            .try_get::<String, _>("attachments_json")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(|value| serde_json::from_str(value.as_str()))
            .transpose()?
            .unwrap_or_default(),
        repost_of: row
            .try_get::<String, _>("repost_of_json")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(|value| serde_json::from_str(value.as_str()))
            .transpose()?,
        content_labels: row
            .try_get::<String, _>("content_labels_json")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(|value| serde_json::from_str(value.as_str()))
            .transpose()?
            .unwrap_or_default(),
        source_replica_id: ReplicaId::new(row.get::<String, _>("source_replica_id")),
        source_key: row.get("source_key"),
        source_envelope_id: row.get::<String, _>("source_envelope_id").into(),
        source_blob_hash: opt_col::<String>(&row, "source_blob_hash").map(BlobHash::new),
        source_docs_author: opt_col::<String>(&row, "source_docs_author"),
        derived_at: row.get("derived_at"),
        projection_version: row.get("projection_version"),
    })
}

pub(crate) fn row_to_reaction_projection(
    row: sqlx::sqlite::SqliteRow,
) -> Result<ReactionProjectionRow> {
    Ok(ReactionProjectionRow {
        source_replica_id: ReplicaId::new(row.get::<String, _>("source_replica_id")),
        target_object_id: row.get::<String, _>("target_object_id").into(),
        reaction_id: row.get::<String, _>("reaction_id").into(),
        author_pubkey: row.get("author_pubkey"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
        reaction_key_kind: parse_reaction_key_kind(
            row.get::<String, _>("reaction_key_kind").as_str(),
        )?,
        normalized_reaction_key: row.get("normalized_reaction_key"),
        emoji: opt_col(&row, "emoji"),
        custom_asset_id: opt_col(&row, "custom_asset_id"),
        custom_asset_snapshot: row
            .try_get::<String, _>("custom_asset_snapshot_json")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(|value| serde_json::from_str(value.as_str()))
            .transpose()?,
        status: parse_object_status(row.get::<String, _>("status").as_str())?,
        source_key: row.get("source_key"),
        source_envelope_id: row.get::<String, _>("source_envelope_id").into(),
        derived_at: row.get("derived_at"),
        projection_version: row.get("projection_version"),
    })
}

pub(crate) fn row_to_bookmarked_custom_reaction(
    row: sqlx::sqlite::SqliteRow,
) -> Result<BookmarkedCustomReactionRow> {
    Ok(BookmarkedCustomReactionRow {
        asset_id: row.get("asset_id"),
        owner_pubkey: row.get("owner_pubkey"),
        blob_hash: BlobHash::new(row.get::<String, _>("blob_hash")),
        search_key: row
            .try_get::<String, _>("search_key")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| row.get("asset_id")),
        mime: row.get("mime"),
        bytes: row.get::<i64, _>("bytes") as u64,
        width: row.get::<i64, _>("width") as u32,
        height: row.get::<i64, _>("height") as u32,
        bookmarked_at: row.get("bookmarked_at"),
    })
}

pub(crate) fn row_to_bookmarked_post(row: sqlx::sqlite::SqliteRow) -> Result<BookmarkedPostRow> {
    Ok(BookmarkedPostRow {
        source_object_id: row.get::<String, _>("source_object_id").into(),
        source_envelope_id: row.get::<String, _>("source_envelope_id").into(),
        source_replica_id: ReplicaId::new(row.get::<String, _>("source_replica_id")),
        topic_id: row.get("topic_id"),
        channel_id: row.get("channel_id"),
        author_pubkey: row.get("author_pubkey"),
        created_at: row.get("created_at"),
        object_kind: row.get("object_kind"),
        payload_ref: serde_json::from_str(row.get::<String, _>("payload_ref_json").as_str())?,
        content: opt_col(&row, "content"),
        attachments: serde_json::from_str(row.get::<String, _>("attachments_json").as_str())?,
        // 空文字は None に落とす(row_to_object_projection の root/reply と同じ規約。
        // WP-B16 で非対称を解消)。
        reply_to_object_id: opt_col::<String>(&row, "reply_to_object_id")
            .filter(|value| !value.trim().is_empty())
            .map(EnvelopeId::from),
        root_object_id: opt_col::<String>(&row, "root_object_id")
            .filter(|value| !value.trim().is_empty())
            .map(EnvelopeId::from),
        repost_of: row
            .try_get::<String, _>("repost_of_json")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(|value| serde_json::from_str(value.as_str()))
            .transpose()?,
        bookmarked_at: row.get("bookmarked_at"),
    })
}

pub(crate) fn row_to_direct_message_conversation(
    row: sqlx::sqlite::SqliteRow,
) -> Result<DirectMessageConversationRow> {
    Ok(DirectMessageConversationRow {
        dm_id: row.get("dm_id"),
        peer_pubkey: row.get("peer_pubkey"),
        updated_at: row.get("updated_at"),
        last_message_at: opt_col(&row, "last_message_at"),
        last_message_id: opt_col(&row, "last_message_id"),
        last_message_preview: opt_col(&row, "last_message_preview"),
    })
}

pub(crate) fn row_to_direct_message_message(
    row: sqlx::sqlite::SqliteRow,
) -> Result<DirectMessageMessageRow> {
    Ok(DirectMessageMessageRow {
        dm_id: row.get("dm_id"),
        message_id: row.get("message_id"),
        sender_pubkey: row.get("sender_pubkey"),
        recipient_pubkey: row.get("recipient_pubkey"),
        created_at: row.get("created_at"),
        text: opt_col(&row, "text"),
        reply_to_message_id: opt_col(&row, "reply_to_message_id"),
        attachment_manifest: row
            .try_get::<String, _>("attachment_manifest_json")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(|value| serde_json::from_str(value.as_str()))
            .transpose()?,
        outgoing: row.get::<i64, _>("outgoing") != 0,
        acked_at: opt_col(&row, "acked_at"),
    })
}

pub(crate) fn row_to_notification(row: sqlx::sqlite::SqliteRow) -> Result<NotificationRow> {
    Ok(NotificationRow {
        notification_id: row.get("notification_id"),
        recipient_pubkey: row.get("recipient_pubkey"),
        kind: parse_notification_kind(row.get::<String, _>("kind").as_str())?,
        actor_pubkey: row.get("actor_pubkey"),
        source_envelope_id: row
            .try_get::<String, _>("source_envelope_id")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(EnvelopeId::from),
        source_replica_id: row
            .try_get::<String, _>("source_replica_id")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(ReplicaId::new),
        topic_id: row
            .try_get::<String, _>("topic_id")
            .ok()
            .filter(|value| !value.trim().is_empty()),
        channel_id: row
            .try_get::<String, _>("channel_id")
            .ok()
            .filter(|value| !value.trim().is_empty()),
        object_id: row
            .try_get::<String, _>("object_id")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(EnvelopeId::from),
        dm_id: row
            .try_get::<String, _>("dm_id")
            .ok()
            .filter(|value| !value.trim().is_empty()),
        message_id: row
            .try_get::<String, _>("message_id")
            .ok()
            .filter(|value| !value.trim().is_empty()),
        preview_text: row
            .try_get::<String, _>("preview_text")
            .ok()
            .filter(|value| !value.trim().is_empty()),
        content_labels: opt_col::<String>(&row, "content_labels_json")
            .map(|value| serde_json::from_str(value.as_str()))
            .transpose()?,
        created_at: row.get("created_at"),
        received_at: row.get("received_at"),
        read_at: opt_col(&row, "read_at"),
    })
}

pub(crate) fn row_to_direct_message_outbox(
    row: sqlx::sqlite::SqliteRow,
) -> Result<DirectMessageOutboxRow> {
    Ok(DirectMessageOutboxRow {
        dm_id: row.get("dm_id"),
        message_id: row.get("message_id"),
        peer_pubkey: row.get("peer_pubkey"),
        frame_blob_hash: BlobHash::new(row.get::<String, _>("frame_blob_hash")),
        created_at: row.get("created_at"),
        last_attempt_at: opt_col(&row, "last_attempt_at"),
    })
}

pub(crate) fn row_to_direct_message_tombstone(
    row: sqlx::sqlite::SqliteRow,
) -> Result<DirectMessageTombstoneRow> {
    Ok(DirectMessageTombstoneRow {
        dm_id: row.get("dm_id"),
        message_id: row.get("message_id"),
        deleted_at: row.get("deleted_at"),
    })
}

pub(crate) fn row_to_follow_edge(row: sqlx::sqlite::SqliteRow) -> Result<FollowEdge> {
    Ok(FollowEdge {
        subject_pubkey: row.get::<String, _>("subject_pubkey").into(),
        target_pubkey: row.get::<String, _>("target_pubkey").into(),
        status: parse_follow_edge_status(row.get::<String, _>("status").as_str())?,
        updated_at: row.get("updated_at"),
        envelope_id: row.get::<String, _>("source_envelope_id").into(),
    })
}

pub(crate) fn row_to_block_edge(row: sqlx::sqlite::SqliteRow) -> Result<BlockEdge> {
    Ok(BlockEdge {
        subject_pubkey: row.get::<String, _>("subject_pubkey").into(),
        target_pubkey: row.get::<String, _>("target_pubkey").into(),
        status: parse_block_edge_status(row.get::<String, _>("status").as_str())?,
        updated_at: row.get("updated_at"),
        envelope_id: row.get::<String, _>("source_envelope_id").into(),
    })
}

pub(crate) fn row_to_live_session_projection(
    row: sqlx::sqlite::SqliteRow,
) -> Result<LiveSessionProjectionRow> {
    Ok(LiveSessionProjectionRow {
        session_id: row.get("session_id"),
        topic_id: row.get("topic_id"),
        channel_id: row.get("channel_id"),
        host_pubkey: row.get("host_pubkey"),
        title: row.get("title"),
        description: row.get("description"),
        status: parse_live_status(row.get::<String, _>("status").as_str())?,
        started_at: row.get("started_at"),
        ended_at: opt_col(&row, "ended_at"),
        updated_at: row.get("updated_at"),
        source_replica_id: ReplicaId::new(row.get::<String, _>("source_replica_id")),
        source_key: row.get("source_key"),
        manifest_blob_hash: BlobHash::new(row.get::<String, _>("manifest_blob_hash")),
        derived_at: row.get("derived_at"),
        projection_version: row.get("projection_version"),
        viewer_count: row.get::<i64, _>("viewer_count") as usize,
    })
}

pub(crate) fn row_to_game_room_projection(
    row: sqlx::sqlite::SqliteRow,
) -> Result<GameRoomProjectionRow> {
    Ok(GameRoomProjectionRow {
        room_id: row.get("room_id"),
        topic_id: row.get("topic_id"),
        channel_id: row.get("channel_id"),
        host_pubkey: row.get("host_pubkey"),
        title: row.get("title"),
        description: row.get("description"),
        status: parse_game_status(row.get::<String, _>("status").as_str())?,
        phase_label: opt_col(&row, "phase_label"),
        scores: serde_json::from_str(row.get::<String, _>("scores_json").as_str())?,
        room_kind: row
            .try_get::<String, _>("room_kind")
            .ok()
            .as_deref()
            .map(parse_game_room_kind)
            .transpose()?
            .unwrap_or_default(),
        metaverse: row
            .try_get::<String, _>("metaverse_json")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(|value| serde_json::from_str(value.as_str()))
            .transpose()?,
        updated_at: row.get("updated_at"),
        source_replica_id: ReplicaId::new(row.get::<String, _>("source_replica_id")),
        source_key: row.get("source_key"),
        manifest_blob_hash: BlobHash::new(row.get::<String, _>("manifest_blob_hash")),
        derived_at: row.get("derived_at"),
        projection_version: row.get("projection_version"),
    })
}

pub(crate) fn row_to_author_relationship_projection(
    row: sqlx::sqlite::SqliteRow,
) -> Result<AuthorRelationshipProjectionRow> {
    Ok(AuthorRelationshipProjectionRow {
        local_author_pubkey: row.get("local_author_pubkey"),
        author_pubkey: row.get("author_pubkey"),
        following: row.get("following"),
        followed_by: row.get("followed_by"),
        mutual: row.get("mutual"),
        friend_of_friend: row.get("friend_of_friend"),
        friend_of_friend_via_pubkeys: serde_json::from_str(
            row.get::<String, _>("friend_of_friend_via_pubkeys_json")
                .as_str(),
        )?,
        derived_at: row.get("derived_at"),
    })
}

pub(crate) fn row_to_muted_author(row: sqlx::sqlite::SqliteRow) -> Result<MutedAuthorRow> {
    Ok(MutedAuthorRow {
        author_pubkey: row.get("author_pubkey"),
        muted_at: row.get("muted_at"),
    })
}

pub(crate) fn follow_edge_status_name(status: &FollowEdgeStatus) -> &'static str {
    match status {
        FollowEdgeStatus::Active => "active",
        FollowEdgeStatus::Revoked => "revoked",
    }
}

pub(crate) fn parse_follow_edge_status(value: &str) -> Result<FollowEdgeStatus> {
    match value {
        "active" => Ok(FollowEdgeStatus::Active),
        "revoked" => Ok(FollowEdgeStatus::Revoked),
        _ => anyhow::bail!("unknown follow edge status: {value}"),
    }
}

pub(crate) fn block_edge_status_name(status: &BlockEdgeStatus) -> &'static str {
    match status {
        BlockEdgeStatus::Active => "active",
        BlockEdgeStatus::Revoked => "revoked",
    }
}

pub(crate) fn parse_block_edge_status(value: &str) -> Result<BlockEdgeStatus> {
    match value {
        "active" => Ok(BlockEdgeStatus::Active),
        "revoked" => Ok(BlockEdgeStatus::Revoked),
        _ => anyhow::bail!("unknown block edge status: {value}"),
    }
}

pub(crate) fn object_status_name(status: &ObjectStatus) -> &'static str {
    match status {
        ObjectStatus::Active => "active",
        ObjectStatus::Edited => "edited",
        ObjectStatus::Deleted => "deleted",
        ObjectStatus::Tombstoned => "tombstoned",
    }
}

pub(crate) fn parse_object_status(value: &str) -> Result<ObjectStatus> {
    match value {
        "active" => Ok(ObjectStatus::Active),
        "edited" => Ok(ObjectStatus::Edited),
        "deleted" => Ok(ObjectStatus::Deleted),
        "tombstoned" => Ok(ObjectStatus::Tombstoned),
        _ => anyhow::bail!("unknown object status: {value}"),
    }
}

pub(crate) fn reaction_key_kind_name(kind: &ReactionKeyKind) -> &'static str {
    match kind {
        ReactionKeyKind::Emoji => "emoji",
        ReactionKeyKind::CustomAsset => "custom_asset",
    }
}

pub(crate) fn parse_reaction_key_kind(value: &str) -> Result<ReactionKeyKind> {
    match value {
        "emoji" => Ok(ReactionKeyKind::Emoji),
        "custom_asset" => Ok(ReactionKeyKind::CustomAsset),
        _ => anyhow::bail!("unknown reaction key kind: {value}"),
    }
}

pub(crate) fn notification_kind_name(kind: &NotificationKind) -> &'static str {
    match kind {
        NotificationKind::Mention => "mention",
        NotificationKind::Reply => "reply",
        NotificationKind::Repost => "repost",
        NotificationKind::QuoteRepost => "quote_repost",
        NotificationKind::DirectMessage => "direct_message",
        NotificationKind::Followed => "followed",
    }
}

pub(crate) fn parse_notification_kind(value: &str) -> Result<NotificationKind> {
    match value {
        "mention" => Ok(NotificationKind::Mention),
        "reply" => Ok(NotificationKind::Reply),
        "repost" => Ok(NotificationKind::Repost),
        "quote_repost" => Ok(NotificationKind::QuoteRepost),
        "direct_message" => Ok(NotificationKind::DirectMessage),
        "followed" => Ok(NotificationKind::Followed),
        _ => anyhow::bail!("unknown notification kind: {value}"),
    }
}

pub(crate) fn live_status_name(status: &LiveSessionStatus) -> &'static str {
    match status {
        LiveSessionStatus::Scheduled => "scheduled",
        LiveSessionStatus::Live => "live",
        LiveSessionStatus::Paused => "paused",
        LiveSessionStatus::Ended => "ended",
    }
}

pub(crate) fn parse_live_status(value: &str) -> Result<LiveSessionStatus> {
    match value {
        "scheduled" => Ok(LiveSessionStatus::Scheduled),
        "live" => Ok(LiveSessionStatus::Live),
        "paused" => Ok(LiveSessionStatus::Paused),
        "ended" => Ok(LiveSessionStatus::Ended),
        _ => anyhow::bail!("unknown live session status: {value}"),
    }
}

pub(crate) fn game_status_name(status: &GameRoomStatus) -> &'static str {
    match status {
        GameRoomStatus::Waiting => "waiting",
        GameRoomStatus::Running => "running",
        GameRoomStatus::Paused => "paused",
        GameRoomStatus::Ended => "ended",
    }
}

pub(crate) fn game_room_kind_name(kind: &GameRoomKind) -> &'static str {
    match kind {
        GameRoomKind::ScoreGame => "score_game",
        GameRoomKind::MetaverseRoom => "metaverse_room",
    }
}

pub(crate) fn parse_game_room_kind(value: &str) -> Result<GameRoomKind> {
    match value {
        "" | "score_game" => Ok(GameRoomKind::ScoreGame),
        "metaverse_room" => Ok(GameRoomKind::MetaverseRoom),
        _ => anyhow::bail!("unknown game room kind: {value}"),
    }
}

pub(crate) fn parse_game_status(value: &str) -> Result<GameRoomStatus> {
    match value {
        "open" | "waiting" => Ok(GameRoomStatus::Waiting),
        "in_progress" | "running" => Ok(GameRoomStatus::Running),
        "paused" => Ok(GameRoomStatus::Paused),
        "finished" | "ended" => Ok(GameRoomStatus::Ended),
        _ => anyhow::bail!("unknown game room status: {value}"),
    }
}
