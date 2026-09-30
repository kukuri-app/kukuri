//! W1 AC-2 の browser↔native の有界な読み出し（ADR 0056 §9）の native 側。
//!
//! QUIC over WebRTC DataChannel（ADR 0057）を注入した `IrohDocsNode` を起動し、公開の replica に 3 件を置く。
//! 試験だけの HTTP（`signaling_fixture`）で接続を受け、`POST /read-back`（本文は browser の endpoint id）で
//! browser の node の replica を同じ有界な reader で読み返す。native だけで動く。

#![cfg(not(target_family = "wasm"))]

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use anyhow::{Context as _, Result};
use axum::{extract::State, routing::post};
use iroh::{EndpointAddr, EndpointId, TransportAddr, endpoint::transports::CustomTransport};
use kukuri_iroh_node::{DocReadQuery, DocReadResponse, IrohDocsNode, NodeOptions};
use kukuri_webrtc_transport::{SessionEvent, WebRtcConfig, WebRtcTransport, signaling_fixture};

#[path = "../tests/support/web_e2e.rs"]
mod web_e2e;

/// DataChannel が開いた browser の宛先。
type Opened = Arc<Mutex<HashMap<EndpointId, EndpointAddr>>>;

#[tokio::main]
async fn main() -> Result<()> {
    let transport = WebRtcTransport::new(WebRtcConfig {
        bind_ip: signaling_fixture::reachable_ip(),
    });
    let mut events = transport.take_events().context("events")?;
    let custom: Arc<dyn CustomTransport> = transport.clone();
    let node = IrohDocsNode::memory_with(NodeOptions {
        custom_transports: vec![custom],
        ..NodeOptions::default()
    })
    .await?;
    web_e2e::seed(&node, "native").await?;
    let opened = Opened::default();
    let record = opened.clone();
    tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            if let SessionEvent::Opened { remote, addr, .. } = event {
                let addr = EndpointAddr::from_parts(remote, [TransportAddr::Custom(addr)]);
                record.lock().expect("opened").insert(remote, addr);
            }
        }
    });
    let app = signaling_fixture::offer_routes(transport, node.endpoint().id()).merge(
        axum::Router::new()
            .route("/read-back", post(read_back))
            .with_state((node, opened)),
    );
    signaling_fixture::serve(app).await
}

async fn read_back(
    State((node, opened)): State<(Arc<IrohDocsNode>, Opened)>,
    body: String,
) -> String {
    let read = async {
        let remote: EndpointId = body.trim().parse()?;
        let peer = opened
            .lock()
            .expect("opened")
            .get(&remote)
            .cloned()
            .context("no session with the browser")?;
        web_e2e::read_newest(&node, peer, "browser").await
    };
    read.await
        .unwrap_or_else(|error| format!("error: {error:#}"))
}
