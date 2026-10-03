//! #1214 AC-5: command の dispatch 表（Tauri と web-runtime が共用する。ADR 0056 §6・§7）。
//! frontend が呼ぶ command の引数の key が表と一致し、呼んでいる間に runtime が替わった結果は返さず、表に無い command は
//! この platform では使えないと返す。

use super::*;
use crate::accounts::{add_account, ensure_accounts_initialized};
use crate::command::{gate_commands, runtime_commands, session_commands};
use serde_json::Value;
use std::collections::BTreeSet;

const MODE: IdentityStorageMode = IdentityStorageMode::FileOnly;

struct TestGate {
    host: Arc<ClientHost>,
    startup: ClientStartupState,
    lock: tokio::sync::Mutex<()>,
}

impl ClientGate for TestGate {
    fn host(&self) -> Option<Arc<ClientHost>> {
        Some(self.host.clone())
    }

    fn startup(&self) -> &ClientStartupState {
        &self.startup
    }

    fn operation_lock(&self) -> &tokio::sync::Mutex<()> {
        &self.lock
    }

    fn require_running(&self) -> Result<(), CommandError> {
        Ok(())
    }
}

async fn test_gate(dir: &Path) -> TestGate {
    let db = ensure_accounts_initialized(dir, MODE).await.unwrap();
    let runtime =
        DesktopRuntime::new_with_config_and_identity(&db, TransportNetworkConfig::loopback(), MODE)
            .await
            .unwrap();
    TestGate {
        host: ClientHost::from_runtime(dir.to_path_buf(), Arc::new(runtime))
            .await
            .unwrap(),
        startup: ClientStartupState::new(ClientStartupStatus::Ready),
        lock: tokio::sync::Mutex::new(()),
    }
}

#[tokio::test]
async fn a_result_finished_after_the_runtime_changed_is_not_returned() {
    let _resource = lock_test_resource(TestResource::IdentityStorage).await;
    let dir = tempdir().unwrap();
    let gate = test_gate(dir.path()).await;
    let ctx = DispatchContext::default();
    let args = serde_json::json!({ "request": { "passphrase": "correct horse battery staple" } });
    // 鍵の書き出しは KDF を blocking の thread で行うので、1 回 poll した時点では終わっていない。
    let mut call = Box::pin(dispatch_command(&gate, &ctx, "export_account_key", args));
    assert!(futures_util::poll!(&mut call).is_pending());
    let account = add_account(dir.path(), MODE, &KukuriKeys::generate(), None, false)
        .await
        .unwrap();
    let next = DesktopRuntime::new_with_config_and_identity(
        account_db_path(dir.path(), &account.id),
        TransportNetworkConfig::loopback(),
        MODE,
    )
    .await
    .unwrap();
    gate.host
        .replace_runtime(Arc::new(next))
        .await
        .unwrap()
        .shutdown()
        .await;
    assert_eq!(call.await.unwrap_err().code, STALE_RUNTIME_CODE);

    // 替わった後に呼べば、今の runtime の結果を返す。
    let config = dispatch_command(&gate, &ctx, "get_community_node_config", Value::Null).await;
    assert!(config.is_ok(), "{config:?}");
    gate.host.shutdown().await;
}

#[tokio::test]
async fn a_command_outside_the_table_is_unsupported_on_the_platform() {
    let _resource = lock_test_resource(TestResource::IdentityStorage).await;
    let dir = tempdir().unwrap();
    let gate = test_gate(dir.path()).await;
    let error = dispatch_command(
        &gate,
        &DispatchContext::default(),
        "open_external_url",
        serde_json::json!({ "url": "https://example.com" }),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, UNSUPPORTED_PLATFORM_CODE);
    gate.host.shutdown().await;
}

/// frontend の `invokeDesktop('<command>', { <key>: ... })` の command 名と、引数の object の top-level の key。
/// 引数が object の literal でない呼出しは key を `None` にする。
fn invocations(source: &str) -> Vec<(String, Option<BTreeSet<String>>)> {
    let bytes = source.as_bytes();
    let mut found = Vec::new();
    let mut at = 0;
    while let Some(offset) = source[at..].find("invokeDesktop") {
        let mut i = at + offset + "invokeDesktop".len();
        at = i;
        if bytes[i] == b'<' {
            let mut depth = 0;
            while i < bytes.len() {
                match bytes[i] {
                    b'<' => depth += 1,
                    b'>' => depth -= 1,
                    _ => {}
                }
                i += 1;
                if depth == 0 {
                    break;
                }
            }
        }
        if bytes.get(i) != Some(&b'(') {
            continue;
        }
        i = skip_space(bytes, i + 1);
        let quote = bytes[i];
        if !matches!(quote, b'\'' | b'"') {
            continue;
        }
        let end = i + 1 + source[i + 1..].find(quote as char).unwrap();
        let name = source[i + 1..end].to_string();
        i = skip_space(bytes, end + 1);
        let keys = match bytes[i] {
            b')' => Some(BTreeSet::new()),
            b',' if bytes[skip_space(bytes, i + 1)] == b'{' => {
                Some(object_keys(source, skip_space(bytes, i + 1) + 1))
            }
            _ => None,
        };
        found.push((name, keys));
    }
    found
}

fn skip_space(bytes: &[u8], mut i: usize) -> usize {
    while bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    i
}

/// `{` の次から、深さ 1 の key を読む（値・文字列・入れ子は飛ばす）。
fn object_keys(source: &str, mut i: usize) -> BTreeSet<String> {
    let bytes = source.as_bytes();
    let mut keys = BTreeSet::new();
    loop {
        i = skip_space(bytes, i);
        if bytes[i] == b'}' {
            return keys;
        }
        let start = i;
        while bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' {
            i += 1;
        }
        keys.insert(source[start..i].to_string());
        // 値（無ければ省略形）を、深さ 1 の `,` か `}` まで飛ばす。
        let mut depth = 0;
        loop {
            match bytes[i] {
                b'\'' | b'"' | b'`' => {
                    let quote = bytes[i];
                    i += 1;
                    while bytes[i] != quote {
                        i += if bytes[i] == b'\\' { 2 } else { 1 };
                    }
                }
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' => depth -= 1,
                b'}' if depth == 0 => return keys,
                b'}' => depth -= 1,
                b',' if depth == 0 => {
                    i += 1;
                    break;
                }
                _ => {}
            }
            i += 1;
        }
    }
}

fn camel_case(snake: &str) -> String {
    let mut parts = snake.split('_');
    let mut out = parts.next().unwrap_or_default().to_string();
    for part in parts {
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.push_str(chars.as_str());
        }
    }
    out
}

fn frontend_sources(dir: &Path, out: &mut Vec<(std::path::PathBuf, String)>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        if path.is_dir() {
            frontend_sources(&path, out);
        } else if (name.ends_with(".ts") || name.ends_with(".tsx")) && !name.contains(".test.") {
            out.push((path.clone(), fs::read_to_string(&path).unwrap()));
        }
    }
}

#[test]
fn frontend_invocations_use_the_command_names_and_argument_keys_of_the_table() {
    let table: std::collections::BTreeMap<_, BTreeSet<String>> = gate_commands()
        .into_iter()
        .chain(runtime_commands())
        .chain(session_commands())
        .map(|(name, args)| (name, args.iter().map(|arg| camel_case(arg)).collect()))
        .collect();
    let mut sources = Vec::new();
    frontend_sources(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps/desktop/src"),
        &mut sources,
    );
    let mut checked = BTreeSet::new();
    for (path, source) in &sources {
        for (name, keys) in invocations(source) {
            let (Some(expected), Some(keys)) = (table.get(name.as_str()), keys) else {
                continue;
            };
            assert_eq!(&keys, expected, "{name} in {}", path.display());
            checked.insert(name);
        }
    }
    // 走査が frontend の呼出しを読めていること（委譲の command の大半は frontend から呼ばれる）。
    assert!(
        checked.len() > 100,
        "checked only {} commands",
        checked.len()
    );
}
