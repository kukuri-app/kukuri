use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

#[allow(unused_imports)]
use crate::*;

pub(crate) const PNPM_VERSION: &str = "10.16.1";

pub(crate) static NEXTEST_AVAILABLE: OnceLock<bool> = OnceLock::new();
pub(crate) static PNPM_AVAILABLE: OnceLock<bool> = OnceLock::new();

pub(crate) fn root_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(1)
        .expect("workspace root")
        .to_path_buf()
}

pub(crate) fn desktop_dir() -> PathBuf {
    root_dir().join("apps").join("desktop")
}

#[cfg(feature = "harness")]
pub(crate) fn artifacts_dir(name: &str) -> PathBuf {
    root_dir()
        .join("test-results")
        .join("kukuri")
        .join(name.replace('/', "-"))
}

/// `cargo xtask`（`cargo run`）が xtask 自身へ渡す package 情報の環境変数。
///
/// ring などの build script はこれらを `rerun-if-env-changed` で監視する。xtask 経由の
/// cargo と直接の cargo で値が食い違うと、そのたびに依存 crate が再 build される。
/// 子プロセスへは引き継がず、どちらから起動しても同じ環境で build させる。
const CARGO_RUN_PACKAGE_ENV_PREFIXES: [&str; 2] = ["CARGO_PKG_", "CARGO_MANIFEST_"];
const CARGO_RUN_PACKAGE_ENV_NAMES: [&str; 3] = [
    "CARGO_CRATE_NAME",
    "CARGO_BIN_NAME",
    "CARGO_PRIMARY_PACKAGE",
];

pub(crate) fn is_cargo_run_package_env(name: &str) -> bool {
    CARGO_RUN_PACKAGE_ENV_PREFIXES
        .iter()
        .any(|prefix| name.starts_with(prefix))
        || CARGO_RUN_PACKAGE_ENV_NAMES.contains(&name)
}

/// 子プロセス用の `Command`。`cargo run` 由来の package 情報を取り除く。
pub(crate) fn child_command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut command = Command::new(program);
    for (name, _) in std::env::vars_os() {
        if name.to_str().is_some_and(is_cargo_run_package_env) {
            command.env_remove(name);
        }
    }
    command
}

pub(crate) fn env_refs(envs: &[(String, String)]) -> Vec<(&str, &str)> {
    envs.iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HostPlatform {
    Unix,
    Windows,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct CommandSpec {
    program: String,
    args: Vec<String>,
}

impl CommandSpec {
    fn direct(binary: &str, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            program: binary.to_string(),
            args: args.into_iter().map(Into::into).collect(),
        }
    }
}

pub(crate) fn host_platform() -> HostPlatform {
    if cfg!(windows) {
        HostPlatform::Windows
    } else {
        HostPlatform::Unix
    }
}

pub(crate) fn node_command_spec(
    platform: HostPlatform,
    binary: &str,
    args: impl IntoIterator<Item = impl Into<String>>,
) -> CommandSpec {
    let args = args.into_iter().map(Into::into).collect::<Vec<_>>();
    match platform {
        HostPlatform::Unix => CommandSpec::direct(binary, args),
        HostPlatform::Windows => {
            let mut shell_args = vec!["/C".to_string(), binary.to_string()];
            shell_args.extend(args);
            CommandSpec::direct("cmd", shell_args)
        }
    }
}

pub(crate) fn pnpm_command_spec(
    platform: HostPlatform,
    pnpm_available: bool,
    args: impl IntoIterator<Item = impl Into<String>>,
) -> CommandSpec {
    let args = args.into_iter().map(Into::into).collect::<Vec<_>>();
    if pnpm_available {
        node_command_spec(platform, "pnpm", args)
    } else {
        let mut fallback = match platform {
            HostPlatform::Unix => vec![format!("pnpm@{PNPM_VERSION}")],
            HostPlatform::Windows => vec!["--yes".to_string(), format!("pnpm@{PNPM_VERSION}")],
        };
        fallback.extend(args);
        node_command_spec(platform, "npx", fallback)
    }
}

pub(crate) fn run(
    binary: &str,
    args: impl IntoIterator<Item = impl Into<String>>,
    cwd: &Path,
) -> Result<()> {
    run_with_env(binary, args, cwd, &[])
}

pub(crate) fn run_with_env(
    binary: &str,
    args: impl IntoIterator<Item = impl Into<String>>,
    cwd: &Path,
    envs: &[(&str, &str)],
) -> Result<()> {
    run_spec_with_env(&CommandSpec::direct(binary, args), cwd, envs)
}

pub(crate) fn run_with_owned_env(
    binary: &str,
    args: impl IntoIterator<Item = impl Into<String>>,
    cwd: &Path,
    envs: &[(String, String)],
) -> Result<()> {
    let env_refs = envs
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect::<Vec<_>>();
    run_with_env(binary, args, cwd, &env_refs)
}

pub(crate) fn run_spec_with_env(
    spec: &CommandSpec,
    cwd: &Path,
    envs: &[(&str, &str)],
) -> Result<()> {
    let label = format_command(spec);
    run_timed_step(label, || {
        let status = child_command(spec.program.as_str())
            .args(spec.args.iter())
            .current_dir(cwd)
            .envs(envs.iter().copied())
            .status()
            .with_context(|| format!("failed to execute {}", spec.program))?;
        if !status.success() {
            bail!("{} exited with status {status}", spec.program);
        }
        Ok(())
    })
}

pub(crate) fn run_capture(
    binary: &str,
    args: impl IntoIterator<Item = impl Into<String>>,
    cwd: &Path,
) -> Result<()> {
    run_capture_spec(&CommandSpec::direct(binary, args), cwd)
}

pub(crate) fn run_capture_spec(spec: &CommandSpec, cwd: &Path) -> Result<()> {
    let label = format_command(spec);
    run_timed_step(label, || {
        let output = child_command(spec.program.as_str())
            .args(spec.args.iter())
            .current_dir(cwd)
            .output()
            .with_context(|| format!("failed to execute {}", spec.program))?;
        if !output.status.success() {
            bail!("{} exited with status {}", spec.program, output.status);
        }
        Ok(())
    })
}

pub(crate) fn run_pnpm(
    args: impl IntoIterator<Item = impl Into<String>>,
    cwd: &Path,
) -> Result<()> {
    run_pnpm_with_env(args, cwd, &[])
}

pub(crate) fn run_pnpm_with_env(
    args: impl IntoIterator<Item = impl Into<String>>,
    cwd: &Path,
    envs: &[(&str, &str)],
) -> Result<()> {
    let platform = host_platform();
    let available = *PNPM_AVAILABLE.get_or_init(|| {
        run_capture_spec(&node_command_spec(platform, "pnpm", ["--version"]), cwd).is_ok()
    });
    run_spec_with_env(&pnpm_command_spec(platform, available, args), cwd, envs)
}

pub(crate) fn run_timed_step<T>(
    label: impl Into<String>,
    operation: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let label = label.into();
    println!("[xtask] start {label}");
    let started = Instant::now();
    match operation() {
        Ok(value) => {
            println!(
                "[xtask] done  {label} ({})",
                format_duration(started.elapsed())
            );
            Ok(value)
        }
        Err(error) => {
            eprintln!(
                "[xtask] fail  {label} ({})",
                format_duration(started.elapsed())
            );
            Err(error)
        }
    }
}

pub(crate) fn format_command(spec: &CommandSpec) -> String {
    if spec.args.is_empty() {
        spec.program.clone()
    } else {
        format!("{} {}", spec.program, spec.args.join(" "))
    }
}

pub(crate) fn format_duration(duration: Duration) -> String {
    if duration.as_secs() >= 60 {
        let total_seconds = duration.as_secs();
        let minutes = total_seconds / 60;
        let seconds = total_seconds % 60;
        format!("{minutes}m{seconds:02}s")
    } else if duration.as_secs_f64() >= 1.0 {
        format!("{:.1}s", duration.as_secs_f64())
    } else {
        format!("{}ms", duration.as_millis())
    }
}

pub(crate) fn nextest_available() -> bool {
    *NEXTEST_AVAILABLE
        .get_or_init(|| run_capture("cargo", ["nextest", "--version"], &root_dir()).is_ok())
}

pub(crate) fn is_ci() -> bool {
    std::env::var_os("CI").is_some() || std::env::var_os("GITHUB_ACTIONS").is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_command_uses_direct_exec_on_unix() {
        let spec = node_command_spec(HostPlatform::Unix, "pnpm", ["test"]);
        assert_eq!(
            spec,
            CommandSpec {
                program: "pnpm".to_string(),
                args: vec!["test".to_string()],
            }
        );
    }

    #[test]
    fn node_command_uses_cmd_shell_on_windows() {
        let spec = node_command_spec(HostPlatform::Windows, "npx", ["--yes", "pnpm@10.16.1"]);
        assert_eq!(
            spec,
            CommandSpec {
                program: "cmd".to_string(),
                args: vec![
                    "/C".to_string(),
                    "npx".to_string(),
                    "--yes".to_string(),
                    "pnpm@10.16.1".to_string(),
                ],
            }
        );
    }

    #[test]
    fn pnpm_fallback_uses_npx_wrapper_when_pnpm_is_unavailable() {
        let spec = pnpm_command_spec(HostPlatform::Windows, false, ["lint"]);
        assert_eq!(
            spec,
            CommandSpec {
                program: "cmd".to_string(),
                args: vec![
                    "/C".to_string(),
                    "npx".to_string(),
                    "--yes".to_string(),
                    format!("pnpm@{PNPM_VERSION}"),
                    "lint".to_string(),
                ],
            }
        );
    }

    #[test]
    fn cargo_run_package_env_is_not_inherited_by_children() {
        for name in [
            "CARGO_PKG_NAME",
            "CARGO_PKG_VERSION_MAJOR",
            "CARGO_MANIFEST_DIR",
            "CARGO_MANIFEST_PATH",
            "CARGO_CRATE_NAME",
            "CARGO_BIN_NAME",
            "CARGO_PRIMARY_PACKAGE",
        ] {
            assert!(is_cargo_run_package_env(name), "{name}");
        }
        for name in [
            "CARGO",
            "CARGO_HOME",
            "CARGO_TARGET_DIR",
            "CARGO_INCREMENTAL",
            "PATH",
        ] {
            assert!(!is_cargo_run_package_env(name), "{name}");
        }
    }

    #[test]
    fn format_duration_covers_ms_seconds_and_minutes() {
        assert_eq!(format_duration(Duration::from_millis(999)), "999ms");
        assert_eq!(format_duration(Duration::from_millis(1000)), "1.0s");
        assert_eq!(format_duration(Duration::from_secs(59)), "59.0s");
        assert_eq!(format_duration(Duration::from_secs(60)), "1m00s");
        assert_eq!(format_duration(Duration::from_secs(605)), "10m05s");
    }
}
