use anyhow::{Context, Result, bail};

mod appimage;
mod assets;
mod ci_prune;
mod cn;
mod desktop;
mod exec;
mod ipc;
mod linuxdeploy;
mod operator_neutrality;
mod oversized;
mod packages;
mod refactoring_audit;
mod release;
mod rust;
#[cfg(feature = "harness")]
mod scenario;

pub(crate) use assets::*;
pub(crate) use cn::*;
pub(crate) use desktop::*;
pub(crate) use exec::*;
pub(crate) use ipc::*;
pub(crate) use operator_neutrality::*;
pub(crate) use oversized::*;
pub(crate) use packages::*;
pub(crate) use release::*;
pub(crate) use rust::*;
#[cfg(feature = "harness")]
pub(crate) use scenario::*;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let Some(command) = args.next() else {
        print_usage();
        bail!("missing xtask command");
    };

    match command.as_str() {
        "doctor" => doctor(),
        "check" => check(),
        "test" => test(),
        "rust-check" => rust_check(),
        "rust-test" => rust_test(),
        "app-api-slow-test" => app_api_slow_test(),
        "tauri-check" => tauri_check(),
        "desktop-lint" => desktop_lint(),
        "desktop-test" => desktop_test(),
        "desktop-storybook" => desktop_storybook(),
        "desktop-browser-test" => desktop_browser_test(),
        "desktop-visual-test" => desktop_visual_test(),
        "desktop-ui-check" => desktop_ui_check(),
        "cn-check" => cn_check(),
        "cn-test" => cn_test(),
        "cn-e2e" => cn_e2e(),
        "desktop-package" => desktop_package(),
        "windows-store-package" => windows_store_package(args),
        "asset-check" => asset_check(),
        "release-check" => {
            let tag = args.next();
            release_check(tag.as_deref())
        }
        "oversized-files" => {
            let update_baseline = match args.next().as_deref() {
                None => false,
                Some("--update-baseline") => true,
                Some(flag) => {
                    print_usage();
                    bail!("unsupported oversized-files flag: {flag}");
                }
            };
            oversized_files(update_baseline)
        }
        "operator-neutrality-check" => operator_neutrality_check(),
        "ci-prune-target" => ci_prune::ci_prune_target(),
        "refactoring-audit-check" => refactoring_audit::refactoring_audit_check(args),
        "ipc-types" => {
            let check = match args.next().as_deref() {
                None => false,
                Some("--check") => true,
                Some(flag) => {
                    print_usage();
                    bail!("unsupported ipc-types flag: {flag}");
                }
            };
            ipc_types(check)
        }
        #[cfg(feature = "harness")]
        "e2e-smoke" => e2e_smoke("desktop_smoke_post_persist"),
        #[cfg(feature = "harness")]
        "scenario" => {
            let name = args.next().context("scenario name is required")?;
            scenario(name.as_str())
        }
        #[cfg(not(feature = "harness"))]
        "e2e-smoke" | "scenario" => bail!("this command requires the default harness feature"),
        _ => {
            print_usage();
            bail!("unsupported xtask command: {command}");
        }
    }
}

fn doctor() -> Result<()> {
    for binary in ["cargo", "rustc", "node"] {
        run_capture(binary, ["--version"], &root_dir())
            .with_context(|| format!("required dependency is missing: {binary}"))?;
    }
    run_pnpm(["--version"], &desktop_dir()).context("required dependency is missing: pnpm")?;

    for required_path in [
        root_dir().join("Cargo.toml"),
        desktop_dir().join("package.json"),
        root_dir()
            .join("harness")
            .join("scenarios")
            .join("desktop_smoke_post_persist.yaml"),
    ] {
        if !required_path.exists() {
            bail!("required path is missing: {}", required_path.display());
        }
    }

    Ok(())
}

fn print_usage() {
    eprintln!(
        "usage: cargo xtask <doctor|check|test|rust-check|rust-test|app-api-slow-test|tauri-check|desktop-lint|desktop-test|desktop-storybook|desktop-browser-test|desktop-visual-test|desktop-ui-check|cn-check|cn-test|cn-e2e|desktop-package|windows-store-package [--allow-dirty] [--output <path>]|asset-check|release-check [tag]|oversized-files [--update-baseline]|operator-neutrality-check|ci-prune-target|refactoring-audit-check [--help]|ipc-types [--check]|e2e-smoke|scenario <name>>"
    );
}
