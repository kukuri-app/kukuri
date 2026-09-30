use super::*;

pub async fn prepare_display_file_fetch(
    node: &Arc<IrohDocsNode>,
    peers: &Arc<PeerAddrBook>,
    hash: iroh_blobs::Hash,
    path: std::path::PathBuf,
) -> Result<DisplayBlobFileFetch> {
    let deadline = Instant::now() + REMOTE_FETCH_TOTAL_TIMEOUT;
    let lease = node
        .network_work
        .acquire(*hash.as_bytes(), deadline)
        .await?;
    let node = node.clone();
    let peers = peers.clone();
    Ok(Box::pin(async move {
        let hash_text = hash.to_string();
        let walk = fetch_bytes_from_remote(
            &node,
            &peers,
            "displayed media",
            &hash_text,
            hash,
            "local blob unavailable",
            FetchMode::Ephemeral,
            Some(&path),
        );
        let result = tokio::select! {
            biased;
            _ = lease.cancelled() => Ok(None),
            result = n0_future::time::timeout(deadline.saturating_duration_since(Instant::now()), walk) => result.unwrap_or(Ok(None)),
        };
        if !lease.finish() {
            return Ok(None);
        }
        match result? {
            Some(_) => Ok(Some(tokio::fs::metadata(path).await?.len())),
            None => Ok(None),
        }
    }))
}
