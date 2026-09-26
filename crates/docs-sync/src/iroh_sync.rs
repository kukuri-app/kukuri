use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use futures_util::StreamExt;
use iroh::EndpointAddr;
use iroh_docs::api::Doc;
use iroh_docs::store::{Query, SortBy, SortDirection};
use iroh_docs::{Author, AuthorId, Capability, NamespaceSecret};
use kukuri_core::{DocsAuthorSeed, ReplicaId};
use kukuri_store::SqliteStore;
use kukuri_transport::{
    BlobPeerHealth, PeerAddrBook, RemoteFetchRetryState, SeedPeer, parse_endpoint_ticket,
};
use tokio::sync::{Mutex, broadcast};
use tokio::task::{JoinHandle, JoinSet};
use tracing::{info, warn};

use crate::access::parse_namespace_secret_hex;
use crate::notices::{entry_stream, notice_stream};
use crate::replicas::public_replica_secret;
use crate::types::{
    DocEvent, DocEventStream, DocFetchPolicy, DocKeyEntry, DocKeyOrder, DocKeyPage, DocKeyQuery,
    DocOp, DocQuery, DocRecord, DocsSync, ReplicaNotice, ReplicaNoticeStream,
};

#[path = "iroh_sync_lifecycle.rs"]
mod lifecycle;
#[path = "iroh_local_source.rs"]
mod local_source;
#[path = "iroh_remote_source.rs"]
mod remote_source;
use kukuri_iroh_node::{IrohDocsNode, remote_fetch};

/// 同時に開いておく namespace の上限(ADR 0055 の docs handle 128)。書込みで開いた bucket も含む。
pub(crate) const MAX_OPEN_REPLICAS: usize = 128;

struct ReplicaHandle {
    /// 使用中の操作は clone を持つ。map だけが持つ handle が、上限を超えたときに閉じる対象になる。
    doc: Arc<Doc>,
    events: broadcast::Sender<ReplicaNotice>,
    closing: bool,
    live_task: Option<JoinHandle<()>>,
    last_used: std::time::Instant,
}

#[derive(Clone)]
pub struct IrohDocsSync {
    node: Arc<IrohDocsNode>,
    remote_cache: Option<Arc<SqliteStore>>,
    replicas: Arc<Mutex<HashMap<String, ReplicaHandle>>>,
    close_tasks: Arc<Mutex<JoinSet<()>>>,
    #[cfg(test)]
    close_hook: Arc<Mutex<Option<lifecycle::TestHook>>>,
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
            remote_cache: None,
            replicas: Arc::new(Mutex::new(HashMap::new())),
            close_tasks: Arc::new(Mutex::new(JoinSet::new())),
            #[cfg(test)]
            close_hook: Arc::new(Mutex::new(None)),
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
        // 呼出元がcloseの待機をcancelしても、開始済みの停止を完了してからnodeを終了する。
        while self.close_tasks.lock().await.join_next().await.is_some() {}
        let handles = {
            let mut replicas = self.replicas.lock().await;
            replicas
                .drain()
                .map(|(_, handle)| handle)
                .collect::<Vec<_>>()
        };
        for mut handle in handles {
            if let Some(task) = handle.live_task.take() {
                task.abort();
            }
        }
    }

    pub(crate) async fn available_sync_peer_ids(&self) -> Vec<String> {
        self.peers.available_peer_ids().await
    }

    pub(crate) async fn replica_secret(&self, replica_id: &ReplicaId) -> Result<NamespaceSecret> {
        if replica_id.as_str().starts_with("bucket::") {
            crate::BucketReplica::parse(replica_id)?;
        }
        if let Some(secret) = crate::access::registered_private_secret(
            replica_id,
            &*self.private_replica_secrets.lock().await,
        ) {
            return Ok(secret);
        }
        public_replica_secret(replica_id)
            .ok_or_else(|| anyhow!("private replica capability is not registered"))
    }

    /// 手元の namespace を開く(R5-H: 旧 sync は撤去した。開いても同期を始めない)。
    ///
    /// 開いた handle は `MAX_OPEN_REPLICAS` まで残し、超えたら使用中でない最も古いものから閉じる。日ごとの bucket へ
    /// 書き続けても、開いている handle と event の task は増え続けない。閉じた namespace の提供は、読む間だけ開く
    /// (`DocReadProtocol`)。
    pub(crate) async fn ensure_replica(&self, replica_id: &ReplicaId) -> Result<Arc<Doc>> {
        // openの競合で同じnamespaceのtaskを複数作らない。
        let mut replicas = self.replicas.lock().await;
        // revokeとopenを直列化する。mapのlock前にsecretをコピーすると、revoke完了後に再openできてしまう。
        let secret = self.replica_secret(replica_id).await?;
        if let Some(handle) = replicas.get_mut(replica_id.as_str()) {
            if handle.closing {
                anyhow::bail!("replica close is pending; retry close before reopening");
            }
            handle.last_used = std::time::Instant::now();
            return Ok(handle.doc.clone());
        }

        let doc = self
            .node
            .docs()
            .import_namespace(Capability::Write(secret))
            .await?;
        let (tx, _) = broadcast::channel(256);
        let mut live = doc.subscribe().await?;
        let live_replica = replica_id.clone();
        let live_events = tx.clone();
        let peer_observations = self.peers.clone();
        let task = tokio::spawn(async move {
            while let Some(item) = live.next().await {
                if let Ok(event) = item {
                    match event {
                        iroh_docs::engine::LiveEvent::InsertLocal { entry } => {
                            let _ = live_events.send(ReplicaNotice::Entry(DocEvent {
                                replica_id: live_replica.clone(),
                                key: String::from_utf8_lossy(entry.key()).to_string(),
                                content_hash: entry.content_hash().to_string(),
                                source_peer: None,
                                docs_author: Some(entry.author().to_string()),
                            }));
                        }
                        iroh_docs::engine::LiveEvent::InsertRemote { from, entry, .. } => {
                            let _ = live_events.send(ReplicaNotice::Entry(DocEvent {
                                replica_id: live_replica.clone(),
                                key: String::from_utf8_lossy(entry.key()).to_string(),
                                content_hash: entry.content_hash().to_string(),
                                source_peer: Some(from.to_string()),
                                docs_author: Some(entry.author().to_string()),
                            }));
                        }
                        iroh_docs::engine::LiveEvent::SyncFinished(sync) => {
                            if sync.result.is_ok() {
                                peer_observations
                                    .record_fetch_success(
                                        sync.peer,
                                        sync.finished
                                            .duration_since(sync.started)
                                            .unwrap_or_default(),
                                    )
                                    .await;
                            } else {
                                peer_observations
                                    .record_fetch_failure(
                                        sync.peer,
                                        kukuri_transport::PeerFetchFailure::TransferFailed,
                                    )
                                    .await;
                            }
                            let _ = live_events.send(ReplicaNotice::SyncFinished);
                        }
                        iroh_docs::engine::LiveEvent::PendingContentReady => {
                            let _ = live_events.send(ReplicaNotice::ContentReady);
                        }
                        _ => {}
                    }
                }
            }
        });
        let doc = Arc::new(doc);
        replicas.insert(
            replica_id.as_str().to_string(),
            ReplicaHandle {
                doc: doc.clone(),
                events: tx,
                closing: false,
                live_task: Some(task),
                last_used: std::time::Instant::now(),
            },
        );
        while replicas.len() > MAX_OPEN_REPLICAS {
            // 使用中(clone を持つ操作がある)と停止中の handle は閉じない。全部が使用中なら、終わるまで上限を超えてよい。
            let Some(idle) = replicas
                .iter()
                .filter(|(_, handle)| !handle.closing && Arc::strong_count(&handle.doc) == 1)
                .min_by_key(|(_, handle)| handle.last_used)
                .map(|(id, _)| id.clone())
            else {
                break;
            };
            let mut handle = replicas.remove(&idle).expect("idle replica handle");
            if let Some(task) = handle.live_task.take() {
                task.abort();
            }
            if let Err(error) = handle.doc.close().await {
                warn!(replica = %idle, error = %error, "failed to close an idle docs replica");
            }
        }
        Ok(doc)
    }

    async fn sender(&self, replica_id: &ReplicaId) -> Result<broadcast::Sender<ReplicaNotice>> {
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
impl IrohDocsSync {
    /// 上限つきの key の一覧。`author` があれば、その docs author の entry だけを読む。
    async fn key_page(
        &self,
        replica_id: &ReplicaId,
        author: Option<AuthorId>,
        query: DocKeyQuery,
    ) -> Result<DocKeyPage> {
        // `limit` が 0 でも replica は開く(`MemoryDocsSync` と同じ。権限の無い replica はここで失敗する)。
        let doc = self.ensure_replica(replica_id).await?;
        if query.limit == 0 {
            return Ok(DocKeyPage::default());
        }
        let direction = match query.order {
            DocKeyOrder::Ascending => SortDirection::Asc,
            DocKeyOrder::Descending => SortDirection::Desc,
        };
        let query_limit = query.limit;
        let builder = match author {
            Some(author) => Query::author(author).key_prefix(query.prefix),
            None => Query::key_prefix(query.prefix),
        };
        let stream = doc
            .get_many(
                builder
                    .sort_by(SortBy::KeyAuthor, direction)
                    .limit(query.limit as u64)
                    .build(),
            )
            .await?;
        tokio::pin!(stream);
        let mut entries = Vec::new();
        let mut skipped_keys = 0usize;
        let mut scanned = 0usize;
        while let Some(entry) = stream.next().await {
            let entry = entry?;
            scanned += 1;
            // 飛ばした分を読み足さない(追加の query を発行しない)。返す件数が `limit` より減るだけである。
            // 飛ばした entry も `scanned` に数え、打ち切られたかどうかを呼び出し側へ伝える(#1257)。
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
        Ok(DocKeyPage {
            entries,
            reached_limit: scanned >= query_limit,
        })
    }
}

pub(crate) fn bounded_exact_query(key: &str, limit: usize) -> Query {
    Query::key_exact(key)
        .sort_by(SortBy::KeyAuthor, SortDirection::Asc)
        .limit(limit as u64)
        .build()
}

#[async_trait]
impl DocsSync for IrohDocsSync {
    async fn query_local_source(
        &self,
        replica: &ReplicaId,
        key: &str,
        author: Option<&str>,
        limit: usize,
    ) -> Result<Vec<DocRecord>> {
        self.read_cached_local_source(replica, key, author, limit)
            .await
    }

    async fn close_replica(&self, replica_id: &ReplicaId) -> Result<()> {
        self.close_replica_owned(replica_id, false).await
    }

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
        self.close_replica_owned(replica_id, true).await
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
                let _ = sender.send(ReplicaNotice::Entry(DocEvent {
                    replica_id: replica_id.clone(),
                    key,
                    content_hash: content_hash.to_string(),
                    source_peer: None,
                    docs_author: Some(author.to_string()),
                }));
            }
            DocOp::SetBytes { key, value } => {
                let content_hash = doc
                    .set_bytes(author, key.as_bytes().to_vec(), value)
                    .await?;
                self.supersede_legacy_entry(&doc, &legacy, key.as_str())
                    .await?;
                let _ = sender.send(ReplicaNotice::Entry(DocEvent {
                    replica_id: replica_id.clone(),
                    key,
                    content_hash: content_hash.to_string(),
                    source_peer: None,
                    docs_author: Some(author.to_string()),
                }));
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
        let exact = matches!(&query, DocQuery::Exact(_)).then(|| query.clone());
        let records = self.collect_records(replica_id, indexed_query(query), policy);
        self.with_private_cache(replica_id, exact, 8, records.await?)
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
        let records = self.collect_records(replica_id, bounded_exact_query(key, limit), policy);
        let exact = Some(DocQuery::Exact(key.into()));
        self.with_private_cache(replica_id, exact, limit, records.await?)
            .await
    }

    async fn query_replica_keys(
        &self,
        replica_id: &ReplicaId,
        query: DocKeyQuery,
    ) -> Result<DocKeyPage> {
        self.key_page(replica_id, None, query).await
    }

    async fn query_replica_keys_by_author(
        &self,
        replica_id: &ReplicaId,
        docs_author: &str,
        query: DocKeyQuery,
    ) -> Result<DocKeyPage> {
        // 手がかりは信用しない入力。docs author の id として読めない値は「無い」として扱う。
        let Ok(author) = AuthorId::from_str(docs_author) else {
            self.ensure_replica(replica_id).await?;
            return Ok(DocKeyPage::default());
        };
        self.key_page(replica_id, Some(author), query).await
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
        Ok(entry_stream(sender.subscribe()))
    }

    async fn subscribe_replica_notices(
        &self,
        replica_id: &ReplicaId,
    ) -> Result<ReplicaNoticeStream> {
        let sender = self.sender(replica_id).await?;
        Ok(notice_stream(sender.subscribe()))
    }

    async fn import_peer_ticket(&self, ticket: &str) -> Result<()> {
        let endpoint_addr = parse_endpoint_ticket(ticket)?;
        self.peers.insert_imported_peer_addr(endpoint_addr).await?;
        Ok(())
    }

    async fn learn_peer(&self, endpoint_id: &str) -> Result<()> {
        // reader の候補台帳だけを更新する(R5-H: replica へ同期先を配り直す旧 sync は撤去した)。
        let relay_urls = self.node.relay_urls().await;
        self.peers
            .record_learned_peer(endpoint_id, &relay_urls)
            .await?;
        Ok(())
    }

    async fn set_seed_peers(&self, peers: Vec<SeedPeer>) -> Result<()> {
        let relay_urls = self.node.relay_urls().await;
        self.peers.set_seed_peers(peers, &relay_urls).await?;
        Ok(())
    }

    async fn assist_peer_ids(&self) -> Result<Vec<String>> {
        Ok(self.available_sync_peer_ids().await)
    }

    async fn remote_readers(
        &self,
        replica: &ReplicaId,
        private_secret: Option<[u8; 32]>,
        scope_peers: Vec<SeedPeer>,
    ) -> Result<Vec<Arc<dyn DocsSync>>> {
        self.remote_readers_owned(replica, private_secret, scope_peers)
            .await
    }
}

#[cfg(test)]
#[path = "iroh_sync_key_query_tests.rs"]
mod key_query_tests;

#[cfg(test)]
mod tests {
    use super::*;

    // リトライ状態のテストは共通実装側(kukuri-transport::peers)へ移動した(WP-H2)。

    // #1221 R5-H: 日ごとの bucket へ書き続けても、開いている handle と event の task は上限で止まる。
    // 閉じた bucket の内容は残り、手元から読める。
    #[tokio::test]
    async fn writes_across_many_day_buckets_keep_open_handles_at_the_limit() -> Result<()> {
        let node = IrohDocsNode::memory().await?;
        let docs = IrohDocsSync::new(node.clone());
        let bucket = |day: u64| -> Result<ReplicaId> {
            Ok(crate::BucketReplica::new(
                crate::BucketScope::Topic {
                    topic_id: "handles".into(),
                },
                crate::TimeBucket::from_index(day)?,
            )?
            .replica_id())
        };
        let days = MAX_OPEN_REPLICAS as u64 + 16;
        let mut written = 0;
        let mut open = Vec::new();
        for total in [days, days * 10] {
            while written < total {
                docs.apply_doc_op(
                    &bucket(written)?,
                    DocOp::SetBytes {
                        key: "objects/post/state".into(),
                        value: written.to_be_bytes().to_vec(),
                    },
                )
                .await?;
                written += 1;
            }
            let replicas = docs.replicas.lock().await;
            let tasks = replicas
                .values()
                .filter(|handle| handle.live_task.is_some())
                .count();
            open.push((replicas.len(), tasks));
        }
        assert_eq!(
            open,
            vec![(MAX_OPEN_REPLICAS, MAX_OPEN_REPLICAS); 2],
            "open handles and event tasks do not grow with the number of days"
        );
        let rows = docs
            .query_replica_with_policy(
                &bucket(0)?,
                DocQuery::Exact("objects/post/state".into()),
                DocFetchPolicy::LocalOnly,
            )
            .await?;
        assert_eq!(rows[0].value, 0u64.to_be_bytes().to_vec());
        docs.shutdown().await;
        node.shutdown().await?;
        Ok(())
    }

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
            keys.entries
                .iter()
                .map(|entry| entry.key.as_str())
                .collect::<Vec<_>>(),
            vec![valid_key]
        );
        assert!(
            !keys.reached_limit,
            "two entries were read under a limit of eight: the prefix is exhausted"
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
            head.entries.is_empty(),
            "the non-utf8 key sorts first and the listing does not read past the limit"
        );
        // #1257: 飛ばした entry も「読んだ件数」に数える。呼び出し側は、返った件数が 0 でも、
        // この prefix が尽きていないことが分かる。
        assert!(head.reached_limit);

        docs.shutdown().await;
        node.shutdown().await.expect("shutdown node");
    }
}
