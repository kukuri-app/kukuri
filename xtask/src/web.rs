//! Web クライアントの build と実ブラウザの試験（ADR 0060 §1・§4、#1220 W8）。

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
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

/// web-runtime の wasm を `wasm-bindgen --target web` で `apps/desktop/web-runtime-pkg` へ出し、Web の mode の Vite の
/// build で `apps/desktop/dist-web` を作る。wasm の C の依存の build に clang と llvm-ar が要る（Linux）。
fn web_build(community_node_base_url: &str) -> Result<()> {
    let root = root_dir();
    run_with_env(
        "cargo",
        [
            "build",
            "-p",
            "kukuri-web-runtime",
            "--target",
            "wasm32-unknown-unknown",
            "--release",
        ],
        &root,
        &[
            ("CC_wasm32_unknown_unknown", "clang"),
            ("AR_wasm32_unknown_unknown", "llvm-ar"),
        ],
    )?;
    let target = target_dir();
    let wasm = target.join("wasm32-unknown-unknown/release/kukuri_web_runtime.wasm");
    let pkg = desktop_dir().join("web-runtime-pkg");
    run(
        "wasm-bindgen",
        [
            "--target".to_string(),
            "web".to_string(),
            // Worker の script（鍵の導出。desktop-runtime の kdf.rs の link_to!）を data URL でなく別 file にする（CSP）。
            "--split-linked-modules".to_string(),
            "--out-dir".to_string(),
            pkg.display().to_string(),
            wasm.display().to_string(),
        ],
        &root,
    )?;
    run_pnpm_with_env(
        ["exec", "vite", "build"],
        &desktop_dir(),
        &[
            ("VITE_KUKURI_TARGET", "web"),
            (
                "VITE_KUKURI_COMMUNITY_NODE_BASE_URL",
                community_node_base_url,
            ),
        ],
    )
}

/// Web の build を、同じ process の Community Node・native の相手（harness の `web_e2e_fixture`）と実ブラウザで試す。
/// driver は `apps/desktop/tests/web-e2e/main-flow.mjs`（WebdriverIO。chromedriver は `CHROMEDRIVER`、無ければ自動）。
pub(crate) fn web_e2e() -> Result<()> {
    let root = root_dir();
    web_build(&format!("http://{WEB_E2E_CN_ADDR}"))?;
    run(
        "cargo",
        ["build", "-p", "kukuri-harness", "--bin", "web_e2e_fixture"],
        &root,
    )?;
    let target = target_dir();
    let fixture = target
        .join("debug")
        .join(format!("web_e2e_fixture{}", std::env::consts::EXE_SUFFIX));
    with_cn_postgres(|| {
        let mut child = child_command(&fixture)
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
            ["tests/web-e2e/main-flow.mjs"],
            &desktop_dir(),
            &[("KUKURI_WEB_E2E_ORIGIN", origin.as_str())],
        );
        let _ = child.kill();
        let _ = child.wait();
        result
    })
}
