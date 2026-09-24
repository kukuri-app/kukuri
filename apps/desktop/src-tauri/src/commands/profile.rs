use kukuri_desktop_runtime::{
    AuthorRequest, ListNotificationsPageRequest, ListSocialConnectionsRequest, NotificationIdRequest, SetMyProfileRequest,
};

use crate::state::{CommandError, DesktopState, map_error};

#[tauri::command]
pub async fn get_my_profile(
    state: tauri::State<'_, DesktopState>,
) -> Result<kukuri_core::Profile, CommandError> {
    state.runtime().get_my_profile().await.map_err(map_error)
}

#[tauri::command]
pub async fn set_my_profile(
    state: tauri::State<'_, DesktopState>,
    request: SetMyProfileRequest,
) -> Result<kukuri_core::Profile, CommandError> {
    state
        .runtime()
        .set_my_profile(request)
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn follow_author(
    state: tauri::State<'_, DesktopState>,
    request: AuthorRequest,
) -> Result<kukuri_app_api::AuthorSocialView, CommandError> {
    state
        .runtime()
        .follow_author(request)
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn unfollow_author(
    state: tauri::State<'_, DesktopState>,
    request: AuthorRequest,
) -> Result<kukuri_app_api::AuthorSocialView, CommandError> {
    state
        .runtime()
        .unfollow_author(request)
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn get_author_social_view(
    state: tauri::State<'_, DesktopState>,
    request: AuthorRequest,
) -> Result<kukuri_app_api::AuthorSocialView, CommandError> {
    state
        .runtime()
        .get_author_social_view(request)
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn mute_author(
    state: tauri::State<'_, DesktopState>,
    request: AuthorRequest,
) -> Result<kukuri_app_api::AuthorSocialView, CommandError> {
    state
        .runtime()
        .mute_author(request)
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn unmute_author(
    state: tauri::State<'_, DesktopState>,
    request: AuthorRequest,
) -> Result<kukuri_app_api::AuthorSocialView, CommandError> {
    state
        .runtime()
        .unmute_author(request)
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn block_author(
    state: tauri::State<'_, DesktopState>,
    request: AuthorRequest,
) -> Result<kukuri_app_api::AuthorSocialView, CommandError> {
    state
        .runtime()
        .block_author(request)
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn unblock_author(
    state: tauri::State<'_, DesktopState>,
    request: AuthorRequest,
) -> Result<kukuri_app_api::AuthorSocialView, CommandError> {
    state
        .runtime()
        .unblock_author(request)
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn list_social_connections(
    state: tauri::State<'_, DesktopState>,
    request: ListSocialConnectionsRequest,
) -> Result<Vec<kukuri_app_api::AuthorSocialView>, CommandError> {
    state
        .runtime()
        .list_social_connections(request)
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn list_notifications_page(
    state: tauri::State<'_, DesktopState>,
    request: ListNotificationsPageRequest,
) -> Result<kukuri_app_api::NotificationPageView, CommandError> {
    state.runtime().list_notifications_page(request).await.map_err(map_error)
}

#[tauri::command]
pub async fn mark_notification_read(
    state: tauri::State<'_, DesktopState>,
    request: NotificationIdRequest,
) -> Result<kukuri_app_api::NotificationStatusView, CommandError> {
    state
        .runtime()
        .mark_notification_read(request)
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn mark_all_notifications_read(
    state: tauri::State<'_, DesktopState>,
) -> Result<kukuri_app_api::NotificationStatusView, CommandError> {
    state
        .runtime()
        .mark_all_notifications_read()
        .await
        .map_err(map_error)
}

#[tauri::command]
pub async fn get_notification_status(
    state: tauri::State<'_, DesktopState>,
) -> Result<kukuri_app_api::NotificationStatusView, CommandError> {
    state
        .runtime()
        .get_notification_status()
        .await
        .map_err(map_error)
}
