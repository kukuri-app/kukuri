use kukuri_app_api::{
    CommunityIndexPostResolveInput, GameScoreView, PostLiveView, SocialConnectionKind,
};

use kukuri_core::{
    ChannelAudienceKind, ChannelRef, DomeCustomizationV1, DomeSessionInputKindV1,
    DomeTransitionAdmissionRequestV1, DomeTransitionAdmissionTicketV1, GameRoomStatus,
    MetaverseAssetKind, MetaverseRoomEventV1, SpatialContextV1, TimelineScope,
};
use kukuri_store::{BookmarkCursor, NotificationCursor, TimelineCursor};
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
    /// #1839: 紹介する Live。
    #[serde(default)]
    pub live: Option<PostLiveView>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bookmarked_media_hash: Option<String>,
    pub role: Option<String>,
    #[cfg(target_family = "wasm")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_ref: Option<WebAttachmentRef>,
}

#[cfg(target_family = "wasm")]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebAttachmentRef {
    pub id: String,
    pub owner: String,
    pub hash: String,
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

/// 自作・保存済みから選んだリアクション（最大 100 件）と名前でセットを作る（#1232 AC-4）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct CreateCustomReactionSetRequest {
    pub name: String,
    pub items: Vec<kukuri_app_api::CustomReactionAssetView>,
}

/// 共有の文字列（`kukuri:reaction-set:<set_hash>`）のセットを取り込む（#1232 AC-4）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct ImportCustomReactionSetRequest {
    pub set_hash: String,
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

/// 公開 topic の保持の設定と解除(#1909。CLI だけの command)。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetTopicRetentionRequest {
    pub topic: String,
    pub retained: bool,
}

/// 公開 topic の保持の期間(日)と容量(bytes)(#1909。CLI だけの command)。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetTopicRetentionLimitsRequest {
    pub retention_days: u32,
    pub capacity_bytes: u64,
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
    #[serde(default)]
    pub source_object_id: Option<String>,
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
pub struct ListNotificationsPageRequest {
    pub cursor: Option<NotificationCursor>,
    #[serde(default)]
    pub before: bool,
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
    // NIP-05 の識別子 `name@domain`(ADR 0064)。空なら欄の無い profile にする。
    pub nip05: Option<String>,
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
    /// #1839: 開始 post の本文(空でもよい)。
    #[serde(default)]
    pub start_post: String,
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
    /// あれば action session の room を作る(SDK。`participants` は空。#1978)。
    #[serde(default)]
    pub action: Option<kukuri_app_api::CreateActionSessionInput>,
}

/// action session の room の一覧(SDK。#1978)。1 頁は 20 件。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListActionRoomsRequest {
    pub topic: String,
    #[serde(default)]
    pub scope: TimelineScope,
    pub application_id: String,
    /// 前の頁の `next_cursor`。無ければ最初の頁。
    #[serde(default)]
    pub cursor: Option<String>,
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

/// 公開 channel(公開スレッド。#1840)を作る。`attachments` と `content_labels` は最初の投稿の添付と自己申告のラベル
/// (タイムラインの投稿と同じ。#1889)。`first_post` と `attachments` が両方空なら最初の投稿を書かない。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct CreatePublicChannelRequest {
    pub topic: String,
    pub title: String,
    #[serde(default)]
    pub first_post: String,
    #[serde(default)]
    pub attachments: Vec<CreateAttachmentRequest>,
    #[serde(default)]
    pub content_labels: Vec<String>,
}

/// 公開 channel を 1 件指す(ID からの取得と、作成者の削除。#1888)。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct GetPublicChannelRequest {
    pub topic: String,
    pub channel_id: String,
}

/// 作成者が公開 channel のタイトルを変える(#1888)。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct UpdatePublicChannelTitleRequest {
    pub topic: String,
    pub channel_id: String,
    pub title: String,
}

/// topic の公開 channel の一覧の 1 ページ(#1840)。`order` の既定は最後の投稿の新しい順、`limit` の既定は 20。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct ListPublicChannelsRequest {
    pub topic: String,
    #[serde(default)]
    pub order: kukuri_store::PublicChannelOrder,
    #[serde(default)]
    pub cursor: Option<kukuri_store::PublicChannelCursor>,
    #[serde(default)]
    pub limit: Option<usize>,
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
pub struct TakePrivateChannelControllerRequest {
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
    /// 前の page の `next_cursor`。無ければ最初の page。
    #[serde(default)]
    pub cursor: Option<String>,
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

/// #1211: 移行先が開くリンクと、受ける投稿の履歴の範囲（移さないときは `None`。AC-3）。リンクは招待の秘密を含む
/// ので Debug で redact する。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct OpenAccountTransferRequest {
    pub link: String,
    #[serde(default)]
    pub history: Option<kukuri_core::AccountTransferHistory>,
}

impl std::fmt::Debug for OpenAccountTransferRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAccountTransferRequest")
            .field("link", &"<redacted>")
            .field("history", &self.history)
            .finish()
    }
}

/// #1211: 確認コードが一致したか。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct DecideAccountTransferRequest {
    pub accept: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct SwitchAccountRequest {
    pub account_id: String,
}
