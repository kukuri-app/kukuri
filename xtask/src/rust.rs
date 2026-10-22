use anyhow::{Result, bail};

#[allow(unused_imports)]
use crate::*;

pub(crate) const SERIAL_RUST_PACKAGE: &str = "kukuri-harness";

pub(crate) fn check() -> Result<()> {
    rust_check()?;
    operator_neutrality_check()?;
    tauri_check()?;
    desktop_lint()
}

pub(crate) fn test() -> Result<()> {
    rust_test(std::iter::empty())?;
    desktop_test()
}

pub(crate) fn rust_check() -> Result<()> {
    run("cargo", ["fmt", "--check"], &root_dir())?;
    let cn_packages = cn_packages()?;

    let mut clippy_args = vec![
        "clippy".to_string(),
        "--workspace".to_string(),
        "--all-targets".to_string(),
    ];
    clippy_args.extend(cargo_exclude_args(&cn_packages));
    clippy_args.extend(["--".to_string(), "-D".to_string(), "warnings".to_string()]);
    run("cargo", clippy_args, &root_dir())
}

/// `extra` は `cargo nextest run` へそのまま渡す（CI の shard は `--partition hash:N/M`）。
pub(crate) fn rust_test(extra: impl Iterator<Item = String>) -> Result<()> {
    let cn_packages = cn_packages()?;
    let extra: Vec<String> = extra.collect();
    if nextest_available() {
        rust_test_with_nextest(&cn_packages, extra)
    } else {
        if is_ci() || !extra.is_empty() {
            bail!("cargo-nextest is required in CI and for nextest args");
        }
        eprintln!(
            "[xtask] warning: cargo-nextest was not found; falling back to cargo test for rust-test"
        );
        rust_test_with_cargo_test(&cn_packages)
    }
}

pub(crate) fn app_api_slow_test() -> Result<()> {
    run_with_env(
        "cargo",
        [
            "test",
            "-p",
            "kukuri-app-api",
            "--features",
            "iroh-integration-tests",
        ],
        &root_dir(),
        &env_refs(&rust_test_stack_envs()),
    )
}

pub(crate) fn rust_test_with_nextest(cn_packages: &[String], extra: Vec<String>) -> Result<()> {
    let stack_env = rust_test_stack_envs();
    let stack_env_refs = env_refs(&stack_env);

    // #1121: harness も同じ nextest 実行に含める。直列実行は `.config/nextest.toml` の
    // `harness-serial` group が担うため、別実行にすると feature 解決が変わって
    // test build をやり直すだけだった。
    let mut nextest_args = vec![
        "nextest".to_string(),
        "run".to_string(),
        "--workspace".to_string(),
    ];
    nextest_args.extend(cargo_exclude_args(cn_packages));
    nextest_args.extend(extra);
    run_with_env("cargo", nextest_args, &root_dir(), &stack_env_refs)?;

    let mut doc_args = vec![
        "test".to_string(),
        "--workspace".to_string(),
        "--doc".to_string(),
    ];
    doc_args.extend(cargo_exclude_args(cn_packages));
    run_with_env("cargo", doc_args, &root_dir(), &stack_env_refs)
}

pub(crate) fn rust_test_with_cargo_test(cn_packages: &[String]) -> Result<()> {
    let mut serial_stack_env = rust_test_stack_envs();
    serial_stack_env.push(("RUST_TEST_THREADS".to_string(), "1".to_string()));
    let serial_stack_env_refs = env_refs(&serial_stack_env);

    let mut regular_test_args = vec![
        "test".to_string(),
        "--workspace".to_string(),
        "--lib".to_string(),
        "--bins".to_string(),
        "--tests".to_string(),
    ];
    let mut excluded = cn_packages.to_vec();
    excluded.push(SERIAL_RUST_PACKAGE.to_string());
    regular_test_args.extend(cargo_exclude_args(&excluded));
    run_with_env(
        "cargo",
        regular_test_args,
        &root_dir(),
        &serial_stack_env_refs,
    )?;

    run_with_env(
        "cargo",
        [
            "test",
            "-p",
            SERIAL_RUST_PACKAGE,
            "--lib",
            "--bins",
            "--tests",
        ],
        &root_dir(),
        &serial_stack_env_refs,
    )?;

    let doc_stack_env = rust_test_stack_envs();
    let doc_stack_env_refs = env_refs(&doc_stack_env);
    let mut doc_args = vec![
        "test".to_string(),
        "--workspace".to_string(),
        "--doc".to_string(),
    ];
    doc_args.extend(cargo_exclude_args(cn_packages));
    run_with_env("cargo", doc_args, &root_dir(), &doc_stack_env_refs)
}

/// Raise the per-thread stack used while running the Rust test suites.
///
/// The async integration tests (notably the desktop-runtime private channel
/// restore tests that drive several runtimes at once) overflow the default
/// 2 MiB worker-thread stack under nextest, where the harness `main`
/// `stack_size` override does not apply. Setting `RUST_MIN_STACK` covers both
/// nextest and `cargo test`, including each tokio worker thread. Respect an
/// explicit operator override when one is already present.
pub(crate) fn rust_test_stack_envs() -> Vec<(String, String)> {
    if std::env::var_os("RUST_MIN_STACK").is_some() {
        return Vec::new();
    }
    vec![("RUST_MIN_STACK".to_string(), (64 * 1024 * 1024).to_string())]
}

pub(crate) fn cargo_exclude_args(packages: &[String]) -> Vec<String> {
    let mut args = Vec::with_capacity(packages.len() * 2);
    for package in packages {
        args.push("--exclude".to_string());
        args.push(package.clone());
    }
    args
}

pub(crate) fn cargo_package_args(command: &str, packages: &[String]) -> Vec<String> {
    let mut args = Vec::with_capacity(1 + packages.len() * 2);
    args.push(command.to_string());
    for package in packages {
        args.push("-p".to_string());
        args.push(package.clone());
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cargo_exclude_args_expands_each_package() {
        let packages = vec!["package-a".to_string(), "package-b".to_string()];
        assert_eq!(
            cargo_exclude_args(&packages),
            vec![
                "--exclude".to_string(),
                "package-a".to_string(),
                "--exclude".to_string(),
                "package-b".to_string(),
            ]
        );
    }

    #[test]
    fn cargo_package_args_prefixes_command_and_expands_packages() {
        let packages = vec!["package-a".to_string(), "package-b".to_string()];
        assert_eq!(
            cargo_package_args("clippy", &packages),
            vec![
                "clippy".to_string(),
                "-p".to_string(),
                "package-a".to_string(),
                "-p".to_string(),
                "package-b".to_string(),
            ]
        );
    }
}
