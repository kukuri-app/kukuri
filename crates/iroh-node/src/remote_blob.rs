//! Read a hash-addressed cached blob in bounded chunks without importing it
//! into the legacy iroh-blobs store.

#[cfg(not(target_family = "wasm"))]
use std::path::Path;
use std::str::FromStr;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use iroh::EndpointAddr;
use iroh::endpoint::{Connection, Endpoint};
use iroh::protocol::{AcceptError, ProtocolHandler};
use iroh_blobs::Hash;
use kukuri_store::ContentCacheStore;
use n0_future::time::timeout;
use tokio::sync::Semaphore;

use crate::remote_fetch::{BlobTooLarge, RemoteCacheDeferred};

pub(crate) const REMOTE_BLOB_ALPN: &[u8] = b"/kukuri/remote-blob/1";
const CHUNK_BYTES: usize = 1024 * 1024;
const DEADLINE: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub(crate) struct RemoteBlobProtocol {
    cache: Arc<OnceLock<Arc<dyn ContentCacheStore>>>,
    permits: Arc<Semaphore>,
}

impl std::fmt::Debug for RemoteBlobProtocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteBlobProtocol").finish_non_exhaustive()
    }
}

impl RemoteBlobProtocol {
    pub(crate) fn new(cache: Arc<OnceLock<Arc<dyn ContentCacheStore>>>) -> Self {
        Self {
            cache,
            permits: Arc::new(Semaphore::new(8)),
        }
    }

    async fn serve(&self, connection: &Connection) -> Result<()> {
        let (mut send, mut recv) = connection.accept_bi().await?;
        let request = recv.read_to_end(128).await?;
        let hash = Hash::from_str(std::str::from_utf8(&request)?)?;
        let Some(cache) = self.cache.get() else {
            send.write_all(&[0]).await?;
            send.finish()?;
            send.stopped().await?;
            return Ok(());
        };
        let Some(len) = cache.remote_content_len("blob", &hash.to_string()).await? else {
            send.write_all(&[0]).await?;
            send.finish()?;
            send.stopped().await?;
            return Ok(());
        };
        send.write_all(&[1]).await?;
        send.write_all(&len.to_be_bytes()).await?;
        let mut offset = 0;
        while offset < len {
            let chunk = cache
                .remote_content_chunk(
                    "blob",
                    &hash.to_string(),
                    offset,
                    CHUNK_BYTES.min(usize::try_from(len - offset)?),
                )
                .await?
                .context("cached blob disappeared during transfer")?;
            ensure!(
                !chunk.is_empty(),
                "cached blob ended before its declared size"
            );
            offset += u64::try_from(chunk.len())?;
            send.write_all(&chunk).await?;
        }
        send.finish()?;
        send.stopped().await?;
        Ok(())
    }
}

impl ProtocolHandler for RemoteBlobProtocol {
    async fn accept(&self, connection: Connection) -> std::result::Result<(), AcceptError> {
        let Ok(_permit) = self.permits.try_acquire() else {
            return Ok(());
        };
        timeout(DEADLINE, self.serve(&connection))
            .await
            .context("cached blob transfer timed out")
            .and_then(|result| result)
            .map_err(|error| AcceptError::from_boxed(error.into_boxed_dyn_error()))
    }
}

pub(crate) async fn fetch(
    endpoint: &Endpoint,
    peer: EndpointAddr,
    hash: Hash,
    max_bytes: Option<u64>,
    local_cache: Option<&dyn ContentCacheStore>,
) -> Result<Option<Vec<u8>>> {
    let mut bytes = Vec::new();
    let found = fetch_into(
        endpoint,
        peer,
        hash,
        max_bytes,
        local_cache,
        BlobOutput::Memory(&mut bytes),
    )
    .await?;
    Ok(found.map(|_| bytes))
}

#[cfg(not(target_family = "wasm"))]
pub(crate) async fn fetch_to_file(
    endpoint: &Endpoint,
    peer: EndpointAddr,
    hash: Hash,
    path: &Path,
) -> Result<Option<u64>> {
    let mut file = tokio::fs::File::create(path).await?;
    fetch_into(
        endpoint,
        peer,
        hash,
        None,
        None,
        BlobOutput::File(&mut file),
    )
    .await
}

/// 取得した blob の書き先。
pub(crate) enum BlobOutput<'a> {
    Memory(&'a mut Vec<u8>),
    /// 表示用の file への取得は native だけ（ADR 0056 §5）。
    #[cfg(not(target_family = "wasm"))]
    File(&'a mut tokio::fs::File),
}

impl BlobOutput<'_> {
    pub(crate) async fn write(&mut self, chunk: &[u8]) -> Result<()> {
        match self {
            Self::Memory(bytes) => bytes.extend_from_slice(chunk),
            #[cfg(not(target_family = "wasm"))]
            Self::File(file) => tokio::io::AsyncWriteExt::write_all(*file, chunk).await?,
        }
        Ok(())
    }

    pub(crate) async fn flush(&mut self) -> Result<()> {
        #[cfg(not(target_family = "wasm"))]
        if let Self::File(file) = self {
            tokio::io::AsyncWriteExt::flush(*file).await?;
        }
        Ok(())
    }
}

async fn fetch_into(
    endpoint: &Endpoint,
    peer: EndpointAddr,
    hash: Hash,
    max_bytes: Option<u64>,
    local_cache: Option<&dyn ContentCacheStore>,
    mut output: BlobOutput<'_>,
) -> Result<Option<u64>> {
    let connection = endpoint.connect(peer, REMOTE_BLOB_ALPN).await?;
    let (mut send, mut recv) = connection.open_bi().await?;
    send.write_all(hash.to_string().as_bytes()).await?;
    send.finish()?;
    let mut present = [0];
    recv.read_exact(&mut present).await?;
    if present[0] == 0 {
        connection.close(0u32.into(), b"cached blob missing");
        return Ok(None);
    }
    ensure!(present[0] == 1, "invalid cached blob response");
    let mut length = [0; 8];
    recv.read_exact(&mut length).await?;
    let length = u64::from_be_bytes(length);
    if let Some(limit) = max_bytes
        && length > limit
    {
        return Err(BlobTooLarge { limit }.into());
    }
    let mut reservation = local_cache.map(|cache| cache.empty_remote_cache_reservation());
    if let (Some(cache), Some(reservation)) = (local_cache, reservation.as_mut())
        && length <= cache.remote_cache_capacity()
        && !cache
            .reserve_remote_cache_bytes(reservation, length)
            .await?
    {
        return Err(RemoteCacheDeferred.into());
    }
    let mut received = 0u64;
    let mut hasher = blake3::Hasher::new();
    while received < length {
        let mut chunk = vec![0; CHUNK_BYTES.min(usize::try_from(length - received)?)];
        recv.read_exact(&mut chunk).await?;
        hasher.update(&chunk);
        output.write(&chunk).await?;
        received += u64::try_from(chunk.len())?;
    }
    ensure!(
        hasher.finalize().as_bytes() == hash.as_bytes(),
        "cached blob hash mismatch"
    );
    connection.close(0u32.into(), b"cached blob complete");
    Ok(Some(length))
}

#[cfg(test)]
#[cfg(not(target_family = "wasm"))]
mod tests {
    use super::*;
    use crate::IrohDocsNode;

    #[tokio::test]
    async fn cached_blob_is_reprovided_without_legacy_store_import() -> Result<()> {
        let provider = IrohDocsNode::memory().await?;
        let requester = IrohDocsNode::memory().await?;
        let cache = Arc::new(kukuri_store::SqliteStore::connect_memory().await?);
        provider.install_remote_cache(cache.clone())?;
        let bytes = b"cached remote bytes";
        let hash = Hash::new(bytes);
        ensure!(
            cache
                .put_remote_content("blob", &hash.to_string(), "blob", bytes)
                .await?,
            "fixture must fit"
        );
        assert!(!provider.blobs().blobs().has(hash).await?);
        assert_eq!(
            fetch(
                requester.endpoint(),
                provider.endpoint().addr(),
                hash,
                None,
                None
            )
            .await?,
            Some(bytes.to_vec())
        );
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("display.bin");
        assert_eq!(
            fetch_to_file(
                requester.endpoint(),
                provider.endpoint().addr(),
                hash,
                &path
            )
            .await?,
            Some(bytes.len() as u64)
        );
        assert_eq!(tokio::fs::read(path).await?, bytes);
        provider.shutdown().await?;
        requester.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn file_backed_cache_reprovides_without_loading_the_legacy_store() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let provider = IrohDocsNode::memory().await?;
        let requester = IrohDocsNode::memory().await?;
        let cache =
            Arc::new(kukuri_store::SqliteStore::connect_file(dir.path().join("cache.db")).await?);
        provider.install_remote_cache(cache.clone())?;
        let bytes = vec![5u8; 2 * 1024 * 1024 + 7];
        let source = dir.path().join("source.bin");
        tokio::fs::write(&source, &bytes).await?;
        let hash = Hash::new(&bytes);
        cache
            .put_remote_blob_file(&hash.to_string(), &source)
            .await?;
        assert!(!provider.blobs().blobs().has(hash).await?);
        let display = dir.path().join("display.bin");
        assert_eq!(
            fetch_to_file(
                requester.endpoint(),
                provider.endpoint().addr(),
                hash,
                &display
            )
            .await?,
            Some(bytes.len() as u64)
        );
        assert_eq!(tokio::fs::read(display).await?, bytes);
        provider.shutdown().await?;
        requester.shutdown().await?;
        Ok(())
    }
}
