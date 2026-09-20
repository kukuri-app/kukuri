use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use iroh_docs::NamespaceSecret;
use kukuri_core::ReplicaId;
use tokio::sync::{Mutex, broadcast};
use tokio_stream::wrappers::BroadcastStream;

use crate::access::{ensure_private_replica_access, parse_namespace_secret_hex};
use crate::replicas::value_hash;
use crate::types::{
    DocEvent, DocEventStream, DocFetchPolicy, DocKeyEntry, DocKeyOrder, DocKeyQuery, DocOp,
    DocQuery, DocRecord, DocsSync,
};

type ReplicaRecords = HashMap<String, Vec<u8>>;
type MemoryReplicaMap = HashMap<String, ReplicaRecords>;

#[derive(Clone, Default)]
pub struct MemoryDocsSync {
    records: Arc<Mutex<MemoryReplicaMap>>,
    events: Arc<Mutex<HashMap<String, broadcast::Sender<DocEvent>>>>,
    private_replica_secrets: Arc<Mutex<HashMap<String, NamespaceSecret>>>,
    /// この docs が書き込みに使う docs author の id(ADR 0053)。既定は `None`(docs author を持たない docs)。
    docs_author: Option<String>,
}

impl MemoryDocsSync {
    /// 1 つの docs author の名義で書く docs。record・key の entry・event に、その id が付く。
    pub fn with_docs_author(docs_author: impl Into<String>) -> Self {
        Self {
            docs_author: Some(docs_author.into()),
            ..Self::default()
        }
    }
}

#[async_trait]
impl DocsSync for MemoryDocsSync {
    async fn open_replica(&self, replica_id: &ReplicaId) -> Result<()> {
        ensure_private_replica_access(replica_id, &self.private_replica_secrets).await?;
        self.records
            .lock()
            .await
            .entry(replica_id.as_str().to_string())
            .or_default();
        self.events
            .lock()
            .await
            .entry(replica_id.as_str().to_string())
            .or_insert_with(|| broadcast::channel(256).0);
        Ok(())
    }

    async fn apply_doc_op(&self, replica_id: &ReplicaId, op: DocOp) -> Result<()> {
        self.open_replica(replica_id).await?;
        let mut records = self.records.lock().await;
        let replica = records.entry(replica_id.as_str().to_string()).or_default();
        match op {
            DocOp::SetJson { key, value } => {
                let bytes = serde_json::to_vec(&value)?;
                replica.insert(key.clone(), bytes.clone());
                let _ = self
                    .events
                    .lock()
                    .await
                    .get(replica_id.as_str())
                    .cloned()
                    .context("missing events sender")?
                    .send(DocEvent {
                        replica_id: replica_id.clone(),
                        key,
                        content_hash: value_hash(bytes),
                        source_peer: None,
                        docs_author: self.docs_author.clone(),
                    });
            }
            DocOp::SetBytes { key, value } => {
                let hash = value_hash(&value);
                replica.insert(key.clone(), value);
                let _ = self
                    .events
                    .lock()
                    .await
                    .get(replica_id.as_str())
                    .cloned()
                    .context("missing events sender")?
                    .send(DocEvent {
                        replica_id: replica_id.clone(),
                        key,
                        content_hash: hash,
                        source_peer: None,
                        docs_author: self.docs_author.clone(),
                    });
            }
            DocOp::DeletePrefix { prefix } => {
                replica.retain(|key, _| !key.starts_with(prefix.as_str()));
            }
        }
        Ok(())
    }

    async fn query_replica_with_policy(
        &self,
        replica_id: &ReplicaId,
        query: DocQuery,
        _policy: DocFetchPolicy,
    ) -> Result<Vec<DocRecord>> {
        self.open_replica(replica_id).await?;
        let records = self.records.lock().await;
        let items = records
            .get(replica_id.as_str())
            .cloned()
            .unwrap_or_default();
        let mut rows = items
            .into_iter()
            .filter(|(key, _)| match &query {
                DocQuery::Exact(exact) => key == exact,
                DocQuery::Prefix(prefix) => key.starts_with(prefix.as_str()),
                DocQuery::All => true,
            })
            .map(|(key, value)| DocRecord {
                content_hash: value_hash(&value),
                content_len: value.len() as u64,
                key,
                value,
                docs_author: self.docs_author.clone(),
            })
            .collect::<Vec<_>>();
        rows.sort_by(|left, right| left.key.cmp(&right.key));
        Ok(rows)
    }

    async fn query_replica_keys(
        &self,
        replica_id: &ReplicaId,
        query: DocKeyQuery,
    ) -> Result<Vec<DocKeyEntry>> {
        self.open_replica(replica_id).await?;
        let records = self.records.lock().await;
        let mut rows = records
            .get(replica_id.as_str())
            .into_iter()
            .flatten()
            .filter(|(key, _)| key.starts_with(query.prefix.as_str()))
            .map(|(key, value)| DocKeyEntry {
                key: key.clone(),
                content_hash: value_hash(value),
                content_len: value.len() as u64,
                docs_author: self.docs_author.clone(),
            })
            .collect::<Vec<_>>();
        rows.sort_by(|left, right| match query.order {
            DocKeyOrder::Ascending => left.key.cmp(&right.key),
            DocKeyOrder::Descending => right.key.cmp(&left.key),
        });
        rows.truncate(query.limit);
        Ok(rows)
    }

    async fn local_docs_author(&self) -> Result<Option<String>> {
        Ok(self.docs_author.clone())
    }

    async fn query_replica_by_author(
        &self,
        replica_id: &ReplicaId,
        docs_author: &str,
        key: &str,
        policy: DocFetchPolicy,
    ) -> Result<Option<DocRecord>> {
        // この docs の record は、すべて `self.docs_author` の名義。他の docs author の record は無い。
        if self.docs_author.as_deref() != Some(docs_author) {
            return Ok(None);
        }
        Ok(self
            .query_replica_with_policy(replica_id, DocQuery::Exact(key.to_string()), policy)
            .await?
            .into_iter()
            .next())
    }

    async fn subscribe_replica(&self, replica_id: &ReplicaId) -> Result<DocEventStream> {
        self.open_replica(replica_id).await?;
        let sender = self
            .events
            .lock()
            .await
            .get(replica_id.as_str())
            .cloned()
            .context("missing replica events")?;
        let stream = futures_util::StreamExt::filter_map(
            BroadcastStream::new(sender.subscribe()),
            |item| async move { item.ok().map(Ok) },
        );
        Ok(Box::pin(stream))
    }

    async fn register_private_replica_secret(
        &self,
        replica_id: &ReplicaId,
        namespace_secret_hex: &str,
    ) -> Result<()> {
        let secret = parse_namespace_secret_hex(namespace_secret_hex)?;
        self.private_replica_secrets
            .lock()
            .await
            .insert(replica_id.as_str().to_string(), secret);
        Ok(())
    }

    async fn remove_private_replica_secret(&self, replica_id: &ReplicaId) -> Result<()> {
        self.private_replica_secrets
            .lock()
            .await
            .remove(replica_id.as_str());
        Ok(())
    }

    async fn import_peer_ticket(&self, _ticket: &str) -> Result<()> {
        Ok(())
    }
}
