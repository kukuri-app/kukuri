//! cache の database の object store と索引（ADR 0058 §3・ADR 0059 §2）。
//!
//! projection の行は `{ r: <行>, <索引の値>... }`（`crate::rows`）。索引は SQLite の索引と同じ列の並びで、降順の読み出しは
//! cursor の向きで作る。値の無い索引の欄（`undefined`）の行はその索引に載らない（SQLite の部分索引と同じ）。

use js_sys::Array;
use wasm_bindgen::JsValue;
use web_sys::{IdbDatabase, IdbObjectStore, IdbObjectStoreParameters};

pub(crate) const ENVELOPES: &str = "envelopes";
pub(crate) const PROFILES: &str = "profiles";
pub(crate) const FOLLOWS: &str = "follows";
pub(crate) const BLOCKS: &str = "blocks";
pub(crate) const OBJECTS: &str = "objects";
pub(crate) const ADULT_HASHES: &str = "adult_hashes";
pub(crate) const ADULT_REFS: &str = "adult_refs";
pub(crate) const LIVE_SESSIONS: &str = "live_sessions";
pub(crate) const GAME_ROOMS: &str = "game_rooms";
pub(crate) const PRESENCE: &str = "presence";
pub(crate) const DOME_CONNECTIONS: &str = "dome_connections";
pub(crate) const DOME_HOSTING: &str = "dome_hosting";
pub(crate) const REACTIONS: &str = "reactions";
pub(crate) const REACTION_BOOKMARKS: &str = "reaction_bookmarks";
pub(crate) const POST_BOOKMARKS: &str = "post_bookmarks";
pub(crate) const MUTES: &str = "mutes";
pub(crate) const DOCS_AUTHORS: &str = "docs_authors";
pub(crate) const PARTICIPANTS: &str = "participants";
pub(crate) const DM_CONVERSATIONS: &str = "dm_conversations";
pub(crate) const DM_MESSAGES: &str = "dm_messages";
pub(crate) const DM_OUTBOX: &str = "dm_outbox";
pub(crate) const DM_TOMBSTONES: &str = "dm_tombstones";
pub(crate) const NOTIFICATIONS: &str = "notifications";
pub(crate) const OBSERVATIONS: &str = "observations";
pub(crate) const WITHDRAWALS: &str = "withdrawals";
pub(crate) const WITHDRAWAL_OUTBOX: &str = "withdrawal_outbox";
pub(crate) const ACCOUNT_SYNC: &str = "account_sync";
pub(crate) const PRIVATE_CHANNELS: &str = "private_channels";
pub(crate) const PRIVATE_EPOCHS: &str = "private_epochs";
pub(crate) const INDEX_GRANTS: &str = "index_grants";
pub(crate) const INDEX_STOPS: &str = "index_stops";
pub(crate) const PEER_CANDIDATES: &str = "peer_candidates";
pub(crate) const META: &str = "meta";

fn paths(parts: &[&str]) -> JsValue {
    match parts {
        [single] => JsValue::from_str(single),
        parts => parts
            .iter()
            .map(|part| JsValue::from_str(part))
            .collect::<Array>()
            .into(),
    }
}

/// object store を作り、`indexes`（名前と key の欄の並び）を付ける。
fn table(
    db: &IdbDatabase,
    name: &str,
    key: &[&str],
    indexes: &[(&str, &[&str])],
) -> Result<IdbObjectStore, JsValue> {
    let parameters = IdbObjectStoreParameters::new();
    parameters.set_key_path(&paths(key));
    let store = db.create_object_store_with_optional_parameters(name, &parameters)?;
    for (index, key) in indexes {
        match key {
            [single] => store.create_index_with_str(index, single)?,
            key => store.create_index_with_str_sequence(index, &paths(key))?,
        };
    }
    Ok(store)
}

/// projection・account の行の object store（cache の内容の store は `super::create_stores`）。
pub(super) fn create_projection_stores(db: &IdbDatabase) -> Result<(), JsValue> {
    table(
        db,
        ENVELOPES,
        &["r.id"],
        &[
            ("topic", &["topic", "r.created_at", "r.id"]),
            ("thread", &["topic", "root", "r.created_at", "r.id"]),
        ],
    )?;
    table(db, PROFILES, &["r.pubkey"], &[])?;
    for name in [FOLLOWS, BLOCKS] {
        table(
            db,
            name,
            &["r.subject_pubkey", "r.target_pubkey"],
            &[("target", &["r.target_pubkey"])],
        )?;
    }
    table(
        db,
        OBJECTS,
        &["r.object_id"],
        &[
            ("timeline", &["r.topic_id", "r.created_at", "r.object_id"]),
            (
                "channel",
                &["r.topic_id", "r.channel_id", "r.created_at", "r.object_id"],
            ),
            (
                "author",
                &[
                    "r.author_pubkey",
                    "r.channel_id",
                    "r.created_at",
                    "r.object_id",
                ],
            ),
            (
                "thread",
                &["r.topic_id", "root", "r.created_at", "r.object_id"],
            ),
            (
                "thread_channel",
                &[
                    "r.topic_id",
                    "r.channel_id",
                    "root",
                    "r.created_at",
                    "r.object_id",
                ],
            ),
            ("repost", &["repost"]),
        ],
    )?;
    table(db, ADULT_HASHES, &["r.blob_hash"], &[])?;
    table(
        db,
        ADULT_REFS,
        &["r.object_id", "r.blob_hash"],
        &[("hash", &["r.blob_hash", "r.object_id"])],
    )?;
    table(
        db,
        LIVE_SESSIONS,
        &["r.session_id"],
        &[(
            "channel",
            &["r.topic_id", "r.channel_id", "r.started_at", "r.session_id"],
        )],
    )?;
    table(
        db,
        GAME_ROOMS,
        &["r.room_id"],
        &[(
            "channel",
            &["r.topic_id", "r.channel_id", "r.updated_at", "r.room_id"],
        )],
    )?;
    table(
        db,
        PRESENCE,
        &[
            "r.topic_id",
            "r.channel_id",
            "r.session_id",
            "r.author_pubkey",
        ],
        &[("expiry", &["r.expires_at"])],
    )?;
    table(db, DOME_CONNECTIONS, &["r.context_id"], &[])?;
    table(db, DOME_HOSTING, &["r.instance_id"], &[])?;
    table(
        db,
        REACTIONS,
        &["r.source_replica_id", "r.target_object_id", "r.reaction_id"],
        &[
            (
                "target",
                &[
                    "r.source_replica_id",
                    "r.target_object_id",
                    "r.normalized_reaction_key",
                    "r.reaction_id",
                ],
            ),
            (
                "author",
                &["r.author_pubkey", "r.updated_at", "r.reaction_id"],
            ),
        ],
    )?;
    table(
        db,
        REACTION_BOOKMARKS,
        &["r.asset_id"],
        &[("order", &["r.bookmarked_at", "r.asset_id"])],
    )?;
    table(
        db,
        POST_BOOKMARKS,
        &["r.source_object_id"],
        &[("order", &["r.bookmarked_at", "r.source_object_id"])],
    )?;
    table(db, MUTES, &["r.author_pubkey"], &[])?;
    table(db, DOCS_AUTHORS, &["r.author_pubkey"], &[])?;
    table(
        db,
        PARTICIPANTS,
        &["r.channel_id", "r.epoch_id", "r.participant_pubkey"],
        &[
            ("pubkey", &["r.channel_id", "r.participant_pubkey"]),
            ("active", &["active"]),
            ("active_epoch", &["active_epoch"]),
        ],
    )?;
    let conversations = table(
        db,
        DM_CONVERSATIONS,
        &["r.dm_id"],
        &[("order", &["r.updated_at", "r.dm_id"])],
    )?;
    let unique = web_sys::IdbIndexParameters::new();
    unique.set_unique(true);
    conversations.create_index_with_str_and_optional_parameters(
        "peer",
        "r.peer_pubkey",
        &unique,
    )?;
    table(
        db,
        DM_MESSAGES,
        &["r.dm_id", "r.message_id"],
        &[("timeline", &["r.dm_id", "r.created_at", "r.message_id"])],
    )?;
    table(
        db,
        DM_OUTBOX,
        &["r.dm_id", "r.message_id"],
        &[
            ("order", &["r.created_at", "r.message_id", "r.dm_id"]),
            (
                "peer",
                &["r.peer_pubkey", "r.created_at", "r.message_id", "r.dm_id"],
            ),
            ("fresh", &["fresh"]),
            ("due", &["due"]),
        ],
    )?;
    table(
        db,
        DM_TOMBSTONES,
        &["r.dm_id", "r.message_id"],
        &[("order", &["r.dm_id", "r.deleted_at", "r.message_id"])],
    )?;
    let notifications = table(
        db,
        NOTIFICATIONS,
        &["r.notification_id"],
        &[
            ("inbox", &["r.received_at", "r.notification_id"]),
            ("seq", &["seq"]),
        ],
    )?;
    for name in ["docs_dedupe", "dm_dedupe"] {
        notifications.create_index_with_str_and_optional_parameters(name, name, &unique)?;
    }
    table(
        db,
        OBSERVATIONS,
        &[
            "r.subject_kind",
            "r.subject_id",
            "r.node_base_url",
            "r.capability",
        ],
        &[("observed", &["r.observed_at"])],
    )?;
    table(db, WITHDRAWALS, &["r.target_object_id"], &[])?;
    table(
        db,
        WITHDRAWAL_OUTBOX,
        &["r.withdrawal_envelope_id", "r.replica_id"],
        &[("seq", &["seq"])],
    )?;
    table(db, ACCOUNT_SYNC, &["r.key"], &[])?;
    table(
        db,
        PRIVATE_CHANNELS,
        &["r.channel_key"],
        &[
            ("channel_id", &["r.channel_id"]),
            ("joined", &["joined"]),
            ("topic", &["joined_topic"]),
            ("owner", &["joined_owner"]),
        ],
    )?;
    table(
        db,
        PRIVATE_EPOCHS,
        &["r.channel_id", "r.epoch_id"],
        &[
            ("receive", &["r.receive_key_id"]),
            ("started", &["r.channel_id", "r.started_at", "r.epoch_id"]),
        ],
    )?;
    table(
        db,
        INDEX_GRANTS,
        &["r.base_url", "r.topic_id", "r.channel_id"],
        &[(
            "due",
            &[
                "r.base_url",
                "r.last_checked_at_ms",
                "r.topic_id",
                "r.channel_id",
            ],
        )],
    )?;
    table(db, INDEX_STOPS, &["r.kind", "r.id"], &[])?;
    table(
        db,
        PEER_CANDIDATES,
        &["r.scope", "r.source", "r.endpoint_id"],
        &[
            (
                "window",
                &["r.scope", "r.source", "r.seen_ms", "r.endpoint_id"],
            ),
            ("learned", &["learned"]),
        ],
    )?;
    Ok(())
}
