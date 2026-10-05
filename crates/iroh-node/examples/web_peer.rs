//! browser↔native の試験（W1 AC-2 の有界な読み出し、#1422 W10 の専用 ALPN の交渉）の native 側。
//!
//! 手元の iroh relay を起動し、QUIC over WebRTC DataChannel（ADR 0057）を渡した `IrohDocsNode` を relay 付きで
//! 起動して、公開の replica に 3 件を置く。試験だけの HTTP（`signaling_fixture`）で次を受ける。
//! - `POST /info`: node の endpoint id と relay の URL
//! - `POST /read-back`（本文は browser の endpoint id）: browser への需要の接続を張り（browser がそれを受けて交渉を
//!   始める。ADR 0057 §9）、その接続が custom path へ移ってから browser の node の replica を同じ有界な reader で読み、
//!   custom path を通ったかを添える
//!
//! native だけで動く。

#![cfg(not(target_family = "wasm"))]

use std::sync::Arc;

use anyhow::Result;
use axum::{extract::State, routing::post};
use iroh::{EndpointAddr, EndpointId, RelayUrl, TransportAddr};
use kukuri_iroh_node::{DOC_READ_ALPN, DocReadQuery, DocReadResponse, IrohDocsNode, NodeOptions};
use kukuri_transport::TransportRelayConfig;
use kukuri_webrtc_transport::{WebRtcConfig, WebRtcTransport, signaling_fixture};

#[path = "../tests/support/web_e2e.rs"]
mod web_e2e;

type Peer = (Arc<IrohDocsNode>, RelayUrl, Arc<WebRtcTransport>);

#[tokio::main]
async fn main() -> Result<()> {
    let (relay, _relay_server) = signaling_fixture::spawn_relay().await?;
    let transport = WebRtcTransport::new(WebRtcConfig {
        bind_ip: signaling_fixture::reachable_ip(),
    });
    let node = IrohDocsNode::memory_with(NodeOptions {
        relay_config: TransportRelayConfig {
            iroh_relay_urls: vec![relay.to_string()],
        },
        webrtc: Some(transport.clone()),
        ..NodeOptions::default()
    })
    .await?;
    web_e2e::seed(&node, "native").await?;
    // browser が relay で届くようになってから URL を出す。
    node.endpoint().online().await;
    let app = axum::Router::new()
        .route("/info", post(info))
        .route("/read-back", post(read_back))
        .with_state((node, relay, transport));
    signaling_fixture::serve(app).await
}

async fn info(State((node, relay, _)): State<Peer>) -> String {
    format!("{}\n{relay}", node.endpoint().id())
}

/// relay だけで届く browser の宛先。
fn browser(body: &str, relay: &RelayUrl) -> Result<EndpointAddr> {
    let id: EndpointId = body.trim().parse()?;
    Ok(EndpointAddr::from_parts(
        id,
        [TransportAddr::Relay(relay.clone())],
    ))
}

async fn read_back(State((node, relay, transport)): State<Peer>, body: String) -> String {
    let read = async {
        let peer = browser(&body, &relay)?;
        let demand = web_e2e::demand(&node, peer.clone()).await?;
        web_e2e::read_over_custom(&node, &transport, &demand, peer, "browser").await
    };
    read.await
        .unwrap_or_else(|error| format!("error: {error:#}"))
}
