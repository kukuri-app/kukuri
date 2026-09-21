use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use async_trait::async_trait;
use futures_util::TryStreamExt;
use kukuri_blob_service::{BlobService, BlobStatus, IrohBlobService, StoredBlob};
use kukuri_core::{BlobHash, GossipHint, ReplicaId, TopicId};
use kukuri_docs_sync::{
    DocEventStream, DocFetchPolicy, DocKeyPage, DocKeyQuery, DocOp, DocQuery, DocRecord, DocsSync,
    IrohDocsSync, ReplicaNoticeStream,
};
use kukuri_iroh_node::IrohDocsNode;
use kukuri_transport::{
    ConnectMode, DhtDiscoveryOptions, DiscoveryMode, DiscoverySnapshot, HintStream, HintTransport,
    IrohGossipTransport, PeerSnapshot, SeedPeer, Transport, TransportNetworkConfig,
    TransportRelayConfig,
};
use tokio::sync::{Mutex, RwLock};
use tracing::info;

use crate::discovery::{DiscoveryConfig, normalize_seed_peers};

pub(crate) struct BoundIrohStack {
    pub(crate) node: Arc<IrohDocsNode>,
    pub(crate) transport: Arc<IrohGossipTransport>,
    pub(crate) docs_sync: Arc<IrohDocsSync>,
    pub(crate) blob_service: Arc<IrohBlobService>,
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
    }
}

reloadable_service! {
    pub(crate) struct ReloadableDocsSync wrapping IrohDocsSync;

    #[async_trait]
    impl DocsSync {
        async fn open_replica(replica_id: &ReplicaId) -> Result<()>;
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
        // #1152: trait の既定実装は永続化する `fetch_blob` へ委譲するため、必ず実体へ転送する
        // (成人向け表示 ON の取得は ephemeral で永続化しない。ADR 0046 §6.2)。
        async fn fetch_blob_ephemeral(hash: &BlobHash) -> Result<Option<Vec<u8>>>;
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
    /// アカウントの署名鍵から導出した docs author の種(ADR 0053)。stack を作り直すたびに設定し直す。
    docs_author_seed: Mutex<Option<kukuri_core::DocsAuthorSeed>>,
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
    ) -> Result<Self> {
        let dht_options = effective_dht_options(&dht_options, bootstrap_seed_peers, &relay_config);
        let current = BoundIrohStack::new(
            root,
            network_config.clone(),
            discovery_config,
            bootstrap_seed_peers,
            dht_options.clone(),
            relay_config,
            false,
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
            docs_author_seed: Mutex::new(None),
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

    pub(crate) async fn rebuild(
        &self,
        discovery_config: &DiscoveryConfig,
        bootstrap_seed_peers: &[SeedPeer],
        relay_config: TransportRelayConfig,
    ) -> Result<()> {
        let relay_config = relay_config.normalized();
        let dht_options =
            effective_dht_options(&self.dht_options, bootstrap_seed_peers, &relay_config);
        // Keep the last stack and its peer snapshots until replacement commits.
        // A failed/cancelled bind must not leave current=None forever. Holding
        // the lock also serializes concurrent rebuild/shutdown operations.
        let mut current = self.current.lock().await;
        let previous = current
            .as_ref()
            .context("missing active iroh stack during rebuild")?;
        let transport_peer_state = previous.transport.peer_state().await;
        let docs_peer_state = previous.docs_sync.peer_state().await;
        let blob_peer_state = previous.blob_service.peer_state().await;
        info!(target: "kukuri_connectivity",
            generation = self.generation.load(Ordering::Relaxed),
            relay_url_count = relay_config.iroh_relay_urls.len(),
            discovery_mode = ?discovery_config.mode,
            "rebuilding iroh stack after runtime relay connectivity change"
        );
        previous.shutdown().await;
        let next = BoundIrohStack::new(
            &self.root,
            self.network_config.clone(),
            discovery_config,
            bootstrap_seed_peers,
            dht_options,
            relay_config,
            true,
        )
        .await?;
        next.transport
            .restore_peer_state(transport_peer_state)
            .await?;
        next.docs_sync.restore_peer_state(docs_peer_state).await?;
        // 差し替える前に設定する。設定の無い stack が、端末ごとの docs author で書くことが無いようにする。
        if let Some(seed) = self.docs_author_seed.lock().await.as_ref() {
            next.docs_sync.use_account_docs_author(seed).await?;
        }
        next.blob_service
            .restore_peer_state(blob_peer_state)
            .await?;
        self.transport.replace(next.transport.clone()).await;
        self.docs_sync.replace(next.docs_sync.clone()).await;
        self.blob_service.replace(next.blob_service.clone()).await;
        *current = Some(next);
        let generation = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        info!(target: "kukuri_connectivity", generation, "iroh stack replacement committed");
        Ok(())
    }

    /// Read-only actor probe. A slow actor is not proof that its store needs
    /// rebuilding: a timeout is an error and the caller retries with backoff.
    pub(crate) async fn local_docs_available(&self) -> Result<bool> {
        let current = self.current.lock().await;
        let node = &current.as_ref().context("missing active iroh stack")?.node;
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
    pub(crate) async fn new(
        root: &Path,
        network_config: TransportNetworkConfig,
        discovery_config: &DiscoveryConfig,
        bootstrap_seed_peers: &[SeedPeer],
        dht_options: DhtDiscoveryOptions,
        relay_config: TransportRelayConfig,
        reopening: bool,
    ) -> Result<Self> {
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
        let transport = Arc::new(IrohGossipTransport::from_shared_parts(
            node.endpoint().clone(),
            node.gossip().clone(),
            node.discovery(),
            network_config,
            relay_config.clone(),
        )?);
        let docs_sync = Arc::new(IrohDocsSync::new(node.clone()));
        let blob_service = Arc::new(IrohBlobService::new(node.clone()));
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
mod tests {
    use super::*;
    use kukuri_blob_service::BlobService;
    use kukuri_docs_sync::DocsSync;
    use kukuri_transport::Transport;
    use tempfile::tempdir;
    use tokio::time::{Duration, timeout};

    // #1152 / ADR 0046 §6.2: desktop が実際に使う `ReloadableBlobService` 越しでも、
    // ephemeral 取得は remote の bytes をローカルへ保存せず、状態確認は remote から取得しない。
    // 既定実装へ落ちる method があると、黙って永続化する `fetch_blob` に戻る。
    #[tokio::test]
    async fn reloadable_blob_service_keeps_ephemeral_fetch_and_local_status_non_persistent() {
        let sender_dir = tempdir().expect("sender tempdir");
        let receiver_dir = tempdir().expect("receiver tempdir");
        let config = TransportNetworkConfig::loopback();
        let sender_node = IrohDocsNode::persistent_with_config(sender_dir.path(), config.clone())
            .await
            .expect("sender node");
        let receiver_node =
            IrohDocsNode::persistent_with_config(receiver_dir.path(), config.clone())
                .await
                .expect("receiver node");
        let sender = IrohBlobService::new(sender_node.clone());
        let receiver =
            ReloadableBlobService::new(Arc::new(IrohBlobService::new(receiver_node.clone())));

        let bound_port = sender_node
            .endpoint()
            .bound_sockets()
            .into_iter()
            .find(|addr| addr.port() != 0)
            .map(|addr| addr.port());
        let ticket = kukuri_transport::encode_endpoint_ticket(
            &sender_node.endpoint().addr(),
            &TransportNetworkConfig {
                advertised_host: Some("127.0.0.1".to_string()),
                advertised_port: bound_port,
                ..config.clone()
            },
        )
        .expect("sender ticket");
        receiver
            .import_peer_ticket(&ticket)
            .await
            .expect("import ticket");

        let stored = sender
            .put_blob(b"adult-display-enabled-media".to_vec(), "image/png")
            .await
            .expect("put blob");
        let inner = receiver.current().await;

        assert_eq!(
            receiver
                .local_blob_status(&stored.hash)
                .await
                .expect("local status"),
            BlobStatus::Missing
        );
        assert_eq!(
            inner
                .local_blob_status(&stored.hash)
                .await
                .expect("inner local status"),
            BlobStatus::Missing,
            "local status check must not persist the remote blob"
        );

        let payload = timeout(
            Duration::from_secs(20),
            receiver.fetch_blob_ephemeral(&stored.hash),
        )
        .await
        .expect("ephemeral fetch timeout")
        .expect("ephemeral fetch");
        assert_eq!(payload, Some(b"adult-display-enabled-media".to_vec()));
        assert_eq!(
            inner
                .local_blob_status(&stored.hash)
                .await
                .expect("inner local status after ephemeral fetch"),
            BlobStatus::Missing,
            "ephemeral fetch through the reloadable wrapper must not persist the blob"
        );
    }

    #[test]
    fn runtime_connectivity_rebuild_helper_skips_rebuild_when_relay_urls_are_unchanged() {
        let relay_url = "https://relay.example.com".to_string();
        assert!(!should_rebuild_runtime_connectivity(
            std::slice::from_ref(&relay_url),
            std::slice::from_ref(&relay_url),
        ));
    }

    #[test]
    fn runtime_connectivity_rebuild_helper_rebuilds_for_static_peer_relay_change() {
        let current = "https://relay-a.example.com".to_string();
        let next = "https://relay-b.example.com".to_string();
        assert!(should_rebuild_runtime_connectivity(
            std::slice::from_ref(&current),
            std::slice::from_ref(&next),
        ));
    }

    #[test]
    fn runtime_connectivity_rebuild_helper_rebuilds_for_non_static_peer_relay_change() {
        let current = "https://relay-a.example.com".to_string();
        let next = "https://relay-b.example.com".to_string();
        assert!(should_rebuild_runtime_connectivity(
            std::slice::from_ref(&current),
            std::slice::from_ref(&next),
        ));
    }

    #[tokio::test]
    async fn runtime_connectivity_rebuild_preserves_manual_ticket_peers() {
        let (_relay_map, relay_url, _guard) = iroh::test_utils::run_relay_server()
            .await
            .expect("relay server");
        let dir = tempdir().expect("tempdir");
        let discovery_config = DiscoveryConfig::static_peer_default();
        let stack_a = SharedIrohStack::new(
            &dir.path().join("stack-a"),
            TransportNetworkConfig::loopback(),
            &discovery_config,
            &[],
            DhtDiscoveryOptions::disabled(),
            TransportRelayConfig::default(),
        )
        .await
        .expect("stack a");
        let stack_b = SharedIrohStack::new(
            &dir.path().join("stack-b"),
            TransportNetworkConfig::loopback(),
            &discovery_config,
            &[],
            DhtDiscoveryOptions::disabled(),
            TransportRelayConfig::default(),
        )
        .await
        .expect("stack b");

        let ticket_b = stack_b
            .transport
            .current()
            .await
            .export_ticket()
            .await
            .expect("export ticket b")
            .expect("ticket b value");
        stack_a
            .transport
            .current()
            .await
            .import_ticket(ticket_b.as_str())
            .await
            .expect("import transport ticket");
        stack_a
            .docs_sync
            .current()
            .await
            .import_peer_ticket(ticket_b.as_str())
            .await
            .expect("import docs ticket");
        stack_a
            .blob_service
            .current()
            .await
            .import_peer_ticket(ticket_b.as_str())
            .await
            .expect("import blob ticket");

        let current_guard = stack_a.current.lock().await;
        let current = current_guard
            .as_ref()
            .expect("current stack before rebuild");
        let transport_before = current.transport.peer_state().await;
        let docs_before = current.docs_sync.peer_state().await;
        let blob_before = current.blob_service.peer_state().await;
        drop(current_guard);

        timeout(
            Duration::from_secs(30),
            stack_a.rebuild(
                &discovery_config,
                &[],
                TransportRelayConfig {
                    iroh_relay_urls: vec![relay_url.to_string()],
                },
            ),
        )
        .await
        .expect("stack rebuild timeout")
        .expect("stack rebuild");

        let current_guard = stack_a.current.lock().await;
        let current = current_guard.as_ref().expect("current stack after rebuild");
        let transport_after = current.transport.peer_state().await;
        let docs_after = current.docs_sync.peer_state().await;
        let blob_after = current.blob_service.peer_state().await;
        drop(current_guard);

        assert_eq!(
            transport_after.imported_peers,
            transport_before.imported_peers
        );
        assert_eq!(docs_after.imported_peers, docs_before.imported_peers);
        assert_eq!(blob_after.imported_peers, blob_before.imported_peers);

        timeout(Duration::from_secs(30), stack_a.shutdown_checked())
            .await
            .expect("stack a shutdown timeout")
            .expect("stack a shutdown");
        timeout(Duration::from_secs(30), stack_b.shutdown_checked())
            .await
            .expect("stack b shutdown timeout")
            .expect("stack b shutdown");
    }

    // #1258 TR-1: 起動時に設定した docs author は、stack を作り直しても同じで、`ReloadableDocsSync` 越しに
    // 照会と「docs author と key の組」の読み出しが内側へ届く。
    #[tokio::test]
    async fn account_docs_author_survives_a_stack_rebuild() {
        let dir = tempdir().expect("tempdir");
        let discovery_config = DiscoveryConfig::static_peer_default();
        let stack = SharedIrohStack::new(
            &dir.path().join("stack-docs-author"),
            TransportNetworkConfig::loopback(),
            &discovery_config,
            &[],
            DhtDiscoveryOptions::disabled(),
            TransportRelayConfig::default(),
        )
        .await
        .expect("stack");
        assert_eq!(
            stack.docs_sync.local_docs_author().await.expect("before"),
            None
        );
        let keys = kukuri_core::generate_keys();
        let id = stack
            .use_account_docs_author(keys.derive_docs_author_seed())
            .await
            .expect("use the account docs author");
        let replica = kukuri_docs_sync::topic_replica_id("kukuri:topic:stack-docs-author");
        let write = |key: &'static str| {
            stack.docs_sync.apply_doc_op(
                &replica,
                DocOp::SetBytes {
                    key: key.into(),
                    value: key.as_bytes().to_vec(),
                },
            )
        };
        write("objects/before/envelope").await.expect("write");

        stack
            .rebuild(&discovery_config, &[], TransportRelayConfig::default())
            .await
            .expect("rebuild");
        assert_eq!(
            stack.docs_sync.local_docs_author().await.expect("after"),
            Some(id.clone()),
            "a rebuilt stack must keep writing as the account docs author"
        );
        write("objects/after/envelope").await.expect("write");
        for key in ["objects/before/envelope", "objects/after/envelope"] {
            let record = stack
                .docs_sync
                .query_replica_by_author(&replica, id.as_str(), key, DocFetchPolicy::LocalOnly)
                .await
                .expect("read by docs author")
                .expect("record");
            assert_eq!(record.docs_author.as_deref(), Some(id.as_str()));
        }
        stack.shutdown_checked().await.expect("shutdown");
    }

    /// `ReloadableBlobService` 越しの `unpin_blob` が内側の `IrohBlobService` まで届き、
    /// Metaverse の pin tag と pin 状態を解放すること（#1157）。宣言漏れだと trait の既定実装
    /// （no-op の `Ok(())`）に落ち、GC 後も blob が pin されたまま残る。
    #[tokio::test]
    async fn reloadable_blob_service_forwards_unpin_to_inner_service() {
        let dir = tempdir().expect("tempdir");
        let stack = SharedIrohStack::new(
            &dir.path().join("stack-unpin"),
            TransportNetworkConfig::loopback(),
            &DiscoveryConfig::static_peer_default(),
            &[],
            DhtDiscoveryOptions::disabled(),
            TransportRelayConfig::default(),
        )
        .await
        .expect("stack");
        let (node, inner) = {
            let current_guard = stack.current.lock().await;
            let current = current_guard.as_ref().expect("current stack");
            (current.node.clone(), current.blob_service.clone())
        };
        let pin_tag = |hash: &BlobHash| format!("kukuri/metaverse/pin/{}", hash.as_str());

        let stored = stack
            .blob_service
            .put_blob(b"metaverse asset".to_vec(), "application/octet-stream")
            .await
            .expect("put blob");
        stack
            .blob_service
            .pin_blob(&stored.hash)
            .await
            .expect("pin blob");
        assert_eq!(
            stack
                .blob_service
                .blob_status(&stored.hash)
                .await
                .expect("status after pin"),
            BlobStatus::Pinned
        );
        assert!(
            node.blobs()
                .tags()
                .get(pin_tag(&stored.hash))
                .await
                .expect("pin tag after pin")
                .is_some()
        );

        stack
            .blob_service
            .unpin_blob(&stored.hash)
            .await
            .expect("unpin blob");

        assert!(
            node.blobs()
                .tags()
                .get(pin_tag(&stored.hash))
                .await
                .expect("pin tag after unpin")
                .is_none(),
            "unpin_blob must delete the metaverse pin tag of the inner service"
        );
        assert_eq!(
            inner
                .blob_status(&stored.hash)
                .await
                .expect("inner status after unpin"),
            BlobStatus::Available
        );
        assert_eq!(
            stack
                .blob_service
                .blob_status(&stored.hash)
                .await
                .expect("wrapper status after unpin"),
            BlobStatus::Available
        );

        timeout(Duration::from_secs(30), stack.shutdown_checked())
            .await
            .expect("stack shutdown timeout")
            .expect("stack shutdown");
    }

    #[tokio::test]
    async fn shared_stack_initializes_with_configured_relay_on_first_bind() {
        let (_relay_map, relay_url, _guard) = iroh::test_utils::run_relay_server()
            .await
            .expect("relay server");
        let dir = tempdir().expect("tempdir");
        let discovery_config = DiscoveryConfig::static_peer_default();
        let relay_config = TransportRelayConfig {
            iroh_relay_urls: vec![relay_url.to_string()],
        };
        let stack = SharedIrohStack::new(
            &dir.path().join("stack-relay"),
            TransportNetworkConfig::loopback(),
            &discovery_config,
            &[],
            DhtDiscoveryOptions::disabled(),
            relay_config,
        )
        .await
        .expect("stack");

        let current_guard = stack.current.lock().await;
        let current = current_guard.as_ref().expect("current stack");
        assert_eq!(current.node.relay_urls().await, vec![relay_url.clone()]);
        assert_eq!(
            current
                .transport
                .discovery()
                .await
                .expect("discovery")
                .connect_mode,
            ConnectMode::DirectOrRelay
        );
        drop(current_guard);

        timeout(Duration::from_secs(30), stack.shutdown_checked())
            .await
            .expect("stack shutdown timeout")
            .expect("stack shutdown");
    }
}
