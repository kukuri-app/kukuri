//! #1214 W1 AC-5: Web の JS API（ADR 0056 §6・§7）。同意の前は command を断り、同意の後に runtime が始まる。投稿・
//! private audience の channel・アカウントの作成と切替を Tauri と同じ command で呼び、event を `listen` の callback で
//! 受ける。停止・切替の後に終わった旧世代の結果は返さず、停止の後の callback は呼ばない。再起動の後も IndexedDB に
//! 保存した同意・アカウント・投稿・channel が読める。通報の送信は転送を失敗にし、転送先へ本文を送らない（#703）。

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use serde_json::{Value, json};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_test::wasm_bindgen_test;

use crate::client::{invoke, listen, shutdown, start};

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

    // 表に無い command は、この platform では使えない。
    let unsupported = call("open_external_url", json!({ "url": "https://example.com" }))
        .await
        .unwrap_err();
    assert_eq!(unsupported["code"], "unsupported_platform");
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
