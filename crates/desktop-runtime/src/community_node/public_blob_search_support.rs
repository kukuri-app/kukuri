//! Web の公開 blob の保持端末の検索（#1632 AC-6、ADR 0063 §8）。手元と既知の相手で取れない公開 blob の hash を、
//! 検索を提供する設定済みの Community Node（bootstrap の `public_blob_search`）へ送り、relay URL を持つ候補を取得の
//! 候補にする。送るのは session が成り立ち（同意が成立し）、token がある node だけ。未提供・失敗・満杯は候補なしとし、
//! 今までの取得を続ける。node へ差し込むのは Web だけ（native は DHT で探す）。

use std::sync::{Arc, Weak};
use std::time::Duration;

use iroh::{EndpointAddr, EndpointId, TransportAddr};
use kukuri_cn_protocol::{
    BLOB_PROVIDER_SEARCH_PATH, BlobProviderCandidate, BlobProviderSearchRequest,
    BlobProviderSearchResponse,
};
use kukuri_iroh_node::remote_fetch::PublicBlobProviders;
use n0_future::task::AbortOnDropHandle;

use super::*;

/// 送る期限の上限。CN の HTTP の期限（10 秒）の中で、打ち切った分の応答が届くようにする。
const SEARCH_BUDGET_CAP: Duration = Duration::from_secs(8);

impl DesktopRuntime {
    /// 検索を提供する設定済みの node の最初の 1 つへ、`hash` の保持端末を問い合わせる。
    pub(crate) async fn search_public_blob_providers(
        &self,
        hash: iroh_blobs::Hash,
        budget: Duration,
    ) -> Vec<EndpointAddr> {
        let budget = budget.min(SEARCH_BUDGET_CAP);
        let base_url = self
            .community_node_config
            .lock()
            .await
            .nodes
            .iter()
            .find(|node| {
                node.resolved_urls
                    .as_ref()
                    .is_some_and(|urls| urls.public_blob_search)
            })
            .map(|node| node.base_url.clone());
        let Some(base_url) = base_url.filter(|_| !budget.is_zero()) else {
            return Vec::new();
        };
        match self
            .request_public_blob_providers(&base_url, hash, budget)
            .await
        {
            Ok(candidates) => candidates.iter().filter_map(candidate_addr).collect(),
            Err(error) => {
                warn!(base_url = %base_url, %error, "public blob holder search failed");
                Vec::new()
            }
        }
    }

    async fn request_public_blob_providers(
        &self,
        base_url: &str,
        hash: iroh_blobs::Hash,
        budget: Duration,
    ) -> Result<Vec<BlobProviderCandidate>> {
        if !matches!(
            self.ensure_due_community_node_session(base_url).await?,
            CommunityNodeSessionOutcome::Ready
        ) {
            anyhow::bail!("community node session is not ready");
        }
        let token = load_community_node_token(&self.db_path, self.identity_mode, base_url)
            .await?
            .context("community node authentication is required")?;
        let request = BlobProviderSearchRequest {
            hash: hash.to_string(),
            budget_ms: u64::try_from(budget.as_millis()).unwrap_or(u64::MAX),
        };
        Ok(community_node_http_client()?
            .post(format!("{base_url}{BLOB_PROVIDER_SEARCH_PATH}"))
            .bearer_auth(token.access_token)
            .json(&request)
            .send()
            .await?
            .error_for_status()?
            .json::<BlobProviderSearchResponse>()
            .await?
            .candidates)
    }
}

/// 候補の到達情報（CN が署名つきの住所 record から読んだもの）。読めない値は捨てる。
fn candidate_addr(candidate: &BlobProviderCandidate) -> Option<EndpointAddr> {
    let id = candidate.endpoint_id.parse::<EndpointId>().ok()?;
    let relays = candidate
        .relay_urls
        .iter()
        .filter_map(|url| url.parse().ok())
        .map(TransportAddr::Relay);
    let direct = candidate
        .direct_addrs
        .iter()
        .filter_map(|addr| addr.parse().ok())
        .map(TransportAddr::Ip);
    Some(EndpointAddr::from_parts(id, relays.chain(direct)))
}

/// Web の node へ差し込む検索。runtime は弱い参照で持つ（runtime が止まった後は候補なし）。
struct CommunityNodePublicBlobSearch(Weak<DesktopRuntime>);

impl PublicBlobProviders for CommunityNodePublicBlobSearch {
    fn providers(
        &self,
        hash: iroh_blobs::Hash,
        budget: Duration,
    ) -> Option<n0_future::stream::Boxed<EndpointAddr>> {
        let runtime = self.0.upgrade()?;
        // ブラウザの HTTP は Send でないので別の task で待つ。stream を落とすと task ごと止める（要求も取り消す）。
        let search = AbortOnDropHandle::new(n0_future::task::spawn(async move {
            runtime.search_public_blob_providers(hash, budget).await
        }));
        Some(Box::pin(n0_future::StreamExt::flat_map(
            n0_future::stream::once_future(async move { search.await.unwrap_or_default() }),
            n0_future::stream::iter,
        )))
    }
}

impl DesktopRuntime {
    pub(crate) fn community_node_public_blob_search(
        self: &Arc<Self>,
    ) -> Arc<dyn PublicBlobProviders> {
        Arc::new(CommunityNodePublicBlobSearch(Arc::downgrade(self)))
    }

    /// 今の node と作り直す node へ、Community Node の検索を差し込む。
    #[cfg(target_family = "wasm")]
    pub(crate) async fn use_community_node_public_blob_search(self: &Arc<Self>) {
        let search = self.community_node_public_blob_search();
        if let Err(error) = self.iroh_stack.use_public_blob_providers(search).await {
            warn!(%error, "public blob holder search could not be installed");
        }
    }
}
