//! Web クライアントの JS API（ADR 0056 §6・§7）: `start`・`shutdown`・`invoke`・`listen`。command は desktop-runtime の
//! 表で、Tauri と同じ名前・引数・結果・error の形で呼ぶ。止めた後と runtime の世代が替わった後の結果と event は返さない。

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use kukuri_desktop_runtime::{
    AcceptedAppConsentDocument, ClientGate, ClientHost, ClientHostStart, ClientStartupError,
    ClientStartupState, ClientStartupStatus, CommandError, CommunityNodeConfig, DesktopRuntime,
    DispatchContext, RuntimeBuilder, RuntimeEvent, admit_command, app_consent_status,
    dispatch_command, failed_startup_status, install_platform_storage, record_app_consents,
    require_consent_acceptance_state, validate_app_consent_documents,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use wasm_bindgen::prelude::*;

use crate::lifecycle::{Lifecycle, LifecycleListeners, listen_lifecycle};
use crate::tab_lock::{self, RuntimeLock};
use crate::{BrowserStorage, IndexedDbCache};

/// 保存の key に使う仮想の app data dir。
const APP_DATA_DIR: &str = "/kukuri";
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartConfig {
    /// 初回の起動で保存する Community Node の設定（native の配布の設定にあたる）。
    #[serde(default)]
    community_node_config: CommunityNodeConfig,
}

/// account の runtime を、IndexedDB の保存とメモリの node（WebRTC の transport つき）で組み立てる。
struct WebRuntimeBuilder {
    community_node_config: CommunityNodeConfig,
}

#[async_trait]
impl RuntimeBuilder for WebRuntimeBuilder {
    async fn build(&self, db_path: &Path) -> Result<Arc<DesktopRuntime>, ClientStartupError> {
        let account = db_path
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                ClientStartupError::unknown(format!(
                    "invalid account db path `{}`",
                    db_path.display()
                ))
            })?;
        let store = Arc::new(
            IndexedDbCache::open(account)
                .await
                .map_err(ClientStartupError::from_error)?,
        );
        let runtime = DesktopRuntime::open_in_memory_node(
            db_path.to_path_buf(),
            store,
            true,
            self.community_node_config.clone(),
        )
        .await
        .map_err(ClientStartupError::from_error)?;
        Ok(Arc::new(runtime))
    }
}

struct WebGate {
    host: RwLock<Option<Arc<ClientHost>>>,
    startup: ClientStartupState,
    operation_lock: tokio::sync::Mutex<()>,
    stopping: AtomicBool,
    builder: Arc<WebRuntimeBuilder>,
}

impl ClientGate for WebGate {
    fn host(&self) -> Option<Arc<ClientHost>> {
        self.host.read().expect("host lock poisoned").clone()
    }

    fn startup(&self) -> &ClientStartupState {
        &self.startup
    }

    fn operation_lock(&self) -> &tokio::sync::Mutex<()> {
        &self.operation_lock
    }

    fn require_running(&self) -> Result<(), CommandError> {
        if self.stopping.load(Ordering::SeqCst) {
            Err(CommandError::from("アプリを終了しています。".to_string()))
        } else {
            Ok(())
        }
    }
}

/// `start` から `shutdown` までの 1 回の起動。
struct Client {
    gate: Arc<WebGate>,
    listeners: RefCell<Vec<js_sys::Function>>,
    /// browser の lifecycle の event を host の中断・復帰へ渡す（ADR 0059 §5）。`shutdown` で外す。
    lifecycle: RefCell<Option<LifecycleListeners>>,
    /// runtime を動かす tab の lock（ADR 0059 §4）。持っている間だけ runtime を動かす。
    runtime_lock: RefCell<Option<RuntimeLock>>,
}

thread_local! {
    static CLIENT: RefCell<Option<Rc<Client>>> = const { RefCell::new(None) };
    static STORAGE_INSTALLED: Cell<bool> = const { Cell::new(false) };
}

fn error_value(error: &CommandError) -> JsValue {
    to_js(error).unwrap_or_else(|_| JsValue::from_str(&error.message))
}

fn to_js(value: &impl Serialize) -> Result<JsValue, JsValue> {
    let json =
        serde_json::to_string(value).map_err(|error| JsValue::from_str(&error.to_string()))?;
    js_sys::JSON::parse(&json)
}

fn from_js(value: &JsValue) -> Result<Value, CommandError> {
    if value.is_undefined() {
        return Ok(Value::Null);
    }
    let json: String = js_sys::JSON::stringify(value)
        .map_err(|_| CommandError::from("args are not JSON".to_string()))?
        .into();
    serde_json::from_str(&json).map_err(|error| CommandError::from(error.to_string()))
}

fn current() -> Result<Rc<Client>, JsValue> {
    CLIENT
        .with_borrow(Clone::clone)
        .ok_or_else(|| error_value(&CommandError::from("the client is not started".to_string())))
}

fn consent_db_path() -> PathBuf {
    Path::new(APP_DATA_DIR).join("kukuri.db")
}

async fn install_storage() -> Result<(), CommandError> {
    if STORAGE_INSTALLED.get() {
        return Ok(());
    }
    let storage = BrowserStorage::open().await?;
    install_platform_storage(Box::leak(Box::new(storage)));
    STORAGE_INSTALLED.set(true);
    Ok(())
}

/// 同意があれば host を始めて公開し、起動の状態を返す。
async fn start_host(client: &Rc<Client>) -> ClientStartupStatus {
    let gate = &client.gate;
    let started =
        ClientHost::start_if_consented_with(PathBuf::from(APP_DATA_DIR), gate.builder.clone())
            .await;
    match started {
        Ok(ClientHostStart::Ready(host)) => {
            publish(client, host);
            ClientStartupStatus::Ready
        }
        Ok(ClientHostStart::ConsentRequired(status)) => status,
        Err(error) => failed_startup_status(error, None),
    }
}

/// lock を取り（`steal` なら奪い）、runtime を始めて、起動の状態を返す。取れなければ runtime を始めない
/// （`in_use_elsewhere`）。保存先は lock を取ってから開く。
async fn begin(client: &Rc<Client>, steal: bool) -> ClientStartupStatus {
    let weak = Rc::downgrade(client);
    let lost = move || {
        if let Some(client) = weak.upgrade() {
            n0_future::task::spawn(lose_runtime(client));
        }
    };
    match tab_lock::acquire(steal, lost).await {
        Ok(Some(lock)) => *client.runtime_lock.borrow_mut() = Some(lock),
        Ok(None) => return ClientStartupStatus::InUseElsewhere,
        Err(error) => {
            return failed_startup_status(ClientStartupError::unknown(format!("{error:?}")), None);
        }
    }
    match install_storage().await {
        Ok(()) => start_host(client).await,
        Err(error) => failed_startup_status(ClientStartupError::unknown(error.message), None),
    }
}

/// 別の tab に lock を奪われた: 行っている操作（起動・引継ぎ・同意・account の切替）の終わりを待ってから runtime を止め
/// （世代の終了）、起動の状態を `in_use_elsewhere` に戻して、止め終えてから画面に知らせる。
async fn lose_runtime(client: Rc<Client>) {
    let _guard = client.gate.operation_lock.lock().await;
    client.runtime_lock.borrow_mut().take();
    let host = client.gate.host.write().expect("host lock poisoned").take();
    client
        .gate
        .startup
        .set_status(ClientStartupStatus::InUseElsewhere);
    if let Some(host) = host {
        host.shutdown().await;
    }
    if let Ok(event) = to_js(&RuntimeEvent::StartupStatusChanged) {
        let listeners = client.listeners.borrow().clone();
        for listener in listeners {
            let _ = listener.call1(&JsValue::NULL, &event);
        }
    }
}

/// 利用者の明示の操作で、別の tab から runtime を引き継ぐ（`take_over_runtime`）。
async fn take_over_runtime(client: &Rc<Client>) -> Result<ClientStartupStatus, CommandError> {
    let gate = &client.gate;
    let _guard = gate.operation_lock.lock().await;
    gate.require_running()?;
    if gate.startup.status() != ClientStartupStatus::InUseElsewhere {
        return Err(CommandError::from(
            "the runtime already runs in this tab".to_string(),
        ));
    }
    gate.startup.set_status(ClientStartupStatus::Initializing);
    let status = begin(client, true).await;
    gate.startup.set_status(status.clone());
    Ok(status)
}

/// host を公開し、その event を `listen` の callback へ渡す。止めた後の event は渡さない。
fn publish(client: &Rc<Client>, host: Arc<ClientHost>) {
    let mut events = host.subscribe_events();
    *client.gate.host.write().expect("host lock poisoned") = Some(host);
    let client = Rc::downgrade(client);
    n0_future::task::spawn(async move {
        while let Some(event) = events.next().await {
            let Some(client) = client.upgrade() else {
                break;
            };
            let Ok(value) = to_js(&event) else {
                continue;
            };
            let listeners = client.listeners.borrow().clone();
            for listener in listeners {
                let _ = listener.call1(&JsValue::NULL, &value);
            }
        }
    });
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AcceptAppConsentsArgs {
    documents: Vec<AcceptedAppConsentDocument>,
    language: String,
    age_attested: bool,
}

async fn accept_app_consents(
    client: &Rc<Client>,
    args: AcceptAppConsentsArgs,
) -> Result<ClientStartupStatus, CommandError> {
    validate_app_consent_documents(&args.documents)?;
    let gate = &client.gate;
    let _guard = gate.operation_lock.lock().await;
    gate.require_running()?;
    require_consent_acceptance_state(&gate.startup.status()).map_err(CommandError::from)?;
    record_app_consents(
        &consent_db_path(),
        &args.documents,
        &args.language,
        args.age_attested,
        APP_VERSION,
    )
    .await?;
    if gate.host().is_none() {
        gate.startup.set_status(ClientStartupStatus::Initializing);
        gate.startup.set_status(start_host(client).await);
    } else {
        gate.startup.set_status(ClientStartupStatus::Ready);
    }
    Ok(gate.startup.status())
}

fn to_value(value: impl Serialize) -> Result<Value, CommandError> {
    serde_json::to_value(value).map_err(|error| CommandError::from(anyhow::Error::from(error)))
}

async fn invoke_command(
    client: &Rc<Client>,
    command: &str,
    args: Value,
) -> Result<Value, CommandError> {
    let gate = &client.gate;
    // 別の tab が runtime を動かす間は、起動の状態の読取りと引継ぎだけを受ける（保存先を開かない）。
    match command {
        "take_over_runtime" => return to_value(take_over_runtime(client).await?),
        "get_desktop_startup_status" => {}
        _ if gate.startup.status() == ClientStartupStatus::InUseElsewhere => {
            return Err(CommandError::from(
                "kukuri is running in another tab".to_string(),
            ));
        }
        _ => {}
    }
    admit_command(
        command,
        Some(&gate.startup.status()),
        gate.stopping.load(Ordering::SeqCst),
    )
    .map_err(CommandError::from)?;
    match command {
        "get_app_consent_status" => to_value(app_consent_status(&consent_db_path()).await),
        "accept_app_consents" => {
            let args = serde_json::from_value(args).map_err(|error| {
                CommandError::from(format!("invalid args for {command}: {error}"))
            })?;
            to_value(accept_app_consents(client, args).await?)
        }
        _ => {
            let ctx = DispatchContext {
                app_version: APP_VERSION.to_string(),
            };
            dispatch_command(gate.as_ref(), &ctx, command, args).await
        }
    }
}

/// 起動する。同意があれば アクティブなアカウント（無ければ作る）の runtime を始める。起動の状態を返す。
/// `config` は `{ communityNodeConfig }`（省略可）。
struct DiagConsoleLine(Vec<u8>);
impl std::io::Write for DiagConsoleLine {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl Drop for DiagConsoleLine {
    fn drop(&mut self) {
        if !self.0.is_empty() {
            web_sys::console::log_1(&format!("KDIAG {}", String::from_utf8_lossy(&self.0).trim_end()).into());
        }
    }
}

#[wasm_bindgen]
pub async fn start(config: JsValue) -> Result<JsValue, JsValue> {
    let _ = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_env_filter(tracing_subscriber::EnvFilter::new(
            "warn,kukuri_webrtc_transport=debug,kukuri_iroh_node=info",
        ))
        .with_writer(|| DiagConsoleLine(Vec::new()))
        .try_init();
    let config = match from_js(&config).map_err(|error| error_value(&error))? {
        Value::Null => StartConfig::default(),
        config => serde_json::from_value(config)
            .map_err(|error| error_value(&CommandError::from(error.to_string())))?,
    };
    if CLIENT.with_borrow(Option::is_some) {
        return Err(error_value(&CommandError::from(
            "the client is already started".to_string(),
        )));
    }
    let client = Rc::new(Client {
        gate: Arc::new(WebGate {
            host: RwLock::new(None),
            startup: ClientStartupState::initializing(),
            operation_lock: tokio::sync::Mutex::new(()),
            stopping: AtomicBool::new(false),
            builder: Arc::new(WebRuntimeBuilder {
                community_node_config: config.community_node_config,
            }),
        }),
        listeners: RefCell::default(),
        lifecycle: RefCell::default(),
        runtime_lock: RefCell::default(),
    });
    let weak = Rc::downgrade(&client);
    let lifecycle = listen_lifecycle(move |lifecycle| {
        let Some(host) = weak.upgrade().and_then(|client| client.gate.host()) else {
            return;
        };
        n0_future::task::spawn(async move {
            match lifecycle {
                Lifecycle::Suspend => host.suspend().await,
                Lifecycle::Resume => host.resume().await,
            }
        });
    })?;
    *client.lifecycle.borrow_mut() = Some(lifecycle);
    CLIENT.set(Some(client.clone()));
    let status = {
        let _guard = client.gate.operation_lock.lock().await;
        begin(&client, false).await
    };
    client.gate.startup.set_status(status.clone());
    to_js(&status)
}

/// 止める。行っている操作の終わりを待ってから runtime を止める。`listen` の callback も外す。
#[wasm_bindgen]
pub async fn shutdown() {
    let Some(client) = CLIENT.with_borrow_mut(Option::take) else {
        return;
    };
    client.listeners.borrow_mut().clear();
    client.lifecycle.borrow_mut().take();
    // lock は runtime を止めてから手放す（止める前に別の tab が始めないように）。
    let lock = client.runtime_lock.borrow_mut().take();
    let gate = client.gate.clone();
    drop(client);
    gate.stopping.store(true, Ordering::SeqCst);
    let _guard = gate.operation_lock.lock().await;
    let host = gate.host.write().expect("host lock poisoned").take();
    if let Some(host) = host {
        host.shutdown().await;
    }
    drop(lock);
}

/// command を呼ぶ。結果は Tauri の invoke と同じ形、error は `{ code, message }`。
#[wasm_bindgen]
pub async fn invoke(command: String, args: JsValue) -> Result<JsValue, JsValue> {
    let client = current()?;
    let args = from_js(&args).map_err(|error| error_value(&error))?;
    match invoke_command(&client, &command, args).await {
        Ok(value) => to_js(&value),
        Err(error) => Err(error_value(&error)),
    }
}

/// runtime の event（`RuntimeEvent`）を受け取る callback を足す。`shutdown` で外れる。
#[wasm_bindgen]
pub fn listen(callback: js_sys::Function) -> Result<(), JsValue> {
    current()?.listeners.borrow_mut().push(callback);
    Ok(())
}
