//! browser↔native の保存の試験（#1215 W2・#1216 W3）で、両端が使う blob・docs の record と custom path の待ち方。
//! `examples/web_storage_peer.rs`（native）と `src/browser_tests.rs`（browser）が `#[path]` で読み込む。

use std::time::Duration;

use anyhow::{Context as _, Result};
use iroh::{EndpointAddr, endpoint::Connection};
use kukuri_core::ReplicaId;
use kukuri_docs_sync::{DocFetchPolicy, DocKeyOrder, DocKeyQuery, DocOp, DocsSync, IrohDocsSync};
use kukuri_iroh_node::{DOC_READ_ALPN, IrohDocsNode};

/// 1 MiB の chunk 3 つに分かれる大きさ。
pub const BLOB_BYTES: usize = 2 * 1024 * 1024 + 7;
pub const PREFIX: &str = "indexes/timeline/";
/// record の埋め草。読み出しが custom path を通ったことを、受け取った bytes で判別できる大きさにする。
pub const PADDING: usize = 32 * 1024;

/// `side` ごとに中身の違う blob。
pub fn blob(side: &str) -> Vec<u8> {
    side.bytes().cycle().take(BLOB_BYTES).collect()
}

/// 取得した内容の hash と bytes、custom path で内容の bytes 以上を受け取ったか。
pub fn describe(bytes: &[u8], received_over_custom: u64) -> String {
    format!(
        "{} {} via_custom={}",
        blake3::hash(bytes).to_hex(),
        bytes.len(),
        received_over_custom >= bytes.len() as u64
    )
}

/// `side` の公開の topic の replica。
pub fn replica(side: &str) -> ReplicaId {
    ReplicaId::new(format!("topic::kukuri:topic:web-docs-{side}"))
}

/// `side` の replica に `numbers` の record を書く（account 由来の docs author の名義。ADR 0053）。
pub async fn write_records(
    docs: &IrohDocsSync,
    side: &str,
    numbers: impl IntoIterator<Item = usize>,
) -> Result<()> {
    for number in numbers {
        let op = DocOp::SetBytes {
            key: format!("{PREFIX}{number:04}"),
            value: format!("{side}-{number}{}", ".".repeat(PADDING)).into_bytes(),
        };
        docs.apply_doc_op(&replica(side), op).await?;
    }
    Ok(())
}

/// 相手の `side` の replica を、上限 2 件の key の page と、その先頭の 1 件の record（docs author を指定）で読み、
/// 確かめた record を手元に保持して、結果を 1 行にする。読んだ後も相手の namespace が手元に無い（同期を始めない）
/// ことを添える。
pub async fn read_newest(docs: &IrohDocsSync, peer: EndpointAddr, side: &str) -> Result<String> {
    let replica = replica(side);
    let source = docs.remote_source(peer);
    let query = DocKeyQuery {
        prefix: PREFIX.into(),
        order: DocKeyOrder::Descending,
        limit: 2,
    };
    let page = source.query_replica_keys(&replica, query).await?;
    let newest = page.entries.first().context("the page is empty")?;
    let author = newest.docs_author.clone().context("docs author")?;
    let record = source
        .query_replica_by_author(
            &replica,
            &author,
            &newest.key,
            DocFetchPolicy::LocalThenRemote,
        )
        .await?
        .context("no record")?;
    source
        .persist_verified_record(&replica, &newest.key, Some(&author), &[])
        .await?;
    let keys: Vec<_> = page
        .entries
        .iter()
        .map(|entry| entry.key.as_str())
        .collect();
    let value = String::from_utf8(record.value)?;
    Ok(format!(
        "{} reached_limit={} value={} bytes={} author={author} namespace={}",
        keys.join(","),
        page.reached_limit,
        value.trim_end_matches('.'),
        value.len(),
        docs.has_local_replica(&replica).await?
    ))
}

/// 相手への需要の接続。ブラウザの node はこれを受けて交渉を始め（#1422 AC-2）、足した custom path がこの接続に入る。
pub async fn demand(node: &IrohDocsNode, peer: EndpointAddr) -> Result<Connection> {
    Ok(node.endpoint().connect(peer, DOC_READ_ALPN).await?)
}

/// 需要の接続の選ばれた path が custom（QUIC over WebRTC DataChannel）へ移るまで待つ。
pub async fn on_custom(demand: &Connection) -> Result<()> {
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
    .context("the demand connection did not move to the custom path")
}
