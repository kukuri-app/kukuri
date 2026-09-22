use super::super::*;

#[derive(Clone)]
pub(crate) struct StaticTransport {
    pub(crate) peers: Arc<TokioMutex<PeerSnapshot>>,
    hints: Arc<TokioMutex<HashMap<String, broadcast::Sender<HintEnvelope>>>>,
    local_ticket: String,
}

impl StaticTransport {
    pub(crate) fn new(peers: PeerSnapshot) -> Self {
        Self {
            peers: Arc::new(TokioMutex::new(peers)),
            hints: Arc::new(TokioMutex::new(HashMap::new())),
            local_ticket: "static-peer".into(),
        }
    }

    pub(crate) async fn hint_sender(&self, topic: &TopicId) -> broadcast::Sender<HintEnvelope> {
        let mut guard = self.hints.lock().await;
        guard
            .entry(topic.as_str().to_string())
            .or_insert_with(|| broadcast::channel(64).0)
            .clone()
    }
}

#[derive(Clone, Default)]
pub(crate) struct AssistedDocsSync {
    peer_ids: Vec<String>,
}

impl AssistedDocsSync {
    pub(crate) fn new(peer_ids: Vec<&str>) -> Self {
        Self {
            peer_ids: peer_ids.into_iter().map(str::to_string).collect(),
        }
    }
}

#[async_trait]
impl DocsSync for AssistedDocsSync {
    async fn open_replica(&self, _replica_id: &ReplicaId) -> Result<()> {
        Ok(())
    }

    async fn apply_doc_op(&self, _replica_id: &ReplicaId, _op: DocOp) -> Result<()> {
        Ok(())
    }

    async fn query_replica_with_policy(
        &self,
        _replica_id: &ReplicaId,
        _query: DocQuery,
        _policy: kukuri_docs_sync::DocFetchPolicy,
    ) -> Result<Vec<kukuri_docs_sync::DocRecord>> {
        Ok(Vec::new())
    }

    async fn query_replica_keys(
        &self,
        _replica_id: &ReplicaId,
        _query: kukuri_docs_sync::DocKeyQuery,
    ) -> Result<kukuri_docs_sync::DocKeyPage> {
        Ok(kukuri_docs_sync::DocKeyPage::default())
    }

    async fn subscribe_replica(
        &self,
        _replica_id: &ReplicaId,
    ) -> Result<kukuri_docs_sync::DocEventStream> {
        let (sender, _) = broadcast::channel::<kukuri_docs_sync::DocEvent>(1);
        let stream = BroadcastStream::new(sender.subscribe())
            .filter_map(|item| async move { item.ok().map(Ok) });
        Ok(Box::pin(stream))
    }

    async fn import_peer_ticket(&self, _ticket: &str) -> Result<()> {
        Ok(())
    }

    async fn assist_peer_ids(&self) -> Result<Vec<String>> {
        Ok(self.peer_ids.clone())
    }
}

#[derive(Clone, Default)]
pub(crate) struct TrackingDocsSync {
    pub(crate) restarted_replicas: Arc<TokioMutex<Vec<String>>>,
    pub(crate) subscribe_replicas: Arc<TokioMutex<Vec<String>>>,
}

#[async_trait]
impl DocsSync for TrackingDocsSync {
    async fn open_replica(&self, _replica_id: &ReplicaId) -> Result<()> {
        Ok(())
    }

    async fn apply_doc_op(&self, _replica_id: &ReplicaId, _op: DocOp) -> Result<()> {
        Ok(())
    }

    async fn query_replica_with_policy(
        &self,
        _replica_id: &ReplicaId,
        _query: DocQuery,
        _policy: kukuri_docs_sync::DocFetchPolicy,
    ) -> Result<Vec<kukuri_docs_sync::DocRecord>> {
        Ok(Vec::new())
    }

    async fn query_replica_keys(
        &self,
        _replica_id: &ReplicaId,
        _query: kukuri_docs_sync::DocKeyQuery,
    ) -> Result<kukuri_docs_sync::DocKeyPage> {
        Ok(kukuri_docs_sync::DocKeyPage::default())
    }

    async fn subscribe_replica(
        &self,
        replica_id: &ReplicaId,
    ) -> Result<kukuri_docs_sync::DocEventStream> {
        self.subscribe_replicas
            .lock()
            .await
            .push(replica_id.as_str().to_string());
        let (sender, _) = broadcast::channel::<kukuri_docs_sync::DocEvent>(1);
        let stream = BroadcastStream::new(sender.subscribe())
            .filter_map(|item| async move { item.ok().map(Ok) });
        Ok(Box::pin(stream))
    }

    async fn import_peer_ticket(&self, _ticket: &str) -> Result<()> {
        Ok(())
    }

    async fn restart_replica_sync(&self, replica_id: &ReplicaId) -> Result<()> {
        self.restarted_replicas
            .lock()
            .await
            .push(replica_id.as_str().to_string());
        Ok(())
    }
}

#[derive(Clone, Default)]
pub(crate) struct AssistedBlobService {
    peer_ids: Vec<String>,
}

impl AssistedBlobService {
    pub(crate) fn new(peer_ids: Vec<&str>) -> Self {
        Self {
            peer_ids: peer_ids.into_iter().map(str::to_string).collect(),
        }
    }
}

#[async_trait]
impl BlobService for AssistedBlobService {
    async fn fetch_local_blob(
        &self,
        _hash: &kukuri_core::BlobHash,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        Ok(None)
    }

    async fn put_blob(&self, _data: Vec<u8>, mime: &str) -> Result<StoredBlob> {
        Ok(StoredBlob {
            hash: kukuri_core::BlobHash::new("test-hash"),
            mime: mime.to_string(),
            bytes: 0,
        })
    }

    async fn fetch_blob(&self, _hash: &kukuri_core::BlobHash) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }

    async fn pin_blob(&self, _hash: &kukuri_core::BlobHash) -> Result<()> {
        Ok(())
    }

    async fn blob_status(&self, _hash: &kukuri_core::BlobHash) -> Result<BlobStatus> {
        Ok(BlobStatus::Missing)
    }

    async fn local_blob_status(&self, _hash: &kukuri_core::BlobHash) -> Result<BlobStatus> {
        Ok(BlobStatus::Missing)
    }

    async fn import_peer_ticket(&self, _ticket: &str) -> Result<()> {
        Ok(())
    }

    async fn assist_peer_ids(&self) -> Result<Vec<String>> {
        Ok(self.peer_ids.clone())
    }
}
#[async_trait]
impl Transport for StaticTransport {
    async fn peers(&self) -> Result<PeerSnapshot> {
        Ok(self.peers.lock().await.clone())
    }

    async fn export_ticket(&self) -> Result<Option<String>> {
        Ok(Some(self.local_ticket.clone()))
    }

    async fn import_ticket(&self, _ticket: &str) -> Result<()> {
        Ok(())
    }
}

#[async_trait]
impl HintTransport for StaticTransport {
    async fn subscribe_hints(&self, topic: &TopicId) -> Result<HintStream> {
        let sender = self.hint_sender(topic).await;
        let stream =
            BroadcastStream::new(sender.subscribe()).filter_map(|item| async move { item.ok() });
        Ok(Box::pin(stream))
    }

    async fn unsubscribe_hints(&self, _topic: &TopicId) -> Result<()> {
        Ok(())
    }

    async fn publish_hint(&self, topic: &TopicId, hint: GossipHint) -> Result<()> {
        let sender = self.hint_sender(topic).await;
        let _ = sender.send(HintEnvelope {
            hint,
            received_at: Utc::now().timestamp_millis(),
            source_peer: "static".into(),
        });
        Ok(())
    }
}
#[derive(Clone)]
pub(crate) struct NoopHintTransport;

#[async_trait]
impl HintTransport for NoopHintTransport {
    async fn subscribe_hints(&self, _topic: &TopicId) -> Result<HintStream> {
        Ok(Box::pin(futures_util::stream::empty()))
    }

    async fn unsubscribe_hints(&self, _topic: &TopicId) -> Result<()> {
        Ok(())
    }

    async fn publish_hint(&self, _topic: &TopicId, _hint: GossipHint) -> Result<()> {
        Ok(())
    }
}

#[derive(Clone, Default)]
pub(crate) struct CountingClosingHintTransport {
    pub(crate) subscribe_count: Arc<TokioMutex<usize>>,
}

#[async_trait]
impl HintTransport for CountingClosingHintTransport {
    async fn subscribe_hints(&self, _topic: &TopicId) -> Result<HintStream> {
        *self.subscribe_count.lock().await += 1;
        Ok(Box::pin(futures_util::stream::empty()))
    }

    async fn unsubscribe_hints(&self, _topic: &TopicId) -> Result<()> {
        Ok(())
    }

    async fn publish_hint(&self, _topic: &TopicId, _hint: GossipHint) -> Result<()> {
        Ok(())
    }
}

#[derive(Clone, Default)]
pub(crate) struct TrackingHintTransport {
    hints: Arc<TokioMutex<HashMap<String, broadcast::Sender<HintEnvelope>>>>,
    pub(crate) subscribe_count: Arc<TokioMutex<usize>>,
    pub(crate) unsubscribed_topics: Arc<TokioMutex<Vec<String>>>,
}

impl TrackingHintTransport {
    pub(crate) async fn hint_sender(&self, topic: &TopicId) -> broadcast::Sender<HintEnvelope> {
        let mut guard = self.hints.lock().await;
        guard
            .entry(topic.as_str().to_string())
            .or_insert_with(|| broadcast::channel(64).0)
            .clone()
    }
}

#[async_trait]
impl HintTransport for TrackingHintTransport {
    async fn subscribe_hints(&self, topic: &TopicId) -> Result<HintStream> {
        *self.subscribe_count.lock().await += 1;
        let sender = self.hint_sender(topic).await;
        let stream =
            BroadcastStream::new(sender.subscribe()).filter_map(|item| async move { item.ok() });
        Ok(Box::pin(stream))
    }

    async fn unsubscribe_hints(&self, topic: &TopicId) -> Result<()> {
        self.unsubscribed_topics
            .lock()
            .await
            .push(topic.as_str().to_string());
        Ok(())
    }

    async fn publish_hint(&self, topic: &TopicId, hint: GossipHint) -> Result<()> {
        let sender = self.hint_sender(topic).await;
        let _ = sender.send(HintEnvelope {
            hint,
            received_at: Utc::now().timestamp_millis(),
            source_peer: "tracking".into(),
        });
        Ok(())
    }
}
