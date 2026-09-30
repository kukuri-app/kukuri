//! W1 AC-2: ブラウザの `IrohDocsNode` が QUIC over WebRTC DataChannel で native の node と接続し、互いの
//! replica を同じ有界な reader（件数・bytes・期限）で読む（ADR 0056 §9）。gateway と relay は使わない。
//! headless の Chromium で `scripts/ci/browser_peer_test.sh kukuri-iroh-node web_peer` から実行する。

use std::{sync::Arc, time::Duration};

use iroh::{EndpointAddr, SecretKey, TransportAddr, endpoint::transports::CustomTransport};
use kukuri_webrtc_transport::{SessionEvent, WebRtcConfig, WebRtcTransport, signaling_fixture};
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

use crate::{DocReadQuery, DocReadResponse, IrohDocsNode, NodeOptions};

#[path = "../tests/support/web_e2e.rs"]
mod web_e2e;

wasm_bindgen_test_configure!(run_in_browser);

fn expected(side: &str) -> String {
    let prefix = web_e2e::PREFIX;
    format!("{prefix}0003,{prefix}0002 reached_limit=true value={side}-3")
}

#[wasm_bindgen_test]
async fn browser_and_native_read_each_other_through_the_bounded_reader() {
    let transport = WebRtcTransport::new(WebRtcConfig {});
    let mut events = transport.take_events().expect("events");
    let custom: Arc<dyn CustomTransport> = transport.clone();
    let node = IrohDocsNode::memory_with(NodeOptions {
        custom_transports: vec![custom],
        ..NodeOptions::default()
    })
    .await
    .expect("start the browser node");
    web_e2e::seed(&node, "browser").await.expect("seed");

    let (session, offer) = transport
        .offer(SecretKey::generate().public())
        .await
        .expect("offer");
    let (native, answer) = signaling_fixture::post_offer(node.endpoint().id(), session, &offer)
        .await
        .expect("native answer");
    transport
        .accept_answer(session, &answer)
        .await
        .expect("accept answer");
    let addr = match n0_future::time::timeout(Duration::from_secs(20), events.recv()).await {
        Ok(Some(SessionEvent::Opened { addr, .. })) => addr,
        other => panic!("the session did not open: {other:?}"),
    };
    let native = EndpointAddr::from_parts(native, [TransportAddr::Custom(addr)]);

    let read = web_e2e::read_newest(&node, native, "native")
        .await
        .expect("read the native replica");
    assert_eq!(read, expected("native"));
    let read_back = signaling_fixture::post("/read-back", &node.endpoint().id().to_string())
        .await
        .expect("read back");
    assert_eq!(read_back, expected("browser"));
    node.shutdown().await.expect("shutdown");
}
