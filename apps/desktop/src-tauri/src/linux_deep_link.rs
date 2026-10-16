//! Linuxの`kukuri://`の既定のhandlerの登録（tauri-plugin-deep-link 2.6.0の`register`と同じ結果）。
//! AppImageのlibraryをhostのcommandへ読ませると失敗するため、`LD_LIBRARY_PATH`を渡さない（#1797）。

use std::{fs, process::Command};

use tauri::{AppHandle, Manager};

const MIME_TYPE: &str = "x-scheme-handler/kukuri";

pub(crate) fn register(app: &AppHandle) -> tauri::Result<()> {
    let bin = tauri::utils::platform::current_exe()?;
    let file_name = format!(
        "{}-handler.desktop",
        bin.file_name().unwrap_or_default().to_string_lossy()
    );
    let exec = app.env().appimage.unwrap_or_else(|| bin.into_os_string());
    let target = app.path().data_dir()?.join("applications");
    fs::create_dir_all(&target)?;
    fs::write(
        target.join(&file_name),
        format!(
            "[Desktop Entry]\nType=Application\nName={}\nExec=\"{}\" %u\nTerminal=false\nMimeType={MIME_TYPE}\nNoDisplay=true\n",
            app.package_info().name,
            exec.to_string_lossy()
        ),
    )?;
    Command::new("update-desktop-database")
        .arg(&target)
        .env_remove("LD_LIBRARY_PATH")
        .status()?;
    Command::new("xdg-mime")
        .args(["default", &file_name, MIME_TYPE])
        .env_remove("LD_LIBRARY_PATH")
        .status()?;
    Ok(())
}
