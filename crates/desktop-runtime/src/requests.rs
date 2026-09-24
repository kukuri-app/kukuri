use kukuri_app_api::{CommunityIndexPostResolveInput, GameScoreView, SocialConnectionKind};

use kukuri_core::{
    ChannelAudienceKind, ChannelRef, DomeCustomizationV1, DomeSessionInputKindV1,
    DomeTransitionAdmissionRequestV1, DomeTransitionAdmissionTicketV1, GameRoomStatus,
    MetaverseAssetKind, MetaverseRoomEventV1, SpatialContextV1, TimelineScope,
};
use kukuri_store::{BookmarkCursor, TimelineCursor};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct CreatePostRequest {
    pub topic: String,
    pub content: String,
    pub reply_to: Option<String>,
    #[serde(default)]
    pub channel_ref: ChannelRef,
    #[serde(default)]
    pub attachments: Vec<CreateAttachmentRequest>,
    /// #858: 投稿者自己申告のラベル(既知値は `adult` のみ)。
    #[serde(default)]
    pub content_labels: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum WithdrawalReasonVisibilityRequest {
    Public,
    Private,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum PostWithdrawalReasonRequest {
    AuthorRequest,
    Correction,
    Privacy,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct WithdrawPostRequest {
    pub topic: String,
    pub object_id: String,
    #[serde(default)]
    pub channel_ref: ChannelRef,
    pub replacement_object_id: Option<String>,
    pub reason_visibility: WithdrawalReasonVisibilityRequest,
    pub reason: Option<PostWithdrawalReasonRequest>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct CreateRepostRequest {
    pub topic: String,
    pub source_topic: String,
    pub source_object_id: String,
    #[serde(default)]
    pub commentary: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct CreateAttachmentRequest {
    pub file_name: Option<String>,
    pub mime: String,
    pub byte_size: u64,
    pub data_base64: String,
    pub role: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReactionKeyRequest {
    Emoji {
        emoji: String,
    },
    CustomAsset {
        asset_id: String,
        owner_pubkey: String,
        blob_hash: String,
        search_key: String,
        mime: String,
        bytes: u64,
        width: u32,
        height: u32,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct ToggleReactionRequest {
    pub target_topic_id: String,
    pub target_object_id: String,
    pub reaction_key: ReactionKeyRequest,
    pub channel_ref: Option<ChannelRef>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct CustomReactionCropRect {
    pub x: u32,
    pub y: u32,
    pub size: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct CreateCustomReactionAssetRequest {
    pub upload: CreateAttachmentRequest,
    pub crop_rect: CustomReactionCropRect,
    pub search_key: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct BookmarkCustomReactionRequest {
    pub asset_id: String,
    pub owner_pubkey: String,
    pub blob_hash: String,
    pub search_key: String,
    pub mime: String,
    pub bytes: u64,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct RemoveBookmarkedCustomReactionRequest {
    pub asset_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct BookmarkPostRequest {
    pub topic: String,
    pub object_id: String,
    #[serde(default)]
    pub channel_ref: ChannelRef,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct ListBookmarkedPostsRequest {
    pub cursor: Option<BookmarkCursor>,
    #[serde(default)]
    pub before: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct BookmarkedPostIdsRequest {
    pub object_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ResolveCommunityIndexPostsRequest {
    pub entries: Vec<CommunityIndexPostResolveInput>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct RemoveBookmarkedPostRequest {
    pub object_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct ListRecentReactionsRequest {
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct ListTimelineRequest {
    pub topic: String,
    #[serde(default)]
    pub scope: TimelineScope,
    pub cursor: Option<TimelineCursor>,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct ListThreadRequest {
    pub topic: String,
    pub thread_id: String,
    pub cursor: Option<TimelineCursor>,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct ListProfileTimelineRequest {
    pub pubkey: String,
    pub cursor: Option<TimelineCursor>,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct RetryPostElementsRequest {
    pub object_id: String,
    pub body_object_id: Option<String>,
    pub manual: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct ImportPeerTicketRequest {
    pub ticket: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct UnsubscribeTopicRequest {
    pub topic: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct SetTopicGossipEnabledRequest {
    pub topic: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct SetChannelGossipEnabledRequest {
    pub topic: String,
    pub channel: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct GetBlobPreviewRequest {
    pub hash: String,
    pub mime: String,
    #[serde(default)]
    pub metaverse_kind: Option<MetaverseAssetKind>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct GetBlobMediaRequest {
    pub hash: String,
    pub mime: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct AuthorRequest {
    pub pubkey: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct ListSocialConnectionsRequest {
    pub kind: SocialConnectionKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct DirectMessageRequest {
    pub pubkey: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct NotificationIdRequest {
    pub notification_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct ListDirectMessageMessagesRequest {
    pub pubkey: String,
    pub cursor: Option<TimelineCursor>,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct SendDirectMessageRequest {
    pub pubkey: String,
    pub text: Option<String>,
    pub reply_to_message_id: Option<String>,
    #[serde(default)]
    pub attachments: Vec<CreateAttachmentRequest>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct DeleteDirectMessageMessageRequest {
    pub pubkey: String,
    pub message_id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct SetMyProfileRequest {
    pub name: Option<String>,
    pub display_name: Option<String>,
    pub about: Option<String>,
    pub picture_upload: Option<CreateAttachmentRequest>,
    #[serde(default)]
    pub clear_picture: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct InitialProfileRequest {
    pub account_id: String,
    pub profile: SetMyProfileRequest,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct CreateAccountRequest {
    pub account_id: String,
    pub operation_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct ListLiveSessionsRequest {
    pub topic: String,
    #[serde(default)]
    pub scope: TimelineScope,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct CreateLiveSessionRequest {
    pub topic: String,
    #[serde(default)]
    pub channel_ref: ChannelRef,
    pub title: String,
    pub description: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct LiveSessionCommandRequest {
    pub topic: String,
    pub session_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct ListGameRoomsRequest {
    pub topic: String,
    #[serde(default)]
    pub scope: TimelineScope,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct CreateGameRoomRequest {
    pub topic: String,
    #[serde(default)]
    pub channel_ref: ChannelRef,
    pub title: String,
    pub description: String,
    pub participants: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct CreateMetaverseRoomRequest {
    pub topic: String,
    #[serde(default)]
    pub channel_ref: ChannelRef,
    pub title: String,
    pub description: String,
    pub max_peers: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct PublishMetaverseRoomEventRequest {
    pub topic: String,
    pub room_id: String,
    pub peer_id: String,
    pub seq: u64,
    pub event: MetaverseRoomEventV1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct ListMetaverseRoomEventsRequest {
    pub topic: String,
    pub room_id: String,
    pub after_envelope_id: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct ImportMetaverseRoomAssetRequest {
    pub topic: String,
    pub room_id: String,
    pub kind: MetaverseAssetKind,
    pub mime_type: String,
    pub name: Option<String>,
    pub data_base64: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct GetDomeHostingRequest {
    pub spatial_context: SpatialContextV1,
    pub instance_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct StartOwnerDomeHostingRequest {
    #[serde(default)]
    pub expected_generation: Option<u64>,
    pub spatial_context: SpatialContextV1,
    pub instance_id: String,
    pub endpoint_id: String,
    pub lease_duration_millis: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct DelegateDomeHostingRequest {
    #[serde(default)]
    pub expected_generation: Option<u64>,
    pub spatial_context: SpatialContextV1,
    pub instance_id: String,
    pub node_id: String,
    pub base_url: String,
    pub lease_duration_millis: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct CloseDomeHostingRequest {
    #[serde(default)]
    pub expected_generation: Option<u64>,
    pub spatial_context: SpatialContextV1,
    pub instance_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct SubmitDomeSessionInputRequest {
    #[serde(default)]
    pub expected_generation: Option<u64>,
    pub spatial_context: SpatialContextV1,
    pub instance_id: String,
    pub sequence: u64,
    pub input: DomeSessionInputKindV1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct PrepareDomeTransitionRequest {
    pub request: DomeTransitionAdmissionRequestV1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct CommitDomeTransitionRequest {
    pub ticket: DomeTransitionAdmissionTicketV1,
    pub position: [i64; 3],
    pub rotation: [i64; 3],
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct AbortDomeTransitionRequest {
    pub ticket: DomeTransitionAdmissionTicketV1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct CommitDomeLayoutRequest {
    pub spatial_context: SpatialContextV1,
    pub instance_id: String,
    pub operation_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ResyncDomeSnapshotsRequest {
    pub spatial_context: SpatialContextV1,
    pub instance_id: String,
    pub after_sequence: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct CreatePrivateChannelRequest {
    pub topic: String,
    pub label: String,
    #[serde(default)]
    pub audience_kind: ChannelAudienceKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct ExportPrivateChannelInviteRequest {
    pub topic: String,
    pub channel_id: String,
    pub expires_at: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct ImportPrivateChannelInviteRequest {
    pub token: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct ExportChannelAccessTokenRequest {
    pub topic: String,
    pub channel_id: String,
    pub expires_at: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct ImportChannelAccessTokenRequest {
    pub token: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct PreviewChannelAccessTokenRequest {
    pub token: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct ExportFriendOnlyGrantRequest {
    pub topic: String,
    pub channel_id: String,
    pub expires_at: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct ImportFriendOnlyGrantRequest {
    pub token: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct ExportFriendPlusShareRequest {
    pub topic: String,
    pub channel_id: String,
    pub expires_at: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct ImportFriendPlusShareRequest {
    pub token: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct FreezePrivateChannelRequest {
    pub topic: String,
    pub channel_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct RotatePrivateChannelRequest {
    pub topic: String,
    pub channel_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct SetPrivateChannelEntryDomeRequest {
    pub topic: String,
    pub channel_id: String,
    pub entry_dome_instance_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct LeavePrivateChannelRequest {
    pub topic: String,
    pub channel_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct ListJoinedPrivateChannelsRequest {
    pub topic: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct UpdateGameRoomRequest {
    pub topic: String,
    pub room_id: String,
    pub status: GameRoomStatus,
    pub phase_label: Option<String>,
    pub scores: Vec<GameScoreView>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct UpdateMetaverseRoomRequest {
    pub topic: String,
    pub room_id: String,
    pub status: GameRoomStatus,
    pub customization: DomeCustomizationV1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct MoveDomeRequest {
    pub source_topic: String,
    pub move_id: String,
    pub source_instance_id: String,
    pub target_context: SpatialContextV1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ListDomeConnectionTopologyRequest {
    pub spatial_context: SpatialContextV1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct CreateDomeConnectionProposalRequest {
    pub proposal_id: String,
    pub spatial_context: SpatialContextV1,
    pub proposer_instance_id: String,
    pub receiver_instance_id: String,
    pub proposer_direction: kukuri_core::DomeDirection,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct AcceptDomeConnectionProposalRequest {
    pub spatial_context: SpatialContextV1,
    pub proposal_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct WithdrawDomeConnectionProposalRequest {
    pub spatial_context: SpatialContextV1,
    pub proposal_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct RevokeDomeConnectionRequest {
    pub spatial_context: SpatialContextV1,
    pub connection_id: String,
}

/// #859: アカウント鍵エクスポート要求。passphrase は秘密情報のため Debug で redact する。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ExportAccountKeyRequest {
    pub passphrase: String,
}

impl std::fmt::Debug for ExportAccountKeyRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExportAccountKeyRequest")
            .field("passphrase", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct PreviewAccountKeyImportRequest {
    pub export: String,
}

/// #859: アカウント鍵インポート要求。passphrase は秘密情報のため Debug で redact する。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct ImportAccountKeyRequest {
    pub export: String,
    pub passphrase: String,
    #[serde(default)]
    pub label: Option<String>,
}

impl std::fmt::Debug for ImportAccountKeyRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImportAccountKeyRequest")
            .field("export", &self.export)
            .field("passphrase", &"<redacted>")
            .field("label", &self.label)
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct SwitchAccountRequest {
    pub account_id: String,
}
