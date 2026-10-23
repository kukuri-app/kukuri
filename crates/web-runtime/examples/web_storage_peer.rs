//! browser↔native の保存の試験（#1215 W2 の blob、#1216 W3 の docs の record）の native 側。
//!
//! 手元の iroh relay を起動し、QUIC over WebRTC DataChannel（ADR 0057）を渡した `IrohDocsNode` で `IrohBlobService` と
//! `IrohDocsSync`（account 由来の docs author）を作り、blob を 1 つと、公開の replica に record を 3 件置く。
//! 試験だけの HTTP（`signaling_fixture`）で次を受ける。本文の 1 行目は browser の endpoint id、最後の行は custom path なら
//! `1`。custom path なら、browser への需要の接続が custom path へ移ってから読む。
//! - `POST /info`: node の endpoint id、relay の URL、置いた blob の hash
//! - `POST /fetch`（2 行目は blob の hash）: browser から blob を取得し、`storage_e2e::describe` の 1 行を返す。
//! - `POST /docs-read`（2 行目は replica）: browser の replica を `storage_e2e::read_newest` で読み、custom path で
//!   record の埋め草以上を受け取ったか（`via_custom`）を添える。
//! - `POST /report`・`POST /report-redirect`・`POST /report-count`: 通報の送信の試験（#1214 W1 AC-5）。`/report` は
//!   受けた数を数え、`/report-redirect` は `/report` へ転送する。`/report-count` は受けた数を返す。
//! - `POST /account-key-export`（本文は passphrase）: native で生成した鍵を書き出し、export と公開鍵の 2 行を返す。
//!   `POST /account-key-import`（本文は export と passphrase の 2 行）: browser の export を取り込み、公開鍵を返す
//!   （鍵の export・import の互換。#1220 AC-2c）。
//! - `POST /holder-info`（#1910 AC-3）: この node（browser の読み先の近隣）の gossip の近隣にいる保持者が作った公開
//!   channel の topic・channel ID・保持者の endpoint ID・metadata の replica と key を 5 行で返す。
//!
//! native だけで動く。

#![cfg(not(target_family = "wasm"))]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context as _, Result};
use axum::http::{StatusCode, header};
use axum::{extract::State, routing::post};
use iroh::{EndpointAddr, RelayUrl, TransportAddr, endpoint::Connection};
use kukuri_app_api::{AppService, ScopeDisplayRequest, ScopeDisplayTarget, ServiceHandles};
use kukuri_blob_service::{BlobService, IrohBlobService};
use kukuri_core::{BlobHash, ChannelId, KukuriKeys, TimelineScope, TopicId};
use kukuri_docs_sync::{BucketReplica, BucketScope, IrohDocsSync, TimeBucket};
use kukuri_iroh_node::{IrohDocsNode, NodeOptions};
use kukuri_store::MemoryStore;
use kukuri_transport::{
    DiscoveryMode, FakeNetwork, FakeTransport, HintStream, HintTransport, IrohGossipTransport,
    SeedPeer, Transport as _, TransportNetworkConfig, TransportRelayConfig,
};
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
    reports: AtomicUsize,
    holder: Holder,
}

/// #1910 AC-3: browser の読み先の近隣（この node）の gossip の近隣にいる保持者と、その保持者が作った公開 channel の
/// metadata の置き場所。browser は保持者を知らず、この node の紹介で候補に入れる。
struct Holder {
    topic: String,
    channel_id: String,
    endpoint_id: String,
    replica: String,
    key: String,
    _node: Arc<IrohDocsNode>,
    _app: AppService,
    _bridge: Arc<IrohGossipTransport>,
    _bridge_hints: tokio::sync::Mutex<HintStream>,
}

fn gossip_transport(
    node: &Arc<IrohDocsNode>,
    relay: &RelayUrl,
) -> Result<Arc<IrohGossipTransport>> {
    Ok(Arc::new(
        IrohGossipTransport::from_shared_parts(
            node.endpoint().clone(),
            node.gossip().clone(),
            node.discovery(),
            TransportNetworkConfig::default(),
            TransportRelayConfig {
                iroh_relay_urls: vec![relay.to_string()],
            },
        )?
        .with_topic_peer_introductions(node.topic_peer_introductions()),
    ))
}

/// 保持者の node を作り、公開 topic の列を開いて gossip に居させる。この node（bridge）は保持者を seed に同じ topic の
/// hint を購読し、近隣になってから保持者が公開 channel を作る。
async fn holder(bridge_node: &Arc<IrohDocsNode>, relay: &RelayUrl) -> Result<Holder> {
    let bridge = gossip_transport(bridge_node, relay)?;
    let node = IrohDocsNode::memory_with(NodeOptions {
        relay_config: TransportRelayConfig {
            iroh_relay_urls: vec![relay.to_string()],
        },
        ..NodeOptions::default()
    })
    .await?;
    let transport = gossip_transport(&node, relay)?;
    let keys = KukuriKeys::generate();
    let docs = Arc::new(IrohDocsSync::new(node.clone()));
    docs.use_account_docs_author(&keys.derive_docs_author_seed(), &keys.public_key_hex())
        .await?;
    let store = Arc::new(MemoryStore::default());
    let app = AppService::from_handles(ServiceHandles::new(
        store.clone(),
        store,
        Arc::new(FakeTransport::new("holder", FakeNetwork::default())),
        transport,
        docs,
        Arc::new(IrohBlobService::new(node.clone())),
        keys,
    ));
    app.switch_writer(1);
    let topic = "kukuri:topic:web-introduced-holder".to_string();
    app.set_scope_display(ScopeDisplayRequest {
        observer: "holder".into(),
        target: ScopeDisplayTarget::Timeline {
            topic: topic.clone(),
            scope: TimelineScope::Public,
        },
        visible: true,
    })
    .await?;
    node.endpoint().online().await;
    bridge
        .configure_discovery(
            DiscoveryMode::StaticPeer,
            false,
            vec![SeedPeer {
                endpoint_id: node.endpoint().id().to_string(),
                addr_hint: None,
            }],
            Vec::new(),
        )
        .await?;
    let bridge_hints = bridge.subscribe_hints(&TopicId::new(&topic)).await?;
    let hint_topic = format!("hint/{topic}");
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        while !bridge
            .peers()
            .await
            .map(|peers| {
                peers
                    .topic_diagnostics
                    .iter()
                    .any(|diag| diag.topic == hint_topic && diag.peer_count > 0)
            })
            .unwrap_or(false)
        {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    })
    .await
    .context("the holder did not become a neighbor of the bridge")?;
    let channel = app
        .create_public_channel(
            &topic,
            "ブラウザが読む公開スレッド",
            "最初の投稿",
            Vec::new(),
            Vec::new(),
        )
        .await?;
    // metadata の置き場所（ADR 0068 §2: 作成時刻の bucket の `channels/list/<秒 20 桁>-<ID>/<ID>`）。
    let seconds = ChannelId::new(&channel.channel_id)
        .public_channel_created_at_ms()
        .context("public channel id")?
        .div_euclid(1_000);
    let replica = BucketReplica::new(
        BucketScope::Topic {
            topic_id: topic.clone(),
        },
        TimeBucket::from_unix_seconds(seconds)?,
    )?
    .replica_id();
    Ok(Holder {
        topic,
        key: format!(
            "channels/list/{seconds:020}-{id}/{id}",
            id = channel.channel_id
        ),
        channel_id: channel.channel_id,
        endpoint_id: node.endpoint().id().to_string(),
        replica: replica.as_str().to_string(),
        _node: node,
        _app: app,
        _bridge: bridge,
        _bridge_hints: tokio::sync::Mutex::new(bridge_hints),
    })
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
    storage_e2e::write_records(&docs, &storage_e2e::replica("native"), "native", 1..=3).await?;
    let holder = holder(&node, &relay).await?;
    // browser が relay で届くようになってから URL を出す。
    node.endpoint().online().await;
    let app = axum::Router::new()
        .route("/info", post(info))
        .route("/fetch", post(fetch))
        .route("/docs-read", post(docs_read))
        .route("/report", post(report).options(preflight))
        .route("/report-redirect", post(report_redirect).options(preflight))
        .route("/report-count", post(report_count))
        .route("/account-key-export", post(account_key_export))
        .route("/account-key-import", post(account_key_import))
        .route("/holder-info", post(holder_info))
        .with_state(Arc::new(Peer {
            node,
            relay,
            transport,
            blobs,
            docs,
            blob,
            reports: AtomicUsize::new(0),
            holder,
        }));
    signaling_fixture::serve(app).await
}

/// 通報は JSON の本文を送るので、browser は送る前に CORS の確認をする。
async fn preflight() -> impl axum::response::IntoResponse {
    [
        (header::ACCESS_CONTROL_ALLOW_METHODS, "POST"),
        (header::ACCESS_CONTROL_ALLOW_HEADERS, "content-type"),
    ]
}

async fn report(State(peer): State<Arc<Peer>>) -> &'static str {
    peer.reports.fetch_add(1, Ordering::SeqCst);
    "{}"
}

async fn report_redirect() -> impl axum::response::IntoResponse {
    (
        StatusCode::TEMPORARY_REDIRECT,
        [(header::LOCATION, "/report")],
    )
}

async fn report_count(State(peer): State<Arc<Peer>>) -> String {
    peer.reports.load(Ordering::SeqCst).to_string()
}

async fn derive(input: kukuri_core::PassphraseKdf) -> Result<[u8; 32]> {
    input.derive()
}

async fn account_key_export(passphrase: String) -> String {
    let keys = KukuriKeys::generate();
    match kukuri_core::encrypt_account_key_export(&keys, &passphrase, derive).await {
        Ok(export) => format!("{export}\n{}", keys.public_key_hex()),
        Err(error) => format!("error: {error:#}"),
    }
}

async fn account_key_import(body: String) -> String {
    let import = async {
        let (export, passphrase) = body.split_once('\n').context("export and passphrase")?;
        let keys = kukuri_core::decrypt_account_key_export(export, passphrase, derive).await?;
        anyhow::Ok(keys.public_key_hex())
    };
    import
        .await
        .unwrap_or_else(|error| format!("error: {error:#}"))
}

async fn holder_info(State(peer): State<Arc<Peer>>) -> String {
    let holder = &peer.holder;
    format!(
        "{}\n{}\n{}\n{}\n{}",
        holder.topic, holder.channel_id, holder.endpoint_id, holder.replica, holder.key
    )
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
        let replica = kukuri_core::ReplicaId::new(body.lines().nth(1).context("replica")?);
        let read = storage_e2e::read_newest(&peer.docs, browser, &replica).await?;
        let received = peer.transport.stats().received_bytes - before;
        anyhow::Ok(format!(
            "{read} via_custom={}",
            received >= storage_e2e::PADDING as u64
        ))
    };
    read.await
        .unwrap_or_else(|error| format!("error: {error:#}"))
}
