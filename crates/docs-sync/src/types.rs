use std::pin::Pin;

use anyhow::Result;
use async_trait::async_trait;
use futures_util::Stream;
use kukuri_core::ReplicaId;
use kukuri_transport::SeedPeer;
use serde::{Deserialize, Serialize};

pub type DocEventStream = Pin<Box<dyn Stream<Item = Result<DocEvent>> + Send>>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DocOp {
    SetJson {
        key: String,
        value: serde_json::Value,
    },
    SetBytes {
        key: String,
        value: Vec<u8>,
    },
    DeletePrefix {
        prefix: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DocQuery {
    Exact(String),
    Prefix(String),
    All,
}

/// key だけを返す上限つきの読み出し(#1239)。entry の本体は読まない。
///
/// replica の総 entry 数に依存しない読み出しの基本形。全件を読む `DocQuery::Prefix` の代わりに使う。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocKeyQuery {
    pub prefix: String,
    pub order: DocKeyOrder,
    /// 返す entry の上限。0 なら何も返さない。
    pub limit: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocKeyOrder {
    Ascending,
    Descending,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocKeyEntry {
    pub key: String,
    pub content_hash: String,
    pub content_len: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DocFetchPolicy {
    LocalOnly,
    LocalThenRemote,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocRecord {
    pub key: String,
    pub value: Vec<u8>,
    pub content_hash: String,
    pub content_len: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocEvent {
    pub replica_id: ReplicaId,
    pub key: String,
    pub content_hash: String,
    pub source_peer: Option<String>,
}

#[async_trait]
pub trait DocsSync: Send + Sync {
    async fn open_replica(&self, replica_id: &ReplicaId) -> Result<()>;
    async fn register_private_replica_secret(
        &self,
        _replica_id: &ReplicaId,
        _namespace_secret_hex: &str,
    ) -> Result<()> {
        Ok(())
    }
    async fn remove_private_replica_secret(&self, _replica_id: &ReplicaId) -> Result<()> {
        Ok(())
    }
    async fn apply_doc_op(&self, replica_id: &ReplicaId, op: DocOp) -> Result<()>;
    async fn query_replica_with_policy(
        &self,
        replica_id: &ReplicaId,
        query: DocQuery,
        policy: DocFetchPolicy,
    ) -> Result<Vec<DocRecord>>;
    async fn query_replica(
        &self,
        replica_id: &ReplicaId,
        query: DocQuery,
    ) -> Result<Vec<DocRecord>> {
        self.query_replica_with_policy(replica_id, query, DocFetchPolicy::LocalThenRemote)
            .await
    }
    /// key の索引だけを使う上限つきの読み出し。
    ///
    /// 既定実装はエラーを返す。全件読みへ黙って落ちると、replica の大きさに比例する経路が戻るため、
    /// 本番の実装と、この読み出しを通る test double は必ず実装する。
    async fn query_replica_keys(
        &self,
        _replica_id: &ReplicaId,
        _query: DocKeyQuery,
    ) -> Result<Vec<DocKeyEntry>> {
        anyhow::bail!("this DocsSync implementation does not support bounded key queries")
    }
    async fn subscribe_replica(&self, replica_id: &ReplicaId) -> Result<DocEventStream>;
    async fn import_peer_ticket(&self, ticket: &str) -> Result<()>;
    async fn learn_peer(&self, _endpoint_id: &str) -> Result<()> {
        Ok(())
    }
    async fn restart_replica_sync(&self, replica_id: &ReplicaId) -> Result<()> {
        self.open_replica(replica_id).await
    }
    async fn set_seed_peers(&self, _peers: Vec<SeedPeer>) -> Result<()> {
        Ok(())
    }
    async fn assist_peer_ids(&self) -> Result<Vec<String>> {
        Ok(Vec::new())
    }
}
