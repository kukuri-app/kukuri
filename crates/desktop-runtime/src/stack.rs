use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use anyhow::{Context, Result};
use async_trait::async_trait;
use futures_util::TryStreamExt;
use kukuri_blob_service::{BlobService, BlobStatus, IrohBlobService, StoredBlob};
use kukuri_core::{
    BlobHash, GossipHint, KukuriKeys, Pubkey, ReplicaId, SealedReceiveOfferV1, TopicId,
    VerifiedReceiveOffer,
};
use kukuri_docs_sync::{
    DocEventStream, DocFetchPolicy, DocKeyPage, DocKeyQuery, DocOp, DocQuery, DocRecord, DocsSync,
    IrohDocsSync, ReplicaNoticeStream,
};
use kukuri_iroh_node::IrohDocsNode;
use kukuri_store::SqliteStore;
use kukuri_transport::{
    ConnectMode, DhtDiscoveryOptions, DiscoveryMode, DiscoverySnapshot, EndpointAddr, HintStream,
    HintTransport, IrohGossipTransport, PeerSnapshot, ReceiveCandidateFence, ReceiveOfferLease,
    ReceiveOfferSubscription, SeedPeer, Transport, TransportNetworkConfig, TransportRelayConfig,
};
#[cfg(test)]
use tokio::sync::oneshot;
use tokio::sync::{Mutex, RwLock};
use tracing::info;

use crate::discovery::{DiscoveryConfig, normalize_seed_peers};

pub(crate) struct BoundIrohStack {
    pub(crate) node: Arc<IrohDocsNode>,
    pub(crate) transport: Arc<IrohGossipTransport>,
    pub(crate) docs_sync: Arc<IrohDocsSync>,
    pub(crate) blob_service: Arc<IrohBlobService>,
}

enum StackOpen {
    Initial(Arc<SqliteStore>),
    Reopen(Arc<SqliteStore>),
}

/// ホットスワップ可能なサービスラッパーを 1 つ生成する。
///
/// `SharedIrohStack::rebuild` は iroh ノードを作り直すたびに transport / docs-sync /
/// blob-service の実体を差し替える。各ラッパーは `Arc<RwLock<Arc<T>>>` を保持し、
/// trait メソッドはすべて「その時点の実体を `current()` で取り出して素通し」する純転送で、
/// 3 種で構造もメソッド転送も同型だったため declarative macro で 1 箇所に集約する。
///
/// 文法: `struct <Name> wrapping <Inner>;` に続けて、実装する trait ごとに
/// `#[async_trait] impl <Trait> { async fn <m>(<args>) -> <ret>; ... }` を並べる
/// (メソッドは `&self` を暗黙に取り、`self.current().await.<m>(args).await` へ転送される)。
macro_rules! reloadable_service {
    (
        $(#[$struct_meta:meta])*
        $vis:vis struct $name:ident wrapping $inner:ty;
        $(
            $(#[$impl_meta:meta])*
            impl $trait:path {
                $(
                    async fn $method:ident ( $( $arg:ident : $arg_ty:ty ),* $(,)? ) -> $ret:ty;
                )*
            }
        )*
    ) => {
        #[derive(Clone)]
        $(#[$struct_meta])*
        $vis struct $name {
            inner: Arc<RwLock<Arc<$inner>>>,
        }

        impl $name {
            pub(crate) fn new(inner: Arc<$inner>) -> Self {
                Self {
                    inner: Arc::new(RwLock::new(inner)),
                }
            }

            pub(crate) async fn current(&self) -> Arc<$inner> {
                self.inner.read().await.clone()
            }

            pub(crate) async fn replace(&self, inner: Arc<$inner>) {
                *self.inner.write().await = inner;
            }
        }

        $(
            $(#[$impl_meta])*
            impl $trait for $name {
                $(
                    async fn $method(&self, $( $arg : $arg_ty ),* ) -> $ret {
                        self.current().await.$method($( $arg ),*).await
                    }
                )*
            }
        )*
    };
}

reloadable_service! {
    pub(crate) struct ReloadableTransport wrapping IrohGossipTransport;

    #[async_trait]
    impl Transport {
        async fn peers() -> Result<PeerSnapshot>;
        async fn export_ticket() -> Result<Option<String>>;
        async fn import_ticket(ticket: &str) -> Result<()>;
        async fn configure_discovery(
            mode: DiscoveryMode,
            env_locked: bool,
            configured_seed_peers: Vec<SeedPeer>,
            bootstrap_seed_peers: Vec<SeedPeer>,
        ) -> Result<()>;
        async fn discovery() -> Result<DiscoverySnapshot>;
    }

    #[async_trait]
    impl HintTransport {
        async fn subscribe_hints(topic: &TopicId) -> Result<HintStream>;
        async fn unsubscribe_hints(topic: &TopicId) -> Result<()>;
        async fn publish_hint(topic: &TopicId, hint: GossipHint) -> Result<()>;
        async fn resolve_receive_destination(recipient: &Pubkey) -> Result<Option<EndpointAddr>>;
        async fn receive_candidate_fence() -> Result<ReceiveCandidateFence>;
        async fn offer_receive_candidates(source: &str, recipient: &Pubkey, candidates: Vec<EndpointAddr>, fence: ReceiveCandidateFence) -> Result<()>;
        async fn clear_receive_candidates(source: Option<&str>) -> Result<()>;
        async fn invalidate_receive_destination(recipient: &Pubkey, endpoint_id: &str) -> Result<()>;
        async fn verify_receive_provider(sender: &Pubkey, provider: EndpointAddr) -> Result<()>;
        async fn subscribe_receive_offers(recipient: &Pubkey) -> Result<ReceiveOfferSubscription>;
        async fn resubscribe_receive_offers_if_current(recipient: &Pubkey, expected: ReceiveOfferLease) -> Result<Option<ReceiveOfferSubscription>>;
        async fn subscribe_receive_offers_if_vacant(recipient: &Pubkey) -> Result<Option<ReceiveOfferSubscription>>;
        async fn receive_offer_transport_instance() -> Result<u64>;
        async fn unsubscribe_receive_offers(recipient: &Pubkey, lease: ReceiveOfferLease) -> Result<()>;
        async fn publish_receive_offer(
            recipient: &Pubkey, destination: EndpointAddr, offer: SealedReceiveOfferV1,
        ) -> Result<()>;
    }
}

reloadable_service! {
    pub(crate) struct ReloadableDocsSync wrapping IrohDocsSync;

    #[async_trait]
    impl DocsSync {
        async fn query_local_source(
            replica: &ReplicaId, key: &str, author: Option<&str>, limit: usize,
        ) -> Result<Vec<DocRecord>>;
        async fn open_replica(replica_id: &ReplicaId) -> Result<()>;
        async fn close_replica(replica_id: &ReplicaId) -> Result<()>;
        async fn register_private_replica_secret(
            replica_id: &ReplicaId,
            namespace_secret_hex: &str,
        ) -> Result<()>;
        async fn remove_private_replica_secret(replica_id: &ReplicaId) -> Result<()>;
        async fn apply_doc_op(replica_id: &ReplicaId, op: DocOp) -> Result<()>;
        async fn query_replica_with_policy(
            replica_id: &ReplicaId,
            query: DocQuery,
            policy: DocFetchPolicy,
        ) -> Result<Vec<DocRecord>>;
        // #1248: 宣言が無いと trait の既定実装(読んでから切り詰める)に落ち、query の上限が効かない。
        async fn query_replica_exact_bounded(
            replica_id: &ReplicaId,
            key: &str,
            limit: usize,
            policy: DocFetchPolicy,
        ) -> Result<Vec<DocRecord>>;
        // #1239: 宣言が無いと trait の既定実装(エラー)に落ちる。上限つきの読み出しは必ず内側へ転送する。
        async fn query_replica_keys(
            replica_id: &ReplicaId,
            query: DocKeyQuery,
        ) -> Result<DocKeyPage>;
        async fn query_replica_keys_by_author(
            replica_id: &ReplicaId,
            docs_author: &str,
            query: DocKeyQuery,
        ) -> Result<DocKeyPage>;
        // #1258: 宣言が無いと trait の既定実装に落ちる(docs author なし / 読み出しはエラー)。
        async fn local_docs_author() -> Result<Option<String>>;
        async fn query_replica_by_author(
            replica_id: &ReplicaId,
            docs_author: &str,
            key: &str,
            policy: DocFetchPolicy,
        ) -> Result<Option<DocRecord>>;
        async fn subscribe_replica(replica_id: &ReplicaId) -> Result<DocEventStream>;
        // #1239: 宣言が無いと trait の既定実装(entry だけ)に落ち、取りこぼしと同期の区切りが購読側へ届かない。
        async fn subscribe_replica_notices(replica_id: &ReplicaId) -> Result<ReplicaNoticeStream>;
        async fn import_peer_ticket(ticket: &str) -> Result<()>;
        async fn learn_peer(endpoint_id: &str) -> Result<()>;
        async fn restart_replica_sync(replica_id: &ReplicaId) -> Result<()>;
        async fn set_seed_peers(peers: Vec<SeedPeer>) -> Result<()>;
        async fn assist_peer_ids() -> Result<Vec<String>>;
        async fn public_bucket_readers(replica: &ReplicaId) -> Result<Vec<Arc<dyn DocsSync>>>;
    }
}

reloadable_service! {
    pub(crate) struct ReloadableBlobService wrapping IrohBlobService;

    // 宣言の無い trait メソッドは内側へ転送されず、trait の既定実装に落ちる(#1157)。
    // `fetch_blob_ephemeral_bounded` は CN の indexer 専用で desktop からは呼ばれないため
    // 宣言しない(既定実装は bail なので、誤って呼ばれても無制限取得にはならない)。
    #[async_trait]
    impl BlobService {
        async fn put_blob(data: Vec<u8>, mime: &str) -> Result<StoredBlob>;
        async fn fetch_blob(hash: &BlobHash) -> Result<Option<Vec<u8>>>;
        async fn fetch_local_blob(hash: &BlobHash) -> Result<Option<Vec<u8>>>;
        async fn prepare_display_fetch(hash: &BlobHash) -> Result<kukuri_blob_service::DisplayBlobFetch>;
        // #1152: trait の既定実装は永続化する `fetch_blob` へ委譲するため、必ず実体へ転送する
        // (成人向け表示 ON の取得は ephemeral で永続化しない。ADR 0046 §6.2)。
        async fn fetch_blob_ephemeral(hash: &BlobHash) -> Result<Option<Vec<u8>>>;
        async fn fetch_verified_receive_offer_payload(
            offer: &VerifiedReceiveOffer, provider: EndpointAddr,
        ) -> Result<Vec<u8>>;
        async fn pin_blob(hash: &BlobHash) -> Result<()>;
        async fn unpin_blob(hash: &BlobHash) -> Result<()>;
        async fn blob_status(hash: &BlobHash) -> Result<BlobStatus>;
        async fn local_blob_status(hash: &BlobHash) -> Result<BlobStatus>;
        async fn import_peer_ticket(ticket: &str) -> Result<()>;
        async fn learn_peer(endpoint_id: &str) -> Result<()>;
        async fn set_seed_peers(peers: Vec<SeedPeer>) -> Result<()>;
        async fn assist_peer_ids() -> Result<Vec<String>>;
    }
}

pub(crate) struct SharedIrohStack {
    pub(crate) current: Mutex<Option<BoundIrohStack>>,
    generation: AtomicU64,
    pub(crate) transport: Arc<ReloadableTransport>,
    pub(crate) docs_sync: Arc<ReloadableDocsSync>,
    pub(crate) blob_service: Arc<ReloadableBlobService>,
    pub(crate) root: PathBuf,
    pub(crate) network_config: TransportNetworkConfig,
    pub(crate) dht_options: DhtDiscoveryOptions,
    candidate_store: Arc<SqliteStore>,
    /// アカウントの署名鍵から導出した docs author の種(ADR 0053)。stack を作り直すたびに設定し直す。
    docs_author_seed: Mutex<Option<kukuri_core::DocsAuthorSeed>>,
    /// 再構築するendpointでも同じaccountだけを広告する。stack/account寿命に限定する。
    receive_binding_keys: Mutex<Option<Arc<KukuriKeys>>>,
    /// `current` の stack が shutdown 済みか。作り直しが古い stack の shutdown の後で失敗すると、shutdown 済みの stack が残る。
    /// その stack の docs actor への要求は、返事が来ないまま時間切れになりうるので、健全性の確認をせず、作り直しが要るとみなす。
    current_shut_down: AtomicBool,
    /// Test-only cancellation point immediately before rebuild starts shutting down the old stack.
    #[cfg(test)]
    rebuild_before_shutdown_gate: Mutex<Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>>,
}

fn should_rebuild_runtime_connectivity(
    current_relay_urls: &[String],
    next_relay_urls: &[String],
) -> bool {
    current_relay_urls != next_relay_urls
}

pub(crate) fn effective_seed_peers(
    discovery_config: &DiscoveryConfig,
    bootstrap_seed_peers: &[SeedPeer],
) -> Vec<SeedPeer> {
    normalize_seed_peers(
        discovery_config
            .seed_peers
            .iter()
            .cloned()
            .chain(bootstrap_seed_peers.iter().cloned())
            .collect(),
    )
}

pub(crate) fn effective_dht_options(
    dht_options: &DhtDiscoveryOptions,
    bootstrap_seed_peers: &[SeedPeer],
    relay_config: &TransportRelayConfig,
) -> DhtDiscoveryOptions {
    if relay_config.connect_mode() == ConnectMode::DirectOrRelay && !bootstrap_seed_peers.is_empty()
    {
        DhtDiscoveryOptions::disabled()
    } else {
        dht_options.clone()
    }
}

impl SharedIrohStack {
    pub(crate) fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }

    pub(crate) async fn new(
        root: &Path,
        network_config: TransportNetworkConfig,
        discovery_config: &DiscoveryConfig,
        bootstrap_seed_peers: &[SeedPeer],
        dht_options: DhtDiscoveryOptions,
        relay_config: TransportRelayConfig,
        candidate_store: Option<Arc<SqliteStore>>,
    ) -> Result<Self> {
        let dht_options = effective_dht_options(&dht_options, bootstrap_seed_peers, &relay_config);
        let candidate_store = match candidate_store {
            Some(store) => store,
            None => Arc::new(SqliteStore::connect_memory().await?),
        };
        let current = BoundIrohStack::new(
            root,
            network_config.clone(),
            discovery_config,
            bootstrap_seed_peers,
            dht_options.clone(),
            relay_config,
            StackOpen::Initial(candidate_store.clone()),
        )
        .await?;
        let transport = Arc::new(ReloadableTransport::new(current.transport.clone()));
        let docs_sync = Arc::new(ReloadableDocsSync::new(current.docs_sync.clone()));
        let blob_service = Arc::new(ReloadableBlobService::new(current.blob_service.clone()));
        Ok(Self {
            current: Mutex::new(Some(current)),
            generation: AtomicU64::new(0),
            transport,
            docs_sync,
            blob_service,
            root: root.to_path_buf(),
            network_config,
            dht_options,
            candidate_store,
            docs_author_seed: Mutex::new(None),
            receive_binding_keys: Mutex::new(None),
            current_shut_down: AtomicBool::new(false),
            #[cfg(test)]
            rebuild_before_shutdown_gate: Mutex::new(None),
        })
    }

    /// docs の書き込みを、アカウントの署名鍵から導出した docs author の名義にする(ADR 0053 §1)。
    ///
    /// runtime の起動時に、docs へ何かを書く前に呼ぶ。アカウントの切り替えと復元は runtime を作り直すので、
    /// 同じ経路を通る。`rebuild` は、作り直した stack へ同じ設定を入れてから差し替える。
    pub(crate) async fn use_account_docs_author(
        &self,
        seed: kukuri_core::DocsAuthorSeed,
    ) -> Result<String> {
        let current = self.current.lock().await;
        let docs_sync = &current
            .as_ref()
            .context("missing active iroh stack")?
            .docs_sync;
        let id = docs_sync.use_account_docs_author(&seed).await?;
        *self.docs_author_seed.lock().await = Some(seed);
        Ok(id)
    }

    pub(crate) async fn use_account_receive_binding(&self, keys: Arc<KukuriKeys>) -> Result<()> {
        let current = self.current.lock().await;
        current
            .as_ref()
            .context("missing active iroh stack")?
            .node
            .install_receive_binding(keys.clone())
            .await?;
        *self.receive_binding_keys.lock().await = Some(keys);
        Ok(())
    }

    pub(crate) async fn rebuild(
        &self,
        discovery_config: &DiscoveryConfig,
        bootstrap_seed_peers: &[SeedPeer],
        relay_config: TransportRelayConfig,
    ) -> Result<()> {
        let relay_config = relay_config.normalized();
        let dht_options =
            effective_dht_options(&self.dht_options, bootstrap_seed_peers, &relay_config);
        // Keep the last stack until replacement commits. Peer candidates remain
        // in the account store and are read through bounded windows by the new stack.
        // A failed/cancelled bind must not leave current=None forever. Holding
        // the lock also serializes concurrent rebuild/shutdown operations.
        let mut current = self.current.lock().await;
        let previous = current
            .as_ref()
            .context("missing active iroh stack during rebuild")?;
        info!(target: "kukuri_connectivity",
            generation = self.generation.load(Ordering::Relaxed),
            relay_url_count = relay_config.iroh_relay_urls.len(),
            discovery_mode = ?discovery_config.mode,
            "rebuilding iroh stack after runtime relay connectivity change"
        );
        // ここから差し替えが済むまでに失敗または取消されると、`current` には shutdown を
        // 開始した stack が残る。最初の await より前に印を立て、途中で future が drop
        // されても停止中または停止済みの actor を probe しない。
        self.current_shut_down.store(true, Ordering::SeqCst);
        #[cfg(test)]
        if let Some((reached, resume)) = self.rebuild_before_shutdown_gate.lock().await.take() {
            let _ = reached.send(());
            let _ = resume.await;
        }
        previous.shutdown().await;
        let next = BoundIrohStack::new(
            &self.root,
            self.network_config.clone(),
            discovery_config,
            bootstrap_seed_peers,
            dht_options,
            relay_config,
            StackOpen::Reopen(self.candidate_store.clone()),
        )
        .await?;
        // 差し替える前に設定する。設定の無い stack が、端末ごとの docs author で書くことが無いようにする。
        if let Some(seed) = self.docs_author_seed.lock().await.as_ref() {
            next.docs_sync.use_account_docs_author(seed).await?;
        }
        if let Some(keys) = self.receive_binding_keys.lock().await.as_ref() {
            next.node.install_receive_binding(keys.clone()).await?;
        }
        self.transport.replace(next.transport.clone()).await;
        self.docs_sync.replace(next.docs_sync.clone()).await;
        self.blob_service.replace(next.blob_service.clone()).await;
        *current = Some(next);
        self.current_shut_down.store(false, Ordering::SeqCst);
        let generation = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        info!(target: "kukuri_connectivity", generation, "iroh stack replacement committed");
        Ok(())
    }

    /// Read-only actor probe. A slow actor is not proof that its store needs
    /// rebuilding: a timeout is an error and the caller retries with backoff.
    /// ただし、作り直しの失敗で shutdown 済みの stack が残っているときは、確認をせずに使えないとみなす
    /// (shutdown 済みの actor への要求は、返事が来ないまま時間切れになりうる。時間切れをエラーで返すと、
    /// 呼び出し側は作り直しへ進まず、再試行しても回復しない)。
    pub(crate) async fn local_docs_available(&self) -> Result<bool> {
        let current = self.current.lock().await;
        let node = &current.as_ref().context("missing active iroh stack")?.node;
        if self.current_shut_down.load(Ordering::SeqCst) {
            tracing::warn!(target: "kukuri_connectivity",
                generation = self.generation.load(Ordering::Relaxed),
                "the active iroh stack was shut down by a failed rebuild; stack repair required");
            return Ok(false);
        }
        let probe = async {
            let mut namespaces = node.docs().list().await?;
            namespaces.try_next().await?;
            Ok::<_, anyhow::Error>(())
        };
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), probe)
            .await
            .context("local docs actor health probe timed out")?;
        if let Err(error) = &result {
            tracing::warn!(target: "kukuri_connectivity",
                generation = self.generation.load(Ordering::Relaxed), %error,
                "local docs actor unavailable; stack repair required");
        }
        Ok(result.is_ok())
    }

    pub(crate) async fn apply_runtime_connectivity(
        &self,
        discovery_config: &DiscoveryConfig,
        bootstrap_seed_peers: &[SeedPeer],
        relay_config: TransportRelayConfig,
    ) -> Result<()> {
        let relay_config = relay_config.normalized();
        let next_relay_urls = relay_config
            .parsed_relay_urls()?
            .into_iter()
            .map(|url| url.to_string())
            .collect::<Vec<_>>();
        let current_relay_urls = {
            let current = self.current.lock().await;
            current
                .as_ref()
                .context("missing active iroh stack while reading relay urls")?
                .node
                .relay_urls()
                .await
                .into_iter()
                .map(|url| url.to_string())
                .collect::<Vec<_>>()
        };
        if should_rebuild_runtime_connectivity(&current_relay_urls, &next_relay_urls)
            || !self.local_docs_available().await?
        {
            info!(
                current_relay_url_count = current_relay_urls.len(),
                next_relay_url_count = next_relay_urls.len(),
                discovery_mode = ?discovery_config.mode,
                "runtime relay connectivity change requires stack rebuild"
            );
            return self
                .rebuild(discovery_config, bootstrap_seed_peers, relay_config)
                .await;
        }
        let current = self.current.lock().await;
        let current = current
            .as_ref()
            .context("missing active iroh stack while applying runtime connectivity")?;
        current
            .node
            .apply_relay_config(relay_config.clone())
            .await?;
        current.transport.update_relay_config(relay_config).await?;
        current
            .transport
            .configure_discovery(
                discovery_config.mode.clone(),
                discovery_config.env_locked,
                discovery_config.seed_peers.clone(),
                bootstrap_seed_peers.to_vec(),
            )
            .await?;
        let effective_seed_peers = effective_seed_peers(discovery_config, bootstrap_seed_peers);
        current
            .docs_sync
            .set_seed_peers(effective_seed_peers.clone())
            .await?;
        current
            .blob_service
            .set_seed_peers(effective_seed_peers)
            .await?;
        Ok(())
    }

    pub(crate) async fn force_rebuild_runtime_connectivity(
        &self,
        discovery_config: &DiscoveryConfig,
        bootstrap_seed_peers: &[SeedPeer],
        relay_config: TransportRelayConfig,
    ) -> Result<()> {
        self.rebuild(discovery_config, bootstrap_seed_peers, relay_config)
            .await
    }

    pub(crate) async fn shutdown_checked(&self) -> Result<()> {
        if let Some(current) = self.current.lock().await.take() {
            current.transport.shutdown().await;
            current.docs_sync.shutdown().await;
            current.node.clone().shutdown().await?;
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) async fn pause_next_rebuild_before_shutdown(
        &self,
    ) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
        let (reached_tx, reached_rx) = oneshot::channel();
        let (resume_tx, resume_rx) = oneshot::channel();
        *self.rebuild_before_shutdown_gate.lock().await = Some((reached_tx, resume_rx));
        (reached_rx, resume_tx)
    }

    #[cfg(test)]
    pub(crate) async fn endpoint(&self) -> iroh::Endpoint {
        self.current
            .lock()
            .await
            .as_ref()
            .expect("missing active iroh stack")
            .node
            .endpoint()
            .clone()
    }
}

impl BoundIrohStack {
    async fn new(
        root: &Path,
        network_config: TransportNetworkConfig,
        discovery_config: &DiscoveryConfig,
        bootstrap_seed_peers: &[SeedPeer],
        dht_options: DhtDiscoveryOptions,
        relay_config: TransportRelayConfig,
        open: StackOpen,
    ) -> Result<Self> {
        let (reopening, candidate_store) = match open {
            StackOpen::Initial(store) => (false, store),
            StackOpen::Reopen(store) => (true, store),
        };
        let relay_config = relay_config.normalized();
        let node = if reopening {
            IrohDocsNode::reopen_with_discovery_config(
                root,
                network_config.clone(),
                dht_options,
                relay_config.clone(),
            )
            .await?
        } else {
            IrohDocsNode::persistent_with_discovery_config(
                root,
                network_config.clone(),
                dht_options,
                relay_config.clone(),
            )
            .await?
        };
        let transport = Arc::new(
            IrohGossipTransport::from_shared_parts(
                node.endpoint().clone(),
                node.gossip().clone(),
                node.discovery(),
                network_config,
                relay_config.clone(),
            )?
            .with_account_store(candidate_store.clone()),
        );
        let docs_sync = Arc::new(IrohDocsSync::with_account_store(
            node.clone(),
            candidate_store.clone(),
        ));
        let blob_service = Arc::new(IrohBlobService::with_account_store(
            node.clone(),
            candidate_store,
        ));
        transport
            .configure_discovery(
                discovery_config.mode.clone(),
                discovery_config.env_locked,
                discovery_config.seed_peers.clone(),
                bootstrap_seed_peers.to_vec(),
            )
            .await?;
        let effective_seed_peers = effective_seed_peers(discovery_config, bootstrap_seed_peers);
        docs_sync
            .set_seed_peers(effective_seed_peers.clone())
            .await?;
        blob_service.set_seed_peers(effective_seed_peers).await?;
        Ok(Self {
            node,
            transport,
            docs_sync,
            blob_service,
        })
    }

    pub(crate) async fn shutdown(&self) {
        self.transport.shutdown().await;
        self.docs_sync.shutdown().await;
        let _ = self.node.clone().shutdown().await;
    }
}

#[cfg(test)]
mod tests;
