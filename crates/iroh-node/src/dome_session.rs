//! 所有者の端末の Dome host への P2P の session 経路(ADR 0038、#1527)。
//!
//! 受け口は要求の bytes を登録された handler へ渡して応答の bytes を返すだけで、Dome の検証は handler が持つ。
//! participant 側は host の endpoint ごとに接続を 1 本保ち、要求ごとに双方向 stream を 1 本開く。

use std::collections::VecDeque;
use std::sync::{Arc, RwLock as StdRwLock};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use futures_util::future::BoxFuture;
use iroh::endpoint::{Connection, TransportAddrUsage};
use iroh::protocol::{AcceptError, ProtocolHandler};
use iroh::{EndpointAddr, EndpointId, RelayUrl};
use kukuri_core::DOME_SESSION_REQUEST_MAX_BYTES;
use n0_future::time::{Instant, timeout};
use tokio::sync::{Mutex, Semaphore};
use tracing::info;

use crate::IrohDocsNode;

pub(crate) const DOME_SESSION_ALPN: &[u8] = b"/kukuri/dome-session/1";
/// host の同時接続の上限(participant の安全上限)。
const MAX_HOST_CONNECTIONS: usize = 512;
/// 要求の無い接続を host が閉じるまでの時間(participant timeout と同じ)。
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);
/// 接続の確立と、要求 1 件の往復のそれぞれの上限(Community Node の HTTP と同じ)。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// 直接の IP 候補に使う上限。残りの時間で relay を含む候補を試す。
const DIRECT_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// participant 側が保つ接続の数(current と隣接 4 Dome の host)。
const MAX_CLIENT_CONNECTIONS: usize = 5;

/// 要求の bytes から応答の bytes を作る受け口。
pub type DomeSessionHandler = Arc<dyn Fn(Vec<u8>) -> BoxFuture<'static, Vec<u8>> + Send + Sync>;

/// host に接続できない、または要求の往復が成り立たない(host の拒否の応答とは区別する)。
#[derive(Debug)]
pub struct DomeHostUnreachable(pub String);

impl std::fmt::Display for DomeHostUnreachable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "the Dome host device could not be reached: {}",
            self.0
        )
    }
}

impl std::error::Error for DomeHostUnreachable {}

#[derive(Clone)]
pub(crate) struct DomeSessionSlot {
    handler: Arc<StdRwLock<Option<DomeSessionHandler>>>,
    connections: Arc<Semaphore>,
}

impl Default for DomeSessionSlot {
    fn default() -> Self {
        Self {
            handler: Arc::default(),
            connections: Arc::new(Semaphore::new(MAX_HOST_CONNECTIONS)),
        }
    }
}

impl std::fmt::Debug for DomeSessionSlot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DomeSessionSlot")
            .finish_non_exhaustive()
    }
}

impl DomeSessionSlot {
    pub(crate) fn install(&self, handler: Option<DomeSessionHandler>) {
        *self.handler.write().expect("Dome session handler poisoned") = handler;
    }

    fn handler(&self) -> Option<DomeSessionHandler> {
        self.handler
            .read()
            .expect("Dome session handler poisoned")
            .clone()
    }
}

impl ProtocolHandler for DomeSessionSlot {
    async fn accept(&self, connection: Connection) -> std::result::Result<(), AcceptError> {
        let Ok(_permit) = self.connections.try_acquire() else {
            connection.close(1u32.into(), b"busy");
            return Ok(());
        };
        // 1 接続の要求は順に 1 件ずつ処理する(participant の input の順を保つ)。
        while let Ok(Ok((mut send, mut recv))) = timeout(IDLE_TIMEOUT, connection.accept_bi()).await
        {
            let Some(handler) = self.handler() else {
                break;
            };
            let served = timeout(REQUEST_TIMEOUT, async {
                let request = recv.read_to_end(DOME_SESSION_REQUEST_MAX_BYTES).await?;
                send.write_all(&handler(request).await).await?;
                send.finish()?;
                anyhow::Ok(())
            })
            .await;
            if !matches!(served, Ok(Ok(()))) {
                break;
            }
        }
        connection.close(0u32.into(), b"done");
        Ok(())
    }
}

/// participant 側が保つ host との接続(最近使った順、上限つき)。
#[derive(Default)]
pub(crate) struct DomeSessionConnections(Mutex<VecDeque<(EndpointId, Connection)>>);

impl DomeSessionConnections {
    async fn get(&self, id: EndpointId) -> Option<Connection> {
        let mut connections = self.0.lock().await;
        let index = connections.iter().position(|(peer, _)| *peer == id)?;
        let (peer, connection) = connections.remove(index)?;
        if connection.close_reason().is_some() {
            return None;
        }
        connections.push_back((peer, connection.clone()));
        Some(connection)
    }

    async fn insert(&self, id: EndpointId, connection: Connection) -> Connection {
        let mut connections = self.0.lock().await;
        if let Some((_, existing)) = connections
            .iter()
            .find(|(peer, existing)| *peer == id && existing.close_reason().is_none())
        {
            connection.close(0u32.into(), b"duplicate");
            return existing.clone();
        }
        connections.retain(|(peer, _)| *peer != id);
        connections.push_back((id, connection.clone()));
        while connections.len() > MAX_CLIENT_CONNECTIONS {
            if let Some((_, oldest)) = connections.pop_front() {
                oldest.close(0u32.into(), b"evicted");
            }
        }
        connection
    }

    async fn forget(&self, id: EndpointId) {
        self.0.lock().await.retain(|(peer, _)| *peer != id);
    }
}

/// 接続の候補。relay URL を含まない直接の IP 候補(Direct P2P)を先に、relay を含む候補(Relay Supported P2P)を後に置く。
pub(crate) fn dome_host_candidates(
    id: EndpointId,
    known: Option<&EndpointAddr>,
    relay_urls: &[RelayUrl],
) -> Vec<EndpointAddr> {
    let relay_supported = relay_urls.iter().fold(EndpointAddr::new(id), |addr, url| {
        addr.with_relay_url(url.clone())
    });
    known
        .and_then(kukuri_transport::direct_endpoint_addr)
        .into_iter()
        .chain([relay_supported])
        .collect()
}

impl IrohDocsNode {
    pub fn install_dome_session_handler(&self, handler: DomeSessionHandler) {
        self.dome_session.install(Some(handler));
    }

    /// `endpoint_id` の host へ要求を 1 件送り、応答を `response_limit` bytes まで読む。
    /// 接続できない・往復が成り立たないときは [`DomeHostUnreachable`] を返す。
    pub async fn dome_session_request(
        &self,
        endpoint_id: &str,
        request: &[u8],
        response_limit: usize,
    ) -> Result<Vec<u8>> {
        let id = endpoint_id
            .parse::<EndpointId>()
            .context("invalid Dome host endpoint id")?;
        let unreachable = |error: anyhow::Error| anyhow!(DomeHostUnreachable(format!("{error:#}")));
        let connection = match self.dome_session_connections.get(id).await {
            Some(connection) => connection,
            None => {
                let connection = self.connect_dome_host(id).await.map_err(unreachable)?;
                self.dome_session_connections.insert(id, connection).await
            }
        };
        let response = timeout(REQUEST_TIMEOUT, async {
            let (mut send, mut recv) = connection.open_bi().await?;
            send.write_all(request).await?;
            send.finish()?;
            anyhow::Ok(recv.read_to_end(response_limit).await?)
        })
        .await
        .context("the Dome host did not answer in time")
        .and_then(|response| response);
        if response.is_err() {
            self.dome_session_connections.forget(id).await;
        }
        response.map_err(unreachable)
    }

    async fn connect_dome_host(&self, id: EndpointId) -> Result<Connection> {
        let deadline = Instant::now() + CONNECT_TIMEOUT;
        let known = self.endpoint().remote_info(id).await.map(|info| {
            EndpointAddr::from_parts(id, info.into_addrs().map(|addr| addr.into_addr()))
        });
        let candidates = dome_host_candidates(id, known.as_ref(), &self.relay_urls().await);
        let mut last_error = anyhow!("no Dome host candidate");
        for candidate in candidates {
            let relay_supported = candidate.relay_urls().next().is_some();
            let limit = if candidate.ip_addrs().next().is_some() && !relay_supported {
                deadline.min(Instant::now() + DIRECT_CONNECT_TIMEOUT)
            } else {
                deadline
            };
            match timeout(
                limit.saturating_duration_since(Instant::now()),
                self.endpoint().connect(candidate, DOME_SESSION_ALPN),
            )
            .await
            {
                Ok(Ok(connection)) => {
                    self.log_dome_host_path(id, relay_supported).await;
                    return Ok(connection);
                }
                Ok(Err(error)) => last_error = error.into(),
                Err(_) => last_error = anyhow!("timed out connecting to the Dome host"),
            }
            if Instant::now() >= deadline {
                break;
            }
        }
        Err(last_error)
    }

    /// 成立した経路の分類を診断に残す(ADR 0038。access proof と input の本文は出さない)。
    async fn log_dome_host_path(&self, id: EndpointId, relay_supported: bool) {
        let active = self
            .endpoint()
            .remote_info(id)
            .await
            .map(|info| {
                info.addrs()
                    .filter(|addr| matches!(addr.usage(), TransportAddrUsage::Active))
                    .fold((false, false), |(ip, relay), addr| {
                        (ip || addr.addr().is_ip(), relay || addr.addr().is_relay())
                    })
            })
            .unwrap_or_default();
        let path = match (relay_supported, active) {
            (_, (false, true)) => "relay_fallback",
            (true, _) => "relay_supported_p2p",
            (false, _) => "direct_p2p",
        };
        info!(target: "kukuri_connectivity", host = %id, path, "Dome session connection established");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_candidates_come_before_relay_supported_ones() {
        let id = iroh::SecretKey::from_bytes(&[5; 32]).public();
        let relay: RelayUrl = "https://relay.example".parse().expect("relay url");
        let known = EndpointAddr::new(id)
            .with_ip_addr("127.0.0.1:4433".parse().expect("socket addr"))
            .with_relay_url(relay.clone());
        let candidates = dome_host_candidates(id, Some(&known), std::slice::from_ref(&relay));
        assert_eq!(candidates.len(), 2);
        assert!(candidates[0].relay_urls().next().is_none(), "direct first");
        assert!(candidates[0].ip_addrs().next().is_some());
        assert_eq!(candidates[1].relay_urls().next(), Some(&relay));
        assert!(candidates[1].ip_addrs().next().is_none());

        let unknown = dome_host_candidates(id, None, &[]);
        assert_eq!(unknown, vec![EndpointAddr::new(id)], "address lookup only");
    }
}
