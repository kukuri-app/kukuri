use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock, RwLock as StdRwLock};
use std::time::Duration;
#[cfg(not(target_family = "wasm"))]
use web_time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use futures_util::StreamExt;
use iroh::address_lookup::MemoryLookup;
use iroh::endpoint::{Builder as EndpointBuilder, presets};
use iroh::protocol::Router;
use iroh::{Endpoint, EndpointAddr, RelayUrl, Watcher};
use iroh_blobs::api::Store as BlobStore;
#[cfg(not(target_family = "wasm"))]
use iroh_blobs::store::fs::options::Options as BlobStoreOptions;
use iroh_blobs::store::mem::MemStore;
use iroh_docs::actor::SyncHandle;
use iroh_docs::api::DocsApi;
use iroh_docs::engine::{DefaultAuthorStorage, Engine, ProtectCallbackHandler};
use iroh_docs::store::Store as DocsStore;
use iroh_gossip::net::Gossip;
use kukuri_store::ContentCacheStore;
use kukuri_transport::{
    ConnectMode, DhtDiscoveryOptions, RECEIVE_BINDING_ALPN, ReceiveBindingSlot,
    TransportNetworkConfig, TransportRelayConfig, build_endpoint_builder,
    prepare_endpoint_for_discovery, sync_endpoint_relay_config,
};
#[cfg(not(target_family = "wasm"))]
use kukuri_webrtc_transport::WebRtcConfig;
use kukuri_webrtc_transport::{
    DemandHooks, NEGOTIATION_DEADLINE, SIGNALING_ALPN, Signaling, WebRtcTransport,
};
use n0_future::time::timeout;
#[cfg(not(target_family = "wasm"))]
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::account_transfer::{ACCOUNT_TRANSFER_ALPN, AccountTransfer};
use crate::dome_session::{DOME_SESSION_ALPN, DomeSessionConnections, DomeSessionSlot};
use crate::page_read::{DOC_READ_ALPN, DocReadProtocol, PrivateCapabilities, PrivateSecretLookup};
#[cfg(not(target_family = "wasm"))]
use crate::public_blobs::PublicBlobDiscovery;
use crate::remote_blob::{REMOTE_BLOB_ALPN, RemoteBlobProtocol};

#[cfg(test)]
#[cfg(not(target_family = "wasm"))]
use iroh::tls::CaTlsConfig;

#[cfg(not(target_family = "wasm"))]
pub(crate) const ENDPOINT_SECRET_FILE_NAME: &str = "endpoint-secret.json";
#[cfg(not(target_family = "wasm"))]
const DOCS_STORE_FILE_NAME: &str = "docs.redb";
#[cfg(not(target_family = "wasm"))]
const DEFAULT_AUTHOR_FILE_NAME: &str = "default-author";

/// Web の memory store を回収する間隔(ADR 0058 §7)。
#[cfg(target_family = "wasm")]
const MEMORY_GC_INTERVAL: Duration = Duration::from_secs(10);

fn relay_activation_timeout() -> Duration {
    Duration::from_secs(10)
}

fn router_shutdown_timeout() -> Duration {
    if cfg!(target_os = "windows") || std::env::var_os("GITHUB_ACTIONS").is_some() {
        Duration::from_secs(15)
    } else {
        Duration::from_secs(5)
    }
}

async fn spawn_docs(
    root: Option<&Path>,
    endpoint: Endpoint,
    blobs: BlobStore,
    gossip: Gossip,
    protect: Option<ProtectCallbackHandler>,
) -> Result<SpawnedDocs> {
    // Keep the high-level API and its persistent store layout while retaining
    // the SyncHandle for the bounded page reader (`DocReadProtocol`). The
    // high-level Docs::Builder discards this handle after creating the Engine.
    // Web は docs を memory store で持つ（ADR 0058 §1）。永続の store は native だけ。
    let (replica_store, author_store) = match root {
        #[cfg(not(target_family = "wasm"))]
        Some(path) => (
            DocsStore::persistent(path.join(DOCS_STORE_FILE_NAME))?,
            DefaultAuthorStorage::Persistent(path.join(DEFAULT_AUTHOR_FILE_NAME)),
        ),
        #[cfg(target_family = "wasm")]
        Some(_) => anyhow::bail!("a persistent docs store is not available in the browser"),
        None => (DocsStore::memory(), DefaultAuthorStorage::Mem),
    };
    let downloader = blobs.downloader(&endpoint);
    let engine = Engine::spawn(
        endpoint,
        gossip,
        replica_store,
        blobs,
        downloader,
        author_store,
        protect,
    )
    .await?;
    let sync = engine.sync.clone();
    Ok(SpawnedDocs {
        protocol: iroh_docs::protocol::Docs::new(engine),
        sync,
    })
}

struct SpawnedDocs {
    protocol: iroh_docs::protocol::Docs,
    sync: SyncHandle,
}

#[cfg(not(target_family = "wasm"))]
async fn recover_persistent_docs(
    root: &Path,
    endpoint: Endpoint,
    blobs: BlobStore,
    gossip: Gossip,
    original_error: anyhow::Error,
) -> Result<SpawnedDocs> {
    let recovery_dir = move_corrupt_docs_store(root)
        .with_context(|| format!("failed to recover iroh docs store at {}", root.display()))?;
    warn!(
        root = %root.display(),
        recovery_dir = %recovery_dir.display(),
        error = %original_error,
        "recovering corrupt iroh docs store before retrying startup"
    );
    spawn_docs(Some(root), endpoint, blobs, gossip, None)
    .await
    .with_context(|| {
        format!(
            "failed to spawn iroh docs after recovering corrupt store to {}; original error: {original_error:#}",
            recovery_dir.display()
        )
    })
}

#[cfg(not(target_family = "wasm"))]
fn move_corrupt_docs_store(root: &Path) -> Result<PathBuf> {
    let recovery_dir = unique_recovery_dir(root)?;
    std::fs::create_dir_all(&recovery_dir).with_context(|| {
        format!(
            "failed to create iroh docs recovery dir {}",
            recovery_dir.display()
        )
    })?;
    let mut moved_any = false;
    for file_name in [DOCS_STORE_FILE_NAME, DEFAULT_AUTHOR_FILE_NAME] {
        let source = root.join(file_name);
        if !source.exists() {
            continue;
        }
        let target = recovery_dir.join(file_name);
        std::fs::rename(&source, &target).with_context(|| {
            format!(
                "failed to move corrupt iroh docs file {} to {}",
                source.display(),
                target.display()
            )
        })?;
        moved_any = true;
    }
    if !moved_any {
        return Err(anyhow!(
            "no iroh docs store files found to recover in {}",
            root.display()
        ));
    }
    Ok(recovery_dir)
}

#[cfg(not(target_family = "wasm"))]
fn unique_recovery_dir(root: &Path) -> Result<PathBuf> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before UNIX_EPOCH")?;
    for attempt in 0..100 {
        let candidate = root.join(format!(
            "iroh-docs-recovery-{}-{}-{attempt}",
            now.as_secs(),
            now.subsec_nanos()
        ));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(anyhow!(
        "failed to allocate unique iroh docs recovery dir under {}",
        root.display()
    ))
}

pub struct IrohDocsNode {
    endpoint: Endpoint,
    gossip_connection_paths: kukuri_transport::GossipConnectionPaths,
    gossip: Gossip,
    discovery: Arc<MemoryLookup>,
    relay_urls: Arc<StdRwLock<Vec<RelayUrl>>>,
    router: Arc<Router>,
    signaling: Option<Arc<Signaling>>,
    docs: DocsApi,
    blobs: BlobStore,
    remote_cache: Arc<OnceLock<Arc<dyn ContentCacheStore>>>,
    private_capabilities: PrivateCapabilities,
    fetch_peer_health: Arc<kukuri_transport::BlobPeerHealth>,
    receive_binding: ReceiveBindingSlot,
    account_transfer: AccountTransfer,
    pub(crate) dome_session: DomeSessionSlot,
    pub(crate) dome_session_connections: DomeSessionConnections,
    pub(crate) network_work: Arc<crate::network_work::NetworkWorkRuntime>,
    /// 公開 blob の発見（#1632）。停止で外し、DHT と補助 index の client を止める。
    #[cfg(not(target_family = "wasm"))]
    public_blobs: std::sync::Mutex<Option<Arc<PublicBlobDiscovery>>>,
    shutdown_started: AtomicBool,
    shutdown_result: tokio::sync::watch::Sender<Option<std::result::Result<(), String>>>,
}

#[cfg(not(target_family = "wasm"))]
const ENDPOINT_SECRET_FORMAT_VERSION: u32 = 1;

/// endpoint secret の自前永続形式(WP-C5)。iroh::SecretKey の serde 表現
/// (ed25519_dalek へ素通しのバイト配列)に依存すると iroh 更新がディスク互換を
/// 壊しうるため、version + hex 32 bytes の自前スキーマで保存する。
/// 旧形式(`{"secret_key":[..]}`)の読み込み fallback は置かない — 本リリース前の
/// 破壊的変更。旧ファイルは parse 失敗で起動エラーになり、ファイルを削除すれば
/// 新しい endpoint ID で再生成される。
#[cfg(not(target_family = "wasm"))]
#[derive(Serialize, Deserialize)]
struct StoredEndpointSecret {
    version: u32,
    secret_key_hex: String,
}

/// 起動の入力。メモリの store では外から渡す（Web と試験）。
pub struct NodeOptions {
    pub network_config: TransportNetworkConfig,
    pub relay_config: TransportRelayConfig,
    /// 端末の endpoint の秘密鍵。無ければ新しく作る。
    pub secret_key: Option<iroh::SecretKey>,
    /// ブラウザとの直接経路（QUIC over WebRTC DataChannel。ADR 0057）。渡すと接続交渉の ALPN も受ける（#1422）。
    /// 交渉を始めるのは IP の transport を持たないブラウザの node だけ（ADR 0057 §9）。
    pub webrtc: Option<Arc<WebRtcTransport>>,
    /// 試験で、IP の transport を持たず交渉を始めるブラウザの端を native で作る。
    #[cfg(test)]
    pub(crate) browser_like: bool,
}

impl Default for NodeOptions {
    fn default() -> Self {
        Self {
            network_config: TransportNetworkConfig::loopback(),
            relay_config: TransportRelayConfig::default(),
            secret_key: None,
            webrtc: None,
            #[cfg(test)]
            browser_like: false,
        }
    }
}

impl IrohDocsNode {
    pub async fn memory() -> Result<Arc<Self>> {
        Self::memory_with(NodeOptions::default()).await
    }

    /// メモリの store で起動する。endpoint の秘密鍵と custom transport を外から渡す（ADR 0056 §2・§8）。
    pub async fn memory_with(options: NodeOptions) -> Result<Arc<Self>> {
        // Web は閉じた replica を drop し(docs-sync)、どの entry も指さない内容を上流の GC で消す。GC が読む(mark・sweep)
        // のは開いている replica のこの session の書込みと転送中の temp tag だけで、件数に比例しない(ADR 0058 §7)。
        // native の memory store(試験)は GC を使わない。
        #[cfg(target_family = "wasm")]
        let (store, protect) = {
            let (handler, protect) = ProtectCallbackHandler::new();
            let gc_config = iroh_blobs::store::GcConfig {
                interval: MEMORY_GC_INTERVAL,
                add_protected: Some(protect),
            };
            let options = iroh_blobs::store::mem::Options {
                gc_config: Some(gc_config),
            };
            (MemStore::new_with_opts(options), Some(handler))
        };
        #[cfg(not(target_family = "wasm"))]
        let (store, protect) = (MemStore::new(), None);
        Self::spawn(
            (*store).clone(),
            None,
            options,
            DhtDiscoveryOptions::disabled(),
            false,
            protect,
        )
        .await
    }

    #[cfg(not(target_family = "wasm"))]
    pub async fn persistent(root: impl AsRef<Path>) -> Result<Arc<Self>> {
        Self::persistent_with_config(root, TransportNetworkConfig::loopback()).await
    }

    #[cfg(not(target_family = "wasm"))]
    pub async fn persistent_with_config(
        root: impl AsRef<Path>,
        network_config: TransportNetworkConfig,
    ) -> Result<Arc<Self>> {
        Self::persistent_with_discovery_config(
            root,
            network_config,
            DhtDiscoveryOptions::disabled(),
            TransportRelayConfig::default(),
            false,
        )
        .await
    }

    /// `webrtc` なら、ブラウザからの接続交渉に応じる（desktop。ADR 0057 §9）。
    #[cfg(not(target_family = "wasm"))]
    pub async fn persistent_with_discovery_config(
        root: impl AsRef<Path>,
        network_config: TransportNetworkConfig,
        dht_options: DhtDiscoveryOptions,
        relay_config: TransportRelayConfig,
        webrtc: bool,
    ) -> Result<Arc<Self>> {
        Self::load_persistent(
            root.as_ref(),
            network_config,
            dht_options,
            relay_config,
            webrtc,
            true,
        )
        .await
    }

    /// Runtime repair reopens canonical data; it must never turn a failed open
    /// into an empty replacement store or generate a new endpoint identity.
    #[cfg(not(target_family = "wasm"))]
    pub async fn reopen_with_discovery_config(
        root: impl AsRef<Path>,
        network_config: TransportNetworkConfig,
        dht_options: DhtDiscoveryOptions,
        relay_config: TransportRelayConfig,
        webrtc: bool,
    ) -> Result<Arc<Self>> {
        let root = root.as_ref();
        for name in [
            DOCS_STORE_FILE_NAME,
            DEFAULT_AUTHOR_FILE_NAME,
            ENDPOINT_SECRET_FILE_NAME,
        ] {
            anyhow::ensure!(
                root.join(name).is_file(),
                "runtime repair requires the existing {name}"
            );
        }
        Self::load_persistent(
            root,
            network_config,
            dht_options,
            relay_config,
            webrtc,
            false,
        )
        .await
    }

    #[cfg(not(target_family = "wasm"))]
    async fn load_persistent(
        root: &Path,
        network_config: TransportNetworkConfig,
        dht_options: DhtDiscoveryOptions,
        relay_config: TransportRelayConfig,
        webrtc: bool,
        recover_corrupt_docs: bool,
    ) -> Result<Arc<Self>> {
        std::fs::create_dir_all(root)
            .with_context(|| format!("failed to create docs root {}", root.display()))?;
        let options = BlobStoreOptions::new(root);
        let store = iroh_blobs::store::fs::FsStore::load_with_opts(root.join("blobs.db"), options)
            .await
            .with_context(|| format!("failed to load blob store at {}", root.display()))?;
        let result = Self::spawn(
            (*store).clone(),
            Some(root.to_path_buf()),
            NodeOptions {
                webrtc: webrtc.then(|| {
                    WebRtcTransport::new(WebRtcConfig {
                        bind_ip: network_config.bind_addr.ip(),
                    })
                }),
                network_config,
                relay_config,
                secret_key: None,
                #[cfg(test)]
                browser_like: false,
            },
            dht_options,
            recover_corrupt_docs,
            None,
        )
        .await;
        if result.is_err() {
            // Early failures (e.g. relay parsing or bind) happen before a node
            // owns this store. Wait for its actor/DB to close before a caller
            // can reopen it; merely dropping the API races cleanup on Linux.
            // Docs startup may already have closed it; retain the original error.
            let _ = store.shutdown().await;
        }
        result
    }

    async fn spawn(
        store: impl Into<BlobStore>,
        root: Option<PathBuf>,
        options: NodeOptions,
        dht_options: DhtDiscoveryOptions,
        recover_corrupt_docs: bool,
        protect: Option<ProtectCallbackHandler>,
    ) -> Result<Arc<Self>> {
        let NodeOptions {
            network_config,
            relay_config,
            secret_key,
            webrtc,
            #[cfg(test)]
            browser_like,
        } = options;
        #[cfg(not(test))]
        let browser_like = false;
        // 交渉を始めるのは IP の transport を持たない端（ブラウザ）だけ（ADR 0057 §9）。
        let demand = webrtc
            .as_ref()
            .filter(|_| cfg!(target_family = "wasm") || browser_like)
            .map(|_| DemandHooks::default());
        let blobs = store.into();
        let discovery = Arc::new(MemoryLookup::new());
        #[cfg(not(target_family = "wasm"))]
        let secret_key = match root.as_deref() {
            Some(root) => load_endpoint_secret(root)?,
            None => secret_key,
        };
        let relay_config = relay_config.normalized();
        let relay_urls = Arc::new(StdRwLock::new(relay_config.parsed_relay_urls()?));
        let gossip_connection_paths = kukuri_transport::GossipConnectionPaths::default();
        // 公開 blob の発見は、住所の公開・解決と同じ DHT を使う（#1632）。
        #[cfg(not(target_family = "wasm"))]
        let mut dht_options = dht_options;
        #[cfg(not(target_family = "wasm"))]
        let public_blob_dht = match (
            dht_options.public_blob_index.clone(),
            dht_options.resolved_dht_builder(),
        ) {
            (Some(index), Some(builder)) => {
                let dht = builder.build().context("failed to start the DHT node")?;
                dht_options.dht = Some(dht.clone());
                Some((dht, index))
            }
            _ => None,
        };
        let mut endpoint_builder = build_endpoint_builder(
            EndpointBuilder::new(presets::Minimal).relay_mode(relay_config.relay_mode()?),
            &discovery,
            Some(&dht_options),
            Arc::clone(&relay_urls),
        )?;
        endpoint_builder = endpoint_builder.hooks(gossip_connection_paths.clone());
        #[cfg(test)]
        #[cfg(not(target_family = "wasm"))]
        {
            endpoint_builder = endpoint_builder.ca_tls_config(CaTlsConfig::insecure_skip_verify());
        }
        if let Some(secret_key) = secret_key {
            endpoint_builder = endpoint_builder.secret_key(secret_key);
        }
        if let Some(transport) = &webrtc {
            endpoint_builder = endpoint_builder.add_custom_transport(transport.clone());
        }
        if let Some(hooks) = &demand {
            endpoint_builder = endpoint_builder.hooks(hooks.clone());
        }
        // ブラウザには UDP の socket が無い（ADR 0056 §8）。
        #[cfg(not(target_family = "wasm"))]
        {
            endpoint_builder = match network_config.bind_addr {
                std::net::SocketAddr::V4(addr) => endpoint_builder.bind_addr(addr)?,
                std::net::SocketAddr::V6(addr) => endpoint_builder.bind_addr(addr)?,
            };
        }
        #[cfg(all(test, not(target_family = "wasm")))]
        if browser_like {
            endpoint_builder = endpoint_builder.clear_ip_transports();
        }
        #[cfg(target_family = "wasm")]
        let _ = &network_config;
        let endpoint = endpoint_builder
            .bind()
            .await
            .context("failed to bind iroh endpoint for docs node")?;
        #[cfg(not(target_family = "wasm"))]
        if let Some(root) = root.as_deref() {
            save_endpoint_secret(root, endpoint.secret_key())?;
        }
        prepare_endpoint_for_discovery(&endpoint, &discovery, &relay_config).await?;
        let gossip = Gossip::builder().spawn(endpoint.clone());
        let docs = match spawn_docs(
            root.as_deref(),
            endpoint.clone(),
            blobs.clone(),
            gossip.clone(),
            protect,
        )
        .await
        {
            Ok(docs) => docs,
            Err(error) => {
                #[cfg(not(target_family = "wasm"))]
                let error = if let Some(root) = root.as_deref().filter(|_| recover_corrupt_docs) {
                    recover_persistent_docs(
                        root,
                        endpoint.clone(),
                        blobs.clone(),
                        gossip.clone(),
                        error,
                    )
                    .await
                } else {
                    Err(error).context("failed to spawn iroh docs")
                };
                #[cfg(target_family = "wasm")]
                let error: Result<SpawnedDocs> = {
                    let _ = recover_corrupt_docs;
                    Err(error).context("failed to spawn iroh docs")
                };
                match error {
                    Ok(docs) => docs,
                    Err(error) => {
                        endpoint.close().await;
                        let _ = blobs.shutdown().await;
                        return Err(error);
                    }
                }
            }
        };
        #[cfg(not(target_family = "wasm"))]
        let public_blobs = public_blob_dht.map(|(dht, index)| {
            Arc::new(PublicBlobDiscovery::start(
                dht,
                index,
                endpoint.secret_key().clone(),
            ))
        });
        let receive_binding = ReceiveBindingSlot::new(endpoint.id());
        let account_transfer = AccountTransfer::new(endpoint.clone());
        let remote_cache = Arc::new(OnceLock::new());
        let private_capabilities = PrivateCapabilities::default();
        let page_read = DocReadProtocol::new(
            docs.sync,
            docs.protocol.api().clone(),
            blobs.clone(),
            remote_cache.clone(),
            private_capabilities.clone(),
        );
        let remote_blob = RemoteBlobProtocol::new(remote_cache.clone());
        let dome_session = DomeSessionSlot::default();
        let signaling = webrtc
            .map(|transport| {
                Signaling::new(
                    transport,
                    endpoint.clone(),
                    demand.as_ref(),
                    NEGOTIATION_DEADLINE,
                )
            })
            .transpose()?;
        let mut router = Router::builder(endpoint.clone())
            .accept(
                iroh_blobs::ALPN,
                iroh_blobs::BlobsProtocol::new(&blobs, None),
            )
            .accept(iroh_docs::ALPN, docs.protocol.clone())
            .accept(iroh_gossip::ALPN, gossip.clone())
            .accept(RECEIVE_BINDING_ALPN, receive_binding.clone())
            .accept(DOC_READ_ALPN, page_read)
            .accept(REMOTE_BLOB_ALPN, remote_blob)
            .accept(ACCOUNT_TRANSFER_ALPN, account_transfer.clone())
            .accept(DOME_SESSION_ALPN, dome_session.clone());
        if let Some(signaling) = &signaling {
            router = router.accept(SIGNALING_ALPN, signaling.clone());
        }
        let router = router.spawn();

        let node = Arc::new(Self {
            endpoint,
            gossip_connection_paths,
            gossip,
            discovery,
            relay_urls,
            router: Arc::new(router),
            signaling,
            docs: docs.protocol.api().clone(),
            blobs,
            remote_cache,
            private_capabilities,
            fetch_peer_health: Arc::new(kukuri_transport::BlobPeerHealth::default()),
            receive_binding,
            account_transfer,
            dome_session,
            dome_session_connections: DomeSessionConnections::default(),
            network_work: Arc::new(crate::network_work::NetworkWorkRuntime::default()),
            #[cfg(not(target_family = "wasm"))]
            public_blobs: std::sync::Mutex::new(public_blobs),
            shutdown_started: AtomicBool::new(false),
            shutdown_result: tokio::sync::watch::channel(None).0,
        });
        if relay_config.connect_mode() == ConnectMode::DirectOrRelay {
            node.apply_relay_config(relay_config.clone()).await?;
        }
        Ok(node)
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// ブラウザとの直接経路の接続交渉（`NodeOptions::webrtc` を渡したときだけ）。
    pub fn webrtc_signaling(&self) -> Option<&Arc<Signaling>> {
        self.signaling.as_ref()
    }

    pub fn install_remote_cache(&self, cache: Arc<dyn ContentCacheStore>) -> Result<()> {
        self.remote_cache
            .set(cache.clone())
            .map_err(|_| anyhow!("remote cache already installed"))?;
        // 告知の予定は、この cache（account の store）が持つ（#1632）。
        #[cfg(not(target_family = "wasm"))]
        if let Some(public_blobs) = self.public_blobs() {
            public_blobs.start_announcing(cache, self.network_work.clone());
        }
        Ok(())
    }

    #[cfg(not(target_family = "wasm"))]
    pub(crate) fn public_blobs(&self) -> Option<Arc<PublicBlobDiscovery>> {
        self.public_blobs
            .lock()
            .expect("public blob discovery poisoned")
            .clone()
    }

    /// 相手への private の応答で、要求の replica の secret を引く参照(docs-sync が入れる)。
    pub fn set_private_secret_lookup(&self, lookup: Arc<dyn PrivateSecretLookup>) {
        let _ = self.private_capabilities.lookup.set(lookup);
    }

    pub(crate) fn remote_cache(&self) -> Option<&Arc<dyn ContentCacheStore>> {
        self.remote_cache.get()
    }

    pub fn gossip(&self) -> &Gossip {
        &self.gossip
    }

    pub fn gossip_connection_paths(&self) -> kukuri_transport::GossipConnectionPaths {
        self.gossip_connection_paths.clone()
    }

    pub fn discovery(&self) -> Arc<MemoryLookup> {
        self.discovery.clone()
    }

    pub fn fetch_peer_health(&self) -> Arc<kukuri_transport::BlobPeerHealth> {
        self.fetch_peer_health.clone()
    }

    /// QR・専用リンクの移行（#1211）。招待と確認は endpoint ごとに 1 つ。
    pub fn account_transfer(&self) -> &AccountTransfer {
        &self.account_transfer
    }

    pub async fn install_receive_binding(&self, keys: Arc<kukuri_core::KukuriKeys>) -> Result<()> {
        self.receive_binding.install(keys).await
    }

    pub async fn relay_urls(&self) -> Vec<RelayUrl> {
        self.relay_urls
            .read()
            .expect("docs sync relay urls poisoned")
            .clone()
    }

    pub async fn apply_relay_config(&self, relay_config: TransportRelayConfig) -> Result<()> {
        let relay_config = relay_config.normalized();
        let next_relay_urls = relay_config.parsed_relay_urls()?;
        let current_relay_urls = self
            .relay_urls
            .read()
            .expect("docs sync relay urls poisoned")
            .clone();
        sync_endpoint_relay_config(&self.endpoint, &current_relay_urls, &next_relay_urls).await?;
        if !next_relay_urls.is_empty() && current_relay_urls != next_relay_urls {
            if current_relay_urls.is_empty() {
                let endpoint = self.endpoint.clone();
                n0_future::task::spawn(async move {
                    endpoint.online().await;
                });
            }
            let mut addr_watcher = self.endpoint.watch_addr();
            let expected_relays = next_relay_urls.iter().cloned().collect::<BTreeSet<_>>();
            let relay_ready = |addr: &EndpointAddr| {
                addr.relay_urls()
                    .any(|relay_url| expected_relays.contains(relay_url))
            };
            if !relay_ready(&addr_watcher.get()) {
                match timeout(relay_activation_timeout(), async move {
                    let mut stream = addr_watcher.stream();
                    while let Some(addr) = stream.next().await {
                        if relay_ready(&addr) {
                            return Ok::<(), anyhow::Error>(());
                        }
                    }
                    bail!("relay address watcher ended before any configured relay became active")
                })
                .await
                {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        warn!(
                            relay_urls = ?next_relay_urls,
                            error = %error,
                            "configured relay did not become active during initial wait"
                        );
                    }
                    Err(error) => {
                        warn!(
                            relay_urls = ?next_relay_urls,
                            error = %error,
                            "timed out waiting for live relay connectivity; continuing with configured relay"
                        );
                    }
                }
            }
        }
        *self
            .relay_urls
            .write()
            .expect("docs sync relay urls poisoned") = next_relay_urls;
        self.discovery.add_endpoint_info(self.endpoint.addr());
        Ok(())
    }

    pub fn docs(&self) -> &DocsApi {
        &self.docs
    }

    pub fn blobs(&self) -> &BlobStore {
        &self.blobs
    }

    pub async fn shutdown(self: Arc<Self>) -> Result<()> {
        self.network_work.close();
        self.receive_binding.reject_new_requests();
        // WebRTC の交渉の世代も終え、交渉中・開いた session を閉じる（#1214 AC-3）。
        if let Some(signaling) = &self.signaling {
            signaling.reset();
        }
        let mut result = self.shutdown_result.subscribe();
        if !self.shutdown_started.swap(true, Ordering::AcqRel) {
            let node = self.clone();
            // The owned task survives cancellation of a caller or outer timeout.
            n0_future::task::spawn(async move {
                let outcome = node
                    .shutdown_owned()
                    .await
                    .map_err(|error| error.to_string());
                node.shutdown_result.send_replace(Some(outcome));
            });
        }
        loop {
            if let Some(outcome) = result.borrow().clone() {
                return outcome.map_err(|message| anyhow!(message));
            }
            result
                .changed()
                .await
                .context("node shutdown result unavailable")?;
        }
    }

    async fn shutdown_owned(&self) -> Result<()> {
        self.network_work.close();
        // 公開 blob の発見を外して、共有の DHT を止める（住所の公開・解決は endpoint の終了で止まる。#1632）。
        #[cfg(not(target_family = "wasm"))]
        self.public_blobs
            .lock()
            .expect("public blob discovery poisoned")
            .take();
        self.account_transfer.cancel();
        self.receive_binding.clear().await;
        self.dome_session.install(None);
        // Flush before the router invokes BlobsProtocol::shutdown. A later
        // shutdown RPC may legitimately find that actor already closed.
        let blob_flush = self.blobs.sync_db().await;
        match timeout(router_shutdown_timeout(), self.router.shutdown()).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                warn!(error = %error, "failed to shut down iroh docs router cleanly");
            }
            Err(error) => {
                warn!(
                    error = %error,
                    "timed out shutting down iroh docs router; continuing endpoint close"
                );
            }
        }
        self.endpoint.close().await;
        let _ = self.blobs.shutdown().await;
        blob_flush?;
        Ok(())
    }
}

impl Drop for IrohDocsNode {
    fn drop(&mut self) {
        self.network_work.close();
        self.receive_binding.reject_new_requests();
        self.dome_session.install(None);
        if self.shutdown_started.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let router = Arc::clone(&self.router);
            let blobs = self.blobs.clone();
            let endpoint = self.endpoint.clone();
            handle.spawn(async move {
                let _ = router.shutdown().await;
                endpoint.close().await;
                let _ = blobs.shutdown().await;
            });
        }
    }
}

#[cfg(not(target_family = "wasm"))]
fn endpoint_secret_path(root: &Path) -> PathBuf {
    root.join(ENDPOINT_SECRET_FILE_NAME)
}

#[cfg(not(target_family = "wasm"))]
fn load_endpoint_secret(root: &Path) -> Result<Option<iroh::SecretKey>> {
    let path = endpoint_secret_path(root);
    if !path.exists() {
        return Ok(None);
    }
    let bytes = std::fs::read(&path)
        .with_context(|| format!("failed to read endpoint secret at {}", path.display()))?;
    let stored: StoredEndpointSecret = serde_json::from_slice(&bytes)
        .with_context(|| format!("failed to parse endpoint secret at {}", path.display()))?;
    if stored.version != ENDPOINT_SECRET_FORMAT_VERSION {
        bail!(
            "failed to parse endpoint secret at {}: unsupported version {}",
            path.display(),
            stored.version
        );
    }
    let decoded = hex::decode(stored.secret_key_hex.as_str())
        .with_context(|| format!("failed to parse endpoint secret at {}", path.display()))?;
    let secret_bytes: [u8; 32] = decoded.as_slice().try_into().map_err(|_| {
        anyhow!(
            "failed to parse endpoint secret at {}: secret must be 32 bytes",
            path.display()
        )
    })?;
    Ok(Some(iroh::SecretKey::from_bytes(&secret_bytes)))
}

#[cfg(not(target_family = "wasm"))]
fn save_endpoint_secret(root: &Path, secret_key: &iroh::SecretKey) -> Result<()> {
    let path = endpoint_secret_path(root);
    let bytes = serde_json::to_vec(&StoredEndpointSecret {
        version: ENDPOINT_SECRET_FORMAT_VERSION,
        secret_key_hex: hex::encode(secret_key.to_bytes()),
    })
    .with_context(|| format!("failed to serialize endpoint secret at {}", path.display()))?;
    std::fs::write(&path, bytes)
        .with_context(|| format!("failed to write endpoint secret at {}", path.display()))?;
    Ok(())
}
