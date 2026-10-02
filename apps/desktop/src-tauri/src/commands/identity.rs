//! 他のアカウントの表示（native だけ）。アカウントの作成・取込み・切替などは desktop-runtime の表（W1 AC-5）。

use crate::state::{CommandError, DesktopState, map_error};

#[tauri::command]
pub async fn get_account_display(
    state: tauri::State<'_, DesktopState>,
) -> Result<Vec<kukuri_desktop_runtime::AccountDisplay>, CommandError> {
    state.host().account_display().await.map_err(map_error)
}
