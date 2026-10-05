//! browser↔native の試験（W1 AC-2 の有界な読み出し、#1422 W10 AC-1 の交渉）で、両端が使う固定の replica と読み方。
//! `examples/web_peer.rs`（native）と `src/browser_tests.rs`（browser）が `#[path]` で読み込み、
//! 親の module が `DocReadQuery`・`DocReadResponse`・`IrohDocsNode` を use しておく。

use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use iroh::{EndpointAddr, endpoint::Connection};
use iroh_docs::{Capability, NamespaceSecret};
use kukuri_core::ReplicaId;
use kukuri_webrtc_transport::WebRtcTransport;

use super::{DOC_READ_ALPN, DocReadQuery, DocReadResponse, IrohDocsNode};

pub const PREFIX: &str = "indexes/timeline/";
/// 最新の record の埋め草。読み出しが custom path を通ったことを、受け取った bytes で判別できる大きさにする。
pub const NEWEST_PADDING: usize = 32 * 1024;

fn replica(side: &str) -> (ReplicaId, NamespaceSecret) {
    let replica = ReplicaId::new(format!("topic::kukuri:topic:web-e2e-{side}"));
    // 公開の replica の namespace（docs-sync の公開の導出と同じ式）。
    let secret = NamespaceSecret::from_bytes(
        blake3::hash(format!("kukuri-docs:{}", replica.as_str()).as_bytes()).as_bytes(),
    );
    (replica, secret)
}

/// `side` の公開の replica に `<side>-1`〜`<side>-3` の 3 件を置く（最新の 1 件は埋め草つき）。
pub async fn seed(node: &IrohDocsNode, side: &str) -> Result<()> {
    let (_, secret) = replica(side);
    let doc = node
        .docs()
        .import_namespace(Capability::Write(secret))
        .await?;
    let author = node.docs().author_default().await?;
    for index in 1..=3 {
        let padding = if index == 3 { NEWEST_PADDING } else { 0 };
        doc.set_bytes(
            author,
            format!("{PREFIX}000{index}").into_bytes(),
            format!("{side}-{index}{}", ".".repeat(padding)).into_bytes(),
        )
        .await?;
    }
    Ok(())
}

/// 相手の `side` の replica を、上限 2 件の key の page と 1 件の record で読み、結果を 1 行にする。
pub async fn read_newest(node: &IrohDocsNode, peer: EndpointAddr, side: &str) -> Result<String> {
    let (replica, secret) = replica(side);
    let page = DocReadQuery::Keys {
        prefix: PREFIX.into(),
        descending: true,
        limit: 2,
        author: None,
    };
    let DocReadResponse::Keys {
        entries,
        reached_limit,
    } = node
        .query_remote_docs(peer.clone(), &replica, &secret, page)
        .await?
    else {
        bail!("expected a key page");
    };
    let newest = entries.first().context("the page is empty")?.key.clone();
    let record = DocReadQuery::Exact {
        key: newest,
        limit: 1,
        author: None,
    };
    let DocReadResponse::Records(records) = node
        .query_remote_docs(peer, &replica, &secret, record)
        .await?
    else {
        bail!("expected records");
    };
    let keys: Vec<_> = entries.iter().map(|entry| entry.key.as_str()).collect();
    let value = String::from_utf8(records.first().context("no record")?.value.clone())?;
    Ok(format!(
        "{} reached_limit={reached_limit} value={} bytes={}",
        keys.join(","),
        value.trim_end_matches('.'),
        value.len()
    ))
}

/// 相手への需要の接続。交渉の前に張り、交渉で足した custom path がこの接続に入る。
pub async fn demand(node: &IrohDocsNode, peer: EndpointAddr) -> Result<Connection> {
    Ok(node.endpoint().connect(peer, DOC_READ_ALPN).await?)
}

/// 交渉の後、需要の接続の選ばれた path が custom へ移るのを期限まで待ち、`read_newest` で読む。読み出しの間に
/// `transport`（custom path）で最新の record の埋め草以上を受け取ったか（`via_custom`）を添える。
pub async fn read_over_custom(
    node: &IrohDocsNode,
    transport: &WebRtcTransport,
    demand: &Connection,
    peer: EndpointAddr,
    side: &str,
) -> Result<String> {
    n0_future::time::timeout(Duration::from_secs(20), async {
        while !demand
            .paths()
            .iter()
            .any(|path| path.is_selected() && path.remote_addr().is_custom())
        {
            n0_future::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .context("the demand connection did not move to the custom path")?;
    let before = transport.stats().received_bytes;
    let read = read_newest(node, peer, side).await?;
    let received = transport.stats().received_bytes - before;
    Ok(format!(
        "{read} via_custom={}",
        received >= NEWEST_PADDING as u64
    ))
}
