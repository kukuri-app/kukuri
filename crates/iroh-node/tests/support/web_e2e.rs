//! W1 AC-2 の browser↔native の有界な読み出しで、両端が使う固定の replica と読み方。
//! `examples/web_peer.rs`（native）と `src/browser_tests.rs`（browser）が `#[path]` で読み込み、
//! 親の module が `DocReadQuery`・`DocReadResponse`・`IrohDocsNode` を use しておく。

use anyhow::{Context as _, Result, bail};
use iroh::EndpointAddr;
use iroh_docs::{Capability, NamespaceSecret};
use kukuri_core::ReplicaId;

use super::{DocReadQuery, DocReadResponse, IrohDocsNode};

pub const PREFIX: &str = "indexes/timeline/";

fn replica(side: &str) -> (ReplicaId, NamespaceSecret) {
    let replica = ReplicaId::new(format!("topic::kukuri:topic:web-e2e-{side}"));
    // 公開の replica の namespace（docs-sync の公開の導出と同じ式）。
    let secret = NamespaceSecret::from_bytes(
        blake3::hash(format!("kukuri-docs:{}", replica.as_str()).as_bytes()).as_bytes(),
    );
    (replica, secret)
}

/// `side` の公開の replica に `<side>-1`〜`<side>-3` の 3 件を置く。
pub async fn seed(node: &IrohDocsNode, side: &str) -> Result<()> {
    let (_, secret) = replica(side);
    let doc = node
        .docs()
        .import_namespace(Capability::Write(secret))
        .await?;
    let author = node.docs().author_default().await?;
    for index in 1..=3 {
        doc.set_bytes(
            author,
            format!("{PREFIX}000{index}").into_bytes(),
            format!("{side}-{index}").into_bytes(),
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
        "{} reached_limit={reached_limit} value={value}",
        keys.join(",")
    ))
}
