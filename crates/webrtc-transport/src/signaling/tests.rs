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
/// `demand` なら需要の接続のある相手と交渉を始める（ブラウザの端）。
async fn node_with(
    deadline: Duration,
    demand: bool,
    handler: impl FnOnce(Arc<Signaling>) -> Option<Box<dyn iroh::protocol::DynProtocolHandler>>,
) -> Node {
    let transport = WebRtcTransport::new(WebRtcConfig {
        bind_ip: Ipv4Addr::LOCALHOST.into(),
    });
    let hooks = demand.then(DemandHooks::default);
    let mut builder = Endpoint::builder(presets::Minimal)
        .secret_key(SecretKey::generate())
        .relay_mode(RelayMode::Disabled)
        .bind_addr((Ipv4Addr::LOCALHOST, 0))
        .expect("bind addr")
        .add_custom_transport(transport.clone());
    if let Some(hooks) = &hooks {
        builder = builder.hooks(hooks.clone());
    }
    let endpoint = builder.bind().await.expect("bind endpoint");
    let signaling = Signaling::new(
        transport.clone(),
        endpoint.clone(),
        hooks.as_ref(),
        deadline,
    )
    .expect("signaling");
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
    node_with(NEGOTIATION_DEADLINE, false, |signaling| {
        Some(Box::new(signaling))
    })
    .await
}

/// 需要の接続のある相手と交渉を始める node。
async fn initiator(deadline: Duration) -> Node {
    node_with(deadline, true, |signaling| Some(Box::new(signaling))).await
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
    let a = node_with(NEGOTIATION_DEADLINE, false, gated(gate.clone())).await;
    let b = node_with(NEGOTIATION_DEADLINE, false, gated(gate)).await;
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
    let server = node_with(NEGOTIATION_DEADLINE, false, |_| None).await;
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
        .offer(server.endpoint.id(), &[])
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
    let client = node_with(SHORT, false, |signaling| Some(Box::new(signaling))).await;
    let server = node_with(SHORT, false, |_| Some(Box::new(Silent))).await;
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
    let client = node_with(SHORT, false, |signaling| Some(Box::new(signaling))).await;
    let server = {
        let (arrived, gate) = (arrived.clone(), gate.clone());
        node_with(SHORT, false, move |inner| {
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

fn close(connection: &Connection) {
    connection.close(0u32.into(), b"done");
}

fn custom_path_is_open(connection: &Connection) -> bool {
    connection
        .paths()
        .iter()
        .any(|path| path.remote_addr().is_custom())
}

/// #1422 AC-2 J1: 需要の接続を張るだけで交渉が始まり、相手との session は 1 本。後の接続にも同じ custom path が入り、
/// 需要の接続がすべて閉じたら session も閉じて表から消える。
#[tokio::test]
async fn a_demand_connection_shares_one_session_until_the_demand_ends() {
    let client = initiator(NEGOTIATION_DEADLINE).await;
    let server = node().await;
    let first = echo_connection(&client, &server).await;
    eventually("the demand opens a session", || {
        (sessions(&client), sessions(&server)) == (1, 1)
    })
    .await;
    let second = echo_connection(&client, &server).await;
    eventually("the later connection gets the custom path", || {
        custom_path_is_open(&second)
    })
    .await;
    assert_eq!(
        echo(&second, b"over the session").await,
        b"over the session"
    );
    assert_eq!(
        client.signaling.stats(),
        SignalingStats {
            demand_peers: 1,
            attempts: 1
        }
    );
    assert_eq!((sessions(&client), sessions(&server)), (1, 1));
    close(&first);
    close(&second);
    eventually("the end of the demand closes the session", || {
        (
            sessions(&client),
            sessions(&server),
            client.signaling.stats().demand_peers,
        ) == (0, 0, 0)
    })
    .await;
}

/// #1422 AC-2 J2（T2）: 交渉の ALPN の無い相手には `MAX_ATTEMPTS` 回だけ試し、既存の経路の通信は続く。
#[tokio::test]
async fn a_peer_without_the_alpn_is_tried_a_bounded_number_of_times() {
    let client = initiator(SHORT).await;
    let server = node_with(SHORT, false, |_| None).await;
    let connection = echo_connection(&client, &server).await;
    eventually("every attempt is used", || {
        client.signaling.stats().attempts == u64::from(MAX_ATTEMPTS)
    })
    .await;
    n0_future::time::sleep(SHORT * 2).await;
    assert_eq!(client.signaling.stats().attempts, u64::from(MAX_ATTEMPTS));
    assert_eq!((sessions(&client), sessions(&server)), (0, 0));
    assert_eq!(echo(&connection, b"still here").await, b"still here");
}

/// #1422 AC-2 J7: STUN の送信先は relay の host の 3478 番。relay が無ければ送らない。
#[test]
fn stun_goes_to_the_relay_hosts_only() {
    let relays: Vec<RelayUrl> = ["http://127.0.0.1:3340", "https://relay.example.org"]
        .into_iter()
        .map(|url| url.parse().expect("relay url"))
        .collect();
    assert_eq!(
        stun_servers(&relays),
        ["127.0.0.1:3478", "relay.example.org:3478"]
    );
    assert!(stun_servers(&[]).is_empty());
}
