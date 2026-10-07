//! 公開 blob の保持端末の候補の源（#1632、ADR 0063 §3・§8）。native は Mainline DHT の発見、Web は Community Node の
//! 検索を、既知の候補で取れない公開 blob の取得に差し込む。Web は導入前の公開記録の索引の取込みもここから進める。

use std::time::Duration;

use iroh::EndpointAddr;
use tracing::warn;

use crate::IrohDocsNode;

/// 導入前の公開記録の取込みで、1 回に読む行数（種類ごと）と、次の取込みまでの間。
pub(crate) const BACKFILL_ROWS: usize = 128;
pub(crate) const BACKFILL_PAUSE: Duration = Duration::from_millis(100);

/// Web: 導入前の公開記録を、公開参照の索引へ取り込み終えるまで小分けに進める（native は告知の task が行う）。
#[cfg(target_family = "wasm")]
pub(crate) async fn backfill_public_refs(
    cache: std::sync::Arc<dyn kukuri_store::ContentCacheStore>,
) {
    loop {
        match cache.backfill_public_blob_refs_step(BACKFILL_ROWS).await {
            Ok(true) => return,
            Ok(false) => {}
            Err(error) => warn!(%error, "public blob reference backfill failed"),
        }
        n0_future::time::sleep(BACKFILL_PAUSE).await;
    }
}

/// 保持端末の候補の源。
pub trait PublicBlobProviders: Send + Sync {
    /// `hash` の保持端末の候補を、見つかった順に返す。`budget` は取得に残る時間。探せなければ `None`。stream を落とすと
    /// 探すのを止める。
    fn providers(
        &self,
        hash: iroh_blobs::Hash,
        budget: Duration,
    ) -> Option<n0_future::stream::Boxed<EndpointAddr>>;
}

/// 保持端末の候補の源があり、`hash` が検証済みの公開記録に参照されているときだけ、保持端末の検索を始める。
pub(super) async fn public_blob_providers(
    node: &IrohDocsNode,
    hash: iroh_blobs::Hash,
    budget: Duration,
) -> Option<n0_future::stream::Boxed<EndpointAddr>> {
    let providers = node.public_blob_providers()?;
    match node.remote_cache()?.is_public_blob(&hash.to_string()).await {
        Ok(true) => providers.providers(hash, budget),
        Ok(false) => None,
        Err(error) => {
            warn!(hash = %hash, %error, "failed to check whether a blob is public");
            None
        }
    }
}
