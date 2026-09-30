//! ADR 0057 §8 の browser の固定 workload（E1〜E4）。headless の Chromium で
//! `scripts/ci/browser_peer_test.sh` から実行する。browser↔native の相手は `examples/webrtc_peer.rs`。
//! 末尾は #1422 W10 AC-1 の browser↔browser の専用 ALPN の交渉（相手の手元の iroh relay で始めた接続の上）。

use std::{future::poll_fn, time::Duration};

use iroh::{
    Endpoint, EndpointAddr, SecretKey, TransportAddr, endpoint::Connection, protocol::Router,
};
use wasm_bindgen_test::{console_log, wasm_bindgen_test, wasm_bindgen_test_configure};

use super::*;
use crate::test_support::{
    ECHO_ALPN, Echo, WAIT, custom_only_endpoint, echo, negotiate, next_opened, payload,
    selected_path_is_custom,
};

wasm_bindgen_test_configure!(run_in_browser);

fn config() -> WebRtcConfig {
    WebRtcConfig {}
}

async fn connect(endpoint: &Endpoint, remote: EndpointId, addr: CustomAddr) -> Connection {
    let target = EndpointAddr::from_parts(remote, [TransportAddr::Custom(addr)]);
    let connection = n0_future::time::timeout(WAIT, endpoint.connect(target, ECHO_ALPN))
        .await
        .expect("connect timed out")
        .expect("connect");
    assert_eq!(connection.remote_id(), remote);
    assert!(
        selected_path_is_custom(&connection),
        "the custom path must carry the connection"
    );
    connection
}

/// E1（browser↔browser）: 同じ page の 2 つの PeerConnection で 1 MiB を往復する。
#[wasm_bindgen_test]
async fn e1_browser_to_browser_round_trips_a_mebibyte() {
    let client = WebRtcTransport::new(config());
    let server = WebRtcTransport::new(config());
    let mut client_events = client.take_events().expect("events");
    let mut server_events = server.take_events().expect("events");
    let client_endpoint = custom_only_endpoint(Arc::clone(&client)).await;
    let server_endpoint = custom_only_endpoint(Arc::clone(&server)).await;
    let _router = Router::builder(server_endpoint.clone())
        .accept(ECHO_ALPN, Echo)
        .spawn();
    let (_, addr) = negotiate(
        &client,
        &mut client_events,
        client_endpoint.id(),
        &server,
        &mut server_events,
        server_endpoint.id(),
    )
    .await;
    let connection = connect(&client_endpoint, server_endpoint.id(), addr).await;
    let data = payload(1024 * 1024);
    assert_eq!(echo(&connection, &data).await, data);
}

/// E2（browser↔browser）: 1452 byte の datagram の境界と中身が保たれる。
#[wasm_bindgen_test]
async fn e2_browser_datagram_boundaries_survive() {
    let offerer = WebRtcTransport::new(config());
    let answerer = WebRtcTransport::new(config());
    let mut offerer_events = offerer.take_events().expect("events");
    let mut answerer_events = answerer.take_events().expect("events");
    let _sender_endpoint = offerer.bind().expect("bind");
    let mut receiver = answerer.bind().expect("bind");
    let (_, addr) = negotiate(
        &offerer,
        &mut offerer_events,
        SecretKey::generate().public(),
        &answerer,
        &mut answerer_events,
        SecretKey::generate().public(),
    )
    .await;
    let sender = WebRtcSender {
        shared: Arc::clone(&offerer.shared),
    };
    let out = sender.open_session(&addr).expect("open session");

    const COUNT: u16 = 1000;
    const LEN: usize = 1452;
    for index in 0..COUNT {
        let mut datagram = vec![index as u8; LEN];
        datagram[..2].copy_from_slice(&index.to_be_bytes());
        sender.send_datagram(&out, &datagram);
        if index % 32 == 31 {
            n0_future::time::sleep(Duration::from_millis(5)).await;
        }
    }
    let mut seen = 0usize;
    let mut buf = vec![0u8; 2048];
    while seen < COUNT as usize {
        let polled = n0_future::time::timeout(
            Duration::from_millis(500),
            poll_fn(|cx| {
                let mut bufs = [io::IoSliceMut::new(&mut buf)];
                let mut metas = [noq_udp::RecvMeta::default()];
                let mut infos = [RecvInfo::default()];
                receiver
                    .poll_recv(cx, &mut bufs, &mut metas, &mut infos)
                    .map(|result| result.map(|filled| (filled, metas[0].len)))
            }),
        )
        .await;
        let Ok(Ok((1, len))) = polled else {
            break;
        };
        assert_eq!(len, LEN, "a datagram arrived with a different length");
        let index = u16::from_be_bytes([buf[0], buf[1]]);
        assert!(
            buf[2..LEN].iter().all(|byte| *byte == index as u8),
            "a datagram arrived with different contents"
        );
        seen += 1;
    }
    console_log!(
        "E2 browser: sent {COUNT} datagrams of {LEN} bytes, received {seen}, {:?}",
        offerer.stats()
    );
    assert!(seen > 0, "no datagram arrived");
}

struct NativePair {
    transport: Arc<WebRtcTransport>,
    session: SessionId,
    connection: Connection,
    events: mpsc::Receiver<SessionEvent>,
    _endpoint: Endpoint,
}

async fn native_pair() -> NativePair {
    let transport = WebRtcTransport::new(config());
    let mut events = transport.take_events().expect("events");
    let endpoint = custom_only_endpoint(Arc::clone(&transport)).await;
    let (session, offer) = transport
        .offer(SecretKey::generate().public())
        .await
        .expect("offer");
    let (remote, answer) = crate::signaling_fixture::post_offer(endpoint.id(), session, &offer)
        .await
        .expect("native answer");
    transport
        .accept_answer(session, &answer)
        .await
        .expect("accept answer");
    let addr = next_opened(&mut events).await;
    let connection = connect(&endpoint, remote, addr).await;
    NativePair {
        transport,
        session,
        connection,
        events,
        _endpoint: endpoint,
    }
}

/// E1（browser↔native）
#[wasm_bindgen_test]
async fn e1_browser_to_native_round_trips_a_mebibyte() {
    let pair = native_pair().await;
    let data = payload(1024 * 1024);
    assert_eq!(echo(&pair.connection, &data).await, data);
}

/// E3（browser↔native）: 読み出しを止めて 8 MiB を送っても、未送信 bytes が上限に収まり、再開で完走する。
#[wasm_bindgen_test]
async fn e3_browser_to_native_keeps_the_queues_bounded() {
    let pair = native_pair().await;
    let data = payload(8 * 1024 * 1024);
    let (mut send, mut recv) = pair.connection.open_bi().await.expect("open bi");
    let write = async {
        send.write_all(&data).await.expect("write");
        send.finish().expect("finish");
    };
    let read = async {
        n0_future::time::sleep(Duration::from_secs(2)).await;
        recv.read_to_end(data.len() + 1).await.expect("read")
    };
    let ((), echoed) = n0_future::future::zip(write, read).await;
    assert_eq!(echoed, data);
    let stats = pair.transport.stats();
    console_log!("E3 browser: {stats:?}");
    assert!(
        stats.max_buffered_bytes <= (BUFFERED_HIGH_BYTES + MAX_DATAGRAM_BYTES) as u64,
        "buffered bytes exceeded the high water mark: {stats:?}"
    );
}

/// E4（browser↔native）: 転送の途中で session を閉じると、session と task が解放される。
#[wasm_bindgen_test]
async fn e4_browser_closing_a_session_releases_its_resources() {
    let mut pair = native_pair().await;
    let connection = pair.connection.clone();
    wasm_bindgen_futures::spawn_local(async move {
        let data = payload(4 * 1024 * 1024);
        let _ = n0_future::time::timeout(Duration::from_secs(10), echo(&connection, &data)).await;
    });
    n0_future::time::sleep(Duration::from_millis(200)).await;
    pair.transport.close(pair.session);
    loop {
        match n0_future::time::timeout(WAIT, pair.events.recv()).await {
            Ok(Some(SessionEvent::Closed { session, .. })) if session == pair.session => break,
            Ok(Some(_)) => continue,
            other => panic!("expected the closed event: {other:?}"),
        }
    }
    let stats = pair.transport.stats();
    assert_eq!(stats.sessions, 0);
    assert_eq!(stats.running_backends, 0);
    pair.transport.shared.opened(pair.session);
    pair.transport
        .shared
        .deliver(pair.session, Bytes::from_static(b"late"));
    assert_eq!(pair.transport.stats().sessions, 0);
}

/// 相手の手元の iroh relay だけで届き、custom transport と専用 ALPN の交渉を持つ endpoint。
async fn relay_node(relay: &iroh::RelayUrl) -> (Endpoint, Arc<Signaling>, Router) {
    let transport = WebRtcTransport::new(config());
    let endpoint = Endpoint::builder(iroh::endpoint::presets::Minimal)
        .secret_key(SecretKey::generate())
        .relay_mode(iroh::RelayMode::Custom(iroh::RelayMap::from_iter([
            relay.clone()
        ])))
        .add_custom_transport(transport.clone())
        .bind()
        .await
        .expect("bind endpoint");
    endpoint.online().await;
    let signaling = Signaling::new(transport, endpoint.clone(), crate::NEGOTIATION_DEADLINE)
        .expect("signaling");
    let router = Router::builder(endpoint.clone())
        .accept(ECHO_ALPN, Echo)
        .accept(crate::SIGNALING_ALPN, signaling.clone())
        .spawn();
    (endpoint, signaling, router)
}

/// W10 AC-1（browser↔browser）: relay で始めた接続の上で、どちらの端から交渉しても、同じ接続の選ばれた
/// path が custom へ移り、1 MiB の往復が一致する。
#[wasm_bindgen_test]
async fn browser_to_browser_negotiation_moves_the_connection_to_the_custom_path() {
    let relay = crate::signaling_fixture::relay_url().await.expect("relay");
    for from_client in [true, false] {
        let client = relay_node(&relay).await;
        let server = relay_node(&relay).await;
        let relay_addr = |endpoint: &Endpoint| {
            EndpointAddr::from_parts(endpoint.id(), [TransportAddr::Relay(relay.clone())])
        };
        let connection = client
            .0
            .connect(relay_addr(&server.0), ECHO_ALPN)
            .await
            .expect("connect over the relay");
        let (from, to) = if from_client {
            (&client, &server)
        } else {
            (&server, &client)
        };
        from.1.connect(relay_addr(&to.0)).await.expect("negotiate");
        n0_future::time::timeout(WAIT, async {
            while !selected_path_is_custom(&connection) {
                n0_future::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the connection moves to the custom path");
        let data = payload(1024 * 1024);
        assert_eq!(echo(&connection, &data).await, data);
    }
}
