//! #1422 AC-1 の native の固定 fixture。接続は手元の UDP で始め、その上で交渉する。

use std::{net::Ipv4Addr, time::Duration};

use iroh::{
    RelayMode, SecretKey,
    endpoint::{Connection, presets},
    protocol::Router,
};
use tokio::sync::{Barrier, Notify};

use super::*;
use crate::{
    WebRtcConfig,
    test_support::{ECHO_ALPN, Echo, WAIT, echo},
};

const SHORT: Duration = Duration::from_secs(1);

struct Node {
    endpoint: Endpoint,
    transport: Arc<WebRtcTransport>,
    signaling: Arc<Signaling>,
    _router: Router,
}

/// 手元の UDP と custom transport を持つ node。`handler` が交渉の ALPN の受け手（無ければ受けない）。
async fn node_with(
    deadline: Duration,
    handler: impl FnOnce(Arc<Signaling>) -> Option<Box<dyn iroh::protocol::DynProtocolHandler>>,
) -> Node {
    let transport = WebRtcTransport::new(WebRtcConfig {
        bind_ip: Ipv4Addr::LOCALHOST.into(),
    });
    let endpoint = Endpoint::builder(presets::Minimal)
        .secret_key(SecretKey::generate())
        .relay_mode(RelayMode::Disabled)
        .bind_addr((Ipv4Addr::LOCALHOST, 0))
        .expect("bind addr")
        .add_custom_transport(transport.clone())
        .bind()
        .await
        .expect("bind endpoint");
    let signaling =
        Signaling::new(transport.clone(), endpoint.clone(), deadline).expect("signaling");
    let mut router = Router::builder(endpoint.clone()).accept(ECHO_ALPN, Echo);
    if let Some(handler) = handler(signaling.clone()) {
        router = router.accept(SIGNALING_ALPN, handler);
    }
    Node {
        endpoint,
        transport,
        signaling,
        _router: router.spawn(),
    }
}

async fn node() -> Node {
    node_with(NEGOTIATION_DEADLINE, |signaling| Some(Box::new(signaling))).await
}

async fn echo_connection(from: &Node, to: &Node) -> Connection {
    from.endpoint
        .connect(to.endpoint.addr(), ECHO_ALPN)
        .await
        .expect("connect")
}

async fn eventually(what: &str, mut done: impl FnMut() -> bool) {
    n0_future::time::timeout(WAIT, async {
        while !done() {
            n0_future::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{what}"));
}

fn sessions(node: &Node) -> usize {
    node.transport.stats().sessions
}

/// 交渉の ALPN の受け手を、`gate` を通るまで止める。
#[derive(Debug)]
struct Gated {
    inner: Arc<Signaling>,
    arrived: Arc<Notify>,
    gate: Arc<Barrier>,
}

impl ProtocolHandler for Gated {
    async fn accept(&self, connection: Connection) -> std::result::Result<(), AcceptError> {
        self.arrived.notify_one();
        self.gate.wait().await;
        self.inner.accept(connection).await
    }
}

/// 要求を読んで応答しない相手。
#[derive(Debug)]
struct Silent;

impl ProtocolHandler for Silent {
    async fn accept(&self, connection: Connection) -> std::result::Result<(), AcceptError> {
        let (_send, mut recv) = connection.accept_bi().await?;
        let _ = recv.read_to_end(64 * 1024).await;
        connection.closed().await;
        Ok(())
    }
}

/// 交渉の ALPN へ `request` をそのまま送り、応答を読む。
async fn raw_exchange(from: &Node, to: &Node, request: &[u8]) -> Result<Response> {
    let connection = from
        .endpoint
        .connect(to.endpoint.addr(), SIGNALING_ALPN)
        .await?;
    let (mut send, mut recv) = connection.open_bi().await?;
    send.write_all(request).await?;
    send.finish()?;
    decode_response(&recv.read_to_end(1 + MAX_SDP_BYTES).await?)
}

fn rejection(error: &anyhow::Error) -> Option<Rejection> {
    error.downcast_ref::<Rejected>().map(|rejected| rejected.0)
}

#[tokio::test]
async fn either_side_adds_a_custom_path_to_the_existing_connection() {
    for from_client in [true, false] {
        let client = node().await;
        let server = node().await;
        let connection = echo_connection(&client, &server).await;
        let (from, to) = if from_client {
            (&client, &server)
        } else {
            (&server, &client)
        };
        from.signaling
            .connect(to.endpoint.addr())
            .await
            .expect("negotiate");
        eventually("custom path on the existing connection", || {
            connection
                .paths()
                .iter()
                .any(|path| path.remote_addr().is_custom())
        })
        .await;
        assert_eq!(
            echo(&connection, b"after the negotiation").await,
            b"after the negotiation"
        );
        assert_eq!((sessions(&client), sessions(&server)), (1, 1));
    }
}

#[tokio::test]
async fn simultaneous_starts_keep_one_session_per_side() {
    let gate = Arc::new(Barrier::new(2));
    let gated = |gate: Arc<Barrier>| {
        move |inner| {
            Some(Box::new(Gated {
                inner,
                arrived: Arc::new(Notify::new()),
                gate,
            })
                as Box<dyn iroh::protocol::DynProtocolHandler>)
        }
    };
    let a = node_with(NEGOTIATION_DEADLINE, gated(gate.clone())).await;
    let b = node_with(NEGOTIATION_DEADLINE, gated(gate)).await;
    let (from_a, from_b) = tokio::join!(
        a.signaling.connect(b.endpoint.addr()),
        b.signaling.connect(a.endpoint.addr())
    );
    from_a.expect("a negotiates");
    from_b.expect("b negotiates");
    eventually("one session per side", || {
        (sessions(&a), sessions(&b)) == (1, 1)
    })
    .await;
}

#[tokio::test]
async fn a_peer_without_the_alpn_keeps_the_existing_connection() {
    let client = node().await;
    let server = node_with(NEGOTIATION_DEADLINE, |_| None).await;
    let connection = echo_connection(&client, &server).await;
    assert!(
        client
            .signaling
            .connect(server.endpoint.addr())
            .await
            .is_err()
    );
    assert_eq!((sessions(&client), sessions(&server)), (0, 0));
    assert_eq!(echo(&connection, b"still here").await, b"still here");
}

#[tokio::test]
async fn a_request_for_another_peer_creates_no_session() {
    let client = node().await;
    let server = node().await;
    let (_, offer) = client
        .transport
        .offer(server.endpoint.id())
        .await
        .expect("offer");
    let request = encode_request(
        SecretKey::generate().public(),
        SessionId::from_bytes([7; 16]),
        &offer,
    );
    let response = raw_exchange(&client, &server, &request)
        .await
        .expect("response");
    assert_eq!(response, Err(Rejection::WrongPeer));
    assert_eq!(sessions(&server), 0);
}

#[tokio::test]
async fn an_offer_over_the_limits_creates_no_session() {
    let client = node().await;
    let server = node().await;
    let candidates = "a=candidate:1 1 udp 1 127.0.0.1 9 typ host\r\n".repeat(33);
    let request = encode_request(
        server.endpoint.id(),
        SessionId::from_bytes([8; 16]),
        &format!("v=0\r\n{candidates}"),
    );
    let response = raw_exchange(&client, &server, &request)
        .await
        .expect("response");
    assert_eq!(response, Err(Rejection::InvalidOffer));
    let oversized = encode_request(
        server.endpoint.id(),
        SessionId::from_bytes([9; 16]),
        &"x".repeat(MAX_SDP_BYTES + 1),
    );
    assert!(raw_exchange(&client, &server, &oversized).await.is_err());
    assert_eq!(sessions(&server), 0);
}

#[tokio::test]
async fn an_unanswered_negotiation_expires_without_a_session() {
    let client = node_with(SHORT, |signaling| Some(Box::new(signaling))).await;
    let server = node_with(SHORT, |_| Some(Box::new(Silent))).await;
    let error = client
        .signaling
        .connect(server.endpoint.addr())
        .await
        .expect_err("expires");
    assert_eq!(rejection(&error), None);
    assert_eq!(sessions(&client), 0);
}

#[tokio::test]
async fn an_answer_after_the_generation_ends_creates_no_session() {
    let arrived = Arc::new(Notify::new());
    let gate = Arc::new(Barrier::new(2));
    let client = node_with(SHORT, |signaling| Some(Box::new(signaling))).await;
    let server = {
        let (arrived, gate) = (arrived.clone(), gate.clone());
        node_with(SHORT, move |inner| {
            Some(Box::new(Gated {
                inner,
                arrived,
                gate,
            }))
        })
        .await
    };
    let negotiation = client.signaling.connect(server.endpoint.addr());
    let reset = async {
        arrived.notified().await;
        client.signaling.reset();
        gate.wait().await;
    };
    let (result, ()) = tokio::join!(negotiation, reset);
    assert!(result.is_err());
    eventually("no session is left on either side", || {
        (sessions(&client), sessions(&server)) == (0, 0)
    })
    .await;
}
