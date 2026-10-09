use anyhow::{Context, Result, bail};

#[allow(unused_imports)]
use crate::*;

pub(crate) fn tauri_check() -> Result<()> {
    run(
        "cargo",
        [
            "check",
            "--manifest-path",
            "apps/desktop/src-tauri/Cargo.toml",
        ],
        &root_dir(),
    )
}

/// `apps/desktop/src-tauri` を Android の配布 ABI（arm64-v8a、minSdk 29。#1193 D7）向けに compile する。
/// C を含む依存の build script は NDK の clang と llvm-ar を使う。
pub(crate) fn android_check() -> Result<()> {
    let ndk = ["ANDROID_NDK_HOME", "NDK_HOME", "ANDROID_NDK_LATEST_HOME"]
        .into_iter()
        .find_map(std::env::var_os)
        .context("Android NDK が見つかりません。ANDROID_NDK_HOME か NDK_HOME に NDK の directory を設定してください")?;
    let (host, script, exe) = if cfg!(windows) {
        ("windows-x86_64", ".cmd", ".exe")
    } else {
        ("linux-x86_64", "", "")
    };
    let bin = std::path::Path::new(&ndk)
        .join("toolchains/llvm/prebuilt")
        .join(host)
        .join("bin");
    let clang = bin.join(format!("aarch64-linux-android29-clang{script}"));
    let ar = bin.join(format!("llvm-ar{exe}"));
    run_with_env(
        "cargo",
        [
            "check",
            "--manifest-path",
            "apps/desktop/src-tauri/Cargo.toml",
            "--lib",
            "--target",
            "aarch64-linux-android",
        ],
        &root_dir(),
        &[
            ("CC_aarch64_linux_android", &clang.display().to_string()),
            ("AR_aarch64_linux_android", &ar.display().to_string()),
        ],
    )
}

pub(crate) fn desktop_lint() -> Result<()> {
    run_pnpm(["lint"], &desktop_dir())?;
    run_pnpm(["typecheck"], &desktop_dir())
}

pub(crate) fn desktop_test() -> Result<()> {
    run_pnpm(["test"], &desktop_dir())
}

pub(crate) fn desktop_storybook() -> Result<()> {
    run_pnpm(["storybook:build"], &desktop_dir())
}

pub(crate) fn desktop_browser_test() -> Result<()> {
    run_pnpm(["test:e2e:browser"], &desktop_dir())
}

pub(crate) fn desktop_visual_test() -> Result<()> {
    run_pnpm(["test:e2e:visual"], &desktop_dir())
}

pub(crate) fn desktop_ui_check() -> Result<()> {
    desktop_lint()?;
    desktop_test()?;
    desktop_storybook()?;
    desktop_browser_test()?;
    desktop_visual_test()
}

pub(crate) fn desktop_package() -> Result<()> {
    let signed = std::env::var_os("TAURI_SIGNING_PRIVATE_KEY")
        .is_some_and(|key| !cfg!(target_os = "linux") || !key.is_empty());
    if cfg!(target_os = "linux") && std::env::consts::ARCH != "x86_64" {
        bail!("Linux desktop-package requires an x86_64 host");
    }
    let args = desktop_package_args(std::env::consts::OS, signed)?;
    if !signed {
        println!(
            "[xtask] TAURI_SIGNING_PRIVATE_KEY is not set; building installer without updater artifacts"
        );
    }
    if cfg!(target_os = "linux") {
        // Tauri bundlerには同梱除外の設定がないため、AppImage出力の直前で除く（#1222）。
        crate::linuxdeploy::install_wrapper()?;
        let host_libraries = crate::linuxdeploy::host_libraries_env_value();
        // #1431: 同梱する runtime は審査済みの版を渡す。渡さないと appimagetool が上流の
        // continuous を取得し、上流の差し替えで native compliance の照合が落ちる。
        let runtime = root_dir().join("packaging/linux/appimage/type2-runtime-x86_64");
        let runtime = runtime.to_string_lossy();
        run_pnpm_with_env(
            args,
            &desktop_dir(),
            &[
                (crate::linuxdeploy::HOST_LIBRARIES_ENV, &host_libraries),
                ("LDAI_RUNTIME_FILE", &runtime),
            ],
        )?;
        crate::appimage::verify_package()?;
    } else {
        run_pnpm(args, &desktop_dir())?;
    }
    Ok(())
}

pub(crate) fn windows_store_package(args: impl Iterator<Item = String>) -> Result<()> {
    if !cfg!(target_os = "windows") {
        bail!("windows-store-package requires a Windows host");
    }
    let mut script_args = vec![
        "-NoProfile".to_string(),
        "-File".to_string(),
        root_dir()
            .join("scripts")
            .join("release")
            .join("build-windows-store-msix.ps1")
            .to_string_lossy()
            .into_owned(),
    ];
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--allow-dirty" => script_args.push("-AllowDirty".to_string()),
            "--output" => {
                let value = args
                    .next()
                    .with_context(|| format!("{arg} requires a value"))?;
                script_args.push("-OutputDirectory".to_string());
                script_args.push(value);
            }
            _ => bail!("unsupported windows-store-package flag: {arg}"),
        }
    }
    run("pwsh", script_args, &root_dir())
}

fn desktop_package_args(os: &str, signed: bool) -> Result<Vec<String>> {
    let target = match os {
        "windows" => "x86_64-pc-windows-msvc",
        "linux" if signed => "x86_64-unknown-linux-gnu",
        "linux" => bail!("Linux desktop-package requires TAURI_SIGNING_PRIVATE_KEY"),
        _ => bail!("desktop-package is only supported on Windows and Linux hosts"),
    };

    let mut args = vec![
        "tauri".to_string(),
        "build".to_string(),
        "--target".to_string(),
        target.to_string(),
    ];
    if !signed {
        args.extend([
            "--config".to_string(),
            r#"{"bundle":{"createUpdaterArtifacts":false}}"#.to_string(),
        ]);
    }

    Ok(args)
}

#[cfg(test)]
mod package_tests {
    use super::*;

    #[test]
    fn linux_package_uses_appimage_target_and_keeps_updater_signing() {
        let args = desktop_package_args("linux", true).expect("Linux package is supported");
        assert!(args.iter().any(|value| value == "x86_64-unknown-linux-gnu"));
        assert!(!args.iter().any(|value| value.contains("false")));
    }

    #[test]
    fn linux_package_rejects_missing_signing_key() {
        assert!(desktop_package_args("linux", false).is_err());
    }

    #[test]
    fn windows_package_preserves_signed_and_local_unsigned_builds() {
        for signed in [true, false] {
            let args = desktop_package_args("windows", signed).unwrap();
            assert!(args.iter().any(|value| value == "x86_64-pc-windows-msvc"));
            assert_eq!(args.iter().any(|value| value.contains("false")), !signed);
        }
    }

    #[test]
    fn unsupported_package_host_is_rejected() {
        assert!(desktop_package_args("macos", true).is_err());
    }

    #[test]
    fn store_manifest_fixes_the_partner_center_identity() {
        let manifest = std::fs::read_to_string(
            root_dir().join("apps/desktop/src-tauri/windows/store/Package.appxmanifest"),
        )
        .expect("Store manifest exists");
        for expected in [
            r#"Name="KingYoSun.kukuri""#,
            r#"Publisher="CN=33EB763C-4859-4E44-886F-1784E16DD6D5""#,
            // Packaging replaces this sentinel with the derived app version.
            r#"Version="0.0.0.0""#,
            r#"ProcessorArchitecture="x64""#,
            r#"<uap:Protocol Name="kukuri" />"#,
            r#"<rescap:Capability Name="runFullTrust" />"#,
        ] {
            assert!(manifest.contains(expected), "missing {expected}");
        }
    }

    #[test]
    fn linux_bundle_config_enables_signed_appimage_and_deb() {
        let config: serde_json::Value = serde_json::from_slice(
            &std::fs::read(root_dir().join("apps/desktop/src-tauri/tauri.linux.conf.json"))
                .expect("Linux bundle config exists"),
        )
        .unwrap();
        assert_eq!(config["bundle"]["active"], true);
        assert_eq!(
            config["bundle"]["targets"],
            serde_json::json!(["appimage", "deb"])
        );
        assert_eq!(config["bundle"]["createUpdaterArtifacts"], true);
        assert!(
            config["bundle"]["icon"]
                .as_array()
                .is_some_and(|icons| !icons.is_empty())
        );
        assert_eq!(config["bundle"]["category"], "SocialNetworking");
    }
}
