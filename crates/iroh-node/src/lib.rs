//! iroh ノードの所有権を持つ crate(WP-H2)。
//!
//! Endpoint / Router / Gossip / Docs / Blobs / discovery / relay 設定と、
//! endpoint-secret の永続化・ストア破損からの回復を `IrohDocsNode` が所有する。
//! docs-sync / blob-service / transport(部品借用)/ desktop-runtime はここに依存する。
//! かつては docs-sync が置き場所だったが、「docs-sync が基盤の持ち主」という歪みを
//! 解消するため独立させた(挙動不変の移動)。

mod dome_session;
mod legacy;
mod network_work;
mod node;
mod page_read;
mod remote_blob;
pub mod remote_fetch;

#[cfg(test)]
mod tests;

pub use dome_session::{DomeHostUnreachable, DomeSessionHandler};
pub use legacy::{LegacyStore, adopt_endpoint_secret, remove_dir_step, retire_legacy_layout};
pub use network_work::NetworkAdmissionError;
pub type DisplayAdmissionError = NetworkAdmissionError;
pub use node::IrohDocsNode;
pub use page_read::{
    DOC_READ_ALPN, DocReadKey, DocReadQuery, DocReadRecord, DocReadResponse, PrivateSecretLookup,
};

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
        let hash = hash.parse::<iroh_blobs::Hash>()?;
        Ok(self
            .blobs()
            .blobs()
            .get_bytes(hash)
            .await
            .ok()
            .map(|b| b.to_vec()))
    }
}
