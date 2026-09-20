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
            })
            .collect::<Vec<_>>();
        rows.sort_by(|left, right| match query.order {
            DocKeyOrder::Ascending => left.key.cmp(&right.key),
            DocKeyOrder::Descending => right.key.cmp(&left.key),
        });
        rows.truncate(query.limit);
        Ok(rows)
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
