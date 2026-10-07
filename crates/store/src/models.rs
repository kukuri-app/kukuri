use kukuri_core::{
    AssetRef, BlobHash, CustomReactionAssetSnapshotV1, DirectMessageAttachmentManifestV1,
    EnvelopeId, GameRoomKind, GameRoomStatus, GameScoreEntry, LiveSessionStatus,
    MetaverseRoomStateV1, ObjectStatus, PayloadRef, PostWithdrawalReason, ReactionKeyKind,
    ReplicaId, RepostSourceSnapshotV1, WithdrawalReasonVisibility,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct TimelineCursor {
    pub created_at: i64,
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub object_id: EnvelopeId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<TimelineCursor>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlobCacheStatus {
    Missing,
    Available,
    Pinned,
}

/// 検証済みの投稿の行の `projection_version`(#1248)。
///
/// この版から、投稿の行は署名つき envelope と、読んだ replica の topic / channel を確かめた投稿だけから作る。
/// これより小さい版の行は docs の `state` の申告値を写した行で、migration
/// `20260921000000_drop_unverified_object_projections` が削除する(SQL の値と一致させること)。
pub const VERIFIED_OBJECT_PROJECTION_VERSION: i64 = 3;

/// reaction の行(`reaction_cache`)は、この version から、署名つき envelope と読んだ replica を確かめた reaction だけで作る(#1252)。
pub const VERIFIED_REACTION_PROJECTION_VERSION: i64 = 2;

/// live session・game room の行は、この version から、署名された manifest と読んだ replica を確かめた session だけで作る(#1252)。
pub const VERIFIED_SESSION_PROJECTION_VERSION: i64 = 2;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectProjectionRow {
    pub object_id: EnvelopeId,
    pub topic_id: String,
    pub channel_id: String,
    pub author_pubkey: String,
    pub created_at: i64,
    pub object_kind: String,
    pub root_object_id: Option<EnvelopeId>,
    pub reply_to_object_id: Option<EnvelopeId>,
    pub payload_ref: PayloadRef,
    pub content: Option<String>,
    pub attachments: Vec<AssetRef>,
    pub repost_of: Option<RepostSourceSnapshotV1>,
    /// 投稿者自己申告のラベル(#858、ADR 0046)。旧 projection 行には無いため default。
    #[serde(default)]
    pub content_labels: Vec<String>,
    pub source_replica_id: ReplicaId,
    pub source_key: String,
    pub source_envelope_id: EnvelopeId,
    pub source_blob_hash: Option<BlobHash>,
    /// 著者が投稿の envelope の tag で申告した docs author の id(#1258、ADR 0053)。tag の無い投稿と旧い行は `None`。
    #[serde(default)]
    pub source_docs_author: Option<String>,
    pub derived_at: i64,
    pub projection_version: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PostWithdrawalRow {
    pub target_object_id: EnvelopeId,
    pub target_author_pubkey: String,
    pub source_replica_id: ReplicaId,
    pub withdrawal_envelope_id: EnvelopeId,
    pub withdrawn_at: i64,
    pub generation: u64,
    pub replacement_object_id: Option<EnvelopeId>,
    pub reason_visibility: WithdrawalReasonVisibility,
    pub reason: Option<PostWithdrawalReason>,
}

/// 取り下げの docs への書込み 1 件(R5-H)。元投稿の位置と操作時の bucket の 2 か所を別の行にし、
/// 書けた行だけを消す(片側の失敗を成功として落とさない)。宛先は積んだ時に決め、再開で変えない。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WithdrawalWriteRow {
    pub withdrawal_envelope_id: EnvelopeId,
    pub replica_id: ReplicaId,
    pub target_object_id: EnvelopeId,
    pub envelope: kukuri_core::KukuriEnvelope,
    /// 操作時の bucket の行だけが持つ、元投稿の位置(target locator)。
    pub target_replica_id: Option<ReplicaId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentObservationRow {
    pub subject_kind: String,
    pub subject_id: String,
    pub node_base_url: String,
    pub capability: String,
    pub observed_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReactionProjectionRow {
    pub source_replica_id: ReplicaId,
    pub target_object_id: EnvelopeId,
    pub reaction_id: EnvelopeId,
    pub author_pubkey: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub reaction_key_kind: ReactionKeyKind,
    pub normalized_reaction_key: String,
    pub emoji: Option<String>,
    pub custom_asset_id: Option<String>,
    pub custom_asset_snapshot: Option<CustomReactionAssetSnapshotV1>,
    pub status: ObjectStatus,
    pub source_key: String,
    pub source_envelope_id: EnvelopeId,
    pub derived_at: i64,
    pub projection_version: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookmarkedCustomReactionRow {
    pub asset_id: String,
    pub owner_pubkey: String,
    pub blob_hash: BlobHash,
    pub search_key: String,
    pub mime: String,
    pub bytes: u64,
    pub width: u32,
    pub height: u32,
    pub bookmarked_at: i64,
}

impl BookmarkedCustomReactionRow {
    /// 読み出しの値: search_key が空文字・空白だけの行は asset_id を使う（置いた値は書き換えない。WP-S6 T7）。
    pub fn with_search_key_fallback(mut self) -> Self {
        if self.search_key.trim().is_empty() {
            self.search_key = self.asset_id.clone();
        }
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookmarkedPostRow {
    pub source_object_id: EnvelopeId,
    pub source_envelope_id: EnvelopeId,
    pub source_replica_id: ReplicaId,
    pub topic_id: String,
    pub channel_id: String,
    pub author_pubkey: String,
    pub created_at: i64,
    pub object_kind: String,
    pub payload_ref: PayloadRef,
    pub content: Option<String>,
    pub attachments: Vec<AssetRef>,
    pub reply_to_object_id: Option<EnvelopeId>,
    pub root_object_id: Option<EnvelopeId>,
    pub repost_of: Option<RepostSourceSnapshotV1>,
    pub bookmarked_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct BookmarkCursor {
    pub bookmarked_at: i64,
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub source_object_id: EnvelopeId,
}

/// bookmark した投稿が保護する cache の内容（本文の blob と添付、引用の添付。native と Web で同じ）。
pub fn bookmark_cache_refs(row: &BookmarkedPostRow) -> Vec<(String, String)> {
    let mut refs = Vec::new();
    if let PayloadRef::BlobText { hash, .. } = &row.payload_ref {
        refs.push(("blob".to_string(), hash.as_str().to_string()));
    }
    refs.extend(
        row.attachments
            .iter()
            .map(|attachment| ("blob".to_string(), attachment.hash.as_str().to_string())),
    );
    if let Some(snapshot) = &row.repost_of {
        refs.extend(
            snapshot
                .attachments
                .iter()
                .map(|attachment| ("blob".to_string(), attachment.hash.as_str().to_string())),
        );
    }
    refs.sort();
    refs.dedup();
    refs
}

impl From<&BookmarkedPostRow> for BookmarkCursor {
    fn from(row: &BookmarkedPostRow) -> Self {
        Self {
            bookmarked_at: row.bookmarked_at,
            source_object_id: row.source_object_id.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveSessionProjectionRow {
    pub session_id: String,
    pub revision: i64,
    pub topic_id: String,
    pub channel_id: String,
    pub host_pubkey: String,
    pub title: String,
    pub description: String,
    pub status: LiveSessionStatus,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub updated_at: i64,
    pub source_replica_id: ReplicaId,
    pub source_key: String,
    pub manifest_blob_hash: BlobHash,
    pub derived_at: i64,
    pub projection_version: i64,
    pub viewer_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameRoomProjectionRow {
    pub room_id: String,
    pub score_revision: Option<i64>,
    pub topic_id: String,
    pub channel_id: String,
    pub host_pubkey: String,
    pub title: String,
    pub description: String,
    pub status: GameRoomStatus,
    pub phase_label: Option<String>,
    pub scores: Vec<GameScoreEntry>,
    pub room_kind: GameRoomKind,
    pub metaverse: Option<MetaverseRoomStateV1>,
    pub updated_at: i64,
    pub source_replica_id: ReplicaId,
    pub source_key: String,
    pub manifest_blob_hash: BlobHash,
    pub derived_at: i64,
    pub projection_version: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DomeConnectionProjectionRow {
    pub context_id: String,
    pub topic_id: String,
    pub channel_id: String,
    pub snapshot_json: String,
    pub topology_digest: String,
    pub derived_at: i64,
    pub projection_version: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DomeHostingProjectionRow {
    pub instance_id: String,
    pub context_id: String,
    pub topic_id: String,
    pub channel_id: String,
    pub state_json: String,
    pub lease_epoch: Option<u64>,
    pub session_id: Option<String>,
    pub derived_at: i64,
    pub projection_version: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorRelationshipProjectionRow {
    pub local_author_pubkey: String,
    pub author_pubkey: String,
    pub following: bool,
    pub followed_by: bool,
    pub mutual: bool,
    pub friend_of_friend: bool,
    pub friend_of_friend_via_pubkeys: Vec<String>,
    pub derived_at: i64,
}

impl AuthorRelationshipProjectionRow {
    /// 対象の author の edge から、読むときに関係を求める(#1221 R4-D)。関係の cache と全件の再計算を持たない。
    ///
    /// `via` は自分が follow している相手のうち対象を follow している相手(昇順)。自分が対象を follow していれば
    /// friend-of-friend にしない。どの関係も無ければ `None`。
    pub fn derive(
        local_author_pubkey: &str,
        author_pubkey: &str,
        following: bool,
        followed_by: bool,
        mut via: Vec<String>,
    ) -> Option<Self> {
        if author_pubkey == local_author_pubkey {
            return None;
        }
        if following {
            via.clear();
        }
        via.retain(|pubkey| pubkey != local_author_pubkey);
        via.sort();
        via.dedup();
        (following || followed_by || !via.is_empty()).then(|| Self {
            local_author_pubkey: local_author_pubkey.to_string(),
            author_pubkey: author_pubkey.to_string(),
            following,
            followed_by,
            mutual: following && followed_by,
            friend_of_friend: !via.is_empty(),
            friend_of_friend_via_pubkeys: via,
            derived_at: web_time::SystemTime::now()
                .duration_since(web_time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_millis() as i64)
                .unwrap_or_default(),
        })
    }
}

/// owner が受け取った private channel の参加・退出(#1221 R5-H)。`updated_at` は record の時刻。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrivateChannelParticipantRow {
    pub channel_id: String,
    pub epoch_id: String,
    pub participant_pubkey: String,
    pub left_at: Option<i64>,
    pub updated_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MutedAuthorRow {
    pub author_pubkey: String,
    pub muted_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectMessageConversationRow {
    pub dm_id: String,
    pub peer_pubkey: String,
    pub updated_at: i64,
    pub last_message_at: Option<i64>,
    pub last_message_id: Option<String>,
    pub last_message_preview: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectMessageMessageRow {
    pub dm_id: String,
    pub message_id: String,
    pub sender_pubkey: String,
    pub recipient_pubkey: String,
    pub created_at: i64,
    pub text: Option<String>,
    pub reply_to_message_id: Option<String>,
    pub attachment_manifest: Option<DirectMessageAttachmentManifestV1>,
    pub outgoing: bool,
    pub acked_at: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectMessageOutboxRow {
    pub dm_id: String,
    pub message_id: String,
    pub peer_pubkey: String,
    pub frame_blob_hash: BlobHash,
    pub created_at: i64,
    pub last_attempt_at: Option<i64>,
}

pub const DIRECT_MESSAGE_OUTBOX_PAGE_LIMIT: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectMessageOutboxCursor {
    pub created_at: i64,
    pub message_id: String,
    pub dm_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectMessageOutboxPage {
    pub items: Vec<DirectMessageOutboxRow>,
    pub next_cursor: Option<DirectMessageOutboxCursor>,
    pub cycle_end: Option<DirectMessageOutboxCursor>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectMessageTombstoneRow {
    pub dm_id: String,
    pub message_id: String,
    pub deleted_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum NotificationKind {
    Mention,
    Reply,
    Repost,
    QuoteRepost,
    DirectMessage,
    Followed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationRow {
    pub notification_id: String,
    pub recipient_pubkey: String,
    pub kind: NotificationKind,
    pub actor_pubkey: String,
    pub source_envelope_id: Option<EnvelopeId>,
    pub source_replica_id: Option<ReplicaId>,
    pub topic_id: Option<String>,
    pub channel_id: Option<String>,
    pub object_id: Option<EnvelopeId>,
    pub dm_id: Option<String>,
    pub message_id: Option<String>,
    pub preview_text: Option<String>,
    /// Object-backed notification の署名済み content labels。`None` は旧 row または
    /// safety metadata 未解決を表し、既知の空配列と区別する(#858)。
    #[serde(default)]
    pub content_labels: Option<Vec<String>>,
    pub created_at: i64,
    pub received_at: i64,
    pub read_at: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct NotificationCursor {
    pub received_at: i64,
    pub notification_id: String,
}

impl From<&NotificationRow> for NotificationCursor {
    fn from(row: &NotificationRow) -> Self {
        Self {
            received_at: row.received_at,
            notification_id: row.notification_id.clone(),
        }
    }
}

/// 公開 channel の id（app-api の `PUBLIC_CHANNEL_ID`）。
pub(crate) const PUBLIC_CHANNEL: &str = "public";

/// #1632: 公開投稿の行が参照する blob（本文、添付、repost の添付）。公開でない行は空。
pub fn public_blob_hashes_for_row(row: &ObjectProjectionRow) -> Vec<String> {
    if row.channel_id != PUBLIC_CHANNEL {
        return Vec::new();
    }
    row.source_blob_hash
        .iter()
        .map(|hash| hash.as_str().to_string())
        .chain(
            row.attachments
                .iter()
                .map(|asset| asset.hash.as_str().to_string()),
        )
        .chain(
            row.repost_of
                .iter()
                .flat_map(|repost| repost.attachments.iter())
                .map(|asset| asset.hash.as_str().to_string()),
        )
        .collect()
}

/// #1632: 公開 topic の有効な custom reaction の asset。`target_channel` は対象の投稿の channel（手元に行が無ければ
/// `None`）。
pub fn public_blob_hashes_for_reaction(
    row: &ReactionProjectionRow,
    target_channel: Option<&str>,
) -> Vec<String> {
    match (&row.status, &row.custom_asset_snapshot) {
        (kukuri_core::ObjectStatus::Active, Some(snapshot))
            if target_channel == Some(PUBLIC_CHANNEL) =>
        {
            vec![snapshot.blob_hash.as_str().to_string()]
        }
        _ => Vec::new(),
    }
}

/// #858: projection 行から、成人向けラベル付き投稿(引用 snapshot 含む)が参照する
/// 添付 blob hash を列挙する。blob 取得ゲート(`is_adult_media_hash`)の記録元。
pub fn adult_media_hashes_for_row(row: &ObjectProjectionRow) -> Vec<&str> {
    let mut hashes = Vec::new();
    if kukuri_core::has_adult_content_label(&row.content_labels) {
        hashes.extend(
            row.attachments
                .iter()
                .map(|attachment| attachment.hash.as_str()),
        );
    }
    if let Some(snapshot) = row.repost_of.as_ref()
        && kukuri_core::has_adult_content_label(&snapshot.content_labels)
    {
        hashes.extend(
            snapshot
                .attachments
                .iter()
                .map(|attachment| attachment.hash.as_str()),
        );
    }
    hashes
}
