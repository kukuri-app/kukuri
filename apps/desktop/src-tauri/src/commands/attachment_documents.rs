//! Android の選択済み URI を、明示投稿の間だけ file として扱う（#1197 AC-2b）。
use kukuri_desktop_runtime::CreatePostRequest;
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    io::{Read, Write},
};
use tauri::Manager;

use crate::{
    restore_lifecycle::require_runtime_operation_ready,
    state::{CommandError, DesktopStartupState, DesktopState, map_error},
};

#[derive(Deserialize)]
pub struct AttachmentDocument {
    index: usize,
    uri: String,
}

fn copy_selected(mut reader: impl Read, mut writer: impl Write) -> anyhow::Result<u64> {
    let mut chunk = [0u8; 64 * 1024];
    let mut length = 0;
    loop {
        let count = reader.read(&mut chunk)?;
        if count == 0 {
            return Ok(length);
        }
        writer.write_all(&chunk[..count])?;
        length += count as u64;
    }
}

#[tauri::command]
pub async fn create_post_from_documents(
    app: tauri::AppHandle,
    state: tauri::State<'_, DesktopState>,
    startup: tauri::State<'_, DesktopStartupState>,
    request: CreatePostRequest,
    documents: Vec<AttachmentDocument>,
) -> Result<String, CommandError> {
    crate::desktop_lifecycle::require_running(&app)?;
    require_runtime_operation_ready(&startup.status()).map_err(CommandError::from)?;
    let host = state.host();
    let generation = host.generation();
    let runtime = host.runtime();
    let sizes = request
        .attachments
        .iter()
        .map(|item| item.byte_size)
        .collect::<Vec<_>>();
    let (directory, files) = tauri::async_runtime::spawn_blocking(move || -> anyhow::Result<_> {
        let directory = app
            .state::<crate::attachment_staging::AttachmentStaging>()
            .session(&app.path().app_cache_dir()?)?;
        let mut files = BTreeMap::new();
        for document in documents {
            let expected = sizes
                .get(document.index)
                .ok_or_else(|| anyhow::anyhow!("invalid attachment index"))?;
            anyhow::ensure!(
                !files.contains_key(&document.index),
                "duplicate attachment index"
            );
            let reader = super::user_document::open_user_document(&app, &document.uri, false)?
                .ok_or_else(|| {
                    anyhow::anyhow!("selected attachment is not an Android Content URI")
                })?;
            let mut file = tempfile::NamedTempFile::new_in(directory.path())?;
            anyhow::ensure!(
                copy_selected(reader, &mut file)? == *expected,
                "selected attachment size changed"
            );
            files.insert(document.index, file);
        }
        Ok((directory, files))
    })
    .await
    .map_err(|error| CommandError::from(format!("attachment copy failed: {error}")))?
    .map_err(map_error)?;
    let paths = files
        .iter()
        .map(|(index, file)| (*index, file.path().to_path_buf()))
        .collect();
    require_runtime_operation_ready(&startup.status()).map_err(CommandError::from)?;
    if host.generation() != generation {
        return Err(CommandError::new(
            kukuri_desktop_runtime::STALE_RUNTIME_CODE,
            "runtime changed during attachment copy",
        ));
    }
    let result = runtime
        .create_post_from_files(request, paths)
        .await
        .map_err(map_error);
    // NamedTempFile の owner は保存完了/失敗まで保持し、その後必ず回収する。
    drop(files);
    drop(directory);
    if host.generation() != generation {
        return Err(CommandError::new(
            kukuri_desktop_runtime::STALE_RUNTIME_CODE,
            "runtime changed during attachment post",
        ));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    struct SizedInput(u64);
    impl Read for SizedInput {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            assert!(buffer.len() <= 64 * 1024);
            let count = self.0.min(buffer.len() as u64) as usize;
            buffer[..count].fill(7);
            self.0 -= count as u64;
            Ok(count)
        }
    }

    #[test]
    fn selected_copy_has_constant_buffer_for_small_and_300_mib_inputs() {
        for size in [780, 300 * 1024 * 1024] {
            assert_eq!(
                copy_selected(SizedInput(size), std::io::sink()).unwrap(),
                size
            );
        }
    }

    #[test]
    fn failed_selected_read_leaves_no_staging_file() {
        let root = tempfile::tempdir().unwrap();
        let mut file = tempfile::NamedTempFile::new_in(root.path()).unwrap();
        let path = file.path().to_path_buf();
        struct FailedRead;
        impl Read for FailedRead {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("selected URI expired"))
            }
        }
        assert!(copy_selected(FailedRead, &mut file).is_err());
        drop(file);
        assert!(!path.exists());
    }
}
