use std::collections::{BTreeSet, HashMap};
use std::str::FromStr;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use futures_util::StreamExt;
use iroh::EndpointAddr;
use iroh_docs::api::Doc;
use iroh_docs::store::{Query, SortBy, SortDirection};
use iroh_docs::{Author, AuthorId, Capability, DocTicket, NamespaceSecret};
use kukuri_core::{DocsAuthorSeed, ReplicaId};
use kukuri_transport::{PeerAddrBook, RemoteFetchRetryState, SeedPeer, parse_endpoint_ticket};
use tokio::sync::{Mutex, broadcast};
use tokio::task::JoinHandle;
use tokio_stream::wrappers::BroadcastStream;
use tracing::{info, warn};

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
    /// アカウントの署名鍵から導出した docs author(ADR 0053)。設定するまでは `None`。
    account_docs_author: Arc<Mutex<Option<AccountDocsAuthor>>>,
}

/// 導出した docs author と、それより前にこの保存場所で書き込みに使っていた docs author(端末ごとの乱数鍵)。
#[derive(Clone)]
struct AccountDocsAuthor {
    id: AuthorId,
    legacy: Vec<AuthorId>,
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
            account_docs_author: Arc::new(Mutex::new(None)),
        }
    }

    /// アカウントの署名鍵から導出した docs author を import し、既定の docs author にする(ADR 0053 §1)。戻り値はその id。
    ///
    /// 以後の書き込みは、この docs author の名義になる。それまでの docs author の鍵は保存場所に残す。その名義の entry は
    /// 旧 record として扱い、同じ key を書き直すときと prefix を消すときに、旧い名義の entry も消す
    /// (`supersede_legacy_entry`)。何度呼んでも同じ結果になる。
    pub async fn use_account_docs_author(&self, seed: &DocsAuthorSeed) -> Result<String> {
        let author = Author::from_bytes(seed.expose_secret_bytes());
        let id = author.id();
        let docs = self.node.docs();
        docs.author_import(author).await?;
        docs.author_set_default(id).await?;
        let mut legacy = Vec::new();
        let authors = docs.author_list().await?;
        tokio::pin!(authors);
        while let Some(existing) = authors.next().await {
            let existing = existing?;
            if existing != id {
                legacy.push(existing);
            }
        }
        *self.account_docs_author.lock().await = Some(AccountDocsAuthor { id, legacy });
        Ok(id.to_string())
    }

    /// 書き込みに使う docs author と、旧い名義の docs author。
    async fn write_authors(&self) -> Result<(AuthorId, Vec<AuthorId>)> {
        if let Some(account) = self.account_docs_author.lock().await.clone() {
            return Ok((account.id, account.legacy));
        }
        Ok((self.node.docs().author_default().await?, Vec::new()))
    }

    /// 旧い名義の docs author が同じ key に entry を持っていれば消す。
    ///
    /// 既定の docs author を切り替えた後に同じ key を書き直すと、その key に新旧 2 つの名義の entry が並ぶ。key だけを
    /// 指定して先頭を読む読み手が、旧い値を読まないようにする。調べるのは、旧い名義ごとに key を 1 つ(定数)。
    async fn supersede_legacy_entry(
        &self,
        doc: &Doc,
        legacy: &[AuthorId],
        key: &str,
    ) -> Result<()> {
        for author in legacy {
            if doc
                .get_exact(*author, key.as_bytes(), false)
                .await?
                .is_some()
            {
                let _ = doc.del(*author, key.as_bytes().to_vec()).await?;
            }
        }
        Ok(())
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

    pub(crate) async fn ensure_replica(&self, replica_id: &ReplicaId) -> Result<Doc> {
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
                                docs_author: Some(entry.author().to_string()),
                            });
                        }
                        iroh_docs::engine::LiveEvent::InsertRemote { from, entry, .. } => {
                            let _ = live_events.send(DocEvent {
                                replica_id: live_replica.clone(),
                                key: String::from_utf8_lossy(entry.key()).to_string(),
                                content_hash: entry.content_hash().to_string(),
                                source_peer: Some(from.to_string()),
                                docs_author: Some(entry.author().to_string()),
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
        let mut skipped_keys = 0usize;
        while let Some(entry) = stream.next().await {
            let entry = entry?;
            // 本体の取得より前に key を確かめる。飛ばす entry の本体は取得しない。
            let Some(key) = utf8_key(entry.key()) else {
                skipped_keys += 1;
                continue;
            };
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
                docs_author: Some(entry.author().to_string()),
            });
        }
        warn_skipped_keys(replica_id, skipped_keys);
        Ok(records)
    }
}

/// #1253: iroh-docs の key は任意の byte 列で、public topic の replica は topic id を知る誰もが書ける。
/// UTF-8 でない key は kukuri の key ではないので、その entry だけを飛ばす(読み出し全体を失敗させない)。
fn utf8_key(key: &[u8]) -> Option<String> {
    std::str::from_utf8(key).ok().map(str::to_string)
}

/// 飛ばした entry の記録は、読み出し 1 回につき 1 回だけ出す。key の byte 列は出さない。
fn warn_skipped_keys(replica_id: &ReplicaId, skipped_keys: usize) {
    if skipped_keys > 0 {
        warn!(
            replica = %replica_id.as_str(),
            skipped = skipped_keys,
            "docs read skipped entries whose key is not utf8"
        );
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
        let (author, legacy) = self.write_authors().await?;
        let sender = self.sender(replica_id).await?;

        match op {
            DocOp::SetJson { key, value } => {
                let payload = serde_json::to_vec(&value)?;
                let content_hash = doc
                    .set_bytes(author, key.as_bytes().to_vec(), payload.clone())
                    .await?;
                self.supersede_legacy_entry(&doc, &legacy, key.as_str())
                    .await?;
                let _ = sender.send(DocEvent {
                    replica_id: replica_id.clone(),
                    key,
                    content_hash: content_hash.to_string(),
                    source_peer: None,
                    docs_author: Some(author.to_string()),
                });
            }
            DocOp::SetBytes { key, value } => {
                let content_hash = doc
                    .set_bytes(author, key.as_bytes().to_vec(), value)
                    .await?;
                self.supersede_legacy_entry(&doc, &legacy, key.as_str())
                    .await?;
                let _ = sender.send(DocEvent {
                    replica_id: replica_id.clone(),
                    key,
                    content_hash: content_hash.to_string(),
                    source_peer: None,
                    docs_author: Some(author.to_string()),
                });
            }
            DocOp::DeletePrefix { prefix } => {
                let _ = doc.del(author, prefix.as_bytes().to_vec()).await?;
                // 旧い名義で書いた entry は、その名義でしか消せない。
                for legacy_author in legacy {
                    let _ = doc.del(legacy_author, prefix.as_bytes().to_vec()).await?;
                }
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
        if query.limit == 0 {
            return Ok(Vec::new());
        }
        let doc = self.ensure_replica(replica_id).await?;
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
        let mut skipped_keys = 0usize;
        while let Some(entry) = stream.next().await {
            let entry = entry?;
            // 飛ばした分を読み足さない(追加の query を発行しない)。返す件数が `limit` より減るだけである。
            let Some(key) = utf8_key(entry.key()) else {
                skipped_keys += 1;
                continue;
            };
            entries.push(DocKeyEntry {
                key,
                content_hash: entry.content_hash().to_string(),
                content_len: entry.content_len(),
                docs_author: Some(entry.author().to_string()),
            });
        }
        warn_skipped_keys(replica_id, skipped_keys);
        Ok(entries)
    }

    async fn local_docs_author(&self) -> Result<Option<String>> {
        Ok(self
            .account_docs_author
            .lock()
            .await
            .as_ref()
            .map(|account| account.id.to_string()))
    }

    async fn query_replica_by_author(
        &self,
        replica_id: &ReplicaId,
        docs_author: &str,
        key: &str,
        policy: DocFetchPolicy,
    ) -> Result<Option<DocRecord>> {
        // 手がかりは信用しない入力。docs author の id として読めない値は「無い」として扱う。
        let Ok(author) = AuthorId::from_str(docs_author) else {
            return Ok(None);
        };
        let doc = self.ensure_replica(replica_id).await?;
        // (namespace、docs author、key)の主 key の 1 件引き。同じ key の他の名義の entry は読まない。
        let Some(entry) = doc.get_exact(author, key.as_bytes(), false).await? else {
            return Ok(None);
        };
        let content_hash = entry.content_hash().to_string();
        let Some(value) = self
            .fetch_entry_bytes(content_hash.as_str(), policy)
            .await?
        else {
            return Ok(None);
        };
        Ok(Some(DocRecord {
            key: key.to_string(),
            value,
            content_hash,
            content_len: entry.content_len(),
            docs_author: Some(author.to_string()),
        }))
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

    // public topic の replica は topic id を知る誰もが書け、iroh-docs の key は任意の byte 列である。
    // UTF-8 でない key の entry が 1 件あっても、prefix の読み出しと key の一覧は失敗せず、
    // 読める entry を返す(ADR 0052 §2)。
    #[tokio::test]
    async fn non_utf8_key_does_not_fail_prefix_reads() {
        let node = IrohDocsNode::memory().await.expect("docs node");
        let docs = IrohDocsSync::new(node.clone());
        let replica = crate::topic_replica_id("kukuri:topic:non-utf8-key");
        let doc = docs.ensure_replica(&replica).await.expect("open replica");
        let author = node.docs().author_default().await.expect("default author");
        let valid_key = "objects/valid/envelope";
        doc.set_bytes(author, valid_key.as_bytes().to_vec(), b"valid".to_vec())
            .await
            .expect("write a valid key");
        doc.set_bytes(author, b"objects/\xff\xfe/envelope".to_vec(), b"x".to_vec())
            .await
            .expect("write a key that is not utf8");

        let records = docs
            .query_replica_with_policy(
                &replica,
                DocQuery::Prefix("objects/".into()),
                DocFetchPolicy::LocalOnly,
            )
            .await;
        let keys = docs
            .query_replica_keys(
                &replica,
                DocKeyQuery {
                    prefix: "objects/".into(),
                    order: DocKeyOrder::Ascending,
                    limit: 8,
                },
            )
            .await;
        let errors = [records.as_ref().err(), keys.as_ref().err()]
            .into_iter()
            .flatten()
            .map(|error| format!("{error:#}"))
            .collect::<Vec<_>>();
        assert!(
            errors.is_empty(),
            "reads must not fail because of one non-utf8 key: {errors:?}"
        );
        let records = records.expect("prefix query must not fail because of one non-utf8 key");
        let keys = keys.expect("key listing must not fail because of one non-utf8 key");
        assert_eq!(
            records
                .iter()
                .map(|record| record.key.as_str())
                .collect::<Vec<_>>(),
            vec![valid_key]
        );
        assert_eq!(
            keys.iter()
                .map(|entry| entry.key.as_str())
                .collect::<Vec<_>>(),
            vec![valid_key]
        );

        let exact = docs
            .query_replica_exact_bounded(&replica, valid_key, 2, DocFetchPolicy::LocalOnly)
            .await
            .expect("bounded exact query");
        assert_eq!(exact.len(), 1);

        let all = docs
            .query_replica_with_policy(&replica, DocQuery::All, DocFetchPolicy::LocalOnly)
            .await
            .expect("full query must not fail because of one non-utf8 key");
        assert_eq!(all.len(), 1);

        // UTF-8 でない key だけの prefix は空で返る。上限は読む entry 数にかかり、読み足さない。
        doc.set_bytes(author, b"indexes/\xff".to_vec(), b"x".to_vec())
            .await
            .expect("write a key that is not utf8");
        assert!(
            docs.query_replica_with_policy(
                &replica,
                DocQuery::Prefix("indexes/".into()),
                DocFetchPolicy::LocalOnly,
            )
            .await
            .expect("prefix with only non-utf8 keys")
            .is_empty()
        );
        let head = docs
            .query_replica_keys(
                &replica,
                DocKeyQuery {
                    prefix: "objects/".into(),
                    order: DocKeyOrder::Descending,
                    limit: 1,
                },
            )
            .await
            .expect("bounded key listing");
        assert!(
            head.is_empty(),
            "the non-utf8 key sorts first and the listing does not read past the limit"
        );

        docs.shutdown().await;
        node.shutdown().await.expect("shutdown node");
    }
}
