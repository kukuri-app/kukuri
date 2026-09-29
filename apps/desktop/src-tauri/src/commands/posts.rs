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
use serde::Serialize;

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
) -> Result<Option<PostRetryView>, CommandError> {
    let post = state
        .runtime()
        .retry_post_elements(request)
        .await
        .map_err(map_error)?;
    let Some(post) = post else { return Ok(None) };
    let display_retry_next_at_ms = state
        .runtime()
        .post_display_retry_at(&post)
        .await
        .map_err(map_error)?;
    Ok(Some(PostRetryView {
        post,
        display_retry_next_at_ms,
    }))
}

#[derive(serde::Serialize)]
pub struct PostRetryView {
    #[serde(flatten)]
    post: kukuri_app_api::PostView,
    display_retry_next_at_ms: Option<i64>,
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

#[derive(Serialize)]
pub struct BlobMediaFile {
    pub path: String,
    pub request_id: String,
    pub bytes: u64,
}

#[tauri::command]
pub async fn get_blob_media_file(
    state: tauri::State<'_, DesktopState>,
    request: GetBlobMediaRequest,
    request_id: String,
) -> Result<Option<BlobMediaFile>, CommandError> {
    let (path, mut cancelled) = state
        .media_previews
        .begin(&request_id, &request.mime)
        .map_err(map_error)?;
    let runtime = state.runtime();
    let result = tokio::select! {
        biased;
        _ = cancelled.changed() => Ok(None),
        result = runtime.get_blob_media_file(request, &path) => result.map_err(map_error),
    };
    match result {
        Ok(Some(bytes)) if state.media_previews.contains(&request_id) => Ok(Some(BlobMediaFile {
            path: path.to_string_lossy().into_owned(),
            request_id,
            bytes,
        })),
        Ok(_) => {
            state.media_previews.release(&request_id);
            let _ = tokio::fs::remove_file(path).await;
            Ok(None)
        }
        Err(error) => {
            state.media_previews.release(&request_id);
            let _ = tokio::fs::remove_file(path).await;
            Err(error)
        }
    }
}

#[tauri::command]
pub async fn release_blob_media_file(
    state: tauri::State<'_, DesktopState>,
    request_id: String,
) -> Result<(), CommandError> {
    state.media_previews.release(&request_id);
    Ok(())
}

/// #858: 成人向け表現の表示設定(既定 OFF)。runtime のローカル JSON が canonical。
#[tauri::command]
pub fn get_content_display_settings(
    state: tauri::State<'_, DesktopState>,
) -> Result<kukuri_app_api::ContentDisplaySettings, CommandError> {
    Ok(state.runtime().get_content_display_settings())
}

/// #1419: 成人向けの cache の削除を tokio runtime で始めるため、async の command にする。
#[tauri::command]
pub async fn set_adult_content_display_enabled(
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
