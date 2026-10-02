//! ブラウザの `IrohDocsNode` と native の node・別のブラウザの node が、手元の iroh relay で届く接続の上で専用 ALPN の
//! 交渉を行い（#1422 W10）、QUIC over WebRTC DataChannel の custom path で互いの replica を同じ有界な reader
//! （件数・bytes・期限）で読む（W1 AC-2）。交渉は、ブラウザの node が相手への需要の接続を受けて自動で始める（AC-2）。
//! 需要の接続の選ばれた path が custom へ移ってから読み、読み出しが custom path を通ったことは、最新の record
//! （32 KiB の埋め草つき）の受信 bytes で判定する。
//! headless の Chromium で `scripts/ci/browser_peer_test.sh kukuri-iroh-node web_peer` から実行する。

use std::sync::Arc;

use iroh::{EndpointAddr, EndpointId, RelayUrl, TransportAddr};
use kukuri_transport::TransportRelayConfig;
use kukuri_webrtc_transport::{WebRtcConfig, WebRtcTransport, signaling_fixture};
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

use crate::{DOC_READ_ALPN, DocReadQuery, DocReadResponse, IrohDocsNode, NodeOptions};

#[path = "../tests/support/web_e2e.rs"]
mod web_e2e;

wasm_bindgen_test_configure!(run_in_browser);

fn expected(side: &str) -> String {
    let prefix = web_e2e::PREFIX;
    let bytes = side.len() + 2 + web_e2e::NEWEST_PADDING;
    format!(
        "{prefix}0003,{prefix}0002 reached_limit=true value={side}-3 bytes={bytes} via_custom=true"
    )
}

/// native の相手の node と、relay だけで届く宛先。
async fn native() -> (EndpointAddr, RelayUrl) {
    let info = signaling_fixture::post("/info", "").await.expect("info");
    let (id, relay) = info.split_once('\n').expect("id and relay");
    let relay: RelayUrl = relay.parse().expect("relay url");
    let id: EndpointId = id.parse().expect("endpoint id");
    (via_relay(id, &relay), relay)
}

fn via_relay(id: EndpointId, relay: &RelayUrl) -> EndpointAddr {
    EndpointAddr::from_parts(id, [TransportAddr::Relay(relay.clone())])
}

/// `side` の replica を置いたブラウザの node。
async fn browser_node(relay: &RelayUrl, side: &str) -> (Arc<IrohDocsNode>, Arc<WebRtcTransport>) {
    let transport = WebRtcTransport::new(WebRtcConfig {});
    let node = IrohDocsNode::memory_with(NodeOptions {
        relay_config: TransportRelayConfig {
            iroh_relay_urls: vec![relay.to_string()],
        },
        webrtc: Some(transport.clone()),
        ..NodeOptions::default()
    })
    .await
    .expect("start the browser node");
    web_e2e::seed(&node, side).await.expect("seed");
    // 相手から relay で届くようになってから需要の接続を張る。
    node.endpoint().online().await;
    (node, transport)
}

/// T1（browser→native）: 需要の接続を張るだけで交渉が始まり、両方向の読み出しが custom path を通る。
#[wasm_bindgen_test]
async fn a_demand_connection_to_the_native_node_moves_to_the_custom_path() {
    let (native, relay) = native().await;
    let (node, transport) = browser_node(&relay, "browser").await;
    let demand = web_e2e::demand(&node, native.clone())
        .await
        .expect("demand");
    let read = web_e2e::read_over_custom(&node, &transport, &demand, native, "native")
        .await
        .expect("read the native replica");
    assert_eq!(read, expected("native"));
    let read_back = signaling_fixture::post("/read-back", &node.endpoint().id().to_string())
        .await
        .expect("read back");
    assert_eq!(read_back, expected("browser"));
    assert_eq!(transport.stats().sessions, 1);
    node.shutdown().await.expect("shutdown");
}

/// T1（browser↔browser）: 両端が交渉を始めても、session は 1 本にまとまり、読み出しが custom path を通る。
#[wasm_bindgen_test]
async fn two_browser_nodes_share_one_session_over_the_custom_path() {
    let (_, relay) = native().await;
    let (a, a_transport) = browser_node(&relay, "a").await;
    let (b, b_transport) = browser_node(&relay, "b").await;
    let b_addr = via_relay(b.endpoint().id(), &relay);
    let demand = web_e2e::demand(&a, b_addr.clone()).await.expect("demand");
    let read = web_e2e::read_over_custom(&a, &a_transport, &demand, b_addr, "b")
        .await
        .expect("read the other browser replica");
    assert_eq!(read, expected("b"));
    n0_future::time::timeout(std::time::Duration::from_secs(20), async {
        while (a_transport.stats().sessions, b_transport.stats().sessions) != (1, 1) {
            n0_future::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("one session per side");
    a.shutdown().await.expect("shutdown");
    b.shutdown().await.expect("shutdown");
}
