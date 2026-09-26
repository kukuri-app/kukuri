use std::sync::Arc;

use async_trait::async_trait;
use kukuri_desktop_runtime::{
    AuthorRequest, BookmarkCustomReactionRequest, BookmarkPostRequest,
    CreateCustomReactionAssetRequest, CreatePostRequest, CreateRepostRequest,
    ListBookmarkedPostsRequest, ListProfileTimelineRequest, ListRecentReactionsRequest,
    ListSocialConnectionsRequest, ListThreadRequest, ListTimelineRequest, NotificationIdRequest,
    RemoveBookmarkedCustomReactionRequest, RemoveBookmarkedPostRequest,
    ResolveCommunityIndexPostsRequest, SetMyProfileRequest, ToggleReactionRequest,
    WithdrawPostRequest,
};
use serde::Deserialize;
use serde_json::Value;

use super::{
    command, command_error, content_schema, content_views, decode, encode, host_guards, media,
    media_output, runtime,
};
use crate::{
    protocol::{CommandEffect, ProtocolError, SecretInput},
    registry::{CommandHandler, CommandOutput, CommandRegistration, HandlerContext},
};

#[derive(Clone, Copy)]
enum Operation {
    CreatePost,
    WithdrawPost,
    CreateRepost,
    ListBookmarkedPosts,
    ResolveCommunityIndexPosts,
    BookmarkPost,
    RemoveBookmarkedPost,
    ListTimeline,
    ListThread,
    ListProfileTimeline,
    GetBlobPreviewUrl,
    GetBlobMediaPayload,
    GetContentDisplaySettings,
    SetAdultContentDisplayEnabled,
    ToggleReaction,
    ListMyCustomReactionAssets,
    ListRecentReactions,
    CreateCustomReactionAsset,
    ListBookmarkedCustomReactions,
    BookmarkCustomReaction,
    RemoveBookmarkedCustomReaction,
    GetMyProfile,
    SetMyProfile,
    FollowAuthor,
    UnfollowAuthor,
    GetAuthorSocialView,
    MuteAuthor,
    UnmuteAuthor,
    BlockAuthor,
    UnblockAuthor,
    ListSocialConnections,
    ListNotifications,
    MarkNotificationRead,
    MarkAllNotificationsRead,
    GetNotificationStatus,
}

struct Handler(Operation);

macro_rules! async_call {
    ($runtime:expr, $payload:expr, $request:ty, $method:ident) => {{
        let request: $request = decode($payload)?;
        encode($runtime.$method(request).await.map_err(command_error)?)
    }};
}

macro_rules! async_call0 {
    ($runtime:expr, $method:ident) => {{ encode($runtime.$method().await.map_err(command_error)?) }};
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AdultContentRequest {
    enabled: bool,
}

#[async_trait]
impl CommandHandler for Handler {
    async fn execute(
        &self,
        context: HandlerContext<'_>,
        payload: Value,
        _secret: Option<&SecretInput>,
    ) -> Result<CommandOutput, ProtocolError> {
        let runtime = runtime(&context)?;
        match self.0 {
            Operation::CreatePost => encode(
                runtime
                    .create_post(
                        media::decode_with_files::<CreatePostRequest>(payload, "attachments", true)
                            .await?,
                    )
                    .await
                    .map_err(command_error)?,
            ),
            Operation::WithdrawPost => {
                async_call!(runtime, payload, WithdrawPostRequest, withdraw_post)
            }
            Operation::CreateRepost => {
                async_call!(runtime, payload, CreateRepostRequest, create_repost)
            }
            // #1221 R5-G: 全件でなく、cursor の後ろの 1 ページを返す。
            Operation::ListBookmarkedPosts => async_call!(
                runtime,
                payload,
                ListBookmarkedPostsRequest,
                list_bookmarked_posts_page
            ),
            Operation::ResolveCommunityIndexPosts => async_call!(
                runtime,
                payload,
                ResolveCommunityIndexPostsRequest,
                resolve_community_index_posts
            ),
            Operation::BookmarkPost => {
                async_call!(runtime, payload, BookmarkPostRequest, bookmark_post)
            }
            Operation::RemoveBookmarkedPost => async_call!(
                runtime,
                payload,
                RemoveBookmarkedPostRequest,
                remove_bookmarked_post
            ),
            Operation::ListTimeline => {
                async_call!(runtime, payload, ListTimelineRequest, list_timeline)
            }
            Operation::ListThread => async_call!(runtime, payload, ListThreadRequest, list_thread),
            Operation::ListProfileTimeline => async_call!(
                runtime,
                payload,
                ListProfileTimelineRequest,
                list_profile_timeline
            ),
            Operation::GetBlobPreviewUrl => media_output::export(&context, payload, true).await,
            Operation::GetBlobMediaPayload => media_output::export(&context, payload, false).await,
            Operation::GetContentDisplaySettings => encode(runtime.get_content_display_settings()),
            Operation::SetAdultContentDisplayEnabled => {
                let request: AdultContentRequest = decode(payload)?;
                encode(
                    runtime
                        .set_adult_content_display_enabled(request.enabled)
                        .map_err(command_error)?,
                )
            }
            Operation::ToggleReaction => {
                async_call!(runtime, payload, ToggleReactionRequest, toggle_reaction)
            }
            Operation::ListMyCustomReactionAssets => {
                async_call0!(runtime, list_my_custom_reaction_assets)
            }
            Operation::ListRecentReactions => async_call!(
                runtime,
                payload,
                ListRecentReactionsRequest,
                list_recent_reactions
            ),
            Operation::CreateCustomReactionAsset => encode(
                runtime
                    .create_custom_reaction_asset(
                        media::decode_with_files::<CreateCustomReactionAssetRequest>(
                            payload, "upload", false,
                        )
                        .await?,
                    )
                    .await
                    .map_err(command_error)?,
            ),
            Operation::ListBookmarkedCustomReactions => {
                async_call0!(runtime, list_bookmarked_custom_reactions)
            }
            Operation::BookmarkCustomReaction => async_call!(
                runtime,
                payload,
                BookmarkCustomReactionRequest,
                bookmark_custom_reaction
            ),
            Operation::RemoveBookmarkedCustomReaction => async_call!(
                runtime,
                payload,
                RemoveBookmarkedCustomReactionRequest,
                remove_bookmarked_custom_reaction
            ),
            Operation::GetMyProfile => async_call0!(runtime, get_my_profile),
            Operation::SetMyProfile => encode(
                runtime
                    .set_my_profile(
                        media::decode_with_files::<SetMyProfileRequest>(
                            payload,
                            "picture_upload",
                            false,
                        )
                        .await?,
                    )
                    .await
                    .map_err(command_error)?,
            ),
            Operation::FollowAuthor => {
                async_call!(runtime, payload, AuthorRequest, follow_author)
            }
            Operation::UnfollowAuthor => {
                async_call!(runtime, payload, AuthorRequest, unfollow_author)
            }
            Operation::GetAuthorSocialView => {
                async_call!(runtime, payload, AuthorRequest, get_author_social_view)
            }
            Operation::MuteAuthor => async_call!(runtime, payload, AuthorRequest, mute_author),
            Operation::UnmuteAuthor => async_call!(runtime, payload, AuthorRequest, unmute_author),
            Operation::BlockAuthor => async_call!(runtime, payload, AuthorRequest, block_author),
            Operation::UnblockAuthor => {
                async_call!(runtime, payload, AuthorRequest, unblock_author)
            }
            Operation::ListSocialConnections => async_call!(
                runtime,
                payload,
                ListSocialConnectionsRequest,
                list_social_connections
            ),
            Operation::ListNotifications => async_call0!(runtime, list_notifications),
            Operation::MarkNotificationRead => async_call!(
                runtime,
                payload,
                NotificationIdRequest,
                mark_notification_read
            ),
            Operation::MarkAllNotificationsRead => {
                async_call0!(runtime, mark_all_notifications_read)
            }
            Operation::GetNotificationStatus => async_call0!(runtime, get_notification_status),
        }
    }
}

pub(super) fn registrations() -> Vec<CommandRegistration> {
    use CommandEffect::{Destructive, Read, Write};
    use Operation::*;

    vec![
        entry("create_post", Write, CreatePost),
        entry("withdraw_post", Destructive, WithdrawPost),
        entry("create_repost", Write, CreateRepost),
        entry("list_bookmarked_posts", Read, ListBookmarkedPosts),
        entry(
            "resolve_community_index_posts",
            Read,
            ResolveCommunityIndexPosts,
        ),
        entry("bookmark_post", Write, BookmarkPost),
        entry("remove_bookmarked_post", Destructive, RemoveBookmarkedPost),
        entry("list_timeline", Read, ListTimeline),
        entry("list_thread", Read, ListThread),
        entry("list_profile_timeline", Read, ListProfileTimeline),
        entry("get_blob_preview_url", Read, GetBlobPreviewUrl),
        entry("get_blob_media_payload", Read, GetBlobMediaPayload),
        entry(
            "get_content_display_settings",
            Read,
            GetContentDisplaySettings,
        ),
        entry(
            "set_adult_content_display_enabled",
            Write,
            SetAdultContentDisplayEnabled,
        ),
        entry("toggle_reaction", Write, ToggleReaction),
        entry(
            "list_my_custom_reaction_assets",
            Read,
            ListMyCustomReactionAssets,
        ),
        entry("list_recent_reactions", Read, ListRecentReactions),
        entry(
            "create_custom_reaction_asset",
            Write,
            CreateCustomReactionAsset,
        ),
        entry(
            "list_bookmarked_custom_reactions",
            Read,
            ListBookmarkedCustomReactions,
        ),
        entry("bookmark_custom_reaction", Write, BookmarkCustomReaction),
        entry(
            "remove_bookmarked_custom_reaction",
            Destructive,
            RemoveBookmarkedCustomReaction,
        ),
        entry("get_my_profile", Read, GetMyProfile),
        entry("set_my_profile", Write, SetMyProfile),
        entry("follow_author", Write, FollowAuthor),
        entry("unfollow_author", Destructive, UnfollowAuthor),
        entry("get_author_social_view", Read, GetAuthorSocialView),
        entry("mute_author", Write, MuteAuthor),
        entry("unmute_author", Destructive, UnmuteAuthor),
        entry("block_author", Write, BlockAuthor),
        entry("unblock_author", Destructive, UnblockAuthor),
        entry("list_social_connections", Read, ListSocialConnections),
        entry("list_notifications", Read, ListNotifications),
        entry("mark_notification_read", Write, MarkNotificationRead),
        entry(
            "mark_all_notifications_read",
            Write,
            MarkAllNotificationsRead,
        ),
        entry("get_notification_status", Read, GetNotificationStatus),
    ]
}

fn entry(name: &'static str, effect: CommandEffect, operation: Operation) -> CommandRegistration {
    command(
        name,
        effect,
        false,
        false,
        // account/author/audienceの判定は呼び出す共有runtime/serviceが所有する。
        // ここではdispatcherが実際に評価できる起動前guardだけを宣言する。
        host_guards(),
        (content_schema::input(name), content_views::output(name)),
        Arc::new(Handler(operation)),
    )
}
