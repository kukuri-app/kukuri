//! #1214 W1 AC-5: Web の JS API（ADR 0056 §6・§7）。同意の前は command を断り、同意の後に runtime が始まる。投稿・
//! private audience の channel・アカウントの作成と切替を Tauri と同じ command で呼び、event を `listen` の callback で
//! 受ける。停止・切替の後に終わった旧世代の結果は返さず、停止の後の callback は呼ばない。再起動の後も IndexedDB に
//! 保存した同意・アカウント・投稿・channel が読める。通報の送信は転送を失敗にし、転送先へ本文を送らない（#703）。
//! 鍵の export・import は native と互換で、argon2id の導出の間に main thread の long task が出ない（#1220 AC-2c）。

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use serde_json::{Value, json};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_test::wasm_bindgen_test;

use crate::client::{invoke, listen, shutdown, start};
use kukuri_webrtc_transport::signaling_fixture::post as post_to_peer;

#[wasm_bindgen(inline_js = r#"
export function watch_long_tasks() {
  const durations = [];
  const observer = new PerformanceObserver((list) => {
    for (const entry of list.getEntries()) durations.push(entry.duration);
  });
  observer.observe({ type: "longtask" });
  return () => {
    for (const entry of observer.takeRecords()) durations.push(entry.duration);
    observer.disconnect();
    return durations;
  };
}
export function block_main_thread(ms) {
  const end = performance.now() + ms;
  while (performance.now() < end) {}
}
"#)]
extern "C" {
    /// main thread の long task（50 ms 超）の観測を始め、止めて長さの一覧を返す関数を返す。
    fn watch_long_tasks() -> js_sys::Function;
    fn block_main_thread(ms: f64);
}

/// 観測を止め、それまでの long task の長さを返す。entry は task の後に積まれるので少し待つ。
async fn long_tasks(stop: js_sys::Function) -> Vec<f64> {
    n0_future::time::sleep(Duration::from_millis(200)).await;
    serde_json::from_value(json_of(&stop.call0(&JsValue::NULL).unwrap())).unwrap()
}

fn json_of(value: &JsValue) -> Value {
    if value.is_undefined() {
        return Value::Null;
    }
    serde_json::from_str(&String::from(js_sys::JSON::stringify(value).unwrap())).unwrap()
}

fn invoke_future(
    command: &str,
    args: Value,
) -> impl std::future::Future<Output = Result<JsValue, JsValue>> {
    invoke(
        command.to_string(),
        js_sys::JSON::parse(&args.to_string()).unwrap(),
    )
}

async fn call(command: &str, args: Value) -> Result<Value, Value> {
    invoke_future(command, args)
        .await
        .map(|value| json_of(&value))
        .map_err(|error| json_of(&error))
}

/// `listen` に渡した callback が受け取った event。
struct Events {
    received: Rc<RefCell<Vec<Value>>>,
    _callback: Closure<dyn Fn(JsValue)>,
}

fn listen_events() -> Events {
    let received = Rc::new(RefCell::new(Vec::new()));
    let sink = received.clone();
    let callback = Closure::<dyn Fn(JsValue)>::new(move |event: JsValue| {
        sink.borrow_mut().push(json_of(&event));
    });
    listen(
        callback
            .as_ref()
            .unchecked_ref::<js_sys::Function>()
            .clone(),
    )
    .unwrap();
    Events {
        received,
        _callback: callback,
    }
}

async fn eventually(mut done: impl FnMut() -> bool) -> bool {
    for _ in 0..200 {
        if done() {
            return true;
        }
        n0_future::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

async fn timeline(topic: &str) -> Vec<String> {
    let view = call(
        "list_timeline",
        json!({ "request": { "topic": topic, "cursor": null, "limit": 50 } }),
    )
    .await
    .unwrap();
    view["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["object_id"].as_str().unwrap().to_string())
        .collect()
}

async fn joined_channels(topic: &str) -> Vec<String> {
    let page = call(
        "list_joined_private_channels",
        json!({ "request": { "topic": topic } }),
    )
    .await
    .unwrap();
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|channel| channel["channel_id"].as_str().unwrap().to_string())
        .collect()
}

#[wasm_bindgen_test]
async fn the_client_runs_commands_and_events_and_drops_stale_results_across_restarts() {
    let topic = "kukuri:web-client-test";

    // 同意の前は、runtime の command を断る。
    let status = json_of(&start(JsValue::UNDEFINED).await.unwrap());
    assert_eq!(status["status"], "consent_required", "{status}");
    let rejected = call("create_post", json!({})).await.unwrap_err();
    assert!(
        rejected["message"]
            .as_str()
            .unwrap()
            .contains("requires Ready startup state"),
        "{rejected}"
    );
    let consent = call("get_app_consent_status", Value::Null).await.unwrap();
    let documents: Vec<Value> = consent["documents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|document| json!({ "slug": document["slug"], "version": document["currentVersion"] }))
        .collect();
    let accepted = call(
        "accept_app_consents",
        json!({ "documents": documents, "language": "ja", "ageAttested": true }),
    )
    .await
    .unwrap();
    assert_eq!(accepted, json!({ "status": "ready" }));
    let events = listen_events();

    // 投稿と private audience の channel。投稿で topic の通信状態が変わり、event が届く。
    let post = call(
        "create_post",
        json!({ "request": { "topic": topic, "content": "hello from the web", "reply_to": null } }),
    )
    .await
    .unwrap();
    let post = post.as_str().unwrap().to_string();
    assert_eq!(timeline(topic).await, std::slice::from_ref(&post));
    let channel = call(
        "create_private_channel",
        json!({ "request": { "topic": topic, "label": "web", "audience_kind": "friend_only" } }),
    )
    .await
    .unwrap();
    let channel = channel["channel_id"].as_str().unwrap().to_string();
    assert_eq!(joined_channels(topic).await, std::slice::from_ref(&channel));
    assert!(eventually(|| !events.received.borrow().is_empty()).await);

    // 切替の間に終わった旧世代の結果は返さない。
    let accounts = call("list_accounts", Value::Null).await.unwrap();
    let first = accounts["active_account_id"].as_str().unwrap().to_string();
    let mut stale = Box::pin(invoke_future(
        "list_timeline",
        json!({ "request": { "topic": topic, "cursor": null, "limit": 50 } }),
    ));
    assert!(n0_future::future::poll_once(&mut stale).await.is_none());
    let second = call(
        "create_account",
        json!({ "request": { "account_id": first, "operation_id": "6f9619ff-8b86-d011-b42d-00c04fc964ff" } }),
    )
    .await
    .unwrap();
    assert_eq!(json_of(&stale.await.unwrap_err())["code"], "stale_runtime");
    assert!(timeline(topic).await.is_empty());

    // 停止の間に終わった結果も返さず、停止の後は callback を呼ばない。
    let mut stale = Box::pin(invoke_future(
        "list_timeline",
        json!({ "request": { "topic": topic, "cursor": null, "limit": 50 } }),
    ));
    assert!(n0_future::future::poll_once(&mut stale).await.is_none());
    shutdown().await;
    assert_eq!(json_of(&stale.await.unwrap_err())["code"], "stale_runtime");
    let delivered = events.received.borrow().len();

    // 再起動: 同意・アカウント・投稿・channel は IndexedDB から読める。新しい callback だけが event を受ける。
    assert_eq!(
        json_of(&start(JsValue::NULL).await.unwrap()),
        json!({ "status": "ready" })
    );
    let restarted = listen_events();
    let accounts = call("list_accounts", Value::Null).await.unwrap();
    assert_eq!(accounts["active_account_id"], second["id"]);
    assert_eq!(accounts["accounts"].as_array().unwrap().len(), 2);
    call(
        "switch_account",
        json!({ "request": { "account_id": first } }),
    )
    .await
    .unwrap();
    assert_eq!(timeline(topic).await, [post]);
    assert_eq!(joined_channels(topic).await, [channel]);
    call(
        "create_post",
        json!({ "request": { "topic": topic, "content": "after the restart", "reply_to": null } }),
    )
    .await
    .unwrap();
    assert!(eventually(|| !restarted.received.borrow().is_empty()).await);
    assert_eq!(events.received.borrow().len(), delivered);

    // 鍵の export・import。long task の観測は、main thread を 100 ms 止めると検出する（陽性対照）。
    let stop = watch_long_tasks();
    block_main_thread(100.0);
    assert!(!long_tasks(stop).await.is_empty());
    let passphrase = "correct horse battery staple";
    let native = post_to_peer("/account-key-export", passphrase)
        .await
        .unwrap();
    let (native_export, native_pubkey) = native.split_once('\n').unwrap();
    // export と import の 2 回の導出の間に、main thread の long task が出ない。
    let stop = watch_long_tasks();
    let exported = call(
        "export_account_key",
        json!({ "request": { "passphrase": passphrase } }),
    )
    .await
    .unwrap();
    let preview = call(
        "preview_account_key_import",
        json!({ "request": { "export": native_export } }),
    )
    .await
    .unwrap();
    assert_eq!(preview["public_key"], native_pubkey);
    assert_eq!(preview["already_registered"], false);
    let imported = call(
        "import_account_key",
        json!({ "request": { "export": native_export, "passphrase": passphrase, "label": null } }),
    )
    .await
    .unwrap();
    assert_eq!(long_tasks(stop).await, Vec::<f64>::new());
    // native の export を取り込むと同じ公開鍵になり、Web の export は native で取り込める。
    assert_eq!(imported["pubkey"], native_pubkey);
    let web_export = exported["export"].as_str().unwrap();
    assert!(web_export.starts_with(kukuri_core::ACCOUNT_KEY_EXPORT_PREFIX));
    assert_eq!(
        post_to_peer(
            "/account-key-import",
            &format!("{web_export}\n{passphrase}")
        )
        .await
        .unwrap(),
        exported["public_key"].as_str().unwrap()
    );

    // 表に無い command は、この platform では使えない。
    let unsupported = call("open_external_url", json!({ "url": "https://example.com" }))
        .await
        .unwrap_err();
    assert_eq!(unsupported["code"], "unsupported_platform");
    // live・game・metaverse・Dome は Web の合意の外（ADR 0060 §3）。
    let session = call("list_live_sessions", Value::Null).await.unwrap_err();
    assert_eq!(session["code"], "unsupported_platform");
    shutdown().await;
}

#[wasm_bindgen_test]
async fn a_report_is_not_sent_to_the_redirected_location() {
    let Some(url) = option_env!("KUKURI_PEER_URL") else {
        panic!("run the browser tests through scripts/ci/browser_peer_test.sh");
    };
    let count = || kukuri_webrtc_transport::signaling_fixture::post("/report-count", "");
    let before: usize = count().await.unwrap().parse().unwrap();
    // 転送しない受付には届く（陽性対照）。
    let (status, _) = kukuri_desktop_runtime::post_report(&format!("{url}/report"), b"{}".to_vec())
        .await
        .unwrap();
    assert_eq!(status, 200);
    assert_eq!(count().await.unwrap().parse::<usize>().unwrap(), before + 1);
    // 転送する受付は失敗にし、転送先へ送らない。
    assert!(
        kukuri_desktop_runtime::post_report(&format!("{url}/report-redirect"), b"{}".to_vec())
            .await
            .is_err()
    );
    assert_eq!(count().await.unwrap().parse::<usize>().unwrap(), before + 1);
}

/// W4 AC-3: browser の lifecycle の event が、host の中断・復帰のどちらかへ渡る（ADR 0059 §5）。外した後は渡らない。
#[wasm_bindgen_test]
fn lifecycle_events_map_to_suspend_and_resume_until_the_listeners_are_dropped() {
    use crate::lifecycle::{Lifecycle, listen_lifecycle};

    let window = web_sys::window().unwrap();
    let document = window.document().unwrap();
    let seen = Rc::new(RefCell::new(Vec::new()));
    let sink = seen.clone();
    let listeners = listen_lifecycle(move |lifecycle| sink.borrow_mut().push(lifecycle)).unwrap();
    let fire = || {
        for (target, name) in [
            (window.as_ref(), "pagehide"),
            (window.as_ref(), "offline"),
            (document.as_ref(), "freeze"),
            (window.as_ref(), "online"),
            (window.as_ref(), "pageshow"),
            (document.as_ref(), "resume"),
            // headless の Chromium の page は可視。
            (document.as_ref(), "visibilitychange"),
        ] as [(&web_sys::EventTarget, &str); 7]
        {
            target
                .dispatch_event(&web_sys::Event::new(name).unwrap())
                .unwrap();
        }
    };
    fire();
    use Lifecycle::{Resume, Suspend};
    assert_eq!(
        *seen.borrow(),
        [Suspend, Suspend, Suspend, Resume, Resume, Resume, Resume]
    );
    drop(listeners);
    fire();
    assert_eq!(seen.borrow().len(), 7);
}
