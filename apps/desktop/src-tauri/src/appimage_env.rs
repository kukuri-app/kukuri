//! AppImageが同梱するGIOと、ホスト側モジュールのABI混在を防ぐ。

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
    // EXTRAだけではGIOに組み込まれたホスト探索先が残る。両方を同梱先へ固定する。
    // SAFETY: 呼出元はスレッドを生成する前のmainに限定する。
    unsafe {
        std::env::set_var("GIO_MODULE_DIR", &modules);
        std::env::set_var("GIO_EXTRA_MODULES", &modules);
    }
    Ok(())
}
