//! `apps/desktop/src-tauri` の lib 単体 test を実行する（#1234）。
//!
//! この crate は root workspace の `exclude` に入っているため、`rust-test` の対象にならない。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

#[allow(unused_imports)]
use crate::*;

const MANIFEST_PATH: &str = "apps/desktop/src-tauri/Cargo.toml";
const LIB_NAME: &str = "kukuri_desktop_tauri_lib";
const PACKAGE_BUILD_FLAG: &str = "--package-build";

/// Windows の test exe は Common Controls v6 の manifest を持たず、`TaskDialogIndirect` を
/// 解決できずに `STATUS_ENTRYPOINT_NOT_FOUND` で起動に失敗する。Tauri が manifest を埋め込むのは
/// bin だけなので、test exe の隣に外部 manifest を置いて補う。製品 binary には触れない。
const COMMON_CONTROLS_MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <dependency>
    <dependentAssembly>
      <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*" />
    </dependentAssembly>
  </dependency>
</assembly>
"#;

#[derive(Debug, Eq, PartialEq)]
struct TauriTestOptions {
    /// `desktop-package` の release build を再利用する（CI の package job 用）。
    package_build: bool,
    /// test binary へ渡す引数（test 名の filter など）。
    test_args: Vec<String>,
}

fn parse_options(args: impl Iterator<Item = String>) -> Result<TauriTestOptions> {
    let mut options = TauriTestOptions {
        package_build: false,
        test_args: Vec::new(),
    };
    let mut args = args;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            PACKAGE_BUILD_FLAG => options.package_build = true,
            "--" => {
                options.test_args.extend(args);
                break;
            }
            _ => bail!("unsupported tauri-test flag: {arg}"),
        }
    }
    Ok(options)
}

fn cargo_test_args(package_build: bool, host_os: &str) -> Result<Vec<String>> {
    let mut args = vec!["test".to_string()];
    if package_build {
        // `desktop-package` と同じ profile / target にして、その成果物を再利用する。
        let target = match host_os {
            "linux" => "x86_64-unknown-linux-gnu",
            "windows" => "x86_64-pc-windows-msvc",
            _ => bail!("{PACKAGE_BUILD_FLAG} is only supported on Windows and Linux hosts"),
        };
        args.extend(["--locked", "--release", "--target", target].map(String::from));
    }
    args.extend(["--manifest-path", MANIFEST_PATH, "--lib"].map(String::from));
    Ok(args)
}

pub(crate) fn tauri_test(args: impl Iterator<Item = String>) -> Result<()> {
    let options = parse_options(args)?;
    let cargo_args = cargo_test_args(options.package_build, std::env::consts::OS)?;
    // package の build は `src-tauri/target` に出る。それ以外は `tauri-check` と同じ場所を使う。
    let target_dir = root_dir().join(TAURI_CHECK_TARGET_DIR);
    let target_dir_value = target_dir.to_string_lossy().into_owned();
    let envs: &[(&str, &str)] = if options.package_build {
        &[]
    } else {
        &[("CARGO_TARGET_DIR", target_dir_value.as_str())]
    };

    if cfg!(windows) {
        return run_with_external_manifest(cargo_args, &options.test_args, envs);
    }

    let mut cargo_args = cargo_args;
    if !options.test_args.is_empty() {
        cargo_args.push("--".to_string());
        cargo_args.extend(options.test_args);
    }
    run_with_env("cargo", cargo_args, &root_dir(), envs)
}

fn run_with_external_manifest(
    mut cargo_args: Vec<String>,
    test_args: &[String],
    envs: &[(&str, &str)],
) -> Result<()> {
    cargo_args.extend(["--no-run", "--message-format=json"].map(String::from));
    println!("[xtask] cargo {}", cargo_args.join(" "));
    let output = child_command("cargo")
        .args(&cargo_args)
        .current_dir(root_dir())
        .envs(envs.iter().copied())
        .stderr(std::process::Stdio::inherit())
        .output()
        .context("failed to execute cargo")?;
    if !output.status.success() {
        bail!("cargo exited with status {}", output.status);
    }
    let executable = lib_test_executable(&String::from_utf8_lossy(&output.stdout))
        .context("cargo did not report the lib test executable")?;
    std::fs::write(
        external_manifest_path(&executable),
        COMMON_CONTROLS_MANIFEST,
    )
    .context("failed to write the test executable manifest")?;
    // Windows は manifest の解決結果を exe の path と更新時刻で cache する。manifest なしで一度
    // 起動した exe は、後から manifest を置いても失敗し続けるので、更新時刻を変えて cache を外す。
    std::fs::File::options()
        .write(true)
        .open(&executable)
        .and_then(|file| file.set_modified(std::time::SystemTime::now()))
        .context("failed to update the test executable timestamp")?;

    let manifest_dir = root_dir().join(MANIFEST_PATH);
    let manifest_dir = manifest_dir.parent().expect("manifest has a parent");
    let manifest_dir_value = manifest_dir.to_string_lossy().into_owned();
    // `cargo test` と同じく package の directory で実行する。
    run_with_env(
        executable.to_string_lossy().as_ref(),
        test_args.iter().cloned(),
        manifest_dir,
        &[("CARGO_MANIFEST_DIR", manifest_dir_value.as_str())],
    )
}

fn external_manifest_path(executable: &Path) -> PathBuf {
    let mut name = executable.as_os_str().to_owned();
    name.push(".manifest");
    PathBuf::from(name)
}

/// `cargo test --no-run --message-format=json` の出力から lib 単体 test の実行 file を取り出す。
fn lib_test_executable(messages: &str) -> Option<PathBuf> {
    messages
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|message| message["reason"] == "compiler-artifact")
        .filter(|message| message["profile"]["test"] == true)
        .filter(|message| message["target"]["name"] == LIB_NAME)
        .find_map(|message| message["executable"].as_str().map(PathBuf::from))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(args: &[&str]) -> Result<TauriTestOptions> {
        parse_options(args.iter().map(|value| value.to_string()))
    }

    #[test]
    fn default_run_tests_only_the_lib_target_in_the_dev_profile() {
        let args = cargo_test_args(false, "windows").unwrap();
        assert_eq!(args, ["test", "--manifest-path", MANIFEST_PATH, "--lib"]);
    }

    #[test]
    fn package_build_matches_the_desktop_package_profile_and_target() {
        let args = cargo_test_args(true, "linux").unwrap();
        assert_eq!(
            args,
            [
                "test",
                "--locked",
                "--release",
                "--target",
                "x86_64-unknown-linux-gnu",
                "--manifest-path",
                MANIFEST_PATH,
                "--lib",
            ]
        );
        assert!(cargo_test_args(true, "macos").is_err());
    }

    #[test]
    fn options_forward_test_filters_and_reject_unknown_flags() {
        assert_eq!(
            options(&["--package-build", "--", "tracing::tests", "--nocapture"]).unwrap(),
            TauriTestOptions {
                package_build: true,
                test_args: vec!["tracing::tests".to_string(), "--nocapture".to_string()],
            }
        );
        assert!(options(&["--release"]).is_err());
    }

    #[test]
    fn lib_test_executable_ignores_dependencies_and_non_test_artifacts() {
        let messages = [
            r#"{"reason":"compiler-artifact","target":{"name":"serde"},"profile":{"test":false},"executable":null}"#,
            r#"{"reason":"compiler-artifact","target":{"name":"kukuri_desktop_tauri_lib"},"profile":{"test":false},"executable":null}"#,
            r#"{"reason":"compiler-artifact","target":{"name":"updater_install"},"profile":{"test":true},"executable":"C:\\t\\updater_install-1.exe"}"#,
            r#"{"reason":"compiler-artifact","target":{"name":"kukuri_desktop_tauri_lib"},"profile":{"test":true},"executable":"C:\\t\\kukuri_desktop_tauri_lib-1.exe"}"#,
            r#"{"reason":"build-finished","success":true}"#,
        ]
        .join("\n");
        assert_eq!(
            lib_test_executable(&messages),
            Some(PathBuf::from(r"C:\t\kukuri_desktop_tauri_lib-1.exe"))
        );
        assert_eq!(lib_test_executable("not json"), None);
    }

    #[test]
    fn external_manifest_sits_next_to_the_executable() {
        assert_eq!(
            external_manifest_path(Path::new(r"C:\t\lib-1.exe")),
            PathBuf::from(r"C:\t\lib-1.exe.manifest")
        );
    }
}
