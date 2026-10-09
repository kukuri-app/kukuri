//! 利用者が Android の保存・選択の画面で選んだ場所（Storage Access Framework の Content URI）を開く（#1197）。

use std::fs::File;
use std::io::Write;

use crate::state::CommandError;

/// `path` が Android の Content URI なら fs plugin で開く。desktop の path は `None` を返し、呼出元が path として扱う。
/// `content://` 以外の URL（`file://` 等）は app の中の file を指しうるので開かない。
/// 書込みは切り詰めずに開く（backup は中身のある file に書かない。text は呼出元が切り詰める）。
pub(crate) fn open_user_document<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    path: &str,
    write: bool,
) -> anyhow::Result<Option<File>> {
    #[cfg(target_os = "android")]
    {
        use tauri_plugin_fs::{FilePath, FsExt, OpenOptions};
        if let Ok(FilePath::Url(url)) = path.trim().parse::<FilePath>() {
            anyhow::ensure!(
                url.scheme() == "content",
                "only a location chosen by the user can be opened"
            );
            let mut options = OpenOptions::new();
            if write {
                options.write(true);
            } else {
                options.read(true);
            }
            return app
                .fs()
                .open(url, options)
                .map(Some)
                .map_err(|error| anyhow::anyhow!("failed to open the chosen location: {error}"));
        }
    }
    #[cfg(not(target_os = "android"))]
    let _ = (app, path, write);
    Ok(None)
}

/// 診断・アプリ内 log の text を、Android の保存の画面で選んだ場所へ書く（既存の file なら中身を置き換える）。
/// WebView は `<a download>` を扱わない。
#[tauri::command]
pub async fn write_text_document(
    app_handle: tauri::AppHandle,
    path: String,
    text: String,
) -> Result<(), CommandError> {
    tauri::async_runtime::spawn_blocking(move || -> anyhow::Result<()> {
        let mut file = open_user_document(&app_handle, &path, true)?.ok_or_else(|| {
            anyhow::anyhow!("text is written only to a location chosen on Android")
        })?;
        file.set_len(0)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        Ok(())
    })
    .await
    .map_err(|error| CommandError::from(format!("text document task failed: {error}")))?
    .map_err(|error| CommandError::from(format!("{error:#}")))
}
