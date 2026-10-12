use kukuri_desktop_runtime::{MetaverseAudioStatus, SetMetaverseAudioRequest};

use crate::state::{CommandError, DesktopState, map_error};

/// metaverse の空間音声の状態を画面に合わせる(ADR 0067、#1033)。mic と speaker を使うので GUI だけの command。
#[tauri::command]
pub async fn set_metaverse_audio(
    state: tauri::State<'_, DesktopState>,
    request: SetMetaverseAudioRequest,
) -> Result<MetaverseAudioStatus, CommandError> {
    state
        .runtime()
        .set_metaverse_audio(request)
        .await
        .map_err(map_error)
}
