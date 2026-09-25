use kukuri_core::{
    AssetRole, ChannelAudienceKind, ChannelRef, ChannelSharingState, DomeConnectionProposalV1,
    DomeConnectionRecordV1, DomeConnectionTerminalReasonV1, DomeCustomizationV1, DomeDirection,
    DomeHostingLeaseV1, DomeHostingStateV1, DomeMoveRecordV1, DomeProposalDerivedStatusV1,
    DomeProposalSelectionV1, DomeTopologyResolutionV1, GameRoomKind, GameRoomStatus,
    KukuriEnvelope, LiveSessionStatus, MetaverseAssetKind, MetaverseAssetRef,
    MetaverseResourceBudgetConfig, MetaverseResourceMetricsV1, MetaverseRoomEventEnvelopeContentV1,
    MetaverseRoomEventV1, MetaverseRoomStateV1, SpatialContextV1,
};
use kukuri_store::{BookmarkCursor, NotificationKind, TimelineCursor};
use kukuri_transport::{ConnectMode, ConnectionPath, DiscoveryMode};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ContentObservationView {
    pub node_base_url: String,
    pub capability: String,
    pub observed_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ContentProvenanceView {
    pub canonical_source: String,
    pub observed_via: Vec<ContentObservationView>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct PostWithdrawalView {
    pub withdrawn_at: i64,
    pub replacement_object_id: Option<String>,
    pub reason_visibility: String,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct PostView {
    pub object_id: String,
    pub envelope_id: String,
    pub author_pubkey: String,
    pub author_name: Option<String>,
    pub author_display_name: Option<String>,
    pub author_picture_asset: Option<ProfileAssetView>,
    pub following: bool,
    pub followed_by: bool,
    pub mutual: bool,
    pub friend_of_friend: bool,
    #[serde(default)]
    pub provenance: Option<ContentProvenanceView>,
    #[serde(default)]
    pub withdrawal: Option<PostWithdrawalView>,
    pub content: String,
    pub content_status: BlobViewStatus,
    pub attachments: Vec<AttachmentView>,
    // #858: 投稿者自己申告のラベル(ADR 0046、既知値は `adult` のみ)。
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<Vec<String>>"))]
    pub content_labels: Vec<String>,
    pub created_at: i64,
    pub reply_to: Option<String>,
    pub reply_preview: Option<ReplyPreviewView>,
    pub root_id: Option<String>,
    pub object_kind: String,
    pub published_topic_id: Option<String>,
    pub origin_topic_id: Option<String>,
    pub repost_of: Option<RepostSourceView>,
    pub repost_commentary: Option<String>,
    pub is_threadable: bool,
    pub channel_id: Option<String>,
    pub audience_label: String,
    // 旧 types.ts と同じく front では任意(#[serde(default)] で復元も許容)。
    // Vec には ts(optional) を付けられないため as で Option 扱いにして
    // optional_fields = nullable と組み合わせ `?: T[] | null` を生成する。
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<Vec<ReactionSummaryView>>"))]
    pub reaction_summary: Vec<ReactionSummaryView>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<Vec<ReactionKeyView>>"))]
    pub my_reactions: Vec<ReactionKeyView>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct CommunityIndexPostResolveInput {
    pub key: String,
    pub topic: String,
    pub object_id: String,
    pub author_pubkey: String,
    pub channel_ref: ChannelRef,
    // CNが返した公開投稿の保存先。権限や正本の証明としては使わない。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub source_replica_id: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct CommunityIndexPostActionCapabilitiesView {
    pub open_thread: bool,
    pub reply: bool,
    pub repost: bool,
    pub quote_repost: bool,
    pub react: bool,
    pub copy_link: bool,
    pub bookmark: bool,
    pub withdraw: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct CommunityIndexResolvedPostView {
    pub key: String,
    pub post: Option<PostView>,
    pub capabilities: CommunityIndexPostActionCapabilitiesView,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct CommunityIndexPostResolveResponse {
    pub entries: Vec<CommunityIndexResolvedPostView>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct ReplyPreviewAuthorView {
    pub pubkey: String,
    pub name: Option<String>,
    pub display_name: Option<String>,
    pub picture_asset: Option<ProfileAssetView>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct ReplyPreviewView {
    pub object_id: String,
    pub topic: String,
    pub author: ReplyPreviewAuthorView,
    pub content: String,
    pub content_status: BlobViewStatus,
    pub attachments: Vec<AttachmentView>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<Vec<String>>"))]
    pub content_labels: Vec<String>,
    pub root_id: Option<String>,
    pub reply_to: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct ReactionKeyView {
    pub reaction_key_kind: String,
    pub normalized_reaction_key: String,
    pub emoji: Option<String>,
    pub custom_asset: Option<CustomReactionAssetView>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct ReactionSummaryView {
    pub reaction_key_kind: String,
    pub normalized_reaction_key: String,
    pub emoji: Option<String>,
    pub custom_asset: Option<CustomReactionAssetView>,
    pub count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct ReactionStateView {
    pub target_object_id: String,
    pub source_replica_id: String,
    pub reaction_summary: Vec<ReactionSummaryView>,
    pub my_reactions: Vec<ReactionKeyView>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct RecentReactionView {
    pub reaction_key_kind: String,
    pub normalized_reaction_key: String,
    pub emoji: Option<String>,
    pub custom_asset: Option<CustomReactionAssetView>,
    pub updated_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct CustomReactionAssetView {
    pub asset_id: String,
    pub owner_pubkey: String,
    pub blob_hash: String,
    pub search_key: String,
    pub mime: String,
    pub bytes: u64,
    pub width: u32,
    pub height: u32,
}

pub type BookmarkedCustomReactionView = CustomReactionAssetView;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct BookmarkedPostView {
    pub bookmarked_at: i64,
    pub post: PostView,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct BookmarkedPostPageView {
    pub items: Vec<BookmarkedPostView>,
    pub newer_cursor: Option<BookmarkCursor>,
    pub older_cursor: Option<BookmarkCursor>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreateCustomReactionAssetInput {
    pub search_key: String,
    pub mime: String,
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct RepostSourceView {
    pub source_object_id: String,
    pub source_topic_id: String,
    pub source_author_pubkey: String,
    pub source_author_name: Option<String>,
    pub source_author_display_name: Option<String>,
    pub source_author_picture_asset: Option<ProfileAssetView>,
    pub source_object_kind: String,
    pub content: String,
    pub attachments: Vec<AttachmentView>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<Vec<String>>"))]
    pub content_labels: Vec<String>,
    pub reply_to: Option<String>,
    pub root_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub enum BlobViewStatus {
    Missing,
    Available,
    Pinned,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct AttachmentView {
    pub hash: String,
    pub mime: String,
    pub bytes: u64,
    pub role: String,
    pub status: BlobViewStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<ContentProvenanceView>,
}

/// #858: 成人向け表現の表示設定(ADR 0046)。canonical source は desktop-runtime の
/// ローカル JSON(`<db_path>.content-display.json`)。既定 OFF。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ContentDisplaySettings {
    pub adult_content_enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct BlobMediaPayload {
    pub bytes_base64: String,
    pub mime: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileInput {
    pub name: Option<String>,
    pub display_name: Option<String>,
    pub about: Option<String>,
    pub picture_upload: Option<PendingAttachment>,
    #[serde(default)]
    pub clear_picture: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct ProfileAssetView {
    pub hash: String,
    pub mime: String,
    pub bytes: u64,
    // 現行 types.ts と同じく値は常に 'profile_avatar'(literal)。
    #[cfg_attr(feature = "ts", ts(type = "'profile_avatar'"))]
    pub role: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct AuthorSocialView {
    pub author_pubkey: String,
    pub name: Option<String>,
    pub display_name: Option<String>,
    pub about: Option<String>,
    pub picture_asset: Option<ProfileAssetView>,
    pub updated_at: Option<i64>,
    pub following: bool,
    pub followed_by: bool,
    pub mutual: bool,
    pub friend_of_friend: bool,
    pub friend_of_friend_via_pubkeys: Vec<String>,
    #[serde(default)]
    pub provenance: Option<ContentProvenanceView>,
    #[serde(default)]
    pub muted: bool,
    #[serde(default)]
    pub blocking: bool,
    #[serde(default)]
    pub blocked_by: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum SocialConnectionKind {
    Following,
    Followed,
    Muted,
    Blocking,
    BlockedBy,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingAttachment {
    pub mime: String,
    pub bytes: Vec<u8>,
    pub role: AssetRole,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct DirectMessageStatusView {
    pub peer_pubkey: String,
    pub dm_id: String,
    pub mutual: bool,
    pub send_enabled: bool,
    pub pending_outbox_count: usize,
    pub pending_outbox_has_more: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct DirectMessageMessageView {
    pub dm_id: String,
    pub message_id: String,
    pub sender_pubkey: String,
    pub recipient_pubkey: String,
    pub created_at: i64,
    pub text: String,
    pub reply_to_message_id: Option<String>,
    pub attachments: Vec<AttachmentView>,
    pub outgoing: bool,
    pub delivered: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct DirectMessageConversationView {
    pub dm_id: String,
    pub peer_pubkey: String,
    pub peer_name: Option<String>,
    pub peer_display_name: Option<String>,
    pub peer_picture_asset: Option<ProfileAssetView>,
    pub updated_at: i64,
    pub last_message_at: Option<i64>,
    pub last_message_id: Option<String>,
    pub last_message_preview: Option<String>,
    pub status: DirectMessageStatusView,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct NotificationView {
    pub notification_id: String,
    pub kind: NotificationKind,
    pub actor_pubkey: String,
    pub actor_name: Option<String>,
    pub actor_display_name: Option<String>,
    pub actor_picture_asset: Option<ProfileAssetView>,
    pub source_envelope_id: Option<String>,
    pub source_replica_id: Option<String>,
    pub topic_id: Option<String>,
    pub channel_id: Option<String>,
    pub object_id: Option<String>,
    pub thread_root_object_id: Option<String>,
    pub dm_id: Option<String>,
    pub message_id: Option<String>,
    pub preview_text: Option<String>,
    #[serde(default)]
    pub content_labels: Option<Vec<String>>,
    pub created_at: i64,
    pub received_at: i64,
    pub read_at: Option<i64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct NotificationStatusView {
    pub unread_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct NotificationPageView {
    pub items: Vec<NotificationView>,
    pub newer_cursor: Option<kukuri_store::NotificationCursor>,
    pub older_cursor: Option<kukuri_store::NotificationCursor>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct LiveSessionView {
    pub session_id: String,
    pub host_pubkey: String,
    pub title: String,
    pub description: String,
    pub status: LiveSessionStatus,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub viewer_count: usize,
    pub joined_by_me: bool,
    pub channel_id: Option<String>,
    pub audience_label: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct GameRoomView {
    pub room_id: String,
    pub host_pubkey: String,
    pub title: String,
    pub description: String,
    pub status: GameRoomStatus,
    pub phase_label: Option<String>,
    pub scores: Vec<GameScoreView>,
    pub room_kind: GameRoomKind,
    pub metaverse: Option<MetaverseRoomStateV1>,
    pub dome_hosting: Option<DomeHostingStateV1>,
    pub manifest_blob_hash: String,
    pub updated_at: i64,
    pub channel_id: Option<String>,
    pub audience_label: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct GameScoreView {
    pub participant_id: String,
    pub label: String,
    pub score: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreateLiveSessionInput {
    pub title: String,
    pub description: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreateGameRoomInput {
    pub title: String,
    pub description: String,
    pub participants: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreateMetaverseRoomInput {
    pub title: String,
    pub description: String,
    pub max_peers: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateGameRoomInput {
    pub status: GameRoomStatus,
    pub phase_label: Option<String>,
    pub scores: Vec<GameScoreView>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateMetaverseRoomInput {
    pub status: GameRoomStatus,
    pub customization: DomeCustomizationV1,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MoveDomeInput {
    pub move_id: String,
    pub source_instance_id: String,
    pub target_context: SpatialContextV1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartOwnerDomeHostingInput {
    pub expected_generation: Option<u64>,
    pub spatial_context: SpatialContextV1,
    pub instance_id: String,
    pub endpoint_id: String,
    pub lease_duration_millis: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrepareCommunityNodeDomeHostingInput {
    pub expected_generation: Option<u64>,
    pub spatial_context: SpatialContextV1,
    pub instance_id: String,
    pub node_id: String,
    pub api_base_url: String,
    pub lease_duration_millis: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivateCommunityNodeDomeHostingInput {
    pub spatial_context: SpatialContextV1,
    pub instance_id: String,
    pub signed_acceptance_json: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloseDomeHostingInput {
    pub expected_generation: Option<u64>,
    pub spatial_context: SpatialContextV1,
    pub instance_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct DomeHostingView {
    pub instance_id: String,
    pub state: DomeHostingStateV1,
    pub lease: Option<DomeHostingLeaseV1>,
    pub signed_lease_json: Option<String>,
    pub signed_activation_json: Option<String>,
    pub signed_close_json: Option<String>,
    pub instance_manifest_json: String,
    pub preset_manifest_json: Option<String>,
    pub participants: u32,
    pub sleeping: bool,
    pub resource_budget: MetaverseResourceBudgetConfig,
    pub resource_metrics: MetaverseResourceMetricsV1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmitDomeSessionInput {
    #[serde(default)]
    pub expected_generation: Option<u64>,
    pub spatial_context: SpatialContextV1,
    pub instance_id: String,
    pub sequence: u64,
    pub input: kukuri_core::DomeSessionInputKindV1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrepareDomeTransitionInput {
    pub request: kukuri_core::DomeTransitionAdmissionRequestV1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitDomeTransitionInput {
    pub ticket: kukuri_core::DomeTransitionAdmissionTicketV1,
    pub position: [i64; 3],
    pub rotation: [i64; 3],
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AbortDomeTransitionInput {
    pub ticket: kukuri_core::DomeTransitionAdmissionTicketV1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitDomeLayoutInput {
    pub spatial_context: SpatialContextV1,
    pub instance_id: String,
    pub operation_id: String,
    /// Community Node hosted Dome supplies the host-signed candidate as JSON.
    /// Owner-hosted Dome captures it from the local runtime when omitted.
    pub signed_candidate_json: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum DomeLayoutCommitOutcome {
    NoOp,
    Committed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct DomeLayoutCommitView {
    pub outcome: DomeLayoutCommitOutcome,
    pub operation_id: String,
    pub revision: u64,
    pub manifest_blob_hash: String,
    pub signed_commit_json: Option<String>,
    pub hosting: DomeHostingView,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResyncDomeSnapshotsInput {
    pub spatial_context: SpatialContextV1,
    pub instance_id: String,
    pub after_sequence: u64,
}

pub type DomeMoveView = DomeMoveRecordV1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct DomeConnectionProposalView {
    pub proposal: DomeConnectionProposalV1,
    pub selection: Option<DomeProposalSelectionV1>,
    pub status: DomeProposalDerivedStatusV1,
    pub terminal_reason: Option<DomeConnectionTerminalReasonV1>,
    pub connection_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct DomeConnectionView {
    pub record: DomeConnectionRecordV1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct DomeConnectionTopologyView {
    pub proposals: Vec<DomeConnectionProposalView>,
    pub connections: Vec<DomeConnectionView>,
    pub resolution: DomeTopologyResolutionV1,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreateDomeConnectionProposalInput {
    pub proposal_id: String,
    pub spatial_context: SpatialContextV1,
    pub proposer_instance_id: String,
    pub receiver_instance_id: String,
    pub proposer_direction: DomeDirection,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AcceptDomeConnectionProposalInput {
    pub spatial_context: SpatialContextV1,
    pub proposal_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WithdrawDomeConnectionProposalInput {
    pub spatial_context: SpatialContextV1,
    pub proposal_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RevokeDomeConnectionInput {
    pub spatial_context: SpatialContextV1,
    pub connection_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishMetaverseRoomEventInput {
    pub room_id: String,
    pub peer_id: String,
    pub seq: u64,
    pub event: MetaverseRoomEventV1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct MetaverseRoomEventView {
    pub envelope_id: String,
    pub content: MetaverseRoomEventEnvelopeContentV1,
    #[cfg_attr(feature = "ts", ts(type = "Record<string, unknown>"))]
    pub envelope: KukuriEnvelope,
    pub received_at: i64,
    pub source_peer: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportMetaverseRoomAssetInput {
    pub room_id: String,
    pub kind: MetaverseAssetKind,
    pub mime_type: String,
    pub name: Option<String>,
    pub bytes: Vec<u8>,
}

pub type MetaverseAssetRefView = MetaverseAssetRef;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct TimelineView {
    pub items: Vec<PostView>,
    pub next_cursor: Option<TimelineCursor>,
    /// このページの範囲の索引にあるが、本体が手元に無く表示できない投稿の数(#1239 AC-4)。取得側が範囲を照合した
    /// ページ(遡ったページ、projection が尽きたページ、thread)でだけ数える。0 なら、画面は何も示さない。
    // front では任意(旧い runtime の応答と mock は持たない)。`?: number | null` を生成する。
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<u32>"))]
    pub unavailable_count: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct DirectMessageTimelineView {
    pub items: Vec<DirectMessageMessageView>,
    pub next_cursor: Option<TimelineCursor>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct JoinedPrivateChannelView {
    pub topic_id: String,
    pub channel_id: String,
    pub label: String,
    pub creator_pubkey: String,
    pub owner_pubkey: String,
    pub joined_via_pubkey: Option<String>,
    pub audience_kind: ChannelAudienceKind,
    pub is_owner: bool,
    pub current_epoch_id: String,
    pub archived_epoch_ids: Vec<String>,
    pub sharing_state: ChannelSharingState,
    pub rotation_required: bool,
    pub participant_count: usize,
    pub stale_participant_count: usize,
    pub entry_dome_instance_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct PrivateChannelEpochCapability {
    pub epoch_id: String,
    pub namespace_secret_hex: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct PrivateChannelCapability {
    pub topic_id: String,
    pub channel_id: String,
    pub label: String,
    pub creator_pubkey: String,
    #[serde(default)]
    pub owner_pubkey: String,
    #[serde(default)]
    pub joined_via_pubkey: Option<String>,
    #[serde(default)]
    pub audience_kind: ChannelAudienceKind,
    #[serde(default)]
    pub current_epoch_id: String,
    #[serde(default)]
    pub current_epoch_secret_hex: String,
    #[serde(default)]
    pub archived_epochs: Vec<PrivateChannelEpochCapability>,
    #[serde(default)]
    pub rotation_required: bool,
    #[serde(default)]
    pub participant_count: usize,
    #[serde(default)]
    pub stale_participant_count: usize,
    #[serde(default)]
    pub namespace_secret_hex: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ChannelAccessTokenKind {
    Invite,
    Grant,
    Share,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct ChannelAccessTokenExport {
    pub kind: ChannelAccessTokenKind,
    pub token: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct ChannelAccessTokenPreview {
    pub kind: ChannelAccessTokenKind,
    pub topic_id: String,
    pub channel_id: String,
    pub channel_label: String,
    pub owner_pubkey: String,
    pub inviter_pubkey: Option<String>,
    pub sponsor_pubkey: Option<String>,
    pub epoch_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct SyncStatus {
    pub connected: bool,
    pub delivery_state: DeliveryState,
    pub last_sync_ts: Option<i64>,
    pub peer_count: usize,
    pub pending_events: usize,
    pub status_detail: String,
    pub last_error: Option<String>,
    pub configured_peers: Vec<String>,
    pub subscribed_topics: Vec<String>,
    pub active_path: ConnectionPath,
    pub fallback_peer_ids: Vec<String>,
    pub topic_diagnostics: Vec<TopicSyncStatus>,
    pub local_author_pubkey: String,
    pub discovery: DiscoveryStatus,
    pub gossip_disabled_topics: Vec<String>,
    pub gossip_disabled_channels: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub enum DeliveryState {
    Live,
    DurableRecovering,
    DurableReady,
    #[default]
    Offline,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct DiscoveryStatus {
    pub mode: DiscoveryMode,
    pub connect_mode: ConnectMode,
    pub active_path: ConnectionPath,
    pub fallback_peer_ids: Vec<String>,
    pub env_locked: bool,
    pub configured_seed_peer_ids: Vec<String>,
    pub bootstrap_seed_peer_ids: Vec<String>,
    pub manual_ticket_peer_ids: Vec<String>,
    pub connected_peer_ids: Vec<String>,
    pub docs_assist_peer_ids: Vec<String>,
    pub blob_assist_peer_ids: Vec<String>,
    pub local_endpoint_id: String,
    pub last_discovery_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct TopicSyncStatus {
    pub topic: String,
    pub joined: bool,
    pub delivery_state: DeliveryState,
    pub peer_count: usize,
    pub connected_peers: Vec<String>,
    pub docs_assist_peer_ids: Vec<String>,
    pub configured_peer_ids: Vec<String>,
    pub missing_peer_ids: Vec<String>,
    pub active_path: ConnectionPath,
    pub rendezvous_peer_ids: Vec<String>,
    pub fallback_peer_ids: Vec<String>,
    pub last_received_at: Option<i64>,
    pub last_docs_activity_at: Option<i64>,
    pub status_detail: String,
    pub last_error: Option<String>,
}
