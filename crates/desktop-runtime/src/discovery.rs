use std::path::Path;

use anyhow::{Context, Result, anyhow};
use kukuri_transport::{ConnectMode, DiscoveryMode, SeedPeer, parse_seed_peer};
use serde::{Deserialize, Serialize};

use crate::paths::discovery_config_path;

pub(crate) const DISCOVERY_MODE_ENV: &str = "KUKURI_DISCOVERY_MODE";
pub(crate) const DISCOVERY_SEEDS_ENV: &str = "KUKURI_DISCOVERY_SEEDS";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct DiscoveryConfig {
    pub mode: DiscoveryMode,
    pub connect_mode: ConnectMode,
    pub env_locked: bool,
    pub seed_peers: Vec<SeedPeer>,
}

impl DiscoveryConfig {
    pub(crate) fn static_peer_default() -> Self {
        Self {
            mode: DiscoveryMode::StaticPeer,
            connect_mode: ConnectMode::DirectOnly,
            env_locked: false,
            seed_peers: Vec::new(),
        }
    }

    pub(crate) fn seeded_dht_default() -> Self {
        Self {
            mode: DiscoveryMode::SeededDht,
            connect_mode: ConnectMode::DirectOnly,
            env_locked: false,
            seed_peers: Vec::new(),
        }
    }

    pub(crate) fn from_stored(stored: StoredDiscoveryConfig, env_locked: bool) -> Self {
        Self {
            mode: stored.mode,
            connect_mode: ConnectMode::DirectOnly,
            env_locked,
            seed_peers: normalize_seed_peers(stored.seed_peers),
        }
    }

    pub(crate) fn stored(&self) -> StoredDiscoveryConfig {
        StoredDiscoveryConfig {
            mode: self.mode.clone(),
            seed_peers: normalize_seed_peers(self.seed_peers.clone()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct SetDiscoverySeedsRequest {
    pub seed_entries: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StoredDiscoveryConfig {
    #[serde(default)]
    mode: DiscoveryMode,
    #[serde(default)]
    seed_peers: Vec<SeedPeer>,
}

pub(crate) async fn load_discovery_config_from_file(
    db_path: &Path,
) -> Result<Option<StoredDiscoveryConfig>> {
    let path = discovery_config_path(db_path);
    let Some(raw) = crate::storage::read_file(&path)
        .await
        .with_context(|| format!("failed to read discovery config `{}`", path.display()))?
    else {
        return Ok(None);
    };
    let config = serde_json::from_slice::<StoredDiscoveryConfig>(&raw)
        .with_context(|| format!("failed to parse discovery config `{}`", path.display()))?;
    Ok(Some(config))
}

pub(crate) async fn save_discovery_config(
    db_path: &Path,
    config: &StoredDiscoveryConfig,
) -> Result<()> {
    let path = discovery_config_path(db_path);
    let json = serde_json::to_vec_pretty(config)
        .with_context(|| format!("failed to encode discovery config `{}`", path.display()))?;
    crate::storage::write_file(&path, &json)
        .await
        .with_context(|| format!("failed to write discovery config `{}`", path.display()))
}

pub(crate) async fn resolve_discovery_config_from_env(db_path: &Path) -> Result<DiscoveryConfig> {
    let env_mode = std::env::var(DISCOVERY_MODE_ENV).ok();
    let env_seeds = std::env::var(DISCOVERY_SEEDS_ENV).ok();
    let env_locked = env_mode.is_some() || env_seeds.is_some();

    if env_locked {
        let mode = match env_mode.as_deref() {
            Some(value) => parse_discovery_mode(value)?,
            None => DiscoveryMode::SeededDht,
        };
        let seed_peers = parse_seed_entries_from_csv(env_seeds.as_deref().unwrap_or(""))?;
        return Ok(DiscoveryConfig {
            mode,
            connect_mode: ConnectMode::DirectOnly,
            env_locked: true,
            seed_peers,
        });
    }

    if let Some(stored) = load_discovery_config_from_file(db_path).await? {
        return Ok(DiscoveryConfig::from_stored(stored, false));
    }
    // ブラウザには UDP が無く、DHT が成り立たない（ADR 0056 §3）。
    if cfg!(target_family = "wasm") {
        return Ok(DiscoveryConfig::static_peer_default());
    }
    Ok(DiscoveryConfig::seeded_dht_default())
}

pub(crate) fn parse_discovery_mode(value: &str) -> Result<DiscoveryMode> {
    match value.trim() {
        "static_peer" => Ok(DiscoveryMode::StaticPeer),
        "seeded_dht" => Ok(DiscoveryMode::SeededDht),
        other => Err(anyhow!(
            "invalid {} value `{}` (expected static_peer or seeded_dht)",
            DISCOVERY_MODE_ENV,
            other
        )),
    }
}

pub(crate) fn parse_seed_entries(entries: &[String]) -> Result<Vec<SeedPeer>> {
    parse_seed_entries_from_iter(entries.iter().map(String::as_str))
}

pub(crate) fn parse_seed_entries_from_csv(value: &str) -> Result<Vec<SeedPeer>> {
    parse_seed_entries_from_iter(value.split(','))
}

pub(crate) fn parse_seed_entries_from_iter<'a>(
    entries: impl IntoIterator<Item = &'a str>,
) -> Result<Vec<SeedPeer>> {
    let mut parsed = Vec::new();
    for entry in entries {
        let trimmed = entry.trim();
        if trimmed.is_empty() {
            continue;
        }
        parsed.push(parse_seed_peer(trimmed)?);
    }
    Ok(normalize_seed_peers(parsed))
}

pub(crate) fn normalize_seed_peers(peers: Vec<SeedPeer>) -> Vec<SeedPeer> {
    let mut deduped = std::collections::BTreeMap::new();
    for peer in peers {
        deduped.insert(peer.display(), peer);
    }
    deduped.into_values().collect()
}
