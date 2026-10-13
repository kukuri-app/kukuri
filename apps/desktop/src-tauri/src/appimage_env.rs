//! AppImageが同梱するGIO・GStreamerと、ホスト側のモジュールやファイルとの混在を防ぐ。

use std::{io, path::PathBuf};

/// # Safety
/// GTK等の初期化・スレッド生成より前のmainからだけ呼ぶこと。
pub(crate) unsafe fn configure() -> io::Result<()> {
    let Some(app_dir) = std::env::var_os("APPDIR") else {
        return Ok(());
    };
    let modules = PathBuf::from(app_dir).join("usr/lib/gio/modules");
    if !modules.is_absolute() || !modules.join("libgiognutls.so").is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "AppImageの同梱GIO TLSモジュールが見つかりません。AppImageを再取得してください。",
        ));
    }
    // 同梱GStreamerのplugin registryは、起動ごとにmount先が変わるため毎回作り直しになる（#1795）。
    // scannerを使い回して待ちを短くし、ホストと共有するregistryファイルは書き換えない。
    let registry = std::env::var_os("XDG_CACHE_HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join("app.kukuri.desktop/gstreamer-1.0/registry.bin"));
    // EXTRAだけではGIOに組み込まれたホスト探索先が残る。両方を同梱先へ固定する。
    // SAFETY: 呼出元はスレッドを生成する前のmainに限定する。
    unsafe {
        std::env::set_var("GIO_MODULE_DIR", &modules);
        std::env::set_var("GIO_EXTRA_MODULES", &modules);
        std::env::set_var("GST_REGISTRY_REUSE_PLUGIN_SCANNER", "yes");
        if let Some(registry) = registry {
            std::env::set_var("GST_REGISTRY_1_0", registry);
        }
    }
    Ok(())
}
