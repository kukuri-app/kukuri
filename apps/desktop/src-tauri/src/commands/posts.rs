use kukuri_desktop_runtime::GetBlobMediaRequest;

use crate::state::{CommandError, DesktopState, map_error};
use serde::Serialize;

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
