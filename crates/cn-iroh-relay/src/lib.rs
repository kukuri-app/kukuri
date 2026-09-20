use std::net::SocketAddr;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use iroh_relay::defaults::DEFAULT_RELAY_QUIC_PORT;
use iroh_relay::server::{
    AllowAll, CertConfig, ClientRateLimit, Limits, QuicConfig, RelayConfig as HttpRelayConfig,
    Server, ServerConfig, TlsConfig,
};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};

const DEFAULT_HTTP_BIND_ADDR: &str = "127.0.0.1:3340";
const DEFAULT_TLS_CERT_PATH: &str = "/certs/default.crt";
const DEFAULT_TLS_KEY_PATH: &str = "/certs/default.key";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IrohRelayTlsConfig {
    pub https_bind_addr: Option<SocketAddr>,
    pub quic_bind_addr: Option<SocketAddr>,
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
}

impl IrohRelayTlsConfig {
    fn effective_quic_bind_addr(&self) -> Option<SocketAddr> {
        self.quic_bind_addr.or_else(|| {
            self.https_bind_addr
                .map(|bind_addr| SocketAddr::new(bind_addr.ip(), DEFAULT_RELAY_QUIC_PORT))
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IrohRelayConfig {
    pub http_bind_addr: SocketAddr,
    pub tls: Option<IrohRelayTlsConfig>,
    pub client_rx_limit: Option<IrohRelayClientRxLimit>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IrohRelayClientRxLimit {
    pub bytes_per_second: NonZeroU32,
    pub max_burst_bytes: Option<NonZeroU32>,
}

impl IrohRelayClientRxLimit {
    pub fn request_policy(self) -> kukuri_transport::RequestRatePolicy {
        kukuri_transport::RequestRatePolicy {
            limit: u64::from(self.bytes_per_second.get()),
            window: std::time::Duration::from_secs(1),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IrohRelayPublicOriginMode {
    HttpOnly,
    LocalHttps,
    UpstreamTlsTermination,
}

impl IrohRelayConfig {
    pub fn from_env() -> Result<Self> {
        Self::from_lookup(&|var_name| std::env::var(var_name).ok())
    }

    fn from_lookup<F>(lookup: &F) -> Result<Self>
    where
        F: Fn(&str) -> Option<String>,
    {
        Ok(Self {
            http_bind_addr: parse_socket_addr_lookup(
                lookup,
                "COMMUNITY_NODE_IROH_RELAY_HTTP_BIND_ADDR",
                DEFAULT_HTTP_BIND_ADDR,
            )?,
            tls: parse_tls_config_from_lookup(lookup)?,
            client_rx_limit: parse_client_rx_limit_from_lookup(lookup)?,
        })
    }

    pub fn public_origin_mode(&self) -> IrohRelayPublicOriginMode {
        match self.tls.as_ref() {
            Some(tls) if tls.https_bind_addr.is_some() => IrohRelayPublicOriginMode::LocalHttps,
            Some(_) => IrohRelayPublicOriginMode::UpstreamTlsTermination,
            None => IrohRelayPublicOriginMode::HttpOnly,
        }
    }
}

pub struct SpawnedIrohRelay {
    _server: Server,
    http_addr: SocketAddr,
    https_addr: Option<SocketAddr>,
    quic_addr: Option<SocketAddr>,
}

impl SpawnedIrohRelay {
    pub fn http_addr(&self) -> SocketAddr {
        self.http_addr
    }

    pub fn https_addr(&self) -> Option<SocketAddr> {
        self.https_addr
    }

    pub fn quic_addr(&self) -> Option<SocketAddr> {
        self.quic_addr
    }
}

pub async fn spawn_server(config: IrohRelayConfig) -> Result<SpawnedIrohRelay> {
    let (relay_tls, quic) = match config.tls.as_ref() {
        Some(tls_config) => {
            let server_config = load_tls_materials(tls_config)?;
            let relay_tls = tls_config.https_bind_addr.map(|https_bind_addr| {
                TlsConfig::new(
                    https_bind_addr,
                    CertConfig::Manual {
                        server_config: server_config.clone(),
                    },
                )
            });
            let quic = tls_config.effective_quic_bind_addr().map(|bind_addr| {
                let mut quic = QuicConfig::new(bind_addr);
                if relay_tls.is_none() {
                    quic.server_config = Some(server_config.clone());
                }
                quic
            });
            (relay_tls, quic)
        }
        None => (None, None),
    };

    let mut limits = Limits::default();
    limits.client_rx = config.client_rx_limit.map(|limit| {
        let shared_policy = limit.request_policy();
        // The upstream relay limiter remains the byte-accounting adapter. Keeping it as the only
        // enforcement point avoids double charging while using the same typed policy vocabulary.
        let bytes_per_second = NonZeroU32::new(
            u32::try_from(shared_policy.limit).expect("relay policy originates from NonZeroU32"),
        )
        .expect("relay request policy remains nonzero");
        let mut client_rx = ClientRateLimit::new(bytes_per_second);
        client_rx.max_burst_bytes = limit.max_burst_bytes;
        client_rx
    });

    let mut relay_config = HttpRelayConfig::new(config.http_bind_addr);
    relay_config.tls = relay_tls;
    relay_config.limits = limits;
    relay_config.key_cache_capacity = Some(1024);
    relay_config.access = Arc::new(AllowAll);

    let mut server_config = ServerConfig::default();
    server_config.relay = Some(relay_config);
    server_config.quic = quic;
    let server = Server::spawn(server_config)
        .await
        .context("failed to spawn iroh relay")?;
    let http_addr = server
        .http_addr()
        .context("iroh relay did not expose an http listener")?;
    let https_addr = server.https_addr();
    let quic_addr = server.quic_addr();
    Ok(SpawnedIrohRelay {
        _server: server,
        http_addr,
        https_addr,
        quic_addr,
    })
}

pub fn init_tracing() {
    kukuri_cn_runtime_support::init_tracing("info,kukuri_cn_iroh_relay=debug");
}

fn parse_tls_config_from_lookup<F>(lookup: &F) -> Result<Option<IrohRelayTlsConfig>>
where
    F: Fn(&str) -> Option<String>,
{
    let https_bind_addr =
        optional_socket_addr_lookup(lookup, "COMMUNITY_NODE_IROH_RELAY_HTTPS_BIND_ADDR")?;
    let quic_bind_addr =
        optional_socket_addr_lookup(lookup, "COMMUNITY_NODE_IROH_RELAY_QUIC_BIND_ADDR")?;
    let cert_path = optional_path_lookup(lookup, "COMMUNITY_NODE_IROH_RELAY_TLS_CERT_PATH");
    let key_path = optional_path_lookup(lookup, "COMMUNITY_NODE_IROH_RELAY_TLS_KEY_PATH");

    if https_bind_addr.is_none()
        && quic_bind_addr.is_none()
        && cert_path.is_none()
        && key_path.is_none()
    {
        return Ok(None);
    }

    if https_bind_addr.is_none() && quic_bind_addr.is_none() {
        bail!(
            "COMMUNITY_NODE_IROH_RELAY_HTTPS_BIND_ADDR or COMMUNITY_NODE_IROH_RELAY_QUIC_BIND_ADDR is required when enabling iroh relay TLS/QUIC"
        );
    }
    Ok(Some(IrohRelayTlsConfig {
        https_bind_addr,
        quic_bind_addr,
        cert_path: cert_path.unwrap_or_else(|| PathBuf::from(DEFAULT_TLS_CERT_PATH)),
        key_path: key_path.unwrap_or_else(|| PathBuf::from(DEFAULT_TLS_KEY_PATH)),
    }))
}

fn parse_client_rx_limit_from_lookup<F>(lookup: &F) -> Result<Option<IrohRelayClientRxLimit>>
where
    F: Fn(&str) -> Option<String>,
{
    let bytes_per_second = optional_nonzero_u32_lookup(
        lookup,
        "COMMUNITY_NODE_IROH_RELAY_CLIENT_RX_BYTES_PER_SECOND",
    )?;
    let max_burst_bytes = optional_nonzero_u32_lookup(
        lookup,
        "COMMUNITY_NODE_IROH_RELAY_CLIENT_RX_MAX_BURST_BYTES",
    )?;

    match (bytes_per_second, max_burst_bytes) {
        (None, None) => Ok(None),
        (Some(bytes_per_second), max_burst_bytes) => Ok(Some(IrohRelayClientRxLimit {
            bytes_per_second,
            max_burst_bytes,
        })),
        (None, Some(_)) => bail!(
            "COMMUNITY_NODE_IROH_RELAY_CLIENT_RX_BYTES_PER_SECOND is required when setting COMMUNITY_NODE_IROH_RELAY_CLIENT_RX_MAX_BURST_BYTES"
        ),
    }
}

fn parse_socket_addr_lookup<F>(lookup: &F, var_name: &str, default: &str) -> Result<SocketAddr>
where
    F: Fn(&str) -> Option<String>,
{
    lookup_value(lookup, var_name)
        .unwrap_or_else(|| default.to_string())
        .parse()
        .with_context(|| format!("failed to parse {var_name}"))
}

fn optional_socket_addr_lookup<F>(lookup: &F, var_name: &str) -> Result<Option<SocketAddr>>
where
    F: Fn(&str) -> Option<String>,
{
    lookup_value(lookup, var_name)
        .map(|value| {
            value
                .parse()
                .with_context(|| format!("failed to parse {var_name}"))
        })
        .transpose()
}

fn optional_nonzero_u32_lookup<F>(lookup: &F, var_name: &str) -> Result<Option<NonZeroU32>>
where
    F: Fn(&str) -> Option<String>,
{
    lookup_value(lookup, var_name)
        .map(|value| {
            let parsed = value
                .parse::<u32>()
                .with_context(|| format!("failed to parse {var_name}"))?;
            NonZeroU32::new(parsed)
                .ok_or_else(|| anyhow::anyhow!("{var_name} must be greater than zero"))
        })
        .transpose()
}

fn optional_path_lookup<F>(lookup: &F, var_name: &str) -> Option<PathBuf>
where
    F: Fn(&str) -> Option<String>,
{
    lookup_value(lookup, var_name).map(PathBuf::from)
}

fn lookup_value<F>(lookup: &F, var_name: &str) -> Option<String>
where
    F: Fn(&str) -> Option<String>,
{
    lookup(var_name)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn load_tls_materials(config: &IrohRelayTlsConfig) -> Result<rustls::ServerConfig> {
    let certs = load_certs(config.cert_path.as_path())?;
    if certs.is_empty() {
        bail!(
            "iroh relay certificate file `{}` did not contain any certificates",
            config.cert_path.display()
        );
    }
    let private_key = load_secret_key(config.key_path.as_path())?;
    let server_config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("protocols supported by ring")
    .with_no_client_auth()
    .with_single_cert(certs.clone(), private_key)
    .context("failed to build iroh relay tls server config")?;
    Ok(server_config)
}

fn load_certs(path: &Path) -> Result<Vec<CertificateDer<'static>>> {
    CertificateDer::pem_file_iter(path)
        .with_context(|| format!("failed to open certificate file `{}`", path.display()))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .with_context(|| format!("failed to read certificates from `{}`", path.display()))
}

fn load_secret_key(path: &Path) -> Result<PrivateKeyDer<'static>> {
    PrivateKeyDer::from_pem_file(path)
        .with_context(|| format!("failed to read private key from `{}`", path.display()))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::net::{IpAddr, Ipv4Addr};

    use rcgen::{CertifiedKey, generate_simple_self_signed};
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn from_env_defaults_to_http_only() {
        let config = config_from_vars(&[]).expect("config");

        assert_eq!(
            config.http_bind_addr,
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 3340)
        );
        assert!(config.tls.is_none());
        assert!(config.client_rx_limit.is_none());
    }

    #[test]
    fn from_env_parses_client_rx_limit() {
        let config = config_from_vars(&[
            (
                "COMMUNITY_NODE_IROH_RELAY_CLIENT_RX_BYTES_PER_SECOND",
                "1048576",
            ),
            (
                "COMMUNITY_NODE_IROH_RELAY_CLIENT_RX_MAX_BURST_BYTES",
                "2097152",
            ),
        ])
        .expect("config");

        assert_eq!(
            config.client_rx_limit,
            Some(IrohRelayClientRxLimit {
                bytes_per_second: NonZeroU32::new(1_048_576).expect("nonzero"),
                max_burst_bytes: Some(NonZeroU32::new(2_097_152).expect("nonzero")),
            })
        );
    }

    #[test]
    fn from_env_rejects_client_rx_burst_without_rate() {
        let error = config_from_vars(&[(
            "COMMUNITY_NODE_IROH_RELAY_CLIENT_RX_MAX_BURST_BYTES",
            "2097152",
        )])
        .expect_err("config should fail");

        assert!(
            error
                .to_string()
                .contains("CLIENT_RX_BYTES_PER_SECOND is required")
        );
    }

    #[test]
    fn from_env_rejects_zero_client_rx_rate() {
        let error =
            config_from_vars(&[("COMMUNITY_NODE_IROH_RELAY_CLIENT_RX_BYTES_PER_SECOND", "0")])
                .expect_err("config should fail");

        assert!(
            error
                .to_string()
                .contains("CLIENT_RX_BYTES_PER_SECOND must be greater than zero")
        );
    }

    #[test]
    fn from_env_parses_tls_and_quic_inputs() {
        let config = config_from_vars(&[
            ("COMMUNITY_NODE_IROH_RELAY_HTTP_BIND_ADDR", "0.0.0.0:3340"),
            ("COMMUNITY_NODE_IROH_RELAY_HTTPS_BIND_ADDR", "0.0.0.0:3443"),
            ("COMMUNITY_NODE_IROH_RELAY_QUIC_BIND_ADDR", "0.0.0.0:7842"),
            ("COMMUNITY_NODE_IROH_RELAY_TLS_CERT_PATH", "/certs/iroh.crt"),
            ("COMMUNITY_NODE_IROH_RELAY_TLS_KEY_PATH", "/certs/iroh.key"),
        ])
        .expect("config");
        let public_origin_mode = config.public_origin_mode();
        let tls = config.tls.expect("tls config");

        assert_eq!(
            config.http_bind_addr,
            SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 3340)
        );
        assert_eq!(public_origin_mode, IrohRelayPublicOriginMode::LocalHttps);
        assert_eq!(
            tls.https_bind_addr,
            Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 3443))
        );
        assert_eq!(
            tls.quic_bind_addr,
            Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 7842))
        );
        assert_eq!(tls.cert_path, PathBuf::from("/certs/iroh.crt"));
        assert_eq!(tls.key_path, PathBuf::from("/certs/iroh.key"));
    }

    #[test]
    fn from_env_allows_quic_without_https_bind_addr_for_upstream_tls_termination() {
        let config = config_from_vars(&[
            ("COMMUNITY_NODE_IROH_RELAY_HTTP_BIND_ADDR", "0.0.0.0:3340"),
            ("COMMUNITY_NODE_IROH_RELAY_QUIC_BIND_ADDR", "0.0.0.0:7842"),
            ("COMMUNITY_NODE_IROH_RELAY_TLS_CERT_PATH", "/certs/iroh.crt"),
            ("COMMUNITY_NODE_IROH_RELAY_TLS_KEY_PATH", "/certs/iroh.key"),
        ])
        .expect("config");
        let public_origin_mode = config.public_origin_mode();
        let tls = config.tls.expect("tls config");

        assert_eq!(
            public_origin_mode,
            IrohRelayPublicOriginMode::UpstreamTlsTermination
        );
        assert_eq!(tls.https_bind_addr, None);
        assert_eq!(
            tls.quic_bind_addr,
            Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 7842))
        );
        assert_eq!(tls.cert_path, PathBuf::from("/certs/iroh.crt"));
        assert_eq!(tls.key_path, PathBuf::from("/certs/iroh.key"));
    }

    #[tokio::test]
    async fn spawn_server_defaults_quic_listener_from_https_bind_addr() {
        if std::net::UdpSocket::bind("127.0.0.1:7842").is_err() {
            return;
        }

        let temp = tempdir().expect("tempdir");
        let cert_path = temp.path().join("relay.crt");
        let key_path = temp.path().join("relay.key");
        let CertifiedKey { cert, signing_key } =
            generate_simple_self_signed(vec!["localhost".to_string(), "127.0.0.1".to_string()])
                .expect("self-signed certificate");
        fs::write(&cert_path, cert.pem()).expect("write cert");
        fs::write(&key_path, signing_key.serialize_pem()).expect("write key");

        let server = spawn_server(IrohRelayConfig {
            http_bind_addr: "127.0.0.1:0".parse().expect("http bind addr"),
            tls: Some(IrohRelayTlsConfig {
                https_bind_addr: Some("127.0.0.1:0".parse().expect("https bind addr")),
                quic_bind_addr: None,
                cert_path,
                key_path,
            }),
            client_rx_limit: None,
        })
        .await
        .expect("spawn relay server");

        assert!(server.https_addr().is_some());
        assert_eq!(
            server.quic_addr(),
            Some(SocketAddr::new(
                IpAddr::V4(Ipv4Addr::LOCALHOST),
                DEFAULT_RELAY_QUIC_PORT,
            ))
        );
    }

    fn config_from_vars(vars: &[(&str, &str)]) -> Result<IrohRelayConfig> {
        let values = vars
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect::<BTreeMap<_, _>>();
        IrohRelayConfig::from_lookup(&|var_name| values.get(var_name).cloned())
    }
}
