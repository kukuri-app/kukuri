//! browser↔native の試験（ADR 0057 §8 の E1・E3・E4）の native 側。
//!
//! 試験だけの HTTP（`signaling_fixture`）で offer と answer を受け渡し、custom transport だけを持つ
//! iroh の Endpoint で echo を返す。専用 ALPN の交渉の browser↔browser の試験のために手元の iroh relay も起動し、
//! `POST /relay` で URL を返す。native だけで動く。

#![cfg(not(target_family = "wasm"))]

use anyhow::Result;
use iroh::{
    Endpoint, RelayMode, SecretKey,
    endpoint::{Connection, presets},
    protocol::{AcceptError, ProtocolHandler, Router},
};
use kukuri_webrtc_transport::{WebRtcConfig, WebRtcTransport, signaling_fixture};

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

#[tokio::main]
async fn main() -> Result<()> {
    let transport = WebRtcTransport::new(WebRtcConfig {
        bind_ip: signaling_fixture::reachable_ip(),
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
    let (relay, _relay_server) = signaling_fixture::spawn_relay().await?;
    let relay = relay.to_string();
    let app = signaling_fixture::offer_routes(transport, endpoint.id())
        .route("/relay", axum::routing::post(move || async move { relay }));
    signaling_fixture::serve(app).await
}
