//! iroh ノードの所有権を持つ crate(WP-H2)。
//!
//! Endpoint / Router / Gossip / Docs / Blobs / discovery / relay 設定と、
//! endpoint-secret の永続化・ストア破損からの回復を `IrohDocsNode` が所有する。
//! docs-sync / blob-service / transport(部品借用)/ desktop-runtime はここに依存する。
//! かつては docs-sync が置き場所だったが、「docs-sync が基盤の持ち主」という歪みを
//! 解消するため独立させた(挙動不変の移動)。
// ブラウザでも動く共用 crate（ADR 0056 §3）。tokio の時刻・task と std の時刻を直接使わない（native では
// n0_future・web_time がそれらの再公開なので、wasm32 の clippy で確かめる）。
#![cfg_attr(
    all(target_family = "wasm", not(test)),
    warn(clippy::disallowed_methods)
)]

mod account_transfer;
// 旧 store の退役は native だけ（file を使う）。
mod dome_session;
#[cfg(not(target_family = "wasm"))]
mod legacy;
mod network_work;
mod node;
mod page_read;
mod remote_blob;
pub mod remote_fetch;

#[cfg(all(test, target_family = "wasm"))]
mod browser_tests;
#[cfg(test)]
#[cfg(not(target_family = "wasm"))]
mod tests;

pub use account_transfer::{
    ACCOUNT_TRANSFER_ALPN, AccountBundleSink, AccountBundleSource, AccountBundleStaging,
    AccountHistoryPage, AccountHistoryResume, AccountHistoryStaging, AccountTransfer,
};
pub use dome_session::{DomeHostUnreachable, DomeSessionHandler};
#[cfg(not(target_family = "wasm"))]
pub use legacy::{LegacyStore, adopt_endpoint_secret, remove_dir_step, retire_legacy_layout};
pub use network_work::NetworkAdmissionError;
pub type DisplayAdmissionError = NetworkAdmissionError;
pub use node::{IrohDocsNode, NodeOptions};
pub use page_read::{
    DOC_READ_ALPN, DocReadKey, DocReadQuery, DocReadRecord, DocReadResponse, PrivateSecretLookup,
};

/// wasm の iroh-blobs は Send でない future を返す。ブラウザでは呼んだ時点で main thread の task へ
/// 移し、結果だけを channel で受け取る（ADR 0056 §4）。待つのをやめたら task も止める。
#[cfg(target_family = "wasm")]
pub fn confine_local<T: Send + 'static>(
    future: impl Future<Output = T> + 'static,
) -> impl Future<Output = T> + Send {
    let (mut sender, receiver) = tokio::sync::oneshot::channel();
    n0_future::task::spawn(async move {
        tokio::select! {
            value = future => {
                let _ = sender.send(value);
            }
            _ = sender.closed() => {}
        }
    });
    // ブラウザの task は panic で module ごと止まるので、送らずに終わることはない。
    async move {
        receiver
            .await
            .expect("a local task always sends its result")
    }
}

/// native の future は Send なので、そのまま返す。
#[cfg(not(target_family = "wasm"))]
pub fn confine_local<F: Future>(future: F) -> F {
    future
}

/// blob の hash の文字列（64 文字の hex）を読む。`iroh_blobs::Hash::from_str` は 64・52 文字以外の長さで panic する
/// （data-encoding の assert。ブラウザでは runtime の task の実行器が壊れる）ので、長さを先に確かめる（#1220 AC-4）。
pub fn parse_blob_hash(hash: &str) -> anyhow::Result<iroh_blobs::Hash> {
    anyhow::ensure!(hash.len() == 64, "invalid blob hash");
    Ok(hash.parse()?)
}

impl IrohDocsNode {
    pub async fn query_remote_docs(
        &self,
        peer: iroh::EndpointAddr,
        replica: &kukuri_core::ReplicaId,
        secret: &iroh_docs::NamespaceSecret,
        query: DocReadQuery,
    ) -> anyhow::Result<DocReadResponse> {
        use kukuri_transport::work_admission::{WorkPersistence, WorkProtocol};

        let request = page_read::Request::new(replica.as_str(), secret, query)?;
        let digest = blake3::hash(&serde_json::to_vec(&(&peer, &request))?);
        let endpoint = self.endpoint().clone();
        let waiter = self.network_work.submit_fetch(
            network_work::FetchRequest {
                identity: network_work::FetchIdentity {
                    // Blob retry services start at 1; zero belongs to this node's docs reads.
                    service: 0,
                    key: format!("docs:{digest}"),
                },
                protocol: WorkProtocol::Docs,
                object: *digest.as_bytes(),
                persistence: WorkPersistence::Ephemeral,
                byte_limit: 1024 * 1024,
                cooling_down: false,
            },
            Box::pin(async move {
                let response = page_read::fetch(&endpoint, peer, request).await?;
                Ok(Some(serde_json::to_vec(&response)?))
            }),
            Box::new(|_| Box::pin(async {})),
        )?;
        let bytes = waiter
            .result()
            .await
            .map_err(|error| anyhow::anyhow!("{error}"))?
            .ok_or_else(|| anyhow::anyhow!("docs read was cancelled"))?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    pub async fn read_local_blob(&self, hash: &str) -> anyhow::Result<Option<Vec<u8>>> {
        let hash = parse_blob_hash(hash)?;
        Ok(self
            .blobs()
            .blobs()
            .get_bytes(hash)
            .await
            .ok()
            .map(|b| b.to_vec()))
    }
}
