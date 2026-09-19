use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock as StdRwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use futures_util::StreamExt;
use iroh::address_lookup::MemoryLookup;
use iroh::endpoint::{Builder as EndpointBuilder, presets};
use iroh::protocol::Router;
use iroh::{Endpoint, EndpointAddr, RelayUrl, Watcher};
use iroh_blobs::api::Store as BlobStore;
use iroh_blobs::store::{fs::options::Options as BlobStoreOptions, mem::MemStore};
use iroh_docs::api::DocsApi;
use iroh_gossip::net::Gossip;
use kukuri_transport::{
    ConnectMode, DhtDiscoveryOptions, TransportNetworkConfig, TransportRelayConfig,
    build_endpoint_builder, prepare_endpoint_for_discovery, sync_endpoint_relay_config,
};
use serde::{Deserialize, Serialize};
use tokio::time::timeout;
use tracing::warn;

#[cfg(test)]
use iroh::tls::CaTlsConfig;

const ENDPOINT_SECRET_FILE_NAME: &str = "endpoint-secret.json";
const DOCS_STORE_FILE_NAME: &str = "docs.redb";
const DEFAULT_AUTHOR_FILE_NAME: &str = "default-author";

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
) -> Result<iroh_docs::protocol::Docs> {
    let docs_builder = match root {
        Some(path) => iroh_docs::protocol::Docs::persistent(path.to_path_buf()),
        None => iroh_docs::protocol::Docs::memory(),
    };
    docs_builder.spawn(endpoint, blobs, gossip).await
}

async fn recover_persistent_docs(
    root: &Path,
    endpoint: Endpoint,
    blobs: BlobStore,
    gossip: Gossip,
    original_error: anyhow::Error,
) -> Result<iroh_docs::protocol::Docs> {
    let recovery_dir = move_corrupt_docs_store(root)
        .with_context(|| format!("failed to recover iroh docs store at {}", root.display()))?;
    warn!(
        root = %root.display(),
        recovery_dir = %recovery_dir.display(),
        error = %original_error,
        "recovering corrupt iroh docs store before retrying startup"
    );
    spawn_docs(
        Some(root),
        endpoint,
        blobs,
        gossip,
    )
    .await
    .with_context(|| {
        format!(
            "failed to spawn iroh docs after recovering corrupt store to {}; original error: {original_error:#}",
            recovery_dir.display()
        )
    })
}

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
    gossip: Gossip,
    discovery: Arc<MemoryLookup>,
    relay_urls: Arc<StdRwLock<Vec<RelayUrl>>>,
    router: Arc<Router>,
    docs: DocsApi,
    blobs: BlobStore,
    shutdown_started: AtomicBool,
    shutdown_result: tokio::sync::watch::Sender<Option<std::result::Result<(), String>>>,
}

const ENDPOINT_SECRET_FORMAT_VERSION: u32 = 1;

/// endpoint secret の自前永続形式(WP-C5)。iroh::SecretKey の serde 表現
/// (ed25519_dalek へ素通しのバイト配列)に依存すると iroh 更新がディスク互換を
/// 壊しうるため、version + hex 32 bytes の自前スキーマで保存する。
/// 旧形式(`{"secret_key":[..]}`)の読み込み fallback は置かない — 本リリース前の
/// 破壊的変更。旧ファイルは parse 失敗で起動エラーになり、ファイルを削除すれば
/// 新しい endpoint ID で再生成される。
#[derive(Serialize, Deserialize)]
struct StoredEndpointSecret {
    version: u32,
    secret_key_hex: String,
}

impl IrohDocsNode {
    pub async fn memory() -> Result<Arc<Self>> {
        let store = MemStore::new();
        Self::spawn(
            (*store).clone(),
            None,
            TransportNetworkConfig::loopback(),
            DhtDiscoveryOptions::disabled(),
            TransportRelayConfig::default(),
            false,
        )
        .await
    }

    pub async fn persistent(root: impl AsRef<Path>) -> Result<Arc<Self>> {
        Self::persistent_with_config(root, TransportNetworkConfig::loopback()).await
    }

    pub async fn persistent_with_config(
        root: impl AsRef<Path>,
        network_config: TransportNetworkConfig,
    ) -> Result<Arc<Self>> {
        Self::persistent_with_discovery_config(
            root,
            network_config,
            DhtDiscoveryOptions::disabled(),
            TransportRelayConfig::default(),
        )
        .await
    }

    pub async fn persistent_with_discovery_config(
        root: impl AsRef<Path>,
        network_config: TransportNetworkConfig,
        dht_options: DhtDiscoveryOptions,
        relay_config: TransportRelayConfig,
    ) -> Result<Arc<Self>> {
        Self::load_persistent(
            root.as_ref(),
            network_config,
            dht_options,
            relay_config,
            true,
        )
        .await
    }

    /// Runtime repair reopens canonical data; it must never turn a failed open
    /// into an empty replacement store or generate a new endpoint identity.
    pub async fn reopen_with_discovery_config(
        root: impl AsRef<Path>,
        network_config: TransportNetworkConfig,
        dht_options: DhtDiscoveryOptions,
        relay_config: TransportRelayConfig,
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
        Self::load_persistent(root, network_config, dht_options, relay_config, false).await
    }

    async fn load_persistent(
        root: &Path,
        network_config: TransportNetworkConfig,
        dht_options: DhtDiscoveryOptions,
        relay_config: TransportRelayConfig,
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
            network_config,
            dht_options,
            relay_config,
            recover_corrupt_docs,
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
        network_config: TransportNetworkConfig,
        dht_options: DhtDiscoveryOptions,
        relay_config: TransportRelayConfig,
        recover_corrupt_docs: bool,
    ) -> Result<Arc<Self>> {
        let blobs = store.into();
        let discovery = Arc::new(MemoryLookup::new());
        let endpoint_secret = root
            .as_deref()
            .map(load_endpoint_secret)
            .transpose()?
            .flatten();
        let relay_config = relay_config.normalized();
        let relay_urls = Arc::new(StdRwLock::new(relay_config.parsed_relay_urls()?));
        let mut endpoint_builder = build_endpoint_builder(
            EndpointBuilder::new(presets::Minimal).relay_mode(relay_config.relay_mode()?),
            &discovery,
            Some(&dht_options),
            Arc::clone(&relay_urls),
        )?;
        #[cfg(test)]
        {
            endpoint_builder = endpoint_builder.ca_tls_config(CaTlsConfig::insecure_skip_verify());
        }
        if let Some(secret_key) = endpoint_secret {
            endpoint_builder = endpoint_builder.secret_key(secret_key);
        }
        endpoint_builder = match network_config.bind_addr {
            std::net::SocketAddr::V4(addr) => endpoint_builder.bind_addr(addr)?,
            std::net::SocketAddr::V6(addr) => endpoint_builder.bind_addr(addr)?,
        };
        let endpoint = endpoint_builder
            .bind()
            .await
            .context("failed to bind iroh endpoint for docs node")?;
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
        )
        .await
        {
            Ok(docs) => docs,
            Err(error) => {
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
        let router = Router::builder(endpoint.clone())
            .accept(
                iroh_blobs::ALPN,
                iroh_blobs::BlobsProtocol::new(&blobs, None),
            )
            .accept(iroh_docs::ALPN, docs.clone())
            .accept(iroh_gossip::ALPN, gossip.clone())
            .spawn();

        let node = Arc::new(Self {
            endpoint,
            gossip,
            discovery,
            relay_urls,
            router: Arc::new(router),
            docs: docs.api().clone(),
            blobs,
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

    pub fn gossip(&self) -> &Gossip {
        &self.gossip
    }

    pub fn discovery(&self) -> Arc<MemoryLookup> {
        self.discovery.clone()
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
                tokio::spawn(async move {
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
        let mut result = self.shutdown_result.subscribe();
        if !self.shutdown_started.swap(true, Ordering::AcqRel) {
            let node = self.clone();
            // The owned task survives cancellation of a caller or outer timeout.
            tokio::spawn(async move {
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

fn endpoint_secret_path(root: &Path) -> PathBuf {
    root.join(ENDPOINT_SECRET_FILE_NAME)
}

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
