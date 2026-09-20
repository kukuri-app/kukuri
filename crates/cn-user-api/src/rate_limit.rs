//! 公開 HTTP surface への per-client rate limit(設定は `RateLimitConfig`)。

use anyhow::{Context, Result};
use axum::Router;
use tower_governor::GovernorLayer;
use tower_governor::governor::GovernorConfigBuilder;
use tower_governor::key_extractor::{KeyExtractor, PeerIpKeyExtractor, SmartIpKeyExtractor};

use crate::config::RateLimitConfig;

/// Apply the rate limit layer to `router` when enabled. Layering returns a plain
/// `Router` regardless of the key-extractor type, so both branches unify cleanly.
pub fn apply_rate_limit(router: Router, config: &RateLimitConfig) -> Result<Router> {
    if !config.enabled {
        return Ok(router);
    }
    // key extractor の型だけが違う同一処理だったため generic で 1 本化(WP-H4)。
    // 転送ヘッダを信頼するのは信頼できる reverse proxy の背後だけ(X-Forwarded-For は
    // 直接公開時には攻撃者が制御できるため)。
    if config.trust_forwarded_for {
        apply_governor(router, config, SmartIpKeyExtractor)
    } else {
        apply_governor(router, config, PeerIpKeyExtractor)
    }
}

/// governor layer の構築と、idle bucket の定期回収 task の起動。
///
/// A background task periodically drops idle per-IP buckets so a flood of distinct
/// source IPs cannot grow the limiter's state without bound; `retain_recent` resolves
/// through the `Arc`'s deref to the concrete governor limiter, so no governor-internal
/// type needs to be named here.
fn apply_governor<K>(router: Router, config: &RateLimitConfig, key_extractor: K) -> Result<Router>
where
    K: KeyExtractor + Send + Sync + Clone + 'static,
    K::Key: Send + Sync + 'static,
{
    // The shared policy describes the subject window; tower-governor remains the HTTP/IP adapter
    // that owns atomic buckets and trusted-proxy extraction for this process.
    let policy = config.request_policy();
    let governor = GovernorConfigBuilder::default()
        .per_millisecond(
            (u64::try_from(policy.window.as_millis()).unwrap_or(u64::MAX) / policy.limit.max(1))
                .max(1),
        )
        .burst_size(config.burst)
        .key_extractor(key_extractor)
        .finish()
        .context("failed to build rate limit configuration")?;
    let limiter = governor.limiter().clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            interval.tick().await;
            limiter.retain_recent();
        }
    });
    Ok(router.layer(GovernorLayer::new(governor)))
}
