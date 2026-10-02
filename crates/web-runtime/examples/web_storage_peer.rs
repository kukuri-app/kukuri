//! browser↔native の保存の試験（#1215 W2 の blob、#1216 W3 の docs の record）の native 側。
//!
//! 手元の iroh relay を起動し、QUIC over WebRTC DataChannel（ADR 0057）を渡した `IrohDocsNode` で `IrohBlobService` と
//! `IrohDocsSync`（account 由来の docs author）を作り、blob を 1 つと、公開の replica に record を 3 件置く。
//! 試験だけの HTTP（`signaling_fixture`）で次を受ける。本文の 1 行目は browser の endpoint id、最後の行は custom path なら
//! `1`。custom path なら、browser への需要の接続が custom path へ移ってから読む。
//! - `POST /info`: node の endpoint id、relay の URL、置いた blob の hash
//! - `POST /fetch`（2 行目は blob の hash）: browser から blob を取得し、`storage_e2e::describe` の 1 行を返す。
//! - `POST /docs-read`: browser の replica を `storage_e2e::read_newest` で読み、custom path で record の埋め草以上を
//!   受け取ったか（`via_custom`）を添える。
//!
//! native だけで動く。

#![cfg(not(target_family = "wasm"))]

use std::sync::Arc;

use anyhow::{Context as _, Result};
use axum::{extract::State, routing::post};
use iroh::{EndpointAddr, RelayUrl, TransportAddr, endpoint::Connection};
use kukuri_blob_service::{BlobService, IrohBlobService};
use kukuri_core::{BlobHash, KukuriKeys};
use kukuri_docs_sync::IrohDocsSync;
use kukuri_iroh_node::{IrohDocsNode, NodeOptions};
use kukuri_transport::TransportRelayConfig;
use kukuri_webrtc_transport::{WebRtcConfig, WebRtcTransport, signaling_fixture};

#[path = "../tests/support/storage_e2e.rs"]
mod storage_e2e;

struct Peer {
    node: Arc<IrohDocsNode>,
    relay: RelayUrl,
    transport: Arc<WebRtcTransport>,
    blobs: IrohBlobService,
    docs: IrohDocsSync,
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
        .put_blob(storage_e2e::blob("native"), "application/octet-stream")
        .await?
        .hash;
    let docs = IrohDocsSync::new(node.clone());
    let keys = KukuriKeys::generate();
    docs.use_account_docs_author(&keys.derive_docs_author_seed(), &keys.public_key_hex())
        .await?;
    storage_e2e::write_records(&docs, "native", 1..=3).await?;
    // browser が relay で届くようになってから URL を出す。
    node.endpoint().online().await;
    let app = axum::Router::new()
        .route("/info", post(info))
        .route("/fetch", post(fetch))
        .route("/docs-read", post(docs_read))
        .with_state(Arc::new(Peer {
            node,
            relay,
            transport,
            blobs,
            docs,
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

impl Peer {
    /// 本文の browser（relay だけで届く宛先）と、custom path なら移った後の需要の接続。
    async fn browser(&self, body: &str) -> Result<(String, EndpointAddr, Option<Connection>)> {
        let id = body
            .lines()
            .next()
            .context("browser endpoint id")?
            .to_owned();
        let browser =
            EndpointAddr::from_parts(id.parse()?, [TransportAddr::Relay(self.relay.clone())]);
        let demand = if body.lines().last() == Some("1") {
            let demand = storage_e2e::demand(&self.node, browser.clone()).await?;
            storage_e2e::on_custom(&demand).await?;
            Some(demand)
        } else {
            None
        };
        Ok((id, browser, demand))
    }
}

async fn fetch(State(peer): State<Arc<Peer>>, body: String) -> String {
    let fetch = async {
        let (id, _, _demand) = peer.browser(&body).await?;
        let hash = BlobHash::new(body.lines().nth(1).context("blob hash")?.to_string());
        peer.blobs.learn_peer(&id).await?;
        peer.blobs.learn_content_source(&hash, &id).await?;
        let before = peer.transport.stats().received_bytes;
        let bytes = peer
            .blobs
            .fetch_blob_ephemeral(&hash)
            .await?
            .context("the browser did not provide the blob")?;
        let received = peer.transport.stats().received_bytes - before;
        anyhow::Ok(storage_e2e::describe(&bytes, received))
    };
    fetch
        .await
        .unwrap_or_else(|error| format!("error: {error:#}"))
}

async fn docs_read(State(peer): State<Arc<Peer>>, body: String) -> String {
    let read = async {
        let (_, browser, _demand) = peer.browser(&body).await?;
        let before = peer.transport.stats().received_bytes;
        let read = storage_e2e::read_newest(&peer.docs, browser, "browser").await?;
        let received = peer.transport.stats().received_bytes - before;
        anyhow::Ok(format!(
            "{read} via_custom={}",
            received >= storage_e2e::PADDING as u64
        ))
    };
    read.await
        .unwrap_or_else(|error| format!("error: {error:#}"))
}
