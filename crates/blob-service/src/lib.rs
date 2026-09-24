use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::str::FromStr;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use kukuri_core::{BlobHash, VerifiedReceiveOffer};
use kukuri_iroh_node::{IrohDocsNode, remote_fetch};
use kukuri_transport::{
    EndpointAddr, PeerAddrBook, RemoteFetchRetryState, SeedPeer, parse_endpoint_ticket,
};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, RwLock};

pub use kukuri_iroh_node::remote_fetch::DisplayBlobFetch;

pub const DISPLAY_FETCH_TIMEOUT: std::time::Duration = remote_fetch::REMOTE_FETCH_TOTAL_TIMEOUT;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredBlob {
    pub hash: BlobHash,
    pub mime: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlobStatus {
    Missing,
    Available,
    Pinned,
}

#[async_trait]
pub trait BlobService: Send + Sync {
    async fn put_blob(&self, data: Vec<u8>, mime: &str) -> Result<StoredBlob>;
    async fn fetch_blob(&self, hash: &BlobHash) -> Result<Option<Vec<u8>>>;
    /// ローカルのbytesだけを読む。未対応の実装は取得不可とし、remoteへfallbackしない。
    async fn fetch_local_blob(&self, _hash: &BlobHash) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }
    /// 表示要求のfutureが取得を所有する。drop後に取得を続けず、bytesを保存しない。
    /// 未対応の実装は共有fetchへfallbackしない。
    async fn prepare_display_fetch(&self, _hash: &BlobHash) -> Result<DisplayBlobFetch> {
        anyhow::bail!("cancellable display fetch is not supported")
    }
    /// scan 用の一時取得（#609）: remote から取得した bytes を**ローカルストアへ残さない**。
    ///
    /// ローカルに既在の blob はそのまま読む（別目的で存在するものは消さない）。既定実装は
    /// `fetch_blob` に委譲する（in-memory 実装等、恒久保存の概念が無い実装向け）。
    async fn fetch_blob_ephemeral(&self, hash: &BlobHash) -> Result<Option<Vec<u8>>> {
        self.fetch_blob(hash).await
    }
    /// Bounded scan ingress. Implementations must reject before full allocation;
    /// an implementation without this contract is unavailable rather than an unbounded fallback.
    async fn fetch_blob_ephemeral_bounded(
        &self,
        _hash: &BlobHash,
        _max_bytes: u64,
    ) -> Result<Option<Vec<u8>>> {
        anyhow::bail!("bounded ephemeral blob fetch is not supported")
    }
    /// Only the signed provider may serve this offer's bounded manifest. The
    /// caller must check scope/mutual/epoch before starting this network I/O.
    async fn fetch_verified_receive_offer_payload(
        &self,
        _offer: &VerifiedReceiveOffer,
        _provider: EndpointAddr,
    ) -> Result<Vec<u8>> {
        anyhow::bail!("verified receive offer payload fetch is not supported")
    }
    async fn pin_blob(&self, hash: &BlobHash) -> Result<()>;
    async fn unpin_blob(&self, _hash: &BlobHash) -> Result<()> {
        Ok(())
    }
    async fn blob_status(&self, hash: &BlobHash) -> Result<BlobStatus>;
    /// ローカルの有無だけを返す状態確認（#1152）。remote から取得せず、bytes も読まない。
    ///
    /// `blob_status` はローカルに無い blob を remote から取得・永続化して確かめる。投稿添付の
    /// 状態確認でそれを使うと、成人向け表示の取得ゲート（ADR 0046 §4 / §6.2）を迂回するため、
    /// 表示用 projection の状態はこちらを使う。既定実装を置かないのは、`blob_status` への委譲で
    /// 黙って remote 取得へ戻る実装を作らないため。
    async fn local_blob_status(&self, hash: &BlobHash) -> Result<BlobStatus>;
    async fn import_peer_ticket(&self, ticket: &str) -> Result<()>;
    async fn learn_peer(&self, _endpoint_id: &str) -> Result<()> {
        Ok(())
    }
    async fn set_seed_peers(&self, _peers: Vec<SeedPeer>) -> Result<()> {
        Ok(())
    }
    async fn assist_peer_ids(&self) -> Result<Vec<String>> {
        Ok(Vec::new())
    }
}

#[derive(Clone)]
pub struct IrohBlobService {
    node: Arc<IrohDocsNode>,
    pinned: Arc<RwLock<HashSet<String>>>,
    // ピア台帳・接続候補・リトライ状態は kukuri-transport の共通実装(WP-H2)。
    // 台帳変化の bool は blob-service では使わない(レプリカへの配り直しが無いため)。
    peers: Arc<PeerAddrBook>,
    remote_fetch_retries: Arc<Mutex<RemoteFetchRetryState>>,
}

#[derive(Clone, Default)]
pub struct MemoryBlobService {
    blobs: Arc<RwLock<HashMap<String, Vec<u8>>>>,
    pinned: Arc<RwLock<HashSet<String>>>,
}

impl IrohBlobService {
    pub fn new(node: Arc<IrohDocsNode>) -> Self {
        let peers = Arc::new(PeerAddrBook::with_fetch_health(
            node.endpoint().clone(),
            node.discovery(),
            node.fetch_peer_health(),
        ));
        Self {
            node,
            pinned: Arc::new(RwLock::new(HashSet::new())),
            peers,
            remote_fetch_retries: Arc::new(Mutex::new(RemoteFetchRetryState::default())),
        }
    }

    // 本体ループは remote_fetch(WP-B14)へ移設。characterization テスト専用に残す。
    #[cfg(test)]
    async fn connect_candidates(
        &self,
        imported_peer: &iroh::EndpointAddr,
    ) -> Vec<iroh::EndpointAddr> {
        self.peers.connect_candidates(imported_peer).await
    }

    #[cfg(test)]
    async fn fetch_peers(&self) -> Vec<iroh::EndpointAddr> {
        self.peers.ranked_peers().await
    }

    pub fn with_account_store(
        node: Arc<IrohDocsNode>,
        store: Arc<kukuri_store::SqliteStore>,
    ) -> Self {
        let mut blobs = Self::new(node.clone());
        blobs.peers = Arc::new(PeerAddrBook::with_account_store(
            node.endpoint().clone(),
            node.discovery(),
            node.fetch_peer_health(),
            store,
            "blob",
        ));
        blobs
    }

    async fn is_pinned(&self, hash: &BlobHash) -> Result<bool> {
        Ok(self.pinned.read().await.contains(hash.as_str())
            || self
                .node
                .blobs()
                .tags()
                .get(metaverse_pin_tag(hash))
                .await?
                .is_some())
    }

    async fn available_fetch_peer_ids(&self) -> Vec<String> {
        self.peers.available_peer_ids().await
    }

    async fn record_learned_peer(&self, endpoint_id: &str) -> Result<()> {
        let relay_urls = self.node.relay_urls().await;
        // 台帳変化の bool は使わない(docs-sync と違い配り直す対象が無い)。
        let _ = self
            .peers
            .record_learned_peer(endpoint_id, &relay_urls)
            .await?;
        Ok(())
    }
}

#[async_trait]
impl BlobService for MemoryBlobService {
    async fn prepare_display_fetch(&self, hash: &BlobHash) -> Result<DisplayBlobFetch> {
        let bytes = self.fetch_local_blob(hash).await?;
        Ok(Box::pin(async move { Ok(bytes) }))
    }
    async fn fetch_local_blob(&self, hash: &BlobHash) -> Result<Option<Vec<u8>>> {
        Ok(self.blobs.read().await.get(hash.as_str()).cloned())
    }
    async fn fetch_blob_ephemeral_bounded(
        &self,
        hash: &BlobHash,
        max_bytes: u64,
    ) -> Result<Option<Vec<u8>>> {
        let blobs = self.blobs.read().await;
        match blobs.get(hash.as_str()) {
            Some(bytes) if bytes.len() as u64 > max_bytes => {
                Err(remote_fetch::BlobTooLarge { limit: max_bytes }.into())
            }
            bytes => Ok(bytes.cloned()),
        }
    }
    async fn put_blob(&self, data: Vec<u8>, mime: &str) -> Result<StoredBlob> {
        let hash = BlobHash::new(blake3::hash(&data).to_hex().to_string());
        self.blobs
            .write()
            .await
            .insert(hash.as_str().to_string(), data.clone());
        Ok(StoredBlob {
            hash,
            mime: mime.to_string(),
            bytes: data.len() as u64,
        })
    }

    async fn fetch_blob(&self, hash: &BlobHash) -> Result<Option<Vec<u8>>> {
        Ok(self.blobs.read().await.get(hash.as_str()).cloned())
    }

    async fn pin_blob(&self, hash: &BlobHash) -> Result<()> {
        self.pinned.write().await.insert(hash.as_str().to_string());
        Ok(())
    }

    async fn unpin_blob(&self, hash: &BlobHash) -> Result<()> {
        self.pinned.write().await.remove(hash.as_str());
        Ok(())
    }

    async fn blob_status(&self, hash: &BlobHash) -> Result<BlobStatus> {
        if self.pinned.read().await.contains(hash.as_str()) {
            return Ok(BlobStatus::Pinned);
        }
        Ok(match self.fetch_blob(hash).await? {
            Some(_) => BlobStatus::Available,
            None => BlobStatus::Missing,
        })
    }

    async fn local_blob_status(&self, hash: &BlobHash) -> Result<BlobStatus> {
        // in-memory 実装に remote は無いため、`blob_status` と同じくローカル参照だけで決まる。
        self.blob_status(hash).await
    }

    async fn import_peer_ticket(&self, _ticket: &str) -> Result<()> {
        Ok(())
    }
}

#[async_trait]
impl BlobService for IrohBlobService {
    async fn prepare_display_fetch(&self, hash: &BlobHash) -> Result<DisplayBlobFetch> {
        if let Some(bytes) = self.fetch_local_blob(hash).await? {
            return Ok(Box::pin(async move { Ok(Some(bytes)) }));
        }
        remote_fetch::prepare_display_fetch(
            &self.node,
            &self.peers,
            iroh_blobs::Hash::from_str(hash.as_str())?,
        )
        .await
    }
    async fn fetch_local_blob(&self, hash: &BlobHash) -> Result<Option<Vec<u8>>> {
        let hash = iroh_blobs::Hash::from_str(hash.as_str())?;
        Ok(self
            .node
            .blobs()
            .blobs()
            .get_bytes(hash)
            .await
            .ok()
            .map(|bytes| bytes.to_vec()))
    }
    async fn fetch_blob_ephemeral_bounded(
        &self,
        hash: &BlobHash,
        max_bytes: u64,
    ) -> Result<Option<Vec<u8>>> {
        use tokio::io::AsyncReadExt;
        let expected = iroh_blobs::Hash::from_str(hash.as_str())?;
        let mut bytes = Vec::new();
        let mut reader = self
            .node
            .blobs()
            .blobs()
            .reader(expected)
            .take(max_bytes.saturating_add(1));
        let result = match reader.read_to_end(&mut bytes).await {
            Ok(_) if bytes.len() as u64 > max_bytes => {
                return Err(remote_fetch::BlobTooLarge { limit: max_bytes }.into());
            }
            Ok(_) => Some(bytes),
            Err(_) => {
                // Free any partial local read before beginning a bounded remote transfer.
                drop(bytes);
                remote_fetch::fetch_bytes_ephemeral_bounded_with_cooldown(
                    &self.node,
                    &self.peers,
                    &self.remote_fetch_retries,
                    expected,
                    max_bytes,
                )
                .await?
            }
        };
        if let Some(bytes) = &result {
            anyhow::ensure!(
                iroh_blobs::Hash::new(bytes) == expected,
                "ephemeral blob hash mismatch"
            );
        }
        Ok(result)
    }
    async fn fetch_verified_receive_offer_payload(
        &self,
        offer: &VerifiedReceiveOffer,
        provider: EndpointAddr,
    ) -> Result<Vec<u8>> {
        remote_fetch::fetch_verified_receive_offer_payload(&self.node, offer, provider).await
    }
    async fn put_blob(&self, data: Vec<u8>, mime: &str) -> Result<StoredBlob> {
        let byte_len = data.len() as u64;
        let temp_tag = self.node.blobs().blobs().add_bytes(data).await?;
        Ok(StoredBlob {
            hash: BlobHash::new(temp_tag.hash.to_string()),
            mime: mime.to_string(),
            bytes: byte_len,
        })
    }

    async fn fetch_blob(&self, hash: &BlobHash) -> Result<Option<Vec<u8>>> {
        let hash_text = hash.as_str().to_string();
        let hash = iroh_blobs::Hash::from_str(hash.as_str())?;
        match self.node.blobs().blobs().get_bytes(hash).await {
            Ok(bytes) => Ok(Some(bytes.to_vec())),
            Err(error) => {
                // ループ本体(cooldown ゲート込み)は iroh-node の共通実装(WP-B14)。
                remote_fetch::fetch_bytes_with_cooldown(
                    &self.node,
                    &self.peers,
                    &self.remote_fetch_retries,
                    "blob",
                    hash_text.as_str(),
                    hash,
                    error,
                )
                .await
            }
        }
    }

    async fn fetch_blob_ephemeral(&self, hash: &BlobHash) -> Result<Option<Vec<u8>>> {
        let hash_text = hash.as_str().to_string();
        let hash = iroh_blobs::Hash::from_str(hash.as_str())?;
        match self.node.blobs().blobs().get_bytes(hash).await {
            Ok(bytes) => Ok(Some(bytes.to_vec())),
            Err(error) => {
                // remote からの取得はストアへ書き込まない(safety scan の一時 fetch。#609)。
                remote_fetch::fetch_bytes_ephemeral_with_cooldown(
                    &self.node,
                    &self.peers,
                    &self.remote_fetch_retries,
                    "blob",
                    hash_text.as_str(),
                    hash,
                    error,
                )
                .await
            }
        }
    }

    async fn pin_blob(&self, hash: &BlobHash) -> Result<()> {
        let parsed = iroh_blobs::Hash::from_str(hash.as_str())?;
        self.node
            .blobs()
            .tags()
            .set(metaverse_pin_tag(hash), parsed)
            .await?;
        self.pinned.write().await.insert(hash.as_str().to_string());
        Ok(())
    }

    async fn unpin_blob(&self, hash: &BlobHash) -> Result<()> {
        self.node
            .blobs()
            .tags()
            .delete(metaverse_pin_tag(hash))
            .await?;
        self.pinned.write().await.remove(hash.as_str());
        Ok(())
    }

    async fn blob_status(&self, hash: &BlobHash) -> Result<BlobStatus> {
        if self.is_pinned(hash).await? {
            return Ok(BlobStatus::Pinned);
        }
        Ok(match self.fetch_blob(hash).await? {
            Some(_) => BlobStatus::Available,
            None => BlobStatus::Missing,
        })
    }

    async fn local_blob_status(&self, hash: &BlobHash) -> Result<BlobStatus> {
        if self.is_pinned(hash).await? {
            return Ok(BlobStatus::Pinned);
        }
        let iroh_hash = iroh_blobs::Hash::from_str(hash.as_str())?;
        Ok(if self.node.blobs().blobs().has(iroh_hash).await? {
            BlobStatus::Available
        } else {
            BlobStatus::Missing
        })
    }

    async fn import_peer_ticket(&self, ticket: &str) -> Result<()> {
        let endpoint_addr = parse_endpoint_ticket(ticket)?;
        self.peers.insert_imported_peer_addr(endpoint_addr).await?;
        Ok(())
    }

    async fn learn_peer(&self, endpoint_id: &str) -> Result<()> {
        self.record_learned_peer(endpoint_id).await
    }

    async fn set_seed_peers(&self, peers: Vec<SeedPeer>) -> Result<()> {
        let relay_urls = self.node.relay_urls().await;
        self.peers.set_seed_peers(peers, &relay_urls).await
    }

    async fn assist_peer_ids(&self) -> Result<Vec<String>> {
        Ok(self.available_fetch_peer_ids().await)
    }
}

fn metaverse_pin_tag(hash: &BlobHash) -> Vec<u8> {
    format!("kukuri/metaverse/pin/{}", hash.as_str()).into_bytes()
}

pub const METAVERSE_BLOB_GC_GRACE_MILLIS: i64 = 24 * 60 * 60 * 1_000;
pub const DESKTOP_METAVERSE_BLOB_CACHE_CAPACITY_BYTES: u64 = 1024 * 1024 * 1024;
pub const COMMUNITY_NODE_METAVERSE_BLOB_CACHE_CAPACITY_BYTES: u64 = 10 * 1024 * 1024 * 1024;
pub const METAVERSE_ROLLBACK_REVISION_LIMIT: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetaverseBlobPinReason {
    Current,
    ActiveLease,
    Staging,
    Rollback,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MetaverseBlobPin {
    pub reason: MetaverseBlobPinReason,
    pub reference_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetaverseBlobCacheEntry {
    pub hash: BlobHash,
    pub bytes: u64,
    pub last_accessed_at: i64,
    pub unreferenced_at: Option<i64>,
    pub pins: BTreeSet<MetaverseBlobPin>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetaverseBlobCacheIndex {
    capacity_bytes: u64,
    entries: BTreeMap<BlobHash, MetaverseBlobCacheEntry>,
}

impl MetaverseBlobCacheIndex {
    pub fn new(capacity_bytes: u64) -> Result<Self> {
        if capacity_bytes == 0 {
            anyhow::bail!("metaverse blob cache capacity must be positive");
        }
        Ok(Self {
            capacity_bytes,
            entries: BTreeMap::new(),
        })
    }

    pub fn desktop() -> Self {
        Self::new(DESKTOP_METAVERSE_BLOB_CACHE_CAPACITY_BYTES)
            .expect("desktop metaverse blob cache capacity is positive")
    }

    pub fn community_node() -> Self {
        Self::new(COMMUNITY_NODE_METAVERSE_BLOB_CACHE_CAPACITY_BYTES)
            .expect("Community Node metaverse blob cache capacity is positive")
    }

    pub fn entries(&self) -> impl Iterator<Item = &MetaverseBlobCacheEntry> {
        self.entries.values()
    }

    pub fn total_bytes(&self) -> u64 {
        self.entries.values().map(|entry| entry.bytes).sum()
    }

    pub fn ensure_staging_capacity(&self, hashes: &[(BlobHash, u64)]) -> Result<()> {
        let additional = hashes
            .iter()
            .filter(|(hash, _)| !self.entries.contains_key(hash))
            .map(|(_, bytes)| *bytes)
            .sum::<u64>();
        if self.total_bytes().saturating_add(additional) > self.capacity_bytes {
            anyhow::bail!("metaverse manifest/asset blob cache capacity exceeded");
        }
        Ok(())
    }

    pub fn track_blob(&mut self, hash: BlobHash, bytes: u64, now_millis: i64) {
        self.entries
            .entry(hash.clone())
            .and_modify(|entry| {
                entry.bytes = entry.bytes.max(bytes);
                entry.last_accessed_at = now_millis;
            })
            .or_insert_with(|| MetaverseBlobCacheEntry {
                hash,
                bytes,
                last_accessed_at: now_millis,
                unreferenced_at: Some(now_millis),
                pins: BTreeSet::new(),
            });
    }

    pub fn pin(&mut self, hash: &BlobHash, bytes: u64, pin: MetaverseBlobPin, now_millis: i64) {
        self.track_blob(hash.clone(), bytes, now_millis);
        let entry = self.entries.get_mut(hash).expect("tracked above");
        entry.pins.insert(pin);
        entry.unreferenced_at = None;
        entry.last_accessed_at = now_millis;
    }

    pub fn unpin_reference(&mut self, pin: &MetaverseBlobPin, now_millis: i64) {
        for entry in self.entries.values_mut() {
            if entry.pins.remove(pin) && entry.pins.is_empty() {
                entry.unreferenced_at = Some(now_millis);
            }
        }
    }

    pub fn replace_reference(
        &mut self,
        from: &MetaverseBlobPin,
        to: MetaverseBlobPin,
        now_millis: i64,
    ) {
        for entry in self.entries.values_mut() {
            if entry.pins.remove(from) {
                entry.pins.insert(to.clone());
                entry.unreferenced_at = None;
                entry.last_accessed_at = now_millis;
            }
        }
    }

    pub fn touch(&mut self, hash: &BlobHash, now_millis: i64) {
        if let Some(entry) = self.entries.get_mut(hash) {
            entry.last_accessed_at = now_millis;
        }
    }

    pub fn collect_garbage(&mut self, now_millis: i64) -> Vec<BlobHash> {
        let mut candidates = self
            .entries
            .values()
            .filter(|entry| {
                entry.pins.is_empty()
                    && entry.unreferenced_at.is_some_and(|unreferenced_at| {
                        now_millis.saturating_sub(unreferenced_at) >= METAVERSE_BLOB_GC_GRACE_MILLIS
                    })
            })
            .map(|entry| (entry.last_accessed_at, entry.hash.clone()))
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| left.0.cmp(&right.0).then(left.1.cmp(&right.1)));
        let hashes = candidates
            .into_iter()
            .map(|(_, hash)| hash)
            .collect::<Vec<_>>();
        for hash in &hashes {
            self.entries.remove(hash);
        }
        hashes
    }
}

#[cfg(test)]
mod local_status_tests;

#[cfg(test)]
mod tests;
