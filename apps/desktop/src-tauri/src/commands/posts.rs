use ::tracing::{info, warn};
use kukuri_desktop_runtime::{
    BookmarkedPostIdsRequest, BookmarkPostRequest, CreatePostRequest, CreateRepostRequest,
    GetBlobMediaRequest,
    GetBlobPreviewRequest, ListBookmarkedPostsRequest, ListProfileTimelineRequest,
    ListThreadRequest, ListTimelineRequest,
    RemoveBookmarkedPostRequest, ResolveCommunityIndexPostsRequest, WithdrawPostRequest,
    RetryPostElementsRequest,
};

use crate::state::{CommandError, DesktopState, map_error};

#[tauri::command]
pub async fn create_post(
    state: tauri::State<'_, DesktopState>,
    request: CreatePostRequest,
) -> Result<String, CommandError> {
    state
        .runtime()
        .create_post(request)
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn withdraw_post(
    state: tauri::State<'_, DesktopState>,
    request: WithdrawPostRequest,
) -> Result<String, CommandError> {
    state
        .runtime()
        .withdraw_post(request)
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn create_repost(
    state: tauri::State<'_, DesktopState>,
    request: CreateRepostRequest,
) -> Result<String, CommandError> {
    state
        .runtime()
        .create_repost(request)
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn list_bookmarked_posts_page(
    state: tauri::State<'_, DesktopState>,
    request: ListBookmarkedPostsRequest,
) -> Result<kukuri_app_api::BookmarkedPostPageView, CommandError> {
    state.runtime().list_bookmarked_posts_page(request).await.map_err(map_error)
}

#[tauri::command]
pub async fn bookmarked_post_ids(
    state: tauri::State<'_, DesktopState>,
    request: BookmarkedPostIdsRequest,
) -> Result<Vec<String>, CommandError> {
    state.runtime().bookmarked_post_ids(request).await.map_err(map_error)
}

#[tauri::command]
pub async fn resolve_community_index_posts(
    state: tauri::State<'_, DesktopState>,
    request: ResolveCommunityIndexPostsRequest,
) -> Result<kukuri_app_api::CommunityIndexPostResolveResponse, CommandError> {
    state
        .runtime()
        .resolve_community_index_posts(request)
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn bookmark_post(
    state: tauri::State<'_, DesktopState>,
    request: BookmarkPostRequest,
) -> Result<kukuri_app_api::BookmarkedPostView, CommandError> {
    state
        .runtime()
        .bookmark_post(request)
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn remove_bookmarked_post(
    state: tauri::State<'_, DesktopState>,
    request: RemoveBookmarkedPostRequest,
) -> Result<(), CommandError> {
    state
        .runtime()
        .remove_bookmarked_post(request)
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn list_timeline(
    state: tauri::State<'_, DesktopState>,
    request: ListTimelineRequest,
) -> Result<kukuri_app_api::TimelineView, CommandError> {
    state
        .runtime()
        .list_timeline(request)
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn list_thread(
    state: tauri::State<'_, DesktopState>,
    request: ListThreadRequest,
) -> Result<kukuri_app_api::TimelineView, CommandError> {
    state
        .runtime()
        .list_thread(request)
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn list_profile_timeline(
    state: tauri::State<'_, DesktopState>,
    request: ListProfileTimelineRequest,
) -> Result<kukuri_app_api::TimelineView, CommandError> {
    state
        .runtime()
        .list_profile_timeline(request)
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn retry_post_elements(
    state: tauri::State<'_, DesktopState>,
    request: RetryPostElementsRequest,
) -> Result<Option<kukuri_app_api::PostView>, CommandError> {
    state
        .runtime()
        .retry_post_elements(request)
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn get_blob_preview_url(
    state: tauri::State<'_, DesktopState>,
    request: GetBlobPreviewRequest,
) -> Result<Option<String>, CommandError> {
    state
        .runtime()
        .get_blob_preview_url(request)
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn get_blob_media_payload(
    state: tauri::State<'_, DesktopState>,
    request: GetBlobMediaRequest,
) -> Result<Option<kukuri_app_api::BlobMediaPayload>, CommandError> {
    let hash = request.hash.clone();
    let mime = request.mime.clone();
    info!(hash = %hash, mime = %mime, "received get_blob_media_payload command");
    match state.runtime().get_blob_media_payload(request).await {
        Ok(Some(payload)) => {
            info!(
                hash = %hash,
                mime = %mime,
                bytes_base64_len = payload.bytes_base64.len(),
                "returning get_blob_media_payload response"
            );
            Ok(Some(payload))
        }
        Ok(None) => {
            warn!(hash = %hash, mime = %mime, "get_blob_media_payload returned no blob");
            Ok(None)
        }
        Err(error) => {
            let error = map_error(error);
            warn!(
                hash = %hash,
                mime = %mime,
                error = %error.message,
                "get_blob_media_payload command failed"
            );
            Err(error)
        }
    }
}

/// #858: 成人向け表現の表示設定(既定 OFF)。runtime のローカル JSON が canonical。
#[tauri::command]
pub fn get_content_display_settings(
    state: tauri::State<'_, DesktopState>,
) -> Result<kukuri_app_api::ContentDisplaySettings, CommandError> {
    Ok(state.runtime().get_content_display_settings())
}

#[tauri::command]
pub fn set_adult_content_display_enabled(
    state: tauri::State<'_, DesktopState>,
    enabled: bool,
) -> Result<kukuri_app_api::ContentDisplaySettings, CommandError> {
    info!(
        enabled,
        "received set_adult_content_display_enabled command"
    );
    state
        .runtime()
        .set_adult_content_display_enabled(enabled)
        .map_err(map_error)
}
