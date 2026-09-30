//! ブラウザの `IrohDocsNode` と native の node が、手元の iroh relay で届く接続の上で専用 ALPN の交渉を行い
//! （#1422 W10 AC-1）、QUIC over WebRTC DataChannel の custom path で互いの replica を同じ有界な reader
//! （件数・bytes・期限）で読む（W1 AC-2）。交渉はどちらの端からでも始められる。
//! headless の Chromium で `scripts/ci/browser_peer_test.sh kukuri-iroh-node web_peer` から実行する。

use std::sync::Arc;

use iroh::{EndpointAddr, EndpointId, RelayUrl, TransportAddr, endpoint::TransportAddrUsage};
use kukuri_transport::TransportRelayConfig;
use kukuri_webrtc_transport::{WebRtcConfig, WebRtcTransport, signaling_fixture};
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

use crate::{DocReadQuery, DocReadResponse, IrohDocsNode, NodeOptions};

#[path = "../tests/support/web_e2e.rs"]
mod web_e2e;

wasm_bindgen_test_configure!(run_in_browser);

fn expected(side: &str) -> String {
    let prefix = web_e2e::PREFIX;
    format!("{prefix}0003,{prefix}0002 reached_limit=true value={side}-3")
}

/// native の相手の node と、relay だけで届く宛先。
async fn native() -> (EndpointAddr, RelayUrl) {
    let info = signaling_fixture::post("/info", "").await.expect("info");
    let (id, relay) = info.split_once('\n').expect("id and relay");
    let relay: RelayUrl = relay.parse().expect("relay url");
    let id: EndpointId = id.parse().expect("endpoint id");
    (
        EndpointAddr::from_parts(id, [TransportAddr::Relay(relay.clone())]),
        relay,
    )
}

async fn browser_node(relay: &RelayUrl) -> Arc<IrohDocsNode> {
    let node = IrohDocsNode::memory_with(NodeOptions {
        relay_config: TransportRelayConfig {
            iroh_relay_urls: vec![relay.to_string()],
        },
        webrtc: Some(WebRtcTransport::new(WebRtcConfig {})),
        ..NodeOptions::default()
    })
    .await
    .expect("start the browser node");
    web_e2e::seed(&node, "browser").await.expect("seed");
    // native から relay で届くようになってから交渉する。
    node.endpoint().online().await;
    node
}

/// 相手への custom path が使われている。
async fn custom_path_is_active(node: &IrohDocsNode, remote: EndpointId) -> bool {
    node.endpoint()
        .remote_info(remote)
        .await
        .is_some_and(|info| {
            info.addrs().any(|addr| {
                addr.addr().is_custom() && matches!(addr.usage(), TransportAddrUsage::Active)
            })
        })
}

#[wasm_bindgen_test]
async fn the_browser_starts_the_negotiation_and_both_read_over_the_custom_path() {
    let (native, relay) = native().await;
    let node = browser_node(&relay).await;
    node.webrtc_signaling()
        .expect("webrtc")
        .connect(native.clone())
        .await
        .expect("negotiate");
    let read = web_e2e::read_newest(&node, native.clone(), "native")
        .await
        .expect("read the native replica");
    assert_eq!(read, expected("native"));
    assert!(custom_path_is_active(&node, native.id).await);
    let read_back = signaling_fixture::post("/read-back", &node.endpoint().id().to_string())
        .await
        .expect("read back");
    assert_eq!(read_back, expected("browser"));
    node.shutdown().await.expect("shutdown");
}

#[wasm_bindgen_test]
async fn the_native_node_starts_the_negotiation_and_both_read_over_the_custom_path() {
    let (native, relay) = native().await;
    let node = browser_node(&relay).await;
    let read_back = signaling_fixture::post("/connect", &node.endpoint().id().to_string())
        .await
        .expect("negotiate from the native node");
    assert_eq!(read_back, expected("browser"));
    assert!(custom_path_is_active(&node, native.id).await);
    let read = web_e2e::read_newest(&node, native, "native")
        .await
        .expect("read the native replica");
    assert_eq!(read, expected("native"));
    node.shutdown().await.expect("shutdown");
}
