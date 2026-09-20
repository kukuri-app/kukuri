use std::collections::{BTreeSet, HashMap};
use std::str::FromStr;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use futures_util::StreamExt;
use iroh::EndpointAddr;
use iroh_docs::api::Doc;
use iroh_docs::store::{Query, SortBy, SortDirection};
use iroh_docs::{Capability, DocTicket, NamespaceSecret};
use kukuri_core::ReplicaId;
use kukuri_transport::{PeerAddrBook, RemoteFetchRetryState, SeedPeer, parse_endpoint_ticket};
use tokio::sync::{Mutex, broadcast};
use tokio::task::JoinHandle;
use tokio_stream::wrappers::BroadcastStream;
use tracing::info;

use crate::access::parse_namespace_secret_hex;
use crate::replicas::public_replica_secret;
use crate::types::{
    DocEvent, DocEventStream, DocFetchPolicy, DocKeyEntry, DocKeyOrder, DocKeyQuery, DocOp,
    DocQuery, DocRecord, DocsSync,
};
use kukuri_iroh_node::{IrohDocsNode, remote_fetch};

struct ReplicaHandle {
    doc: Doc,
    events: broadcast::Sender<DocEvent>,
    sync_peer_ids: BTreeSet<String>,
    live_task: JoinHandle<()>,
}

#[derive(Clone, Debug, Default)]
pub struct DocsPeerState {
    pub learned_peers: Vec<EndpointAddr>,
    pub imported_peers: Vec<EndpointAddr>,
}

#[derive(Clone)]
pub struct IrohDocsSync {
    node: Arc<IrohDocsNode>,
    replicas: Arc<Mutex<HashMap<String, ReplicaHandle>>>,
    // ピア台帳・接続候補・リトライ状態は kukuri-transport の共通実装(WP-H2)。
    // 台帳変化(bool)を見て reapply_sync_peers を呼ぶのはこちら側の責務。
    peers: Arc<PeerAddrBook>,
    private_replica_secrets: Arc<Mutex<HashMap<String, NamespaceSecret>>>,
    remote_fetch_retries: Arc<Mutex<RemoteFetchRetryState>>,
}

impl IrohDocsSync {
    pub fn new(node: Arc<IrohDocsNode>) -> Self {
        let peers = Arc::new(PeerAddrBook::new(node.endpoint().clone(), node.discovery()));
        Self {
            node,
            replicas: Arc::new(Mutex::new(HashMap::new())),
            peers,
            private_replica_secrets: Arc::new(Mutex::new(HashMap::new())),
            remote_fetch_retries: Arc::new(Mutex::new(RemoteFetchRetryState::default())),
        }
    }

    pub async fn shutdown(&self) {
        let handles = {
            let mut replicas = self.replicas.lock().await;
            replicas
                .drain()
                .map(|(_, handle)| handle)
                .collect::<Vec<_>>()
        };
        for handle in handles {
            handle.live_task.abort();
        }
    }

    async fn sync_peers(&self) -> Vec<EndpointAddr> {
        self.peers.merged_peers().await
    }

    pub async fn peer_state(&self) -> DocsPeerState {
        DocsPeerState {
            learned_peers: self.peers.learned_peers_snapshot().await,
            imported_peers: self.peers.imported_peers_snapshot().await,
        }
    }

    pub async fn restore_peer_state(&self, state: DocsPeerState) -> Result<()> {
        for endpoint_addr in state.learned_peers {
            let _ = self.peers.insert_learned_peer_addr(endpoint_addr).await;
        }
        for endpoint_addr in state.imported_peers {
            self.peers.insert_imported_peer_addr(endpoint_addr).await;
        }
        // 復元後の reapply は docs-sync 側の責務(共通台帳は行わない)。
        self.reapply_sync_peers().await
    }

    // 本体ループは remote_fetch(WP-B14)へ移設。characterization テスト専用に残す。
    #[cfg(test)]
    async fn connect_candidates(&self, imported_peer: &EndpointAddr) -> Vec<EndpointAddr> {
        self.peers.connect_candidates(imported_peer).await
    }

    pub(crate) async fn available_sync_peer_ids(&self) -> Vec<String> {
        self.peers.available_peer_ids().await
    }

    async fn reapply_sync_peers(&self) -> Result<()> {
        let peers = self.sync_peers().await;
        let peer_ids = peers
            .iter()
            .map(|peer| peer.id.to_string())
            .collect::<BTreeSet<_>>();
        let mut replicas = self.replicas.lock().await;
        for handle in replicas.values_mut() {
            doc_start_sync(&handle.doc, peers.clone()).await?;
            handle.sync_peer_ids = peer_ids.clone();
        }
        Ok(())
    }

    async fn replica_secret(&self, replica_id: &ReplicaId) -> Result<NamespaceSecret> {
        if let Some(secret) = self
            .private_replica_secrets
            .lock()
            .await
            .get(replica_id.as_str())
            .cloned()
        {
            return Ok(secret);
        }
        public_replica_secret(replica_id)
            .ok_or_else(|| anyhow!("private replica capability is not registered"))
    }

    async fn ensure_replica(&self, replica_id: &ReplicaId) -> Result<Doc> {
        let imported = self.sync_peers().await;
        let imported_ids = imported
            .iter()
            .map(|peer| peer.id.to_string())
            .collect::<BTreeSet<_>>();

        if let Some(handle) = self.replicas.lock().await.get_mut(replica_id.as_str()) {
            if handle.sync_peer_ids != imported_ids && !imported.is_empty() {
                doc_start_sync(&handle.doc, imported.clone()).await?;
                handle.sync_peer_ids = imported_ids;
            }
            return Ok(handle.doc.clone());
        }

        let secret = self.replica_secret(replica_id).await?;
        let doc = if imported.is_empty() {
            self.node
                .docs()
                .import_namespace(Capability::Write(secret))
                .await?
        } else {
            self.node
                .docs()
                .import(DocTicket {
                    capability: Capability::Write(secret),
                    nodes: imported.clone(),
                })
                .await?
        };
        doc_start_sync(&doc, imported.clone()).await?;
        let (tx, _) = broadcast::channel(256);
        let mut live = doc.subscribe().await?;
        let live_replica = replica_id.clone();
        let live_events = tx.clone();
        let task = tokio::spawn(async move {
            while let Some(item) = live.next().await {
                if let Ok(event) = item {
                    match event {
                        iroh_docs::engine::LiveEvent::InsertLocal { entry } => {
                            let _ = live_events.send(DocEvent {
                                replica_id: live_replica.clone(),
                                key: String::from_utf8_lossy(entry.key()).to_string(),
                                content_hash: entry.content_hash().to_string(),
                                source_peer: None,
                            });
                        }
                        iroh_docs::engine::LiveEvent::InsertRemote { from, entry, .. } => {
                            let _ = live_events.send(DocEvent {
                                replica_id: live_replica.clone(),
                                key: String::from_utf8_lossy(entry.key()).to_string(),
                                content_hash: entry.content_hash().to_string(),
                                source_peer: Some(from.to_string()),
                            });
                        }
                        _ => {}
                    }
                }
            }
        });
        self.replicas.lock().await.insert(
            replica_id.as_str().to_string(),
            ReplicaHandle {
                doc: doc.clone(),
                events: tx,
                sync_peer_ids: imported_ids,
                live_task: task,
            },
        );
        Ok(doc)
    }

    async fn sender(&self, replica_id: &ReplicaId) -> Result<broadcast::Sender<DocEvent>> {
        self.ensure_replica(replica_id).await?;
        let guard = self.replicas.lock().await;
        let sender = guard
            .get(replica_id.as_str())
            .map(|handle| handle.events.clone())
            .context("missing replica sender")?;
        Ok(sender)
    }

    async fn fetch_entry_bytes(
        &self,
        content_hash: &str,
        policy: DocFetchPolicy,
    ) -> Result<Option<Vec<u8>>> {
        let hash = iroh_blobs::Hash::from_str(content_hash)?;
        match self.node.blobs().blobs().get_bytes(hash).await {
            Ok(bytes) => Ok(Some(bytes.to_vec())),
            Err(error) => {
                if policy == DocFetchPolicy::LocalOnly {
                    info!(
                            hash = %content_hash,
                            error = %error,
                            "docs entry fetch local miss under local-only policy"
                    );
                    return Ok(None);
                }
                // ループ本体(cooldown ゲート込み)は iroh-node の共通実装(WP-B14)。
                remote_fetch::fetch_bytes_with_cooldown(
                    &self.node,
                    &self.peers,
                    &self.remote_fetch_retries,
                    "docs entry",
                    content_hash,
                    hash,
                    error,
                )
                .await
            }
        }
    }

    /// query の結果を record にする。entry の本体が取得できない record は飛ばす。
    async fn collect_records(
        &self,
        replica_id: &ReplicaId,
        query: Query,
        policy: DocFetchPolicy,
    ) -> Result<Vec<DocRecord>> {
        let doc = self.ensure_replica(replica_id).await?;
        let stream = doc.get_many(query).await?;
        tokio::pin!(stream);
        let mut records = Vec::new();
        while let Some(entry) = stream.next().await {
            let entry = entry?;
            let key = String::from_utf8(entry.key().to_vec()).context("docs key is not utf8")?;
            let content_hash = entry.content_hash().to_string();
            let Some(value) = self
                .fetch_entry_bytes(content_hash.as_str(), policy)
                .await?
            else {
                continue;
            };
            records.push(DocRecord {
                key,
                value,
                content_hash,
                content_len: entry.content_len(),
            });
        }
        Ok(records)
    }
}

/// #1239: 必ず key の索引(`SortBy::KeyAuthor`)で読む query を組む。
///
/// iroh-docs は、著者の指定が無い既定の並び(`AuthorKey`)の query を namespace 全体の table scan として
/// 実行する。key を 1 つ指定した読み出しでも replica の総 entry 数に比例してしまうため、既定の並びを使わない。
pub(crate) fn indexed_query(query: DocQuery) -> Query {
    let builder = match query {
        DocQuery::Exact(key) => Query::key_exact(key),
        DocQuery::Prefix(prefix) => Query::key_prefix(prefix),
        DocQuery::All => Query::all(),
    };
    builder
        .sort_by(SortBy::KeyAuthor, SortDirection::Asc)
        .build()
}

/// #1248: key を 1 つ指定し、読む entry 数に上限を置く query。同じ key の entry は docs author の昇順で返る。
pub(crate) fn bounded_exact_query(key: &str, limit: usize) -> Query {
    Query::key_exact(key)
        .sort_by(SortBy::KeyAuthor, SortDirection::Asc)
        .limit(limit as u64)
        .build()
}

#[async_trait]
impl DocsSync for IrohDocsSync {
    async fn open_replica(&self, replica_id: &ReplicaId) -> Result<()> {
        let _ = self.ensure_replica(replica_id).await?;
        Ok(())
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
        if let Some(handle) = self.replicas.lock().await.remove(replica_id.as_str()) {
            handle.live_task.abort();
        }
        Ok(())
    }

    async fn apply_doc_op(&self, replica_id: &ReplicaId, op: DocOp) -> Result<()> {
        let doc = self.ensure_replica(replica_id).await?;
        let author = self.node.docs().author_default().await?;
        let sender = self.sender(replica_id).await?;

        match op {
            DocOp::SetJson { key, value } => {
                let payload = serde_json::to_vec(&value)?;
                let content_hash = doc
                    .set_bytes(author, key.as_bytes().to_vec(), payload.clone())
                    .await?;
                let _ = sender.send(DocEvent {
                    replica_id: replica_id.clone(),
                    key,
                    content_hash: content_hash.to_string(),
                    source_peer: None,
                });
            }
            DocOp::SetBytes { key, value } => {
                let content_hash = doc
                    .set_bytes(author, key.as_bytes().to_vec(), value)
                    .await?;
                let _ = sender.send(DocEvent {
                    replica_id: replica_id.clone(),
                    key,
                    content_hash: content_hash.to_string(),
                    source_peer: None,
                });
            }
            DocOp::DeletePrefix { prefix } => {
                let _ = doc.del(author, prefix.as_bytes().to_vec()).await?;
            }
        }
        Ok(())
    }

    async fn query_replica_with_policy(
        &self,
        replica_id: &ReplicaId,
        query: DocQuery,
        policy: DocFetchPolicy,
    ) -> Result<Vec<DocRecord>> {
        self.collect_records(replica_id, indexed_query(query), policy)
            .await
    }

    async fn query_replica_exact_bounded(
        &self,
        replica_id: &ReplicaId,
        key: &str,
        limit: usize,
        policy: DocFetchPolicy,
    ) -> Result<Vec<DocRecord>> {
        if limit == 0 {
            // 読む件数にかかわらず replica は開く(権限の無い private replica は失敗する)。
            let _ = self.ensure_replica(replica_id).await?;
            return Ok(Vec::new());
        }
        self.collect_records(replica_id, bounded_exact_query(key, limit), policy)
            .await
    }

    async fn query_replica_keys(
        &self,
        replica_id: &ReplicaId,
        query: DocKeyQuery,
    ) -> Result<Vec<DocKeyEntry>> {
        // `limit` が 0 でも replica は開く(`MemoryDocsSync` と同じ。権限の無い replica はここで失敗する)。
        let doc = self.ensure_replica(replica_id).await?;
        if query.limit == 0 {
            return Ok(Vec::new());
        }
        let direction = match query.order {
            DocKeyOrder::Ascending => SortDirection::Asc,
            DocKeyOrder::Descending => SortDirection::Desc,
        };
        let stream = doc
            .get_many(
                Query::key_prefix(query.prefix)
                    .sort_by(SortBy::KeyAuthor, direction)
                    .limit(query.limit as u64)
                    .build(),
            )
            .await?;
        tokio::pin!(stream);
        let mut entries = Vec::new();
        while let Some(entry) = stream.next().await {
            let entry = entry?;
            entries.push(DocKeyEntry {
                key: String::from_utf8(entry.key().to_vec()).context("docs key is not utf8")?,
                content_hash: entry.content_hash().to_string(),
                content_len: entry.content_len(),
            });
        }
        Ok(entries)
    }

    async fn subscribe_replica(&self, replica_id: &ReplicaId) -> Result<DocEventStream> {
        let sender = self.sender(replica_id).await?;
        let stream = futures_util::StreamExt::filter_map(
            BroadcastStream::new(sender.subscribe()),
            |item| async move { item.ok().map(Ok) },
        );
        Ok(Box::pin(stream))
    }

    async fn import_peer_ticket(&self, ticket: &str) -> Result<()> {
        let endpoint_addr = parse_endpoint_ticket(ticket)?;
        self.peers.insert_imported_peer_addr(endpoint_addr).await;
        self.reapply_sync_peers().await?;
        Ok(())
    }

    async fn learn_peer(&self, endpoint_id: &str) -> Result<()> {
        let relay_urls = self.node.relay_urls().await;
        // 台帳に変化があったときだけ全レプリカへ同期先を配り直す(bool は docs-sync 固有の挙動)。
        if self
            .peers
            .record_learned_peer(endpoint_id, &relay_urls)
            .await?
        {
            self.reapply_sync_peers().await?;
        }
        Ok(())
    }

    async fn restart_replica_sync(&self, replica_id: &ReplicaId) -> Result<()> {
        let peers = self.sync_peers().await;
        let peer_ids = peers
            .iter()
            .map(|peer| peer.id.to_string())
            .collect::<BTreeSet<_>>();
        let existing_doc = self
            .replicas
            .lock()
            .await
            .get(replica_id.as_str())
            .map(|handle| handle.doc.clone());
        if let Some(doc) = existing_doc
            && doc_start_sync(&doc, peers.clone()).await.is_ok()
        {
            if let Some(handle) = self.replicas.lock().await.get_mut(replica_id.as_str()) {
                handle.sync_peer_ids = peer_ids;
            }
            return Ok(());
        }
        if let Some(handle) = self.replicas.lock().await.remove(replica_id.as_str()) {
            handle.live_task.abort();
        }
        let _ = self.ensure_replica(replica_id).await?;
        Ok(())
    }

    async fn set_seed_peers(&self, peers: Vec<SeedPeer>) -> Result<()> {
        let relay_urls = self.node.relay_urls().await;
        self.peers.set_seed_peers(peers, &relay_urls).await?;
        // seed 差し替え後の reapply は docs-sync 固有の挙動(共通台帳は行わない)。
        self.reapply_sync_peers().await
    }

    async fn assist_peer_ids(&self) -> Result<Vec<String>> {
        Ok(self.available_sync_peer_ids().await)
    }
}

async fn doc_start_sync(doc: &Doc, peers: Vec<EndpointAddr>) -> Result<()> {
    doc.start_sync(peers).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use kukuri_transport::TransportNetworkConfig;
    use tempfile::tempdir;
    use tokio::time::{Duration, sleep, timeout};

    // 接続候補の順序は外部挙動(characterization、WP-H2)。
    // direct(remote_info 由来)→ relay 付き → 元の値、の優先順位を固定する。
    // blob-service 側の同名観点テストと対になる。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn connect_candidates_prefers_direct_remote_info_before_relay_hint() {
        let sender_dir = tempdir().expect("sender tempdir");
        let receiver_dir = tempdir().expect("receiver tempdir");
        let config = TransportNetworkConfig::loopback();

        let sender_node = IrohDocsNode::persistent_with_config(sender_dir.path(), config.clone())
            .await
            .expect("sender node");
        let receiver_node = IrohDocsNode::persistent_with_config(receiver_dir.path(), config)
            .await
            .expect("receiver node");

        let receiver = IrohDocsSync::new(receiver_node.clone());
        let relay_url = "https://relay.example.invalid/".parse().expect("relay url");
        let sender_addr = EndpointAddr::new(sender_node.endpoint().id()).with_relay_url(relay_url);

        let seeded = sender_node
            .endpoint()
            .connect(receiver_node.endpoint().addr(), iroh_gossip::ALPN)
            .await
            .expect("seed connection");
        drop(seeded);

        timeout(Duration::from_secs(5), async {
            loop {
                if receiver_node
                    .endpoint()
                    .remote_info(sender_node.endpoint().id())
                    .await
                    .is_some()
                {
                    return;
                }
                sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("receiver should learn sender remote info");

        let candidates = receiver.connect_candidates(&sender_addr).await;
        assert!(!candidates.is_empty());
        assert_ne!(candidates[0], sender_addr);
        assert!(candidates[0].relay_urls().next().is_none());
        assert_eq!(candidates.last(), Some(&sender_addr));

        receiver.shutdown().await;
        let _ = sender_node.shutdown().await;
        let _ = receiver_node.shutdown().await;
    }

    // リトライ状態のテストは共通実装側(kukuri-transport::peers)へ移動した(WP-H2)。

    // #1248: 同じ key には docs author ごとの entry がある。key 指定の読み出しは全部を返すので、
    // 上限つきの読み出しは query の `limit` で読む entry 数を抑える(読んでから切り詰めるのではない)。
    #[tokio::test]
    async fn exact_bounded_query_limits_the_entries_of_one_key() {
        let node = IrohDocsNode::memory().await.expect("docs node");
        let docs = IrohDocsSync::new(node.clone());
        let replica = crate::topic_replica_id("kukuri:topic:exact-bounded");
        let doc = docs.ensure_replica(&replica).await.expect("open replica");
        let key = "objects/shared/envelope";
        for index in 0..3u8 {
            let author = node.docs().author_create().await.expect("docs author");
            doc.set_bytes(author, key.as_bytes().to_vec(), vec![b'a' + index])
                .await
                .expect("write the same key as another docs author");
        }
        doc.set_bytes(
            node.docs().author_default().await.expect("default author"),
            b"objects/shared/envelope-longer".to_vec(),
            b"another key".to_vec(),
        )
        .await
        .expect("write a key that extends the exact key");

        let all = docs
            .query_replica_with_policy(
                &replica,
                DocQuery::Exact(key.into()),
                DocFetchPolicy::LocalOnly,
            )
            .await
            .expect("exact query");
        assert_eq!(all.len(), 3, "one entry per docs author");

        let bounded = docs
            .query_replica_exact_bounded(&replica, key, 2, DocFetchPolicy::LocalOnly)
            .await
            .expect("bounded exact query");
        assert_eq!(bounded.len(), 2);
        assert_eq!(
            bounded
                .iter()
                .map(|record| record.content_hash.clone())
                .collect::<Vec<_>>(),
            all.iter()
                .take(2)
                .map(|record| record.content_hash.clone())
                .collect::<Vec<_>>(),
            "the bounded query returns the head of the same order"
        );
        assert!(bounded.iter().all(|record| record.key == key));
        assert!(
            docs.query_replica_exact_bounded(&replica, key, 0, DocFetchPolicy::LocalOnly)
                .await
                .expect("zero limit")
                .is_empty()
        );

        docs.shutdown().await;
        node.shutdown().await.expect("shutdown node");
    }
}
