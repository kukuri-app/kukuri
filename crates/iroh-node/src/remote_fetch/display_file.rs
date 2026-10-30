use super::*;

/// 取得して `path` へ書く。`transfer` は端末 1 件の転送の期限で、全体の期限はその 2 倍(既定の 30 秒より短くしない)。
pub async fn prepare_display_file_fetch(
    node: &Arc<IrohDocsNode>,
    peers: &Arc<PeerAddrBook>,
    hash: iroh_blobs::Hash,
    path: std::path::PathBuf,
    transfer: Duration,
) -> Result<DisplayBlobFileFetch> {
    let deadline = Instant::now() + REMOTE_FETCH_TOTAL_TIMEOUT.max(transfer * 2);
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
            transfer,
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
