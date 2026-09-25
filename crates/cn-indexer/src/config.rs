//! cn-indexer の起動設定と relay validation 起動 gate（#413 / ADR 0025 §6.4）。
//!
//! indexing = Model C（docs replica sync participant）は peer discovery を成立させるために relay を
//! 前提にする。CN は relay 抜き構成を許容するため、indexing 起動時に relay を **config 検査**として
//! validate する。自前 relay（operator config の `features.iroh_relay` 有効）または外部 relay URL
//! （`external_relay_urls`）のどちらかが設定されていることを必須とし、両方未設定なら indexing を
//! 起動しない（fail-closed）。
//!
//! 到達性の実測（liveness probe）はしない。設定の有無のみで判定することで決定論的にテストでき、
//! `IrohDocsNode` の「relay 未活性でも継続」挙動と矛盾しない。

use anyhow::{Context, Result, bail};

use kukuri_cn_safety::{GeneralAction, Visibility};
use kukuri_cn_safety_runtime::{
    SAFETY_SIGNING_KEY_ENV, SafetyRuntimeConfig, SafetyRuntimeProviderEntry,
    SafetyRuntimeProvidersConfig,
};
use kukuri_transport::{SeedPeer, parse_seed_peer};

/// safety provider slot（#406。operator config の `safety.providers.*` と 1:1）の env 名。
/// 値は provider 実装名（現状 `mock` のみ。#391 / #411 で本番実装名を追加）。
/// `<SLOT>_REQUIRED`（bool）で required 宣言を併記できる。
pub const SAFETY_PROVIDER_KNOWN_CSAM_ENV: &str = "COMMUNITY_NODE_SAFETY_PROVIDER_KNOWN_CSAM";
pub const SAFETY_PROVIDER_GENERAL_ENV: &str = "COMMUNITY_NODE_SAFETY_PROVIDER_GENERAL";
pub const SAFETY_PROVIDER_UNKNOWN_CSAM_ENV: &str = "COMMUNITY_NODE_SAFETY_PROVIDER_UNKNOWN_CSAM";
/// signed moderation event を発行するか（operator config の
/// `safety.events.emit_signed_moderation_events` に対応。既定 true）。
pub const SAFETY_EMIT_SIGNED_EVENTS_ENV: &str = "COMMUNITY_NODE_SAFETY_EMIT_SIGNED_EVENTS";
/// signed event 無効時に risk signal issuer として使う node id(定数は cn-safety-runtime と共有)。
pub use kukuri_cn_safety_runtime::SAFETY_ISSUER_NODE_ID_ENV;
/// 自前 relay を cn-indexer の iroh runtime へ渡す公開 URL。
pub const CONNECTIVITY_URLS_ENV: &str = "COMMUNITY_NODE_CONNECTIVITY_URLS";
/// suspected 判定の classifier スコア閾値（1-100。未設定なら policy 既定 70。ADR 0028 §2.2）。
pub const SAFETY_SUSPECTED_THRESHOLD_ENV: &str = "COMMUNITY_NODE_SAFETY_SUSPECTED_THRESHOLD";
/// suspected advisory の配布 visibility（`local` / `subscribed_nodes` / `public`。
/// 未設定なら既定 `local`。ADR 0028 §2.4 / §2.7）。
pub const SAFETY_SUSPECTED_SIGNAL_VISIBILITY_ENV: &str =
    "COMMUNITY_NODE_SAFETY_SUSPECTED_SIGNAL_VISIBILITY";
/// nsfw / objectionable の suspected に対する action（`label` / `hold` / `exclude`。
/// 未設定なら既定 `label` = content advisory 付きで index。ADR 0028 §8.7）。`allow` は受理しない。
pub const SAFETY_GENERAL_ACTION_ENV: &str = "COMMUNITY_NODE_SAFETY_GENERAL_ACTION";

/// relay validation の結果。fail-closed gate の単一判定点。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelayValidation {
    /// 自前 relay（operator が iroh_relay capability を有効化）で成立。
    OwnRelay,
    /// 外部 relay URL 設定で成立。
    ExternalRelay,
    /// 自前 relay と外部 relay の両方で成立。
    OwnAndExternalRelay,
}

/// cn-indexer の relay 構成。
///
/// `has_own_relay` は operator config の `features.iroh_relay` 由来（自前 relay を提供する構成か）、
/// `external_relay_urls` は cn-indexer 自身が discovery / relay-assist に使う外部 relay URL。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RelayConfig {
    pub has_own_relay: bool,
    pub own_relay_urls: Vec<String>,
    pub external_relay_urls: Vec<String>,
}

impl RelayConfig {
    pub fn new(has_own_relay: bool, external_relay_urls: Vec<String>) -> Self {
        Self {
            has_own_relay,
            own_relay_urls: Vec::new(),
            external_relay_urls: normalize_relay_urls(external_relay_urls),
        }
    }

    pub fn with_own_relay_urls(mut self, own_relay_urls: Vec<String>) -> Self {
        self.own_relay_urls = normalize_relay_urls(own_relay_urls);
        self
    }

    /// iroh node へ実際に渡す relay URL。自前 / 外部を区別せず重複排除する。
    pub fn runtime_relay_urls(&self) -> Vec<String> {
        normalize_relay_urls(
            self.own_relay_urls
                .iter()
                .chain(self.external_relay_urls.iter())
                .cloned()
                .collect(),
        )
    }

    /// indexing 起動の relay validation gate（ADR 0025 §6.4）。
    ///
    /// 自前 relay も外部 relay も無ければ `Err`（indexing を起動しない）。どちらかが有れば
    /// どの経路で成立したかを返す。
    pub fn validate_for_startup(&self) -> Result<RelayValidation> {
        let has_own = self.has_own_relay && !self.own_relay_urls.is_empty();
        let has_external = !self.external_relay_urls.is_empty();
        match (has_own, has_external) {
            (true, true) => Ok(RelayValidation::OwnAndExternalRelay),
            (true, false) => Ok(RelayValidation::OwnRelay),
            (false, true) => Ok(RelayValidation::ExternalRelay),
            (false, false) => bail!(
                "cn-indexer requires a validated relay to start indexing: enable the node's own \
                 iroh_relay capability with COMMUNITY_NODE_CONNECTIVITY_URLS or configure \
                 COMMUNITY_NODE_INDEXER_EXTERNAL_RELAY_URLS"
            ),
        }
    }
}

/// 空白除去・重複排除した relay URL 一覧。
fn normalize_relay_urls(urls: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    urls.into_iter()
        .map(|url| url.trim().to_string())
        .filter(|url| !url.is_empty())
        .filter(|url| seen.insert(url.clone()))
        .collect()
}

/// cn-indexer の全体設定。
///
/// `Debug` は手動実装で `channel_secret_key` を秘匿する（誤ってログへ暗号鍵を出さない）。
#[derive(Clone)]
pub struct IndexerConfig {
    /// Postgres 接続 URL（supported set / request / channel secret の scope state）。
    pub database_url: String,
    /// docs / blob store の永続ディレクトリ。
    pub data_dir: std::path::PathBuf,
    /// relay 構成（起動 gate）。
    pub relay: RelayConfig,
    /// channel secret を復号する鍵 material（cn-user-api と同じ値）。
    pub channel_secret_key: String,
    /// ArcadeDB index 投影の接続設定。
    pub arcadedb: ArcadeDbConfig,
    /// safety scan runtime（#406）の構成。provider 未構成なら scan service は構築されない。
    pub safety: SafetyRuntimeConfig,
    /// media scan 用の一時 blob fetch（#609）の制限。
    pub media_fetch: MediaFetchConfig,
    /// docs replica sync / remote blob fetch のためのシードピア（#613 T1）。
    ///
    /// `COMMUNITY_NODE_INDEXER_SEED_PEERS`（カンマ区切り、`endpoint_id` または
    /// `endpoint_id@host:port`）から読む。不正な値は起動エラー（fail-closed）。
    pub seed_peers: Vec<SeedPeer>,
    /// 常駐ワーカーのscope窓巡回間隔（#613 T2）。
    pub poll_interval: std::time::Duration,
    /// 投稿取得・検査を同時に進める最大件数。
    pub max_concurrent_posts: usize,
    /// 観測状態の HTTP エンドポイントの待ち受けアドレス（#613 T3）。None なら公開しない。
    pub status_addr: Option<std::net::SocketAddr>,
}

impl std::fmt::Debug for IndexerConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IndexerConfig")
            .field("database_url", &self.database_url)
            .field("data_dir", &self.data_dir)
            .field("relay", &self.relay)
            .field("channel_secret_key", &"<redacted>")
            .field("arcadedb", &self.arcadedb)
            // SafetyRuntimeConfig の Debug は signing_key を秘匿する。
            .field("safety", &self.safety)
            .field("media_fetch", &self.media_fetch)
            .field("seed_peers", &self.seed_peers)
            .field("poll_interval", &self.poll_interval)
            .field("max_concurrent_posts", &self.max_concurrent_posts)
            .field("status_addr", &self.status_addr)
            .finish()
    }
}

/// media scan 用の一時 blob fetch（#609）の制限。
///
/// いずれも fail-closed 側のガード: 超過・時間切れは scan エラー（hold）になり、
/// allow へは落ちない。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaFetchConfig {
    /// scan 対象として取得を許す最大 bytes。超過は `ScanError::Protocol`（fail-closed）。
    pub max_bytes: u64,
    /// 一時 fetch 全体の timeout。超過は `ScanError::Timeout`（fail-closed）。
    pub timeout: std::time::Duration,
}

pub(crate) const MEDIA_FETCH_MAX_BYTES_ENV: &str = "COMMUNITY_NODE_MEDIA_FETCH_MAX_BYTES";
pub(crate) const MEDIA_FETCH_TIMEOUT_SECS_ENV: &str = "COMMUNITY_NODE_MEDIA_FETCH_TIMEOUT_SECS";

/// docs replica sync / remote blob fetch のシードピア指定（#613 T1）。
/// カンマ区切りで `endpoint_id` または `endpoint_id@host:port` を並べる。
pub const SEED_PEERS_ENV: &str = "COMMUNITY_NODE_INDEXER_SEED_PEERS";

/// 常駐ワーカーのscope窓巡回間隔（秒。#613 T2）。未設定なら既定 300 秒。0 は起動エラー。
pub const POLL_INTERVAL_SECS_ENV: &str = "COMMUNITY_NODE_INDEXER_POLL_INTERVAL_SECS";
pub const MAX_CONCURRENT_POSTS_ENV: &str = "COMMUNITY_NODE_INDEXER_MAX_CONCURRENT_POSTS";

/// 観測状態の HTTP エンドポイントの待ち受けアドレス（#613 T3）。
/// 未設定なら HTTP では公開しない。例: `127.0.0.1:8630`。
pub const STATUS_ADDR_ENV: &str = "COMMUNITY_NODE_INDEXER_STATUS_ADDR";

/// scope窓巡回間隔の既定値。
pub const DEFAULT_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(300);

impl Default for MediaFetchConfig {
    fn default() -> Self {
        Self {
            max_bytes: 32 * 1024 * 1024,
            timeout: std::time::Duration::from_secs(30),
        }
    }
}

impl MediaFetchConfig {
    /// 環境変数（`COMMUNITY_NODE_MEDIA_FETCH_*`）から制限を読む。不正値は起動エラー。
    pub fn from_env() -> Result<Self> {
        let default = Self::default();
        let max_bytes = match non_empty_env(MEDIA_FETCH_MAX_BYTES_ENV) {
            Some(value) => value.parse::<u64>().with_context(|| {
                format!("{MEDIA_FETCH_MAX_BYTES_ENV} must be a positive integer (bytes)")
            })?,
            None => default.max_bytes,
        };
        if max_bytes == 0 {
            bail!("{MEDIA_FETCH_MAX_BYTES_ENV} must not be zero");
        }
        let timeout_secs = match non_empty_env(MEDIA_FETCH_TIMEOUT_SECS_ENV) {
            Some(value) => value.parse::<u64>().with_context(|| {
                format!("{MEDIA_FETCH_TIMEOUT_SECS_ENV} must be a positive integer (seconds)")
            })?,
            None => default.timeout.as_secs(),
        };
        if timeout_secs == 0 {
            bail!("{MEDIA_FETCH_TIMEOUT_SECS_ENV} must not be zero");
        }
        Ok(Self {
            max_bytes,
            timeout: std::time::Duration::from_secs(timeout_secs),
        })
    }
}

/// ArcadeDB index 投影の接続設定。
///
/// `Debug` は手動実装で `password` を秘匿する。
#[derive(Clone, PartialEq, Eq)]
pub struct ArcadeDbConfig {
    pub base_url: String,
    pub database: String,
    pub username: String,
    pub password: String,
}

impl std::fmt::Debug for ArcadeDbConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArcadeDbConfig")
            .field("base_url", &self.base_url)
            .field("database", &self.database)
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .finish()
    }
}

impl ArcadeDbConfig {
    /// 環境変数（`COMMUNITY_NODE_ARCADEDB_*`）から接続設定を読む。
    ///
    /// cn-indexer（投影の書き込み）と cn-user-api（query 境界の読み出し、#404）が同じ env を
    /// 共有する。
    pub fn from_env() -> Self {
        Self {
            base_url: std::env::var("COMMUNITY_NODE_ARCADEDB_URL")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| "http://127.0.0.1:2480".to_string()),
            database: std::env::var("COMMUNITY_NODE_ARCADEDB_DATABASE")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| "kukuri_index".to_string()),
            username: std::env::var("COMMUNITY_NODE_ARCADEDB_USER")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| "root".to_string()),
            password: std::env::var("COMMUNITY_NODE_ARCADEDB_PASSWORD").unwrap_or_default(),
        }
    }
}

impl IndexerConfig {
    /// 環境変数から設定を読む。
    ///
    /// relay validation gate（§6.4）は起動側（`run_from_env`）で `relay.validate_for_startup()` を
    /// 呼んで適用する。ここでは値の読み取りのみを行う。
    pub fn from_env() -> Result<Self> {
        let database_url = std::env::var("COMMUNITY_NODE_DATABASE_URL")
            .context("COMMUNITY_NODE_DATABASE_URL is required")?;
        let data_dir = std::env::var("COMMUNITY_NODE_INDEXER_DATA_DIR")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "./data/cn-indexer".to_string())
            .into();
        let has_own_relay =
            kukuri_cn_core::parse_bool_env("COMMUNITY_NODE_INDEXER_OWN_RELAY", false)?;
        let own_relay_urls = if has_own_relay {
            kukuri_cn_core::parse_csv_env(CONNECTIVITY_URLS_ENV)
        } else {
            Vec::new()
        };
        let external_relay_urls =
            kukuri_cn_core::parse_csv_env("COMMUNITY_NODE_INDEXER_EXTERNAL_RELAY_URLS");
        let channel_secret_key = std::env::var("COMMUNITY_NODE_CHANNEL_SECRET_KEY")
            .context("COMMUNITY_NODE_CHANNEL_SECRET_KEY is required")?;
        let arcadedb = ArcadeDbConfig::from_env();
        let safety = SafetyRuntimeConfig {
            providers: SafetyRuntimeProvidersConfig {
                known_csam: safety_provider_entry(
                    non_empty_env(SAFETY_PROVIDER_KNOWN_CSAM_ENV),
                    kukuri_cn_core::parse_bool_env(
                        &format!("{SAFETY_PROVIDER_KNOWN_CSAM_ENV}_REQUIRED"),
                        false,
                    )?,
                ),
                general: safety_provider_entry(
                    non_empty_env(SAFETY_PROVIDER_GENERAL_ENV),
                    kukuri_cn_core::parse_bool_env(
                        &format!("{SAFETY_PROVIDER_GENERAL_ENV}_REQUIRED"),
                        false,
                    )?,
                ),
                unknown_csam: safety_provider_entry(
                    non_empty_env(SAFETY_PROVIDER_UNKNOWN_CSAM_ENV),
                    kukuri_cn_core::parse_bool_env(
                        &format!("{SAFETY_PROVIDER_UNKNOWN_CSAM_ENV}_REQUIRED"),
                        false,
                    )?,
                ),
            },
            signing_key: non_empty_env(SAFETY_SIGNING_KEY_ENV),
            emit_signed_events: kukuri_cn_core::parse_bool_env(
                SAFETY_EMIT_SIGNED_EVENTS_ENV,
                true,
            )?,
            issuer_node_id: non_empty_env(SAFETY_ISSUER_NODE_ID_ENV),
            suspected_threshold: parse_suspected_threshold_env()?,
            suspected_signal_visibility: parse_suspected_signal_visibility_env()?,
            general_action: parse_general_action_env()?,
        };
        Ok(Self {
            database_url,
            data_dir,
            relay: RelayConfig::new(has_own_relay, external_relay_urls)
                .with_own_relay_urls(own_relay_urls),
            channel_secret_key,
            arcadedb,
            safety,
            media_fetch: MediaFetchConfig::from_env()?,
            seed_peers: parse_seed_peers_env()?,
            poll_interval: parse_poll_interval_env()?,
            max_concurrent_posts: parse_max_concurrent_posts_env()?,
            status_addr: parse_status_addr_env()?,
        })
    }
}

fn parse_max_concurrent_posts_env() -> Result<usize> {
    let Some(raw) = non_empty_env(MAX_CONCURRENT_POSTS_ENV) else {
        return Ok(4);
    };
    let value = raw
        .parse::<usize>()
        .with_context(|| format!("{MAX_CONCURRENT_POSTS_ENV} must be a positive integer"))?;
    if value == 0 {
        bail!("{MAX_CONCURRENT_POSTS_ENV} must not be zero");
    }
    Ok(value)
}

/// scope窓巡回間隔 env を読む。0 や非数値は起動エラー（fail-closed）。
fn parse_poll_interval_env() -> Result<std::time::Duration> {
    let Some(raw) = non_empty_env(POLL_INTERVAL_SECS_ENV) else {
        return Ok(DEFAULT_POLL_INTERVAL);
    };
    let secs: u64 = raw.parse().with_context(|| {
        format!("{POLL_INTERVAL_SECS_ENV} must be a positive integer (seconds)")
    })?;
    if secs == 0 {
        bail!("{POLL_INTERVAL_SECS_ENV} must not be zero");
    }
    Ok(std::time::Duration::from_secs(secs))
}

/// 状態エンドポイントの待ち受けアドレス env を読む。不正な値は起動エラー（fail-closed）。
fn parse_status_addr_env() -> Result<Option<std::net::SocketAddr>> {
    let Some(raw) = non_empty_env(STATUS_ADDR_ENV) else {
        return Ok(None);
    };
    let addr = raw.parse::<std::net::SocketAddr>().with_context(|| {
        format!("{STATUS_ADDR_ENV} must be a socket address like 127.0.0.1:8630")
    })?;
    Ok(Some(addr))
}

/// シードピア env を読む。未設定・空は「シード無し」。不正な値は起動エラー（fail-closed）。
fn parse_seed_peers_env() -> Result<Vec<SeedPeer>> {
    match non_empty_env(SEED_PEERS_ENV) {
        Some(value) => parse_seed_peers_csv(value.as_str()),
        None => Ok(Vec::new()),
    }
}

/// カンマ区切りのシードピア指定を読み取る。空要素は無視し、同じ endpoint id は先勝ちで 1 つにする。
pub(crate) fn parse_seed_peers_csv(value: &str) -> Result<Vec<SeedPeer>> {
    let mut peers: Vec<SeedPeer> = Vec::new();
    for entry in value.split(',') {
        let trimmed = entry.trim();
        if trimmed.is_empty() {
            continue;
        }
        let peer = parse_seed_peer(trimmed)
            .with_context(|| format!("invalid seed peer entry `{trimmed}` in {SEED_PEERS_ENV}"))?;
        if !peers
            .iter()
            .any(|existing| existing.endpoint_id == peer.endpoint_id)
        {
            peers.push(peer);
        }
    }
    Ok(peers)
}

/// 空 / 空白のみの env は未設定として扱う。
fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// suspected 閾値 env を読む（1-100 の整数のみ受理。空 / 未設定は policy 既定に委ねる）。
fn parse_suspected_threshold_env() -> Result<Option<u8>> {
    let Some(raw) = non_empty_env(SAFETY_SUSPECTED_THRESHOLD_ENV) else {
        return Ok(None);
    };
    let threshold: u8 = raw.parse().with_context(|| {
        format!("{SAFETY_SUSPECTED_THRESHOLD_ENV} must be an integer between 1 and 100")
    })?;
    if threshold == 0 || threshold > 100 {
        bail!("{SAFETY_SUSPECTED_THRESHOLD_ENV} must be between 1 and 100 (got {threshold})");
    }
    Ok(Some(threshold))
}

/// suspected advisory visibility env を読む（`local` / `subscribed_nodes` / `public`）。
fn parse_suspected_signal_visibility_env() -> Result<Option<Visibility>> {
    let Some(raw) = non_empty_env(SAFETY_SUSPECTED_SIGNAL_VISIBILITY_ENV) else {
        return Ok(None);
    };
    match raw.to_ascii_lowercase().as_str() {
        "local" => Ok(Some(Visibility::Local)),
        "subscribed_nodes" => Ok(Some(Visibility::SubscribedNodes)),
        "public" => Ok(Some(Visibility::Public)),
        other => bail!(
            "{SAFETY_SUSPECTED_SIGNAL_VISIBILITY_ENV} must be one of \
             `local` / `subscribed_nodes` / `public` (got `{other}`)"
        ),
    }
}

/// nsfw / objectionable の suspected に対する action env を読む（`label` / `hold` / `exclude`）。
///
/// ラベル無しの `allow` と未知値は起動エラー（`general_action_operator_tunable_stricter_only`）。
fn parse_general_action_env() -> Result<Option<GeneralAction>> {
    let Some(raw) = non_empty_env(SAFETY_GENERAL_ACTION_ENV) else {
        return Ok(None);
    };
    match raw.to_ascii_lowercase().as_str() {
        "label" => Ok(Some(GeneralAction::Label)),
        "hold" => Ok(Some(GeneralAction::Hold)),
        "exclude" => Ok(Some(GeneralAction::Exclude)),
        other => bail!(
            "{SAFETY_GENERAL_ACTION_ENV} must be one of `label` / `hold` / `exclude` \
             (got `{other}`; `allow` without a content advisory is not accepted)"
        ),
    }
}

/// provider slot env の値から entry を組む。空 / 空白は「slot 未構成」。
fn safety_provider_entry(
    provider: Option<String>,
    required: bool,
) -> Option<SafetyRuntimeProviderEntry> {
    provider.map(|provider| SafetyRuntimeProviderEntry { provider, required })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::{Arc, Mutex, OnceLock};

    const TEST_SECRET: &str = "0000000000000000000000000000000000000000000000000000000000000001";
    const SAFETY_PROVIDER_KNOWN_CSAM_REQUIRED_ENV: &str =
        "COMMUNITY_NODE_SAFETY_PROVIDER_KNOWN_CSAM_REQUIRED";
    const SAFETY_PROVIDER_GENERAL_REQUIRED_ENV: &str =
        "COMMUNITY_NODE_SAFETY_PROVIDER_GENERAL_REQUIRED";
    const SAFETY_PROVIDER_UNKNOWN_CSAM_REQUIRED_ENV: &str =
        "COMMUNITY_NODE_SAFETY_PROVIDER_UNKNOWN_CSAM_REQUIRED";
    const INDEXER_ENV_KEYS: &[&str] = &[
        "COMMUNITY_NODE_DATABASE_URL",
        "COMMUNITY_NODE_INDEXER_DATA_DIR",
        "COMMUNITY_NODE_INDEXER_OWN_RELAY",
        CONNECTIVITY_URLS_ENV,
        "COMMUNITY_NODE_INDEXER_EXTERNAL_RELAY_URLS",
        "COMMUNITY_NODE_CHANNEL_SECRET_KEY",
        "COMMUNITY_NODE_ARCADEDB_URL",
        "COMMUNITY_NODE_ARCADEDB_DATABASE",
        "COMMUNITY_NODE_ARCADEDB_USER",
        "COMMUNITY_NODE_ARCADEDB_PASSWORD",
        SAFETY_PROVIDER_KNOWN_CSAM_ENV,
        SAFETY_PROVIDER_KNOWN_CSAM_REQUIRED_ENV,
        SAFETY_PROVIDER_GENERAL_ENV,
        SAFETY_PROVIDER_GENERAL_REQUIRED_ENV,
        SAFETY_PROVIDER_UNKNOWN_CSAM_ENV,
        SAFETY_PROVIDER_UNKNOWN_CSAM_REQUIRED_ENV,
        SAFETY_SIGNING_KEY_ENV,
        SAFETY_EMIT_SIGNED_EVENTS_ENV,
        SAFETY_ISSUER_NODE_ID_ENV,
        SAFETY_SUSPECTED_THRESHOLD_ENV,
        SAFETY_SUSPECTED_SIGNAL_VISIBILITY_ENV,
        SAFETY_GENERAL_ACTION_ENV,
        MEDIA_FETCH_MAX_BYTES_ENV,
        MEDIA_FETCH_TIMEOUT_SECS_ENV,
        SEED_PEERS_ENV,
        POLL_INTERVAL_SECS_ENV,
        MAX_CONCURRENT_POSTS_ENV,
        STATUS_ADDR_ENV,
    ];

    pub(crate) fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .expect("env lock")
    }

    fn with_clean_indexer_env<T>(f: impl FnOnce() -> T) -> T {
        let _guard = env_lock();
        let snapshot: Vec<(&'static str, Option<String>)> = INDEXER_ENV_KEYS
            .iter()
            .map(|key| (*key, std::env::var(key).ok()))
            .collect();
        for key in INDEXER_ENV_KEYS {
            unsafe { std::env::remove_var(key) };
        }

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));

        for (key, value) in snapshot {
            match value {
                Some(value) => unsafe { std::env::set_var(key, value) },
                None => unsafe { std::env::remove_var(key) },
            }
        }
        match result {
            Ok(value) => value,
            Err(panic) => std::panic::resume_unwind(panic),
        }
    }

    fn set_minimal_indexer_env() {
        unsafe {
            std::env::set_var("COMMUNITY_NODE_DATABASE_URL", "postgres://example");
            std::env::set_var("COMMUNITY_NODE_CHANNEL_SECRET_KEY", "test-channel-secret");
        }
    }

    #[test]
    fn startup_fails_without_own_or_external_relay() {
        let config = RelayConfig::new(false, vec![]);
        assert!(config.validate_for_startup().is_err());
    }

    #[test]
    fn startup_succeeds_with_own_relay() {
        let config = RelayConfig::new(true, vec![])
            .with_own_relay_urls(vec!["https://relay.example.net".to_string()]);
        assert_eq!(
            config.validate_for_startup().unwrap(),
            RelayValidation::OwnRelay
        );
    }

    #[test]
    fn own_relay_env_supplies_runtime_relay_url() {
        with_clean_indexer_env(|| {
            set_minimal_indexer_env();
            unsafe {
                std::env::set_var("COMMUNITY_NODE_INDEXER_OWN_RELAY", "true");
                std::env::set_var(
                    "COMMUNITY_NODE_CONNECTIVITY_URLS",
                    "https://iroh-relay.example.net",
                );
            }

            let config = IndexerConfig::from_env().unwrap();
            assert_eq!(
                config.relay.own_relay_urls,
                vec!["https://iroh-relay.example.net"]
            );
        });
    }

    #[test]
    fn startup_succeeds_with_external_relay() {
        let config = RelayConfig::new(false, vec!["https://relay.example.net".to_string()]);
        assert_eq!(
            config.validate_for_startup().unwrap(),
            RelayValidation::ExternalRelay
        );
    }

    #[test]
    fn startup_reports_both_when_own_and_external_present() {
        let config = RelayConfig::new(true, vec!["https://external-relay.example.net".to_string()])
            .with_own_relay_urls(vec!["https://own-relay.example.net".to_string()]);
        assert_eq!(
            config.validate_for_startup().unwrap(),
            RelayValidation::OwnAndExternalRelay
        );
    }

    #[test]
    fn blank_external_relay_urls_do_not_satisfy_gate() {
        let config = RelayConfig::new(false, vec!["   ".to_string(), String::new()]);
        assert!(config.external_relay_urls.is_empty());
        assert!(config.validate_for_startup().is_err());
    }

    #[test]
    fn external_relay_urls_are_deduplicated() {
        let config = RelayConfig::new(
            false,
            vec![
                "https://relay.example.net".to_string(),
                "https://relay.example.net".to_string(),
                " https://relay.example.net ".to_string(),
            ],
        );
        assert_eq!(config.external_relay_urls.len(), 1);
    }

    #[test]
    fn own_relay_without_connectivity_url_fails_closed() {
        assert!(
            RelayConfig::new(true, vec![])
                .validate_for_startup()
                .is_err()
        );
    }

    #[test]
    fn unset_safety_provider_env_yields_no_entry() {
        // slot 未構成（env 未設定）は entry を作らない → provider 全 slot 未構成なら
        // scan service は構築されない（fail-closed。cn-core 側 contract test で固定）。
        assert!(safety_provider_entry(None, true).is_none());
    }

    #[test]
    fn safety_provider_entry_keeps_name_and_required_flag() {
        let entry = safety_provider_entry(Some("mock".to_string()), true).unwrap();
        assert_eq!(entry.provider, "mock");
        assert!(entry.required);
    }

    #[test]
    fn safety_emit_signed_events_defaults_to_true() {
        // operator config の default（emit_signed_moderation_events = true）と一致する。
        assert!(SafetyRuntimeConfig::default().emit_signed_events);
    }

    #[test]
    fn safety_service_is_absent_without_provider_config() {
        with_clean_indexer_env(|| {
            set_minimal_indexer_env();

            let config = IndexerConfig::from_env().unwrap();
            assert!(config.safety.providers.is_empty());

            let store = Arc::new(kukuri_cn_safety_runtime::MemorySafetyArtifactStore::new());
            let providers =
                kukuri_cn_core::resolve_safety_providers(&config.safety.providers, None).unwrap();
            let service = kukuri_cn_safety_runtime::build_safety_scan_service(
                &config.safety,
                providers,
                store,
            )
            .unwrap();
            assert!(service.is_none());
        });
    }

    #[test]
    fn general_action_env_rejects_allow_and_unknown() {
        // ADR 0028 §8.7 / `general_action_operator_tunable_stricter_only`（env 面）:
        // 未設定は既定（label）、label / hold / exclude を受理、allow と未知値は起動失敗。
        with_clean_indexer_env(|| {
            set_minimal_indexer_env();
            let config = IndexerConfig::from_env().unwrap();
            assert_eq!(config.safety.general_action, None);
            assert_eq!(
                kukuri_cn_safety_runtime::resolve_safety_policy(&config.safety)
                    .unwrap()
                    .general_action,
                GeneralAction::Label
            );

            for (raw, expected) in [
                ("label", GeneralAction::Label),
                ("hold", GeneralAction::Hold),
                ("EXCLUDE", GeneralAction::Exclude),
            ] {
                unsafe {
                    std::env::set_var(SAFETY_GENERAL_ACTION_ENV, raw);
                }
                let config = IndexerConfig::from_env().unwrap();
                assert_eq!(config.safety.general_action, Some(expected), "{raw}");
                assert_eq!(
                    kukuri_cn_safety_runtime::resolve_safety_policy(&config.safety)
                        .unwrap()
                        .general_action,
                    expected
                );
            }

            for raw in ["allow", "quarantine", "labelled"] {
                unsafe {
                    std::env::set_var(SAFETY_GENERAL_ACTION_ENV, raw);
                }
                let error = IndexerConfig::from_env().expect_err(raw).to_string();
                assert!(error.contains(SAFETY_GENERAL_ACTION_ENV), "{error}");
            }
        });
    }

    #[test]
    fn media_fetch_env_overrides_defaults_and_rejects_zero() {
        with_clean_indexer_env(|| {
            set_minimal_indexer_env();

            // 未設定なら既定値（32 MiB / 30 秒）。
            let config = IndexerConfig::from_env().unwrap();
            assert_eq!(config.media_fetch, MediaFetchConfig::default());

            // env で上書きできる。
            unsafe {
                std::env::set_var(MEDIA_FETCH_MAX_BYTES_ENV, "1048576");
                std::env::set_var(MEDIA_FETCH_TIMEOUT_SECS_ENV, "5");
            }
            let config = IndexerConfig::from_env().unwrap();
            assert_eq!(config.media_fetch.max_bytes, 1_048_576);
            assert_eq!(
                config.media_fetch.timeout,
                std::time::Duration::from_secs(5)
            );

            // 0 / 非数値は起動エラー（fail-closed）。
            unsafe {
                std::env::set_var(MEDIA_FETCH_MAX_BYTES_ENV, "0");
            }
            assert!(IndexerConfig::from_env().is_err());
            unsafe {
                std::env::set_var(MEDIA_FETCH_MAX_BYTES_ENV, "not-a-number");
            }
            assert!(IndexerConfig::from_env().is_err());
        });
    }

    // 既存テストで実績のある有効なダミー endpoint id（64 桁 16 進。どの 64 桁でも有効とは
    // 限らないため、この既知の値を再利用する）。
    const SEED_ENDPOINT_A: &str =
        "1111111111111111111111111111111111111111111111111111111111111111";
    const SEED_ENDPOINT_B: &str =
        "2222222222222222222222222222222222222222222222222222222222222222";

    #[test]
    fn seed_peers_csv_parses_entries_with_and_without_addr_hint() {
        let peers = parse_seed_peers_csv(&format!(
            " {SEED_ENDPOINT_A}@127.0.0.1:4433 , {SEED_ENDPOINT_B} ,, "
        ))
        .unwrap();
        assert_eq!(peers.len(), 2);
        assert_eq!(peers[0].endpoint_id, SEED_ENDPOINT_A);
        assert_eq!(peers[0].addr_hint.as_deref(), Some("127.0.0.1:4433"));
        assert_eq!(peers[1].endpoint_id, SEED_ENDPOINT_B);
        assert_eq!(peers[1].addr_hint, None);
    }

    #[test]
    fn seed_peers_csv_deduplicates_by_endpoint_id() {
        let peers = parse_seed_peers_csv(&format!(
            "{SEED_ENDPOINT_A}@127.0.0.1:4433,{SEED_ENDPOINT_A}"
        ))
        .unwrap();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].addr_hint.as_deref(), Some("127.0.0.1:4433"));
    }

    #[test]
    fn seed_peers_csv_rejects_invalid_entries() {
        assert!(parse_seed_peers_csv("not-a-valid-endpoint-id").is_err());
    }

    #[test]
    fn poll_interval_env_defaults_and_rejects_zero() {
        with_clean_indexer_env(|| {
            set_minimal_indexer_env();

            // 未設定なら既定値。
            assert_eq!(
                IndexerConfig::from_env().unwrap().poll_interval,
                DEFAULT_POLL_INTERVAL
            );

            // 有効値は読み取れる。
            unsafe {
                std::env::set_var(POLL_INTERVAL_SECS_ENV, "10");
            }
            assert_eq!(
                IndexerConfig::from_env().unwrap().poll_interval,
                std::time::Duration::from_secs(10)
            );

            // 0 / 非数値は起動エラー（fail-closed）。
            unsafe {
                std::env::set_var(POLL_INTERVAL_SECS_ENV, "0");
            }
            assert!(IndexerConfig::from_env().is_err());
            unsafe {
                std::env::set_var(POLL_INTERVAL_SECS_ENV, "soon");
            }
            assert!(IndexerConfig::from_env().is_err());
        });
    }

    #[test]
    fn max_concurrent_posts_defaults_and_rejects_zero() {
        with_clean_indexer_env(|| {
            set_minimal_indexer_env();
            assert_eq!(IndexerConfig::from_env().unwrap().max_concurrent_posts, 4);
            unsafe {
                std::env::set_var(MAX_CONCURRENT_POSTS_ENV, "8");
            }
            assert_eq!(IndexerConfig::from_env().unwrap().max_concurrent_posts, 8);
            unsafe {
                std::env::set_var(MAX_CONCURRENT_POSTS_ENV, "0");
            }
            assert!(IndexerConfig::from_env().is_err());
        });
    }

    #[test]
    fn status_addr_env_defaults_to_none_and_rejects_invalid() {
        with_clean_indexer_env(|| {
            set_minimal_indexer_env();

            // 未設定なら HTTP では公開しない。
            assert!(IndexerConfig::from_env().unwrap().status_addr.is_none());

            unsafe {
                std::env::set_var(STATUS_ADDR_ENV, "127.0.0.1:8630");
            }
            assert_eq!(
                IndexerConfig::from_env().unwrap().status_addr,
                Some("127.0.0.1:8630".parse().unwrap())
            );

            unsafe {
                std::env::set_var(STATUS_ADDR_ENV, "not-an-addr");
            }
            assert!(IndexerConfig::from_env().is_err());
        });
    }

    #[test]
    fn seed_peers_env_defaults_to_empty_and_rejects_invalid() {
        with_clean_indexer_env(|| {
            set_minimal_indexer_env();

            // 未設定なら空（シード無し）。
            assert!(IndexerConfig::from_env().unwrap().seed_peers.is_empty());

            // 有効値は読み取れる。
            unsafe {
                std::env::set_var(SEED_PEERS_ENV, format!("{SEED_ENDPOINT_A}@127.0.0.1:4433"));
            }
            let config = IndexerConfig::from_env().unwrap();
            assert_eq!(config.seed_peers.len(), 1);

            // 不正値は起動エラー（fail-closed）。
            unsafe {
                std::env::set_var(SEED_PEERS_ENV, "broken-entry");
            }
            assert!(IndexerConfig::from_env().is_err());
        });
    }

    #[test]
    fn safety_env_parses_provider_slots() {
        with_clean_indexer_env(|| {
            set_minimal_indexer_env();
            unsafe {
                std::env::set_var(SAFETY_PROVIDER_KNOWN_CSAM_ENV, " mock ");
                std::env::set_var(SAFETY_PROVIDER_KNOWN_CSAM_REQUIRED_ENV, "true");
                std::env::set_var(SAFETY_PROVIDER_GENERAL_ENV, "mock");
                std::env::set_var(SAFETY_PROVIDER_GENERAL_REQUIRED_ENV, "false");
                std::env::set_var(SAFETY_PROVIDER_UNKNOWN_CSAM_ENV, "mock");
                std::env::set_var(SAFETY_PROVIDER_UNKNOWN_CSAM_REQUIRED_ENV, "1");
                std::env::set_var(SAFETY_SIGNING_KEY_ENV, TEST_SECRET);
                std::env::set_var(SAFETY_EMIT_SIGNED_EVENTS_ENV, "off");
                std::env::set_var(SAFETY_ISSUER_NODE_ID_ENV, " issuer-node ");
            }

            let config = IndexerConfig::from_env().unwrap();
            let known = config.safety.providers.known_csam.as_ref().unwrap();
            assert_eq!(known.provider, "mock");
            assert!(known.required);
            let general = config.safety.providers.general.as_ref().unwrap();
            assert_eq!(general.provider, "mock");
            assert!(!general.required);
            let unknown = config.safety.providers.unknown_csam.as_ref().unwrap();
            assert_eq!(unknown.provider, "mock");
            assert!(unknown.required);
            assert_eq!(config.safety.signing_key.as_deref(), Some(TEST_SECRET));
            assert!(!config.safety.emit_signed_events);
            assert_eq!(config.safety.issuer_node_id.as_deref(), Some("issuer-node"));
        });
    }
}
