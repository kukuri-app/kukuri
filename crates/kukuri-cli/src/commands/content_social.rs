use std::sync::Arc;

use async_trait::async_trait;
use kukuri_desktop_runtime::{
    AuthorRequest, BookmarkCustomReactionRequest, BookmarkMediaRequest, BookmarkPostRequest,
    CreateCustomReactionAssetRequest, CreateCustomReactionSetRequest, CreatePostRequest,
    CreateRepostRequest, ImportCustomReactionSetRequest, ListBookmarkedMediaRequest,
    ListBookmarkedPostsRequest, ListProfileTimelineRequest, ListRecentReactionsRequest,
    ListSocialConnectionsRequest, ListThreadRequest, ListTimelineRequest, MediaBookmarkRequest,
    NotificationIdRequest, RemoveBookmarkedCustomReactionRequest, RemoveBookmarkedPostRequest,
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

// command名・effect・実行対応の手書き正本は下の一つの表だけ(#1780)。
macro_rules! content_commands {
    ($runtime:ident, $context:ident, $payload:ident;
     $( $operation:ident ($name:literal, $effect:ident) => $execute:expr ),+ $(,)?) => {
        #[derive(Clone, Copy)]
        enum Operation { $( $operation ),+ }
        struct Handler(Operation);

        #[async_trait]
        impl CommandHandler for Handler {
            async fn execute(
                &self,
                $context: HandlerContext<'_>,
                $payload: Value,
                _secret: Option<&SecretInput>,
            ) -> Result<CommandOutput, ProtocolError> {
                let $runtime = runtime(&$context)?;
                match self.0 { $( Operation::$operation => $execute ),+ }
            }
        }

        pub(super) fn registrations() -> Vec<CommandRegistration> {
            vec![$( entry($name, CommandEffect::$effect, Operation::$operation) ),+]
        }
    };
}

content_commands!(runtime, context, payload;
    BookmarkMedia("bookmark_media", Write) => async_call!(runtime, payload, BookmarkMediaRequest, bookmark_media),
    GetBookmarkedMedia("get_bookmarked_media", Read) => async_call!(runtime, payload, MediaBookmarkRequest, get_bookmarked_media),
    ListBookmarkedMedia("list_bookmarked_media", Read) => async_call!(runtime, payload, ListBookmarkedMediaRequest, list_bookmarked_media),
    RemoveBookmarkedMedia("remove_bookmarked_media", Destructive) => async_call!(runtime, payload, MediaBookmarkRequest, remove_bookmarked_media),
    CreatePost("create_post", Write) => encode(
        runtime
            .create_post(
                media::decode_with_files::<CreatePostRequest>(payload, "attachments", true)
                    .await?,
            )
            .await
            .map_err(command_error)?,
    ),
    WithdrawPost("withdraw_post", Destructive) => {
        async_call!(runtime, payload, WithdrawPostRequest, withdraw_post)
    },
    CreateRepost("create_repost", Write) => {
        async_call!(runtime, payload, CreateRepostRequest, create_repost)
    },
    // #1221 R5-G: cursorの後ろの1ページを返す。
    ListBookmarkedPosts("list_bookmarked_posts", Read) => async_call!(
        runtime,
        payload,
        ListBookmarkedPostsRequest,
        list_bookmarked_posts_page
    ),
    ResolveCommunityIndexPosts("resolve_community_index_posts", Read) => async_call!(
        runtime,
        payload,
        ResolveCommunityIndexPostsRequest,
        resolve_community_index_posts
    ),
    BookmarkPost("bookmark_post", Write) => {
        async_call!(runtime, payload, BookmarkPostRequest, bookmark_post)
    },
    RemoveBookmarkedPost("remove_bookmarked_post", Destructive) => async_call!(
        runtime,
        payload,
        RemoveBookmarkedPostRequest,
        remove_bookmarked_post
    ),
    ListTimeline("list_timeline", Read) => {
        async_call!(runtime, payload, ListTimelineRequest, list_timeline)
    },
    ListThread("list_thread", Read) => async_call!(runtime, payload, ListThreadRequest, list_thread),
    ListProfileTimeline("list_profile_timeline", Read) => async_call!(
        runtime,
        payload,
        ListProfileTimelineRequest,
        list_profile_timeline
    ),
    GetBlobPreviewUrl("get_blob_preview_url", Read) => media_output::export(&context, payload, true).await,
    GetBlobMediaPayload("get_blob_media_payload", Read) => media_output::export(&context, payload, false).await,
    GetContentDisplaySettings("get_content_display_settings", Read) => encode(runtime.get_content_display_settings()),
    SetAdultContentDisplayEnabled("set_adult_content_display_enabled", Write) => {
        let request: AdultContentRequest = decode(payload)?;
        encode(
            runtime
                .set_adult_content_display_enabled(request.enabled)
                .await
                .map_err(command_error)?,
        )
    },
    ToggleReaction("toggle_reaction", Write) => {
        async_call!(runtime, payload, ToggleReactionRequest, toggle_reaction)
    },
    ListMyCustomReactionAssets("list_my_custom_reaction_assets", Read) => {
        async_call0!(runtime, list_my_custom_reaction_assets)
    },
    ListRecentReactions("list_recent_reactions", Read) => async_call!(
        runtime,
        payload,
        ListRecentReactionsRequest,
        list_recent_reactions
    ),
    CreateCustomReactionAsset("create_custom_reaction_asset", Write) => encode(
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
    ListBookmarkedCustomReactions("list_bookmarked_custom_reactions", Read) => {
        async_call0!(runtime, list_bookmarked_custom_reactions)
    },
    BookmarkCustomReaction("bookmark_custom_reaction", Write) => async_call!(
        runtime,
        payload,
        BookmarkCustomReactionRequest,
        bookmark_custom_reaction
    ),
    RemoveBookmarkedCustomReaction("remove_bookmarked_custom_reaction", Destructive) => async_call!(
        runtime,
        payload,
        RemoveBookmarkedCustomReactionRequest,
        remove_bookmarked_custom_reaction
    ),
    CreateCustomReactionSet("create_custom_reaction_set", Write) => async_call!(
        runtime,
        payload,
        CreateCustomReactionSetRequest,
        create_custom_reaction_set
    ),
    ListMyCustomReactionSets("list_my_custom_reaction_sets", Read) => {
        async_call0!(runtime, list_my_custom_reaction_sets)
    },
    ImportCustomReactionSet("import_custom_reaction_set", Write) => async_call!(
        runtime,
        payload,
        ImportCustomReactionSetRequest,
        import_custom_reaction_set
    ),
    GetMyProfile("get_my_profile", Read) => async_call0!(runtime, get_my_profile),
    SetMyProfile("set_my_profile", Write) => encode(
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
    FollowAuthor("follow_author", Write) => {
        async_call!(runtime, payload, AuthorRequest, follow_author)
    },
    UnfollowAuthor("unfollow_author", Destructive) => {
        async_call!(runtime, payload, AuthorRequest, unfollow_author)
    },
    GetAuthorSocialView("get_author_social_view", Read) => {
        async_call!(runtime, payload, AuthorRequest, get_author_social_view)
    },
    MuteAuthor("mute_author", Write) => async_call!(runtime, payload, AuthorRequest, mute_author),
    UnmuteAuthor("unmute_author", Destructive) => async_call!(runtime, payload, AuthorRequest, unmute_author),
    BlockAuthor("block_author", Write) => async_call!(runtime, payload, AuthorRequest, block_author),
    UnblockAuthor("unblock_author", Destructive) => {
        async_call!(runtime, payload, AuthorRequest, unblock_author)
    },
    ListSocialConnections("list_social_connections", Read) => async_call!(
        runtime,
        payload,
        ListSocialConnectionsRequest,
        list_social_connections
    ),
    ListNotifications("list_notifications", Read) => async_call0!(runtime, list_notifications),
    MarkNotificationRead("mark_notification_read", Write) => async_call!(
        runtime,
        payload,
        NotificationIdRequest,
        mark_notification_read
    ),
    MarkAllNotificationsRead("mark_all_notifications_read", Write) => {
        async_call0!(runtime, mark_all_notifications_read)
    },
    GetNotificationStatus("get_notification_status", Read) => async_call0!(runtime, get_notification_status),
);

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
