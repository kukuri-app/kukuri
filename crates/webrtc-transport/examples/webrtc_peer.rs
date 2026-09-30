//! browser↔native の試験（ADR 0057 §8 の E1・E3・E4）の native 側。
//!
//! 製品の接続交渉は W10（#1422）の専用 ALPN で行う。ここでは試験だけの HTTP（`POST /offer`）で
//! offer と answer を受け渡し、custom transport だけを持つ iroh の Endpoint で echo を返す。
//! 起動すると 1 行目に `KUKURI_WEBRTC_PEER_URL=<url>` を出す。native だけで動く。

#![cfg(not(target_family = "wasm"))]

use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket},
    sync::Arc,
};

use anyhow::{Context as _, Result};
use axum::{extract::State, http::header, routing::post};
use iroh::{
    Endpoint, EndpointId, RelayMode, SecretKey,
    endpoint::{Connection, presets},
    protocol::{AcceptError, ProtocolHandler, Router},
};
use kukuri_webrtc_transport::{SessionId, WebRtcConfig, WebRtcTransport};

const ECHO_ALPN: &[u8] = b"kukuri-test/echo";
const MAX_ECHO_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone)]
struct Echo;

impl ProtocolHandler for Echo {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let (mut send, mut recv) = connection.accept_bi().await?;
        let data = recv
            .read_to_end(MAX_ECHO_BYTES)
            .await
            .map_err(AcceptError::from_err)?;
        send.write_all(&data).await.map_err(AcceptError::from_err)?;
        send.finish()?;
        connection.closed().await;
        Ok(())
    }
}

/// ブラウザが到達できる、loopback ではない手元の IP（パケットは送らない）。
fn reachable_ip() -> IpAddr {
    UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
        .and_then(|socket| {
            socket.connect((Ipv4Addr::new(192, 0, 2, 1), 9))?;
            socket.local_addr()
        })
        .map(|addr| addr.ip())
        .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST))
}

#[derive(Clone)]
struct Peer {
    transport: Arc<WebRtcTransport>,
    id: EndpointId,
}

async fn offer(
    State(peer): State<Peer>,
    body: String,
) -> ([(header::HeaderName, &'static str); 1], String) {
    let cors = [(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")];
    match answer(&peer, &body).await {
        Ok(answer) => (cors, format!("{}\n{answer}", peer.id)),
        Err(error) => (cors, format!("error\n{error:#}")),
    }
}

async fn answer(peer: &Peer, body: &str) -> Result<String> {
    let mut lines = body.splitn(3, '\n');
    let remote: EndpointId = lines.next().context("remote id")?.parse()?;
    let session = lines.next().context("session")?;
    let offer = lines.next().context("offer")?;
    let mut bytes = [0u8; 16];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(
            session.get(index * 2..index * 2 + 2).context("session")?,
            16,
        )?;
    }
    peer.transport
        .answer(remote, SessionId::from_bytes(bytes), offer)
        .await
}

#[tokio::main]
async fn main() -> Result<()> {
    let transport = WebRtcTransport::new(WebRtcConfig {
        bind_ip: reachable_ip(),
    });
    let _events = transport.take_events();
    let endpoint = Endpoint::builder(presets::Minimal)
        .secret_key(SecretKey::generate())
        .relay_mode(RelayMode::Disabled)
        .clear_ip_transports()
        .add_custom_transport(transport.clone())
        .bind()
        .await?;
    let _router = Router::builder(endpoint.clone())
        .accept(ECHO_ALPN, Echo)
        .spawn();
    let listener =
        tokio::net::TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await?;
    println!("KUKURI_WEBRTC_PEER_URL=http://{}", listener.local_addr()?);
    let app = axum::Router::new()
        .route("/offer", post(offer))
        .with_state(Peer {
            transport,
            id: endpoint.id(),
        });
    axum::serve(listener, app).await?;
    Ok(())
}
