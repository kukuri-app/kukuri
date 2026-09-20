//! AppImageへ同梱しない表示系libraryの一覧と、その除外をlinuxdeployへ届けるwrapper（#1222）。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

/// ホストのMesa（libEGL等）と同じ世代で揃える必要があるlibrary。file名の先頭一致で扱う。
/// build hostの古い版を同梱すると、新しいMesaで `eglGetDisplay` が `EGL_BAD_PARAMETER` になる。
pub(crate) const HOST_PROVIDED_LIBRARIES: [&str; 10] = [
    "libwayland-client.so",
    "libwayland-cursor.so",
    "libwayland-egl.so",
    "libwayland-server.so",
    "libxkbcommon.so",
    "libxcb-randr.so",
    "libxcb-render.so",
    "libxcb-shm.so",
    "libXau.so",
    "libXdmcp.so",
];

pub(crate) const HOST_LIBRARIES_ENV: &str = "KUKURI_APPIMAGE_HOST_LIBRARIES";

const WRAPPER: &str = include_str!("linuxdeploy-wrapper.sh");
const WRAPPER_NAME: &str = "linuxdeploy-x86_64.AppImage";
const REAL_NAME: &str = "linuxdeploy-x86_64.real.AppImage";
// Tauri bundlerが取得するものと同じ配布物。内容が変わった場合は除外の再確認後にhashを更新する。
const REAL_URL: &str = "https://github.com/tauri-apps/binary-releases/releases/download/linuxdeploy/linuxdeploy-x86_64.AppImage";
const REAL_SHA256: &str = "e762bea85c8eb0d4b3508d46e5c1f037f717d0f9303ae3b4aafc8b04991fa1ef";

pub(crate) fn host_libraries_env_value() -> String {
    HOST_PROVIDED_LIBRARIES.join(":")
}

/// Tauri bundlerと同じ規則（`dirs::cache_dir()/tauri`）でtools directoryを求める。
fn tauri_tools_dir() -> Result<PathBuf> {
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .context("HOME is not set; cannot locate the Tauri tools directory")?;
    Ok(cache.join("tauri"))
}

pub(crate) fn install_wrapper() -> Result<()> {
    install_wrapper_into(&tauri_tools_dir()?, |target| {
        crate::run(
            "curl",
            [
                "-fsSL",
                "--retry",
                "3",
                "-o",
                &target.to_string_lossy(),
                REAL_URL,
            ],
            &crate::root_dir(),
        )
    })
}

fn install_wrapper_into(dir: &Path, download: impl FnOnce(&Path) -> Result<()>) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let wrapper = dir.join(WRAPPER_NAME);
    let real = dir.join(REAL_NAME);
    // Tauriが先に取得した本体が残っていれば、それをwrapperの呼出先へ移す。
    if std::fs::read(&wrapper).is_ok_and(|bytes| bytes.starts_with(b"\x7fELF")) {
        std::fs::rename(&wrapper, &real)?;
    }
    if !real.is_file() {
        let partial = dir.join(format!("{REAL_NAME}.partial"));
        download(&partial)?;
        std::fs::rename(&partial, &real)?;
    }
    let digest = hex::encode(Sha256::digest(std::fs::read(&real)?));
    if digest != REAL_SHA256 {
        bail!(
            "linuxdeploy の内容が固定した版と一致しません（{digest}）。{} を削除して再実行し、それでも一致しなければ除外の動作を確認してhashを更新してください",
            real.display()
        );
    }
    make_executable(&real)?;
    std::fs::write(&wrapper, WRAPPER)?;
    make_executable(&wrapper)
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    Ok(())
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<()> {
    Ok(())
}

/// `root` 以下で見つかった同梱禁止libraryを、`root` からの相対pathで返す。
pub(crate) fn bundled_host_libraries(root: &Path) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in
            std::fs::read_dir(&dir).with_context(|| format!("cannot read {}", dir.display()))?
        {
            let entry = entry?;
            // symlinkは辿らない。symlink自体も同梱物として数える。
            if entry.file_type()?.is_dir() {
                pending.push(entry.path());
            } else if HOST_PROVIDED_LIBRARIES
                .iter()
                .any(|stem| entry.file_name().to_string_lossy().starts_with(stem))
            {
                found.push(entry.path().strip_prefix(root)?.to_path_buf());
            }
        }
    }
    found.sort();
    Ok(found)
}

pub(crate) fn ensure_no_host_libraries(root: &Path, label: &str) -> Result<()> {
    let found = bundled_host_libraries(root)?;
    if !found.is_empty() {
        bail!(
            "{label} がホスト提供の表示系libraryを同梱しています（新しいMesaで EGL_BAD_PARAMETER になり画面が描画されません、#1222）: {}",
            found
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = crate::root_dir()
            .join("test-results/xtask-linuxdeploy")
            .join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn bundled_display_stack_is_reported_and_clean_appdir_passes() {
        let appdir = scratch("scan");
        let lib = appdir.join("usr/lib");
        std::fs::create_dir_all(lib.join("x86_64-linux-gnu")).unwrap();
        for name in ["libgtk-3.so.0", "libxcb-shape.so.0", "libwayland.txt"] {
            std::fs::write(lib.join(name), b"").unwrap();
        }
        ensure_no_host_libraries(&appdir, "AppDir").unwrap();

        // v0.2.8までの配布物と同じ配置。
        std::fs::write(lib.join("libwayland-client.so.0"), b"").unwrap();
        std::fs::write(lib.join("x86_64-linux-gnu/libxkbcommon.so.0.0.0"), b"").unwrap();
        let error = ensure_no_host_libraries(&appdir, "AppDir")
            .unwrap_err()
            .to_string();
        assert!(error.contains("libwayland-client.so.0"), "{error}");
        assert!(error.contains("libxkbcommon.so.0.0.0"), "{error}");
        assert!(error.contains("EGL_BAD_PARAMETER"), "{error}");
        std::fs::remove_dir_all(appdir).unwrap();
    }

    #[test]
    fn wrapper_install_rejects_unpinned_linuxdeploy() {
        let dir = scratch("pin");
        let error = install_wrapper_into(&dir, |target| {
            std::fs::write(target, b"\x7fELF-not-the-pinned-build")?;
            Ok(())
        })
        .unwrap_err()
        .to_string();
        assert!(error.contains("一致しません"), "{error}");
        assert!(!dir.join(WRAPPER_NAME).exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// 本体の代わりに引数を記録するscriptを置き、wrapperの2段階の呼出しを検査する。
    #[cfg(unix)]
    #[test]
    fn wrapper_splits_output_and_removes_host_libraries_before_it() {
        let dir = scratch("wrapper");
        let appdir = dir.join("kukuri.AppDir");
        let lib = appdir.join("usr/lib");
        std::fs::create_dir_all(&lib).unwrap();
        let wrapper = dir.join(WRAPPER_NAME);
        std::fs::write(&wrapper, WRAPPER).unwrap();
        make_executable(&wrapper).unwrap();
        // 前段でGTK pluginが禁止libraryを持ち込む挙動を模す。後段では残っているものを記録する。
        let real = dir.join(REAL_NAME);
        std::fs::write(
            &real,
            "#!/bin/sh\nlog=\"$(dirname \"$0\")/calls.log\"\n\
             case \" $* \" in *\" --output \"*) echo \"libs: $(ls \"$APPDIR_LIB\" | tr '\\n' ' ')\" >>\"$log\";;\n\
             *) touch \"$APPDIR_LIB/libwayland-client.so.0\" \"$APPDIR_LIB/libXau.so.6\" \"$APPDIR_LIB/libgtk-3.so.0\";; esac\n\
             echo \"$*\" >>\"$log\"\n",
        )
        .unwrap();
        make_executable(&real).unwrap();

        let run = |host_libraries: Option<&str>| {
            let mut command = std::process::Command::new(&wrapper);
            command
                .args(["--appimage-extract-and-run", "--verbosity", "1", "--appdir"])
                .arg(&appdir)
                .args(["--plugin", "gtk", "--output", "appimage"])
                .env("APPDIR_LIB", &lib)
                .env_remove(HOST_LIBRARIES_ENV);
            if let Some(value) = host_libraries {
                command.env(HOST_LIBRARIES_ENV, value);
            }
            assert!(command.status().unwrap().success());
            let log = std::fs::read_to_string(dir.join("calls.log")).unwrap();
            std::fs::remove_file(dir.join("calls.log")).unwrap();
            log
        };

        let log = run(Some(&host_libraries_env_value()));
        let lines: Vec<&str> = log.lines().collect();
        assert_eq!(lines.len(), 3, "{log}");
        assert!(lines[0].ends_with("--plugin gtk") && !lines[0].contains("--output"));
        assert_eq!(lines[1], "libs: libgtk-3.so.0 ");
        assert!(lines[2].contains("--exclude-library libwayland-client.so*"));
        assert!(lines[2].contains("--exclude-library libXdmcp.so*"));
        assert!(lines[2].ends_with("--output appimage"));

        // 環境変数がなければ他のTauri projectと同じ1回の呼出しのまま。
        std::fs::write(lib.join("libwayland-client.so.0"), b"").unwrap();
        let log = run(None);
        let lines: Vec<&str> = log.lines().collect();
        assert_eq!(lines.len(), 2, "{log}");
        assert!(lines[0].starts_with("libs: ") && lines[0].contains("libwayland-client.so.0"));
        assert!(lines[1].ends_with("--plugin gtk --output appimage"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
