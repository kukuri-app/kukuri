#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(target_os = "linux")]
mod appimage_env;
#[cfg(target_os = "linux")]
mod linux_renderer;

fn main() {
    // SAFETY: GTK・Tauri・非同期runtimeより前で、まだスレッドを生成していない。
    #[cfg(target_os = "linux")]
    unsafe {
        linux_renderer::configure();
    }
    // SAFETY: GTK・Tauri・非同期runtimeの初期化より前で、まだスレッドを生成していない。
    #[cfg(target_os = "linux")]
    if let Err(error) = unsafe { appimage_env::configure() } {
        eprintln!("{error}");
        std::process::exit(1);
    }
    kukuri_desktop_tauri_lib::run();
}
