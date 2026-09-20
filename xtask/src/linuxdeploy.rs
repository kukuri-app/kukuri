//! AppImageへ同梱しない表示系libraryの一覧と、AppImage出力の直前にそれを除くwrapper（#1222）。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

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

const WRAPPER: &str = include_str!("appimage-output-wrapper.sh");
// linuxdeployは自身と同じdirectoryの `linuxdeploy-plugin-appimage*` を内蔵版より優先して使う。
const WRAPPER_NAME: &str = "linuxdeploy-plugin-appimage.AppImage";
// `linuxdeploy-plugin-` で始めるとlinuxdeployが別のpluginとして拾うため、別の名前にする。
const REAL_NAME: &str = "kukuri-appimage-output.real.AppImage";
// Tauri bundlerが取得するものと同じ配布物。同梱されるruntimeのhashはnative complianceが照合する。
const REAL_URL: &str = "https://github.com/linuxdeploy/linuxdeploy-plugin-appimage/releases/download/continuous/linuxdeploy-plugin-appimage-x86_64.AppImage";

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
    let is_elf = |path: &Path| std::fs::read(path).is_ok_and(|bytes| bytes.starts_with(b"ELF"));
    // Tauriが先に取得した本体が残っていれば、それをwrapperの呼出先へ移す。
    if is_elf(&wrapper) {
        std::fs::rename(&wrapper, &real)?;
    }
    if !real.is_file() {
        // Tauriは取得失敗時に古い内蔵版へ切り替えるが、runtimeが変わるためここでは失敗させる。
        let partial = dir.join(format!("{REAL_NAME}.partial"));
        download(&partial)?;
        if !is_elf(&partial) {
            bail!("取得した AppImage output plugin がELFではありません: {REAL_URL}");
        }
        std::fs::rename(&partial, &real)?;
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
    fn wrapper_install_keeps_the_plugin_tauri_downloaded_and_rejects_bad_downloads() {
        let dir = scratch("install");
        std::fs::write(dir.join(WRAPPER_NAME), b"ELF-from-tauri").unwrap();
        install_wrapper_into(&dir, |_| panic!("must reuse the existing plugin")).unwrap();
        assert_eq!(
            std::fs::read(dir.join(REAL_NAME)).unwrap(),
            b"ELF-from-tauri"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join(WRAPPER_NAME)).unwrap(),
            WRAPPER
        );
        // 2回目はwrapperを本体として扱わない。
        install_wrapper_into(&dir, |_| panic!("must not download again")).unwrap();
        assert_eq!(
            std::fs::read(dir.join(REAL_NAME)).unwrap(),
            b"ELF-from-tauri"
        );

        let empty = scratch("install-bad");
        let error = install_wrapper_into(&empty, |target| {
            std::fs::write(target, b"<html>not found</html>")?;
            Ok(())
        })
        .unwrap_err()
        .to_string();
        assert!(error.contains("ELFではありません"), "{error}");
        assert!(!empty.join(WRAPPER_NAME).exists() && !empty.join(REAL_NAME).exists());
        std::fs::remove_dir_all(dir).unwrap();
        std::fs::remove_dir_all(empty).unwrap();
    }

    /// 本体の代わりに引数とAppDirの状態を記録するscriptを置き、wrapperの動作を検査する。
    #[cfg(unix)]
    #[test]
    fn wrapper_removes_host_libraries_before_the_real_output_plugin() {
        let dir = scratch("wrapper");
        let lib = dir.join("kukuri.AppDir/usr/lib");
        std::fs::create_dir_all(lib.join("x86_64-linux-gnu")).unwrap();
        let wrapper = dir.join(WRAPPER_NAME);
        std::fs::write(&wrapper, WRAPPER).unwrap();
        make_executable(&wrapper).unwrap();
        let real = dir.join(REAL_NAME);
        std::fs::write(
            &real,
            "#!/bin/sh
echo \"$*|$(cd \"$APPDIR_LIB\" && find . -type f -o -type l | sort | tr '\n' ' ')\" >>\"$(dirname \"$0\")/calls.log\"
",
        )
        .unwrap();
        make_executable(&real).unwrap();

        let run = |args: &[&str], host_libraries: Option<&str>| {
            for name in [
                "libgtk-3.so.0",
                "libwayland-client.so.0",
                "x86_64-linux-gnu/libXau.so.6",
            ] {
                std::fs::write(lib.join(name), b"").unwrap();
            }
            let mut command = std::process::Command::new(&wrapper);
            command
                .args(args)
                .current_dir(&dir)
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
        let value = host_libraries_env_value();

        // linuxdeployの実際の呼出し形式（相対pathの2引数）。
        assert_eq!(
            run(&["--appdir", "kukuri.AppDir"], Some(&value)),
            "--appdir kukuri.AppDir|./libgtk-3.so.0 
"
        );
        assert_eq!(
            run(&["--appdir=kukuri.AppDir"], Some(&value)),
            "--appdir=kukuri.AppDir|./libgtk-3.so.0 
"
        );
        // plugin情報の問合せと、環境変数がない他のTauri projectでは何も消さない。
        for (args, env) in [
            (&["--plugin-api-version"][..], Some(value.as_str())),
            (&["--appdir", "kukuri.AppDir"][..], None),
        ] {
            let log = run(args, env);
            assert!(log.contains("libwayland-client.so.0"), "{log}");
            assert!(log.contains("libXau.so.6"), "{log}");
        }
        // AppDirを取り違えたまま本来のpluginへ進まない。
        let mut command = std::process::Command::new(&wrapper);
        command
            .args(["--appdir", "missing.AppDir"])
            .current_dir(&dir)
            .env(HOST_LIBRARIES_ENV, &value);
        assert!(!command.status().unwrap().success());
        assert!(!dir.join("calls.log").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
