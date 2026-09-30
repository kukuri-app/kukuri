//! native と browser の試験で共通の helper。

use std::time::Duration;

use iroh::{
    Endpoint, EndpointId, RelayMode, SecretKey,
    endpoint::{Connection, presets},
    protocol::{AcceptError, ProtocolHandler},
};
use tokio::sync::mpsc;

use crate::{CustomAddr, SessionEvent, SessionId, WebRtcTransport};

pub(crate) const ECHO_ALPN: &[u8] = b"kukuri-test/echo";
pub(crate) const WAIT: Duration = Duration::from_secs(20);
const MAX_ECHO_BYTES: usize = 16 * 1024 * 1024;

/// 受け取った bytes をそのまま返す。
#[derive(Debug, Clone)]
pub(crate) struct Echo;

impl ProtocolHandler for Echo {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let (mut send, mut recv) = connection.accept_bi().await?;
        let data = recv
            .read_to_end(MAX_ECHO_BYTES)
            .await
            .map_err(AcceptError::from_err)?;
        send.write_all(&data).await.map_err(AcceptError::from_err)?;
        send.finish().map_err(AcceptError::from_err)?;
        connection.closed().await;
        Ok(())
    }
}

/// relay と IP を使わず、custom transport だけを持つ Endpoint。
pub(crate) async fn custom_only_endpoint(transport: std::sync::Arc<WebRtcTransport>) -> Endpoint {
    let builder = Endpoint::builder(presets::Minimal)
        .secret_key(SecretKey::generate())
        .relay_mode(RelayMode::Disabled);
    #[cfg(not(target_family = "wasm"))]
    let builder = builder.clear_ip_transports();
    builder
        .add_custom_transport(transport)
        .bind()
        .await
        .expect("bind endpoint")
}

pub(crate) async fn next_opened(events: &mut mpsc::Receiver<SessionEvent>) -> CustomAddr {
    match n0_future::time::timeout(WAIT, events.recv()).await {
        Ok(Some(SessionEvent::Opened { addr, .. })) => addr,
        Ok(Some(SessionEvent::Closed { reason, .. })) => panic!("session closed: {reason}"),
        Ok(None) => panic!("events closed"),
        Err(_) => panic!("session did not open"),
    }
}

/// offer・answer を試験の中で受け渡し、両端の DataChannel が開くまで待つ。
pub(crate) async fn negotiate(
    offerer: &WebRtcTransport,
    offerer_events: &mut mpsc::Receiver<SessionEvent>,
    offerer_id: EndpointId,
    answerer: &WebRtcTransport,
    answerer_events: &mut mpsc::Receiver<SessionEvent>,
    answerer_id: EndpointId,
) -> (SessionId, CustomAddr) {
    let (session, offer) = offerer.offer(answerer_id).await.expect("offer");
    let answer = answerer
        .answer(offerer_id, session, &offer)
        .await
        .expect("answer");
    offerer
        .accept_answer(session, &answer)
        .await
        .expect("accept answer");
    let addr = next_opened(offerer_events).await;
    next_opened(answerer_events).await;
    (session, addr)
}

pub(crate) fn payload(len: usize) -> Vec<u8> {
    (0..len).map(|index| (index * 31 % 251) as u8).collect()
}

pub(crate) async fn echo(connection: &Connection, data: &[u8]) -> Vec<u8> {
    let (mut send, mut recv) = connection.open_bi().await.expect("open bi");
    let write = async {
        send.write_all(data).await.expect("write");
        send.finish().expect("finish");
    };
    let read = async { recv.read_to_end(data.len() + 1).await.expect("read") };
    let ((), echoed) = n0_future::future::zip(write, read).await;
    echoed
}

pub(crate) fn selected_path_is_custom(connection: &Connection) -> bool {
    connection
        .paths()
        .iter()
        .any(|path| path.is_selected() && path.remote_addr().is_custom())
}
