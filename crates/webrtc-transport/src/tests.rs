//! ADR 0057 §8 の native の固定 workload（E1〜E4・E6）。browser の組は `browser_tests.rs`。

use std::{
    future::poll_fn,
    net::Ipv4Addr,
    time::{Duration, Instant},
};

use iroh::{
    Endpoint, EndpointAddr, SecretKey, TransportAddr, endpoint::Connection, protocol::Router,
};

use super::*;
use crate::test_support::{
    ECHO_ALPN, Echo, WAIT, custom_only_endpoint, echo, negotiate, payload, selected_path_is_custom,
};

fn config() -> WebRtcConfig {
    WebRtcConfig {
        bind_ip: Ipv4Addr::LOCALHOST.into(),
    }
}

struct Pair {
    client: Arc<WebRtcTransport>,
    server: Arc<WebRtcTransport>,
    session: SessionId,
    connection: Connection,
    client_events: mpsc::Receiver<SessionEvent>,
    _router: Router,
    _endpoint: Endpoint,
}

/// custom のアドレスだけで接続した iroh の組。
async fn connected_pair() -> Pair {
    let client = WebRtcTransport::new(config());
    let server = WebRtcTransport::new(config());
    let mut client_events = client.take_events().expect("events");
    let mut server_events = server.take_events().expect("events");
    let client_endpoint = custom_only_endpoint(Arc::clone(&client)).await;
    let server_endpoint = custom_only_endpoint(Arc::clone(&server)).await;
    let router = Router::builder(server_endpoint.clone())
        .accept(ECHO_ALPN, Echo)
        .spawn();
    let (session, addr) = negotiate(
        &client,
        &mut client_events,
        client_endpoint.id(),
        &server,
        &mut server_events,
        server_endpoint.id(),
    )
    .await;
    let target = EndpointAddr::from_parts(server_endpoint.id(), [TransportAddr::Custom(addr)]);
    let connection = tokio::time::timeout(WAIT, client_endpoint.connect(target, ECHO_ALPN))
        .await
        .expect("connect timed out")
        .expect("connect");
    assert_eq!(connection.remote_id(), server_endpoint.id());
    assert!(
        selected_path_is_custom(&connection),
        "the custom path must carry the connection"
    );
    Pair {
        client,
        server,
        session,
        connection,
        client_events,
        _router: router,
        _endpoint: client_endpoint,
    }
}

/// E1: custom のアドレスだけで接続し、1 MiB の bi stream を往復する。
#[tokio::test(flavor = "multi_thread")]
async fn e1_a_mebibyte_round_trips_over_the_data_channel() {
    let pair = connected_pair().await;
    let data = payload(1024 * 1024);
    let echoed = tokio::time::timeout(WAIT, echo(&pair.connection, &data))
        .await
        .expect("echo timed out");
    assert_eq!(echoed, data);
}

/// E2: 1452 byte の datagram を 1000 回渡し、届いた datagram の境界と中身を確かめる。
#[tokio::test(flavor = "multi_thread")]
async fn e2_datagram_boundaries_survive_the_data_channel() {
    let offerer = WebRtcTransport::new(config());
    let answerer = WebRtcTransport::new(config());
    let mut offerer_events = offerer.take_events().expect("events");
    let mut answerer_events = answerer.take_events().expect("events");
    let sender_endpoint = offerer.bind().expect("bind");
    let mut receiver = answerer.bind().expect("bind");
    let offerer_id = SecretKey::generate().public();
    let answerer_id = SecretKey::generate().public();
    let (_, addr) = negotiate(
        &offerer,
        &mut offerer_events,
        offerer_id,
        &answerer,
        &mut answerer_events,
        answerer_id,
    )
    .await;
    let sender = WebRtcSender {
        shared: Arc::clone(&offerer.shared),
    };
    let out = sender.open_session(&addr).expect("open session");
    let _ = sender_endpoint;

    const COUNT: u16 = 1000;
    const LEN: usize = 1452;
    let received = tokio::spawn(async move {
        let mut seen = Vec::new();
        let mut buf = vec![0u8; 2048];
        let deadline = Instant::now() + Duration::from_secs(5);
        while seen.len() < COUNT as usize && Instant::now() < deadline {
            let polled = tokio::time::timeout(
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
                continue;
            };
            assert_eq!(len, LEN, "a datagram arrived with a different length");
            let index = u16::from_be_bytes([buf[0], buf[1]]);
            assert!(
                buf[2..LEN].iter().all(|byte| *byte == index as u8),
                "a datagram arrived with different contents"
            );
            seen.push(index);
        }
        seen
    });
    for index in 0..COUNT {
        let mut datagram = vec![index as u8; LEN];
        datagram[..2].copy_from_slice(&index.to_be_bytes());
        sender.send_datagram(&out, &datagram);
        if index % 32 == 31 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
    let seen = received.await.expect("receiver");
    let stats = offerer.stats();
    eprintln!(
        "E2: sent {COUNT} datagrams of {LEN} bytes, received {}, dropped before send {}, dropped on receive {}",
        seen.len(),
        stats.dropped_send,
        answerer.stats().dropped_recv
    );
    assert!(!seen.is_empty(), "no datagram arrived");
}

/// E3: 受信側が読まない間に 8 MiB を送る。待ちと未送信 bytes が上限に収まり、読み出しの再開で完走する。
#[tokio::test(flavor = "multi_thread")]
async fn e3_a_stalled_reader_keeps_the_queues_bounded() {
    let pair = connected_pair().await;
    let data = payload(8 * 1024 * 1024);
    let (mut send, mut recv) = pair.connection.open_bi().await.expect("open bi");
    let writer = {
        let data = data.clone();
        tokio::spawn(async move {
            send.write_all(&data).await.expect("write");
            send.finish().expect("finish");
        })
    };
    // echo は相手が読むまで返さない。こちらが 2 秒読まない間も送信は続く。
    tokio::time::sleep(Duration::from_secs(2)).await;
    let echoed = tokio::time::timeout(Duration::from_secs(60), recv.read_to_end(data.len() + 1))
        .await
        .expect("read timed out")
        .expect("read");
    writer.await.expect("writer");
    assert_eq!(echoed, data);
    for stats in [pair.client.stats(), pair.server.stats()] {
        eprintln!("E3: {stats:?}");
        assert!(
            stats.max_buffered_bytes <= (BUFFERED_HIGH_BYTES + MAX_DATAGRAM_BYTES) as u64,
            "buffered bytes exceeded the high water mark: {stats:?}"
        );
    }
}

/// E4: 転送の途中で session を閉じると、session・socket・task が解放され、閉じた session には何も戻らない。
#[tokio::test(flavor = "multi_thread")]
async fn e4_closing_a_session_releases_its_resources() {
    let mut pair = connected_pair().await;
    let connection = pair.connection.clone();
    let transfer = tokio::spawn(async move {
        let data = payload(4 * 1024 * 1024);
        let _ = tokio::time::timeout(Duration::from_secs(10), echo(&connection, &data)).await;
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    pair.client.close(pair.session);
    loop {
        match tokio::time::timeout(WAIT, pair.client_events.recv()).await {
            Ok(Some(SessionEvent::Closed { session, .. })) if session == pair.session => break,
            Ok(Some(_)) => continue,
            other => panic!("expected the closed event: {other:?}"),
        }
    }
    let stats = pair.client.stats();
    assert_eq!(stats.sessions, 0);
    assert_eq!(stats.running_backends, 0);
    // 閉じた session の遅れた callback は何も登録しない。
    pair.client.shared.opened(pair.session);
    pair.client
        .shared
        .deliver(pair.session, Bytes::from_static(b"late"));
    assert_eq!(pair.client.stats().sessions, 0);
    transfer.abort();
    let _ = pair.server;
}

/// E6: DataChannel の送信の 5% を捨てても、1 MiB の往復が完走する。
#[tokio::test(flavor = "multi_thread")]
async fn e6_quic_recovers_from_five_percent_loss() {
    let pair = connected_pair().await;
    pair.client.shared.set_test_send_loss(20);
    pair.server.shared.set_test_send_loss(20);
    let data = payload(1024 * 1024);
    let started = Instant::now();
    let echoed = tokio::time::timeout(Duration::from_secs(60), echo(&pair.connection, &data))
        .await
        .expect("echo timed out");
    assert_eq!(echoed, data);
    eprintln!(
        "E6: 1 MiB round trip with 5% send loss took {:?}; client {:?}; server {:?}",
        started.elapsed(),
        pair.client.stats(),
        pair.server.stats()
    );
}

#[test]
fn session_addrs_distinguish_the_two_ends_and_carry_no_secret() {
    let session = SessionId::from_bytes([7; 16]);
    let offerer = session_addr(session, Role::Offerer);
    let answerer = session_addr(session, Role::Answerer);
    assert_ne!(offerer, answerer);
    assert_eq!(offerer.data().len(), 17);
    assert_eq!(
        parse_session_addr(&answerer),
        Some((session, Role::Answerer))
    );
    assert_eq!(
        parse_session_addr(&CustomAddr::from_parts(0x20, &[0; 17])),
        None
    );
}

#[test]
fn remote_sdp_over_the_limits_is_rejected() {
    assert!(check_remote_sdp(&"v".repeat(MAX_SDP_BYTES + 1)).is_err());
    let many = "a=candidate:1 1 udp 1 127.0.0.1 1 typ host\n".repeat(MAX_SDP_CANDIDATES + 1);
    assert!(check_remote_sdp(&many).is_err());
    let long = format!("a=candidate:{}\n", "x".repeat(MAX_CANDIDATE_BYTES));
    assert!(check_remote_sdp(&long).is_err());
    let ok = "a=candidate:1 1 udp 1 127.0.0.1 1 typ host\n".repeat(MAX_SDP_CANDIDATES);
    assert!(check_remote_sdp(&ok).is_ok());
}

/// #1422 AC-2 J7: STUN の送信先を渡したときだけ問い合わせ、応答のアドレスを server reflexive の候補として offer に載せる。
#[tokio::test]
async fn the_offer_carries_a_reflexive_candidate_only_with_stun() {
    let stun = tokio::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("bind stun");
    let server = stun.local_addr().expect("stun addr").to_string();
    let requests = Arc::new(AtomicU64::new(0));
    // 192.0.2.7:4242 を XOR-MAPPED-ADDRESS で返す（RFC 8489 §14.2）。
    let responder = tokio::spawn({
        let requests = Arc::clone(&requests);
        async move {
            let mut buf = [0u8; 512];
            while let Ok((20, from)) = stun.recv_from(&mut buf).await {
                requests.fetch_add(1, Ordering::Relaxed);
                let mut reply = [0u8; 32];
                reply[..2].copy_from_slice(&[0x01, 0x01]);
                reply[3] = 12;
                reply[4..20].copy_from_slice(&buf[4..20]);
                reply[20..24].copy_from_slice(&[0x00, 0x20, 0x00, 0x08]);
                reply[25] = 1;
                reply[26..28].copy_from_slice(&(4242u16 ^ 0x2112).to_be_bytes());
                for (index, byte) in [192u8, 0, 2, 7].into_iter().enumerate() {
                    reply[28 + index] = byte ^ buf[4 + index];
                }
                let _ = stun.send_to(&reply, from).await;
            }
        }
    });
    let transport = WebRtcTransport::new(config());
    let peer = SecretKey::generate().public();
    let (_, plain) = transport.offer(peer, &[]).await.expect("offer");
    assert!(!plain.contains("typ srflx"));
    assert_eq!(requests.load(Ordering::Relaxed), 0);
    let (_, offer) = transport.offer(peer, &[server]).await.expect("offer");
    assert_eq!(requests.load(Ordering::Relaxed), 1);
    assert!(offer.contains("192.0.2.7 4242 typ srflx"), "{offer}");
    responder.abort();
}
