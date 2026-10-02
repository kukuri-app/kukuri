//! browser↔native の blob の送受信の試験（#1215 W2 AC-2）の native 側。
//!
//! 手元の iroh relay を起動し、QUIC over WebRTC DataChannel（ADR 0057）を渡した `IrohDocsNode` で `IrohBlobService` を
//! 作り、blob を 1 つ置く。試験だけの HTTP（`signaling_fixture`）で次を受ける。
//! - `POST /info`: node の endpoint id、relay の URL、置いた blob の hash
//! - `POST /fetch`（本文は `<browser の endpoint id>\n<hash>\n<custom path なら 1>`）: browser から blob を取得し、
//!   `blob_e2e::describe` の 1 行を返す。custom path なら、browser への需要の接続が custom path へ移ってから取得する。
//!
//! native だけで動く。

#![cfg(not(target_family = "wasm"))]

use std::sync::Arc;

use anyhow::{Context as _, Result};
use axum::{extract::State, routing::post};
use iroh::{EndpointAddr, RelayUrl, TransportAddr};
use kukuri_blob_service::{BlobService, IrohBlobService};
use kukuri_core::BlobHash;
use kukuri_iroh_node::{IrohDocsNode, NodeOptions};
use kukuri_transport::TransportRelayConfig;
use kukuri_webrtc_transport::{WebRtcConfig, WebRtcTransport, signaling_fixture};

#[path = "../tests/support/blob_e2e.rs"]
mod blob_e2e;

struct Peer {
    node: Arc<IrohDocsNode>,
    relay: RelayUrl,
    transport: Arc<WebRtcTransport>,
    blobs: IrohBlobService,
    blob: BlobHash,
}

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
    let blobs = IrohBlobService::new(node.clone());
    let blob = blobs
        .put_blob(blob_e2e::blob("native"), "application/octet-stream")
        .await?
        .hash;
    // browser が relay で届くようになってから URL を出す。
    node.endpoint().online().await;
    let app = axum::Router::new()
        .route("/info", post(info))
        .route("/fetch", post(fetch))
        .with_state(Arc::new(Peer {
            node,
            relay,
            transport,
            blobs,
            blob,
        }));
    signaling_fixture::serve(app).await
}

async fn info(State(peer): State<Arc<Peer>>) -> String {
    format!(
        "{}\n{}\n{}",
        peer.node.endpoint().id(),
        peer.relay,
        peer.blob.as_str()
    )
}

async fn fetch(State(peer): State<Arc<Peer>>, body: String) -> String {
    let fetch = async {
        let mut lines = body.lines();
        let id = lines.next().context("browser endpoint id")?;
        let hash = BlobHash::new(lines.next().context("blob hash")?.to_string());
        let browser =
            EndpointAddr::from_parts(id.parse()?, [TransportAddr::Relay(peer.relay.clone())]);
        let _demand = if lines.next() == Some("1") {
            let demand = blob_e2e::demand(&peer.node, browser).await?;
            blob_e2e::on_custom(&demand).await?;
            Some(demand)
        } else {
            None
        };
        peer.blobs.learn_peer(id).await?;
        peer.blobs.learn_content_source(&hash, id).await?;
        let before = peer.transport.stats().received_bytes;
        let bytes = peer
            .blobs
            .fetch_blob_ephemeral(&hash)
            .await?
            .context("the browser did not provide the blob")?;
        let received = peer.transport.stats().received_bytes - before;
        anyhow::Ok(blob_e2e::describe(&bytes, received))
    };
    fetch
        .await
        .unwrap_or_else(|error| format!("error: {error:#}"))
}
