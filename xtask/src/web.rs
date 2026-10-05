//! Web クライアントの build と実ブラウザの試験（ADR 0060 §1・§4、#1220 W8）。

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{Context, Result, bail};

#[allow(unused_imports)]
use crate::*;

/// 試験の Community Node の user-api（`dist-web` に埋め込む URL と、fixture の listen）。
const WEB_E2E_CN_ADDR: &str = "127.0.0.1:4181";

fn target_dir() -> PathBuf {
    let root = root_dir();
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .map(|dir| {
            if dir.is_absolute() {
                dir
            } else {
                root.join(dir)
            }
        })
        .unwrap_or_else(|| root.join("target"))
}

/// Cloudflare Pages の 1 file の上限（ADR 0060 §2）。配信の artifact の各 file をこれ以下にする。
const MAX_ARTIFACT_FILE_BYTES: u64 = 25 * 1024 * 1024;

/// web-runtime の wasm を `wasm-bindgen --target web` で `apps/desktop/web-runtime-pkg` へ出し、Web の mode の Vite の
/// build で `apps/desktop/dist-web` を作る。wasm の C の依存の build に clang と llvm-ar が要る（Linux）。
/// 試験の Community Node を渡さないときは配信の build で、Community Node は配布の設定を使い、wasm を LTO の profile と
/// 名前の section の除去で小さくする。試験の build は、速さと失敗時の調べやすさのため、どちらもしない。
fn web_build(test_community_node: Option<&str>) -> Result<()> {
    let root = root_dir();
    let profile = if test_community_node.is_some() {
        "release"
    } else {
        "web-release"
    };
    run_with_env(
        "cargo",
        [
            "build",
            "-p",
            "kukuri-web-runtime",
            "--target",
            "wasm32-unknown-unknown",
            "--profile",
            profile,
        ],
        &root,
        &[
            ("CC_wasm32_unknown_unknown", "clang"),
            ("AR_wasm32_unknown_unknown", "llvm-ar"),
        ],
    )?;
    let wasm = target_dir().join(format!(
        "wasm32-unknown-unknown/{profile}/kukuri_web_runtime.wasm"
    ));
    let pkg = desktop_dir().join("web-runtime-pkg");
    let mut args = vec![
        "--target".to_string(),
        "web".to_string(),
        // Worker の script（鍵の導出。desktop-runtime の kdf.rs の link_to!）を data URL でなく別 file にする（CSP）。
        "--split-linked-modules".to_string(),
    ];
    if test_community_node.is_none() {
        args.extend([
            "--remove-name-section".to_string(),
            "--remove-producers-section".to_string(),
        ]);
    }
    args.extend([
        "--out-dir".to_string(),
        pkg.display().to_string(),
        wasm.display().to_string(),
    ]);
    run("wasm-bindgen", args, &root)?;
    run_pnpm_with_env(
        ["exec", "vite", "build"],
        &desktop_dir(),
        &[
            ("VITE_KUKURI_TARGET", "web"),
            // 空なら配布の設定（`webRuntime.ts`）。手元の環境の値を配信の build に持ち込まない。
            (
                "VITE_KUKURI_COMMUNITY_NODE_BASE_URL",
                test_community_node.unwrap_or(""),
            ),
        ],
    )
}

/// 配信の artifact（`apps/desktop/dist-web`。ADR 0060 §1・§2、#1220 AC-6）。上限を超える file があれば失敗にする。
pub(crate) fn web_build_artifact() -> Result<()> {
    web_build(None)?;
    let dist = desktop_dir().join("dist-web");
    let mut dirs = vec![dist.clone()];
    let mut total = 0;
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            if metadata.is_dir() {
                dirs.push(entry.path());
                continue;
            }
            if metadata.len() > MAX_ARTIFACT_FILE_BYTES {
                bail!(
                    "{} is {} bytes, over the {MAX_ARTIFACT_FILE_BYTES} bytes limit of a file",
                    entry.path().display(),
                    metadata.len()
                );
            }
            total += metadata.len();
        }
    }
    println!("[xtask] web artifact: {} ({total} bytes)", dist.display());
    Ok(())
}

/// Web の build を、同じ process の Community Node・native の相手（harness の `web_e2e_fixture`）と実ブラウザで試す。
/// driver は `apps/desktop/tests/web-e2e/main-flow.mjs`（WebdriverIO。ブラウザは `KUKURI_WEB_E2E_BROWSER`、driver は
/// `CHROMEDRIVER`・`GECKODRIVER`、無ければ自動）。
/// 引数の scenario（省略すると `main-flow.mjs --list` の全部）を、scenario ごとに新しい fixture で順に回す（#1559）。
/// CI は build の job が `--build-only`、scenario の job が `--no-build <scenario>` で呼ぶ。`--no-compose` は Postgres・valkey を
/// compose で起動せず、`CN_POSTGRES_PORT`・`CN_VALKEY_PORT` の既存のものを使う（Docker の無い macOS の runner。#1220 AC-5b）。
pub(crate) fn web_e2e(args: impl Iterator<Item = String>) -> Result<()> {
    let mut build = true;
    let mut run_scenarios = true;
    let mut compose = true;
    let mut scenarios = Vec::new();
    for arg in args {
        match arg.as_str() {
            "--no-build" => build = false,
            "--build-only" => run_scenarios = false,
            "--no-compose" => compose = false,
            flag if flag.starts_with("--") => bail!("unsupported web-e2e flag: {flag}"),
            _ => scenarios.push(arg),
        }
    }
    if build {
        web_build(Some(&format!("http://{WEB_E2E_CN_ADDR}")))?;
        run(
            "cargo",
            ["build", "-p", "kukuri-harness", "--bin", "web_e2e_fixture"],
            &root_dir(),
        )?;
    }
    if !run_scenarios {
        return Ok(());
    }
    if scenarios.is_empty() {
        let output = child_command("node")
            .args(["tests/web-e2e/main-flow.mjs", "--list"])
            .current_dir(desktop_dir())
            .output()
            .context("failed to list the web e2e scenarios")?;
        anyhow::ensure!(
            output.status.success(),
            "main-flow.mjs --list exited with {}",
            output.status
        );
        scenarios = serde_json::from_slice(&output.stdout)?;
    }
    let fixture = target_dir()
        .join("debug")
        .join(format!("web_e2e_fixture{}", std::env::consts::EXE_SUFFIX));
    let run_all = || {
        // 1 つが失敗しても残りを回し、失敗した scenario をまとめて示す。
        let failed: Vec<&String> = scenarios
            .iter()
            .filter(|scenario| {
                run_scenario(&fixture, scenario)
                    .inspect_err(|error| eprintln!("[xtask] web e2e {scenario}: {error:#}"))
                    .is_err()
            })
            .collect();
        anyhow::ensure!(failed.is_empty(), "web e2e failed: {failed:?}");
        Ok(())
    };
    if compose {
        with_cn_postgres(run_all)
    } else {
        run_all()
    }
}

/// 新しい fixture（Community Node の DB・rendezvous の key・native）を起動し、`scenario` を回して止める。
fn run_scenario(fixture: &Path, scenario: &str) -> Result<()> {
    let mut child = child_command(fixture)
        .envs(cn_test_envs())
        .env("KUKURI_WEB_E2E_DIST", desktop_dir().join("dist-web"))
        .env("KUKURI_WEB_E2E_CN_ADDR", WEB_E2E_CN_ADDR)
        .stdout(Stdio::piped())
        .spawn()
        .context("failed to start web_e2e_fixture")?;
    let stdout = child.stdout.take().context("fixture stdout")?;
    let mut lines = BufReader::new(stdout).lines();
    let origin = loop {
        let Some(line) = lines.next() else {
            let _ = child.wait();
            bail!("web_e2e_fixture exited before it was ready");
        };
        let line = line?;
        println!("[fixture] {line}");
        if let Some(origin) = line.strip_prefix("KUKURI_WEB_E2E_READY=") {
            break origin.to_string();
        }
    };
    std::thread::spawn(move || {
        for line in lines.map_while(Result::ok) {
            println!("[fixture] {line}");
        }
    });
    let result = run_with_env(
        "node",
        ["tests/web-e2e/main-flow.mjs", scenario],
        &desktop_dir(),
        &[("KUKURI_WEB_E2E_ORIGIN", origin.as_str())],
    );
    let _ = child.kill();
    let _ = child.wait();
    result
}
