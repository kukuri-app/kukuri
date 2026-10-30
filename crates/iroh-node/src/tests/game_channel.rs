//! #1979 AC-1: action session の datagram と stream(ADR 0070 §3・§5.2、F-SES-3〜5)。

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Result, ensure};
use iroh::EndpointAddr;
use kukuri_transport::TransportRelayConfig;

use super::game_session::{HostChannel, authorizer_with, eventually, pair};
use crate::{
    GAME_DATAGRAM_MAX_BYTES, GAME_STREAM_MESSAGE_MAX_BYTES, GameDelivery, GamePacket,
    GameSendRefused, GameSessionChannel, IrohDocsNode, NodeOptions,
};

/// `channel` から `count` 件を受け取るまで(期限 30 秒)取り出す。
async fn receive(channel: &GameSessionChannel, count: usize) -> Result<Vec<GamePacket>> {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut packets = Vec::new();
    while packets.len() < count {
        ensure!(
            Instant::now() < deadline,
            "received {} of {count}",
            packets.len()
        );
        packets.extend(channel.take(true, |_| true));
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Ok(packets)
}

async fn host_channel(channel: &HostChannel) -> Arc<GameSessionChannel> {
    for _ in 0..100 {
        if let Some(channel) = channel.lock().expect("host channel").clone() {
            return channel;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the host did not open the session");
}

fn numbered(index: usize, len: usize) -> Vec<u8> {
    let mut bytes = (index as u32).to_be_bytes().to_vec();
    bytes.resize(len, 0x5a);
    bytes
}

fn number(packet: &GamePacket) -> usize {
    u32::from_be_bytes(packet.bytes[..4].try_into().expect("numbered")) as usize
}

/// 実 iroh の 2 node(Direct P2P): 両方の向きで datagram と stream が届く。上限を超える payload は送らない。受け手が
/// 取り出さない datagram は新しい 64 件だけが残る。閉じた session は送らない。
#[tokio::test]
async fn both_deliveries_flow_both_ways_on_a_direct_session_until_it_closes() -> Result<()> {
    let (host, participant, host_id) = pair().await?;
    let released = Arc::new(AtomicUsize::new(0));
    let opened = HostChannel::default();
    host.install_game_session_authorizer(Some(authorizer_with(
        released.clone(),
        Arc::default(),
        opened.clone(),
    )));
    let link = participant.game_session_join(&host_id, b"join").await?;
    let host_side = host_channel(&opened).await;
    let guest = link.channel();

    guest.send(GameDelivery::Datagram, b"move")?;
    guest.send(GameDelivery::Stream, b"catch")?;
    let mut received = receive(&host_side, 2).await?;
    received.sort_by_key(|packet| packet.delivery.as_str());
    assert_eq!(received[0].delivery, GameDelivery::Datagram);
    assert_eq!(&received[0].bytes[..], b"move");
    assert_eq!(received[1].delivery, GameDelivery::Stream);
    assert_eq!(&received[1].bytes[..], b"catch");
    host_side.send(GameDelivery::Stream, b"caught")?;
    host_side.send(GameDelivery::Datagram, b"state")?;
    assert_eq!(receive(guest, 2).await?.len(), 2);
    assert_eq!(guest.path().path, "direct_p2p");
    assert_eq!(host_side.path().path, "direct_p2p");

    // F-SES-4: 上限を超える payload は送らない。
    assert_eq!(
        guest.send(
            GameDelivery::Datagram,
            &vec![0; GAME_DATAGRAM_MAX_BYTES + 1]
        ),
        Err(GameSendRefused::TooLarge)
    );
    assert_eq!(
        guest.send(
            GameDelivery::Stream,
            &vec![0; GAME_STREAM_MESSAGE_MAX_BYTES + 1]
        ),
        Err(GameSendRefused::TooLarge)
    );
    guest.send(GameDelivery::Datagram, &vec![0; GAME_DATAGRAM_MAX_BYTES])?;
    guest.send(
        GameDelivery::Stream,
        &vec![0; GAME_STREAM_MESSAGE_MAX_BYTES],
    )?;
    assert_eq!(receive(&host_side, 2).await?.len(), 2);

    // 取り出さないまま 200 件届くと、受け取った datagram は新しいものから 64 件だけが残る。
    for index in 0..200 {
        guest.send(GameDelivery::Datagram, &numbered(index, 16))?;
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    let kept = host_side.take(true, |_| true);
    assert!(kept.len() <= 64, "kept {}", kept.len());
    assert_eq!(number(kept.last().expect("some datagrams")), 199);

    link.leave();
    eventually(&released, 1).await;
    assert_eq!(
        guest.send(GameDelivery::Stream, b"late"),
        Err(GameSendRefused::Closed)
    );
    assert_eq!(
        host_side.send(GameDelivery::Stream, b"late"),
        Err(GameSendRefused::Closed)
    );
    participant.shutdown().await?;
    host.shutdown().await?;
    Ok(())
}

/// F-SES-5: 受け手が取り出さないと stream の message は捨てずに止まり、送り手は `Backpressure` を受ける。取り出すと
/// 残りが順に全部届く。
#[tokio::test]
async fn a_slow_receiver_pushes_back_on_the_stream_without_losing_messages() -> Result<()> {
    let (host, participant, host_id) = pair().await?;
    let opened = HostChannel::default();
    host.install_game_session_authorizer(Some(authorizer_with(
        Arc::default(),
        Arc::default(),
        opened.clone(),
    )));
    let link = participant.game_session_join(&host_id, b"join").await?;
    let host_side = host_channel(&opened).await;
    let guest = link.channel();

    let mut sent = 0;
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        match guest.send(GameDelivery::Stream, &numbered(sent, 32 * 1024)) {
            Ok(()) => sent += 1,
            Err(GameSendRefused::Backpressure) => break,
            Err(other) => anyhow::bail!("unexpected refusal: {other:?}"),
        }
        ensure!(Instant::now() < deadline, "no backpressure after {sent}");
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    assert!(
        sent * 32 * 1024 > 256 * 1024,
        "more than the receive limit was accepted before pushing back"
    );
    let received = receive(&host_side, sent).await?;
    let order: Vec<usize> = received.iter().map(number).collect();
    assert_eq!(order, (0..sent).collect::<Vec<_>>());
    assert!(
        received
            .iter()
            .all(|packet| packet.delivery == GameDelivery::Stream)
    );
    guest.send(GameDelivery::Stream, &numbered(sent, 8))?;
    assert_eq!(number(&receive(&host_side, 1).await?[0]), sent);

    participant.shutdown().await?;
    host.shutdown().await?;
    Ok(())
}

/// Relay Fallback の機能試験(手元の試験用 relay だけで届く。#1984 の性能の合否とは別): 参加者に IP の transport が
/// 無く Direct P2P が成り立たないとき、packet は relay を通って届き、経路は `relay_fallback` と分かる。
#[tokio::test]
async fn packets_reach_the_host_over_the_relay_when_no_direct_path_exists() -> Result<()> {
    let (relay, _relay_server) = kukuri_webrtc_transport::signaling_fixture::spawn_relay().await?;
    let options = |browser_like| NodeOptions {
        relay_config: TransportRelayConfig {
            iroh_relay_urls: vec![relay.to_string()],
        },
        browser_like,
        ..NodeOptions::default()
    };
    let host = IrohDocsNode::memory_with(options(false)).await?;
    let participant = IrohDocsNode::memory_with(options(true)).await?;
    host.endpoint().online().await;
    participant.endpoint().online().await;
    participant
        .discovery()
        .add_endpoint_info(host.endpoint().addr());
    let opened = HostChannel::default();
    host.install_game_session_authorizer(Some(authorizer_with(
        Arc::default(),
        Arc::default(),
        opened.clone(),
    )));
    let link = participant
        .game_session_join(&host.endpoint().id().to_string(), b"join")
        .await?;
    let host_side = host_channel(&opened).await;
    link.channel().send(GameDelivery::Stream, b"relayed")?;
    link.channel().send(GameDelivery::Datagram, b"state")?;
    let received = receive(&host_side, 2).await?;
    assert_eq!(received.len(), 2);
    assert_eq!(link.channel().path().path, "relay_fallback");
    assert_eq!(host_side.path().path, "relay_fallback");

    participant.shutdown().await?;
    host.shutdown().await?;
    Ok(())
}

/// 固定の遅延(25 ms、7 件ごとに 15 ms 足して順を入れ替える)と損失(10 件ごとに 1 件)を入れる UDP の中継。
/// 捨てた件数を返す。
async fn lossy_proxy(destination: SocketAddr) -> Result<(SocketAddr, Arc<AtomicUsize>)> {
    let socket = Arc::new(tokio::net::UdpSocket::bind("127.0.0.1:0").await?);
    let address = socket.local_addr()?;
    let dropped = Arc::new(AtomicUsize::new(0));
    let counter = dropped.clone();
    n0_future::task::spawn(async move {
        let mut buf = vec![0; 65_536];
        let mut client = None;
        let mut seen = 0usize;
        while let Ok((len, source)) = socket.recv_from(&mut buf).await {
            let target = if source == destination {
                let Some(client) = client else { continue };
                client
            } else {
                client = Some(source);
                destination
            };
            seen += 1;
            if seen.is_multiple_of(10) {
                counter.fetch_add(1, Ordering::SeqCst);
                continue;
            }
            let delay = Duration::from_millis(if seen.is_multiple_of(7) { 40 } else { 25 });
            let (socket, packet) = (socket.clone(), buf[..len].to_vec());
            n0_future::task::spawn(async move {
                tokio::time::sleep(delay).await;
                let _ = socket.send_to(&packet, target).await;
            });
        }
    });
    Ok((address, dropped))
}

/// F-SES-3: 固定の遅延・損失・順不同の下で、datagram は欠けても新しいものが届き、stream は順に全部届く。
#[tokio::test]
async fn datagrams_skip_losses_and_the_stream_stays_complete_over_a_lossy_link() -> Result<()> {
    const ALPN: &[u8] = b"/kukuri/test/lossy-game/1";
    let bind = || async {
        iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
            .relay_mode(iroh::RelayMode::Disabled)
            .alpns(vec![ALPN.to_vec()])
            .bind_addr("127.0.0.1:0".parse::<SocketAddr>()?)?
            .bind()
            .await
            .map_err(anyhow::Error::from)
    };
    let (server, client) = (bind().await?, bind().await?);
    let (proxy, dropped) = lossy_proxy(server.bound_sockets()[0]).await?;
    let accept = n0_future::task::spawn({
        let server = server.clone();
        async move { anyhow::Ok(server.accept().await.expect("incoming").await?) }
    });
    let outgoing = client
        .connect(EndpointAddr::new(server.id()).with_ip_addr(proxy), ALPN)
        .await?;
    let incoming = accept.await??;
    let (sender, receiver) = (
        GameSessionChannel::new(outgoing),
        GameSessionChannel::new(incoming),
    );
    let before = dropped.load(Ordering::SeqCst);

    for index in 0..100 {
        sender.send(GameDelivery::Datagram, &numbered(index, 64))?;
        sender.send(GameDelivery::Stream, &numbered(index, 512))?;
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let mut datagrams = Vec::new();
    let mut stream = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    while stream.len() < 100 {
        ensure!(Instant::now() < deadline, "stream {} of 100", stream.len());
        for packet in receiver.take(true, |_| true) {
            match packet.delivery {
                GameDelivery::Datagram => datagrams.push(number(&packet)),
                GameDelivery::Stream => stream.push(number(&packet)),
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    datagrams.extend(receiver.take(true, |_| true).iter().map(number));
    assert!(
        dropped.load(Ordering::SeqCst) > before,
        "the link dropped packets"
    );
    assert_eq!(
        stream,
        (0..100).collect::<Vec<_>>(),
        "the stream is complete and in order"
    );
    assert!(datagrams.len() < 100, "some datagrams were lost");
    assert!(
        datagrams.iter().any(|index| *index >= 90),
        "newer datagrams arrive after losses: {datagrams:?}"
    );
    Ok(())
}
