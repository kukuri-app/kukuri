use super::*;
use kukuri_core::{BlobHash, ChannelAudienceKind, CreatePrivateChannelInput, KukuriKeys, TopicId};
use std::collections::HashMap;

#[derive(Clone, Default)]
struct CountingDocsSync {
    inner: kukuri_docs_sync::MemoryDocsSync,
    queries: Arc<TokioMutex<Vec<(String, DocQuery)>>>,
    restarts: Arc<TokioMutex<Vec<String>>>,
    /// query が返した record(または key)の総数。replica の大きさに比例する読み出しを検出する。
    records_returned: Arc<std::sync::atomic::AtomicUsize>,
    assist_peer_ids: Vec<String>,
}

impl CountingDocsSync {
    fn with_assist_peer_ids(peer_ids: Vec<&str>) -> Self {
        Self {
            assist_peer_ids: peer_ids.into_iter().map(str::to_string).collect(),
            ..Self::default()
        }
    }

    async fn clear_queries(&self) {
        self.queries.lock().await.clear();
    }

    async fn queries(&self) -> Vec<(String, DocQuery)> {
        self.queries.lock().await.clone()
    }

    /// replica 全件走査(`objects/` の prefix 読み)の回数。
    async fn object_scans(&self) -> usize {
        self.queries
            .lock()
            .await
            .iter()
            .filter(|(_, query)| *query == DocQuery::Prefix("objects/".into()))
            .count()
    }

    async fn restarts(&self) -> usize {
        self.restarts.lock().await.len()
    }

    fn records_returned(&self) -> usize {
        self.records_returned
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    fn reset_records_returned(&self) {
        self.records_returned
            .store(0, std::sync::atomic::Ordering::SeqCst);
    }
}

#[async_trait]
impl DocsSync for CountingDocsSync {
    async fn open_replica(&self, replica_id: &ReplicaId) -> Result<()> {
        self.inner.open_replica(replica_id).await
    }

    async fn register_private_replica_secret(
        &self,
        replica_id: &ReplicaId,
        namespace_secret_hex: &str,
    ) -> Result<()> {
        self.inner
            .register_private_replica_secret(replica_id, namespace_secret_hex)
            .await
    }

    async fn remove_private_replica_secret(&self, replica_id: &ReplicaId) -> Result<()> {
        self.inner.remove_private_replica_secret(replica_id).await
    }

    async fn apply_doc_op(&self, replica_id: &ReplicaId, op: DocOp) -> Result<()> {
        self.inner.apply_doc_op(replica_id, op).await
    }

    async fn query_replica_with_policy(
        &self,
        replica_id: &ReplicaId,
        query: DocQuery,
        _policy: kukuri_docs_sync::DocFetchPolicy,
    ) -> Result<Vec<kukuri_docs_sync::DocRecord>> {
        self.queries
            .lock()
            .await
            .push((replica_id.as_str().to_string(), query.clone()));
        let records = self.inner.query_replica(replica_id, query).await?;
        self.records_returned
            .fetch_add(records.len(), std::sync::atomic::Ordering::SeqCst);
        Ok(records)
    }

    async fn query_replica_keys(
        &self,
        replica_id: &ReplicaId,
        query: kukuri_docs_sync::DocKeyQuery,
    ) -> Result<Vec<kukuri_docs_sync::DocKeyEntry>> {
        let entries = self.inner.query_replica_keys(replica_id, query).await?;
        self.records_returned
            .fetch_add(entries.len(), std::sync::atomic::Ordering::SeqCst);
        Ok(entries)
    }

    async fn subscribe_replica(
        &self,
        replica_id: &ReplicaId,
    ) -> Result<kukuri_docs_sync::DocEventStream> {
        self.inner.subscribe_replica(replica_id).await
    }

    async fn import_peer_ticket(&self, ticket: &str) -> Result<()> {
        self.inner.import_peer_ticket(ticket).await
    }

    async fn assist_peer_ids(&self) -> Result<Vec<String>> {
        Ok(self.assist_peer_ids.clone())
    }

    async fn restart_replica_sync(&self, replica_id: &ReplicaId) -> Result<()> {
        self.restarts
            .lock()
            .await
            .push(replica_id.as_str().to_string());
        Ok(())
    }
}

#[derive(Clone, Default)]
struct HangingRemoteOnMissDocsSync {
    inner: kukuri_docs_sync::MemoryDocsSync,
}

#[async_trait]
impl DocsSync for HangingRemoteOnMissDocsSync {
    async fn open_replica(&self, replica_id: &ReplicaId) -> Result<()> {
        self.inner.open_replica(replica_id).await
    }

    async fn register_private_replica_secret(
        &self,
        replica_id: &ReplicaId,
        namespace_secret_hex: &str,
    ) -> Result<()> {
        self.inner
            .register_private_replica_secret(replica_id, namespace_secret_hex)
            .await
    }

    async fn remove_private_replica_secret(&self, replica_id: &ReplicaId) -> Result<()> {
        self.inner.remove_private_replica_secret(replica_id).await
    }

    async fn apply_doc_op(&self, replica_id: &ReplicaId, op: DocOp) -> Result<()> {
        self.inner.apply_doc_op(replica_id, op).await
    }

    async fn query_replica_with_policy(
        &self,
        replica_id: &ReplicaId,
        query: DocQuery,
        policy: kukuri_docs_sync::DocFetchPolicy,
    ) -> Result<Vec<kukuri_docs_sync::DocRecord>> {
        let records = self.inner.query_replica(replica_id, query).await?;
        if records.is_empty() && policy == kukuri_docs_sync::DocFetchPolicy::LocalThenRemote {
            sleep(Duration::from_secs(30)).await;
        }
        Ok(records)
    }

    async fn subscribe_replica(
        &self,
        replica_id: &ReplicaId,
    ) -> Result<kukuri_docs_sync::DocEventStream> {
        self.inner.subscribe_replica(replica_id).await
    }

    async fn import_peer_ticket(&self, ticket: &str) -> Result<()> {
        self.inner.import_peer_ticket(ticket).await
    }
}

#[derive(Clone, Default)]
struct DelayedBlobService {
    inner: MemoryBlobService,
    remaining_misses: Arc<TokioMutex<HashMap<String, usize>>>,
}

impl DelayedBlobService {
    async fn delay_hash(&self, hash: &BlobHash, misses: usize) {
        self.remaining_misses
            .lock()
            .await
            .insert(hash.as_str().to_string(), misses);
    }
}

#[async_trait]
impl BlobService for DelayedBlobService {
    async fn put_blob(&self, data: Vec<u8>, mime: &str) -> Result<StoredBlob> {
        self.inner.put_blob(data, mime).await
    }

    async fn fetch_blob(&self, hash: &BlobHash) -> Result<Option<Vec<u8>>> {
        let mut guard = self.remaining_misses.lock().await;
        if let Some(remaining) = guard.get_mut(hash.as_str())
            && *remaining > 0
        {
            *remaining -= 1;
            return Ok(None);
        }
        drop(guard);
        self.inner.fetch_blob(hash).await
    }

    async fn pin_blob(&self, hash: &BlobHash) -> Result<()> {
        self.inner.pin_blob(hash).await
    }

    async fn blob_status(&self, hash: &BlobHash) -> Result<BlobStatus> {
        if self
            .remaining_misses
            .lock()
            .await
            .get(hash.as_str())
            .copied()
            .unwrap_or_default()
            > 0
        {
            return Ok(BlobStatus::Missing);
        }
        self.inner.blob_status(hash).await
    }

    async fn local_blob_status(&self, hash: &BlobHash) -> Result<BlobStatus> {
        if self
            .remaining_misses
            .lock()
            .await
            .get(hash.as_str())
            .copied()
            .unwrap_or_default()
            > 0
        {
            return Ok(BlobStatus::Missing);
        }
        self.inner.local_blob_status(hash).await
    }

    async fn import_peer_ticket(&self, ticket: &str) -> Result<()> {
        self.inner.import_peer_ticket(ticket).await
    }
}

#[cfg(feature = "iroh-integration-tests")]
async fn iroh_sync_diagnostics(
    app_a: &AppService,
    app_b: &AppService,
    stack_a: &TestIrohStack,
    stack_b: &TestIrohStack,
    topic: &str,
) -> String {
    let snapshot_a = app_a
        .get_sync_status()
        .await
        .map(|status| format_sync_snapshot(&status, topic))
        .unwrap_or_else(|error| format!("failed to read sync status a: {error}"));
    let snapshot_b = app_b
        .get_sync_status()
        .await
        .map(|status| format_sync_snapshot(&status, topic))
        .unwrap_or_else(|error| format!("failed to read sync status b: {error}"));
    let timeline_a = app_a
        .list_timeline(topic, None, 20)
        .await
        .map(|timeline| {
            timeline
                .items
                .into_iter()
                .map(|post| post.object_id)
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|error| vec![format!("timeline a error: {error}")]);
    let timeline_b = app_b
        .list_timeline(topic, None, 20)
        .await
        .map(|timeline| {
            timeline
                .items
                .into_iter()
                .map(|post| post.object_id)
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|error| vec![format!("timeline b error: {error}")]);
    let notifications_a = app_a
        .list_notifications()
        .await
        .map(|items| {
            items
                .into_iter()
                .map(|item| {
                    format!(
                        "{}:{:?}:{}",
                        item.notification_id,
                        item.kind,
                        item.object_id.unwrap_or_else(|| "-".into())
                    )
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|error| vec![format!("notifications a error: {error}")]);
    let remote_info_a = stack_a
        ._node
        .endpoint()
        .remote_info(stack_b._node.endpoint().id())
        .await
        .is_some();
    let remote_info_b = stack_b
        ._node
        .endpoint()
        .remote_info(stack_a._node.endpoint().id())
        .await
        .is_some();
    format!(
        "snapshot_a={snapshot_a}; snapshot_b={snapshot_b}; remote_info_a={remote_info_a}; remote_info_b={remote_info_b}; timeline_a={timeline_a:?}; timeline_b={timeline_b:?}; notifications_a={notifications_a:?}"
    )
}

fn app_with_hanging_remote_docs(
    store: Arc<MemoryStore>,
    docs_sync: Arc<HangingRemoteOnMissDocsSync>,
    blob_service: Arc<MemoryBlobService>,
    keys: KukuriKeys,
) -> AppService {
    app_service_from_dependencies(
        store.clone(),
        store,
        Arc::new(StaticTransport::new(PeerSnapshot::default())),
        Arc::new(NoopHintTransport),
        docs_sync,
        blob_service,
        keys,
    )
}

mod diagnostics;
mod gossip_toggle;
mod hint_rehydration;
mod hydration_integrity;
mod hydration_integrity_contract;
mod hydration_limits;
mod scale_independence;
mod subscription_restarts;
#[cfg(feature = "iroh-integration-tests")]
mod transport_replication;
mod withdrawal_reflection;
