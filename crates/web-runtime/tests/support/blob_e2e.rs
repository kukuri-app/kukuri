//! browser↔native の blob の送受信の試験（#1215 W2 AC-2）で、両端が使う blob と custom path の待ち方。
//! `examples/web_blob_peer.rs`（native）と `src/browser_tests.rs`（browser）が `#[path]` で読み込む。

use std::time::Duration;

use anyhow::{Context as _, Result};
use iroh::{EndpointAddr, endpoint::Connection};
use kukuri_iroh_node::{DOC_READ_ALPN, IrohDocsNode};

/// 1 MiB の chunk 3 つに分かれる大きさ。
pub const BLOB_BYTES: usize = 2 * 1024 * 1024 + 7;

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

/// 相手への需要の接続。交渉の前に張り、交渉で足した custom path がこの接続に入る。
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
