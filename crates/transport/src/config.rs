use std::collections::BTreeSet;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::str::FromStr;

use anyhow::{Context, Result};
use iroh::{RelayMap, RelayMode, RelayUrl};
#[cfg(not(target_family = "wasm"))]
use n0_mainline::{Dht, DhtBuilder};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransportNetworkConfig {
    pub bind_addr: SocketAddr,
    pub advertised_host: Option<String>,
    pub advertised_port: Option<u16>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryMode {
    #[default]
    StaticPeer,
    SeededDht,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ConnectMode {
    #[default]
    DirectOnly,
    DirectOrRelay,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ConnectionPath {
    #[default]
    DirectP2p,
    RelaySupportedP2p,
    RelayFallback,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransportRelayConfig {
    #[serde(default)]
    pub iroh_relay_urls: Vec<String>,
}

impl TransportRelayConfig {
    pub fn normalized(mut self) -> Self {
        self.iroh_relay_urls = self
            .iroh_relay_urls
            .into_iter()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        self
    }

    pub fn connect_mode(&self) -> ConnectMode {
        if self.iroh_relay_urls.is_empty() {
            ConnectMode::DirectOnly
        } else {
            ConnectMode::DirectOrRelay
        }
    }

    pub fn parsed_relay_urls(&self) -> Result<Vec<RelayUrl>> {
        self.iroh_relay_urls
            .iter()
            .map(|value| {
                value
                    .parse::<RelayUrl>()
                    .with_context(|| format!("invalid iroh relay url `{value}`"))
            })
            .collect()
    }

    pub fn relay_mode(&self) -> Result<RelayMode> {
        if self.iroh_relay_urls.is_empty() {
            return Ok(RelayMode::Disabled);
        }
        let relay_urls = self.parsed_relay_urls()?;
        Ok(RelayMode::Custom(RelayMap::from_iter(relay_urls)))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct SeedPeer {
    pub endpoint_id: String,
    pub addr_hint: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoverySnapshot {
    pub mode: DiscoveryMode,
    pub connect_mode: ConnectMode,
    pub active_path: ConnectionPath,
    pub env_locked: bool,
    pub configured_seed_peer_count: usize,
    pub bootstrap_seed_peer_count: usize,
    pub connected_peer_count: usize,
    pub local_endpoint_id: String,
    pub last_discovery_error: Option<String>,
}

/// 設定画面の詳細で cursor つきのページとして読む peer の一覧の種類(#1221 R2-D)。
/// `Connected`・`Configured`・`Missing` は topic を指定するとその topic の分、`Connected` は省くと全 topic の和。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ConnectivityPeerKind {
    Connected,
    Configured,
    Missing,
    DocsAssist,
    BlobAssist,
    ManualTicket,
    BootstrapSeed,
    ConfiguredSeed,
}

#[derive(Clone, Debug, Default)]
pub struct DhtDiscoveryOptions {
    pub enabled: bool,
    #[cfg(not(target_family = "wasm"))]
    pub dht_builder: Option<DhtBuilder>,
    /// 組み立て済みの DHT node。渡すと住所の公開・解決はこれを使い、自分では作らない（#1632）。
    #[cfg(not(target_family = "wasm"))]
    pub dht: Option<Dht>,
    /// 公開 blob の発見（#1632、ADR 0063）の補助 index。`Some` なら、同じ DHT で hash の告知・検索も行う。
    #[cfg(not(target_family = "wasm"))]
    pub public_blob_index: Option<PublicBlobIndex>,
}

/// 公開 blob の発見の補助 index（UDP の送信元 address → 署名つき endpoint ID）の見つけ方（#1632、ADR 0063）。
#[cfg(not(target_family = "wasm"))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PublicBlobIndex {
    /// kukuri の鍵で署名した一覧（最大 2 台）を DHT から引く。
    ListKey([u8; 32]),
    /// 一覧を引かずに使う server（試験）。
    Servers(Vec<SocketAddrV4>),
}

impl DhtDiscoveryOptions {
    pub fn disabled() -> Self {
        Self::default()
    }

    pub fn seeded_dht() -> Self {
        Self {
            enabled: true,
            ..Self::default()
        }
    }

    #[cfg(not(target_family = "wasm"))]
    pub fn with_bootstrap<T: ToString>(bootstrap: &[T]) -> Self {
        let mut dht_builder = DhtBuilder::default();
        dht_builder.bootstrap(bootstrap);
        Self {
            enabled: true,
            dht_builder: Some(dht_builder),
            ..Self::default()
        }
    }

    #[cfg(not(target_family = "wasm"))]
    pub fn resolved_dht_builder(&self) -> Option<DhtBuilder> {
        if !self.enabled {
            return None;
        }
        Some(self.dht_builder.clone().unwrap_or_default())
    }
}

impl Default for TransportNetworkConfig {
    fn default() -> Self {
        Self {
            bind_addr: SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)),
            advertised_host: None,
            advertised_port: None,
        }
    }
}

impl TransportNetworkConfig {
    pub fn loopback() -> Self {
        Self {
            bind_addr: SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)),
            advertised_host: None,
            advertised_port: None,
        }
    }

    pub fn from_env() -> Result<Self> {
        let bind_addr = std::env::var("KUKURI_BIND_ADDR")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(|value| SocketAddr::from_str(value.trim()))
            .transpose()
            .context("failed to parse KUKURI_BIND_ADDR")?
            .unwrap_or_else(|| SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)));
        let advertised_host = std::env::var("KUKURI_ADVERTISE_HOST")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let advertised_port = std::env::var("KUKURI_ADVERTISE_PORT")
            .ok()
            .map(|value| value.trim().parse::<u16>())
            .transpose()
            .context("failed to parse KUKURI_ADVERTISE_PORT")?;

        Ok(Self {
            bind_addr,
            advertised_host,
            advertised_port,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .expect("env lock")
    }

    fn legacy_env(name: &str) -> String {
        format!("KUKURI_{}_{}", "NEXT", name)
    }

    #[test]
    fn old_next_env_vars_are_not_used() {
        let _guard = env_lock();
        let legacy_bind_addr = legacy_env("BIND_ADDR");
        let legacy_advertise_host = legacy_env("ADVERTISE_HOST");
        let legacy_advertise_port = legacy_env("ADVERTISE_PORT");
        for key in [
            "KUKURI_BIND_ADDR",
            "KUKURI_ADVERTISE_HOST",
            "KUKURI_ADVERTISE_PORT",
            legacy_bind_addr.as_str(),
            legacy_advertise_host.as_str(),
            legacy_advertise_port.as_str(),
        ] {
            unsafe { std::env::remove_var(key) };
        }
        unsafe {
            std::env::set_var(legacy_bind_addr, "127.0.0.1:40123");
            std::env::set_var(legacy_advertise_host, "legacy-host");
            std::env::set_var(legacy_advertise_port, "40123");
        }

        let config = TransportNetworkConfig::from_env().expect("config");

        assert_eq!(
            config.bind_addr,
            SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))
        );
        assert_eq!(config.advertised_host, None);
        assert_eq!(config.advertised_port, None);
    }

    #[test]
    fn seeded_dht_resolves_upstream_dht_builder() {
        let options = DhtDiscoveryOptions::seeded_dht();

        assert!(options.resolved_dht_builder().is_some());
        assert!(
            DhtDiscoveryOptions::disabled()
                .resolved_dht_builder()
                .is_none()
        );
    }
}
