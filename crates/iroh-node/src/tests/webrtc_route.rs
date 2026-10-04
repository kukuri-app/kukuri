//! #1422 W10 AC-2 の native の固定 fixture。ブラウザの端（IP の transport が無く relay だけで届き、交渉を始める）を
//! native で作り、native の node（UDP・relay・交渉に応じるだけ）と手元の iroh relay でつなぐ。

use std::{net::Ipv4Addr, sync::Arc, time::Duration};

use anyhow::{Context as _, Result};
use futures_util::StreamExt;
use iroh::{EndpointAddr, RelayUrl, TransportAddr};
use iroh_gossip::{TopicId, api::Event};
use kukuri_transport::TransportRelayConfig;
use kukuri_webrtc_transport::{WebRtcConfig, WebRtcTransport, signaling_fixture};

use crate::{DOC_READ_ALPN, DocReadQuery, DocReadResponse, IrohDocsNode, NodeOptions};

#[path = "../../tests/support/web_e2e.rs"]
mod web_e2e;

const WAIT: Duration = Duration::from_secs(20);

async fn node(
    relay: &RelayUrl,
    browser_like: bool,
) -> Result<(Arc<IrohDocsNode>, Arc<WebRtcTransport>)> {
    let transport = WebRtcTransport::new(WebRtcConfig {
        bind_ip: Ipv4Addr::LOCALHOST.into(),
    });
    let node = IrohDocsNode::memory_with(NodeOptions {
        relay_config: TransportRelayConfig {
            iroh_relay_urls: vec![relay.to_string()],
        },
        webrtc: Some(transport.clone()),
        browser_like,
        ..NodeOptions::default()
    })
    .await?;
    node.endpoint().online().await;
    Ok((node, transport))
}

/// relay だけで届く宛先。
fn via_relay(node: &IrohDocsNode, relay: &RelayUrl) -> EndpointAddr {
    EndpointAddr::from_parts(node.endpoint().id(), [TransportAddr::Relay(relay.clone())])
}

async fn eventually(what: &str, mut done: impl AsyncFnMut() -> bool) -> Result<()> {
    n0_future::time::timeout(WAIT, async {
        while !done().await {
            n0_future::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .with_context(|| what.to_string())
}

/// 相手への経路に active な custom path があるか（診断が見る `remote_info`）。
async fn custom_is_active(node: &IrohDocsNode, peer: &IrohDocsNode) -> bool {
    node.endpoint()
        .remote_info(peer.endpoint().id())
        .await
        .is_some_and(|info| {
            info.addrs().any(|addr| {
                addr.addr().is_custom()
                    && matches!(addr.usage(), iroh::endpoint::TransportAddrUsage::Active)
            })
        })
}

async fn next_message(receiver: &mut iroh_gossip::api::GossipReceiver) -> Result<Vec<u8>> {
    n0_future::time::timeout(WAIT, async {
        loop {
            if let Event::Received(message) = receiver.next().await.context("topic ended")?? {
                return anyhow::Ok(message.content.to_vec());
            }
        }
    })
    .await
    .context("no gossip message")?
}

/// J3（S1・S2・T3）: relay で始めた需要の接続へ交渉で custom path が足され、gossip の同じ購読・有界な reader・blob の
/// 実データが custom を通る。blob の転送の途中で custom だけを閉じても、同じ接続のまま relay で完走し hash が一致し、
/// gossip の同じ購読と reader も relay で続く。
#[tokio::test]
async fn the_route_moves_to_the_custom_path_and_falls_back_to_the_relay() -> Result<()> {
    let (relay, _relay_server) = signaling_fixture::spawn_relay().await?;
    let (web, web_transport) = node(&relay, true).await?;
    let (native, native_transport) = node(&relay, false).await?;
    let native_addr = via_relay(&native, &relay);
    web_e2e::seed(&native, "native").await?;

    // gossip の購読が最初の需要の接続になり、交渉が始まる。
    let topic = TopicId::from_bytes([7; 32]);
    let (native_sender, _native_receiver) = native.gossip().subscribe(topic, vec![]).await?.split();
    web.discovery().add_endpoint_info(native_addr.clone());
    let (_web_sender, mut web_receiver) = web
        .gossip()
        .subscribe_and_join(topic, vec![native.endpoint().id()])
        .await?
        .split();
    eventually("one session per side", async || {
        (
            web_transport.stats().sessions,
            native_transport.stats().sessions,
        ) == (1, 1)
    })
    .await?;
    eventually("the custom path is active", async || {
        custom_is_active(&web, &native).await
    })
    .await?;
    native_sender.broadcast(b"before".to_vec().into()).await?;
    assert_eq!(next_message(&mut web_receiver).await?, b"before");

    // 有界な reader の実データが custom を通る。
    let demand = web_e2e::demand(&web, native_addr.clone()).await?;
    let read =
        web_e2e::read_over_custom(&web, &web_transport, &demand, native_addr.clone(), "native")
            .await?;
    assert!(read.ends_with("via_custom=true"), "{read}");

    // 4 MiB の blob の転送の途中で custom だけを閉じる。
    let data: Vec<u8> = (0..4 * 1024 * 1024)
        .map(|index| (index % 251) as u8)
        .collect();
    let hash = native.blobs().blobs().add_bytes(data.clone()).await?.hash;
    let connection = web
        .endpoint()
        .connect(native_addr.clone(), iroh_blobs::ALPN)
        .await?;
    let started = web_transport.stats().received_bytes;
    let fetch = tokio::spawn({
        let blobs = web.blobs().clone();
        let connection = connection.clone();
        async move { blobs.remote().fetch(connection, hash).await }
    });
    eventually(
        "the transfer is under way over the custom path",
        async || web_transport.stats().received_bytes >= started + 1024 * 1024,
    )
    .await?;
    assert!(!fetch.is_finished());
    // #1482 J1: 閉じた custom path を iroh がすぐ閉じるので、relay で受信が再開する（64 KiB 届く）まで 2 秒以内。
    let relayed = || web.endpoint().metrics().socket.recv_data_relay.get();
    let before = relayed();
    let closed_at = n0_future::time::Instant::now();
    web.webrtc_signaling().context("webrtc")?.reset();
    let at_reset = web_transport.stats().received_bytes;
    eventually("the transfer resumes over the relay", async || {
        relayed() >= before + 64 * 1024
    })
    .await?;
    let stall = closed_at.elapsed();
    assert!(stall <= Duration::from_secs(2), "stall {stall:?}");
    fetch.await??;
    assert_eq!(web.blobs().blobs().get_bytes(hash).await?, data);
    // 残りは relay で届いた（custom の受信は reset の後に増えていない）。接続はそのままで、開いた path は relay だけ。
    assert_eq!(web_transport.stats().received_bytes, at_reset);
    assert!(connection.close_reason().is_none());
    let paths = connection.paths();
    assert!(!paths.is_empty() && paths.iter().all(|path| path.is_relay()));
    assert!(!custom_is_active(&web, &native).await);
    assert_eq!(web_transport.stats().sessions, 0);

    // 同じ購読と reader は relay で続く。
    native_sender.broadcast(b"after".to_vec().into()).await?;
    assert_eq!(next_message(&mut web_receiver).await?, b"after");
    let read = web_e2e::read_newest(&web, native_addr, "native").await?;
    assert!(read.contains("value=native-3"), "{read}");
    Ok(())
}

/// #1482 J1（接続の直後の喪失）: 新しい接続は、選ばれた custom path で始まり、relay の path は約 335 ms 後（相手の
/// connection ID を待つ再試行）に開く。その前に custom を失うと、custom path は接続の最後の path なので、その時点では閉じられない。
/// relay の path が開いた時点で閉じ直し、同じ接続のまま relay で 2 秒以内に受信が再開する。
#[tokio::test]
async fn a_custom_path_lost_before_the_relay_path_opens_falls_back_to_the_relay() -> Result<()> {
    let (relay, _relay_server) = signaling_fixture::spawn_relay().await?;
    let (web, web_transport) = node(&relay, true).await?;
    let (native, _native_transport) = node(&relay, false).await?;
    let native_addr = via_relay(&native, &relay);
    let _demand = web_e2e::demand(&web, native_addr.clone()).await?;
    eventually("the custom path is active", async || {
        custom_is_active(&web, &native).await
    })
    .await?;
    let data: Vec<u8> = (0..4 * 1024 * 1024)
        .map(|index| (index % 251) as u8)
        .collect();
    let hash = native.blobs().blobs().add_bytes(data.clone()).await?.hash;
    let connection = web
        .endpoint()
        .connect(native_addr.clone(), iroh_blobs::ALPN)
        .await?;
    let started = web_transport.stats().received_bytes;
    let fetch = tokio::spawn({
        let blobs = web.blobs().clone();
        let connection = connection.clone();
        async move { blobs.remote().fetch(connection, hash).await }
    });
    eventually("the transfer starts over the custom path", async || {
        web_transport.stats().received_bytes >= started + 64 * 1024
    })
    .await?;
    assert!(
        !connection.paths().iter().any(|path| path.is_relay()),
        "the relay path is not open yet"
    );
    let relayed = || web.endpoint().metrics().socket.recv_data_relay.get();
    let before = relayed();
    let closed_at = n0_future::time::Instant::now();
    web.webrtc_signaling().context("webrtc")?.reset();
    eventually("the transfer resumes over the relay", async || {
        relayed() >= before + 64 * 1024
    })
    .await?;
    let stall = closed_at.elapsed();
    assert!(stall <= Duration::from_secs(2), "stall {stall:?}");
    fetch.await??;
    assert_eq!(web.blobs().blobs().get_bytes(hash).await?, data);
    assert!(connection.close_reason().is_none());
    Ok(())
}

/// #1571: 接続が確立した直後（相手の connection ID が届く前）に custom path を失うと、その接続は外された custom path
/// だけを持ち、閉じられない。同じ相手への新しい接続は、その path へ最初の送信を向けずに relay で確立して通信でき、
/// 止まった接続は 3 秒で閉じる。
#[tokio::test]
async fn a_custom_path_lost_right_after_a_connection_starts_does_not_stall_the_peer() -> Result<()>
{
    let (relay, _relay_server) = signaling_fixture::spawn_relay().await?;
    let (web, _web_transport) = node(&relay, true).await?;
    let (native, _native_transport) = node(&relay, false).await?;
    let native_addr = via_relay(&native, &relay);
    let _demand = web_e2e::demand(&web, native_addr.clone()).await?;
    eventually("the custom path is active", async || {
        custom_is_active(&web, &native).await
    })
    .await?;
    let data: Vec<u8> = (0..64 * 1024).map(|index| (index % 251) as u8).collect();
    let hash = native.blobs().blobs().add_bytes(data.clone()).await?.hash;
    let stuck = web
        .endpoint()
        .connect(native_addr.clone(), iroh_blobs::ALPN)
        .await?;
    // 接続が返った直後、相手の connection ID（確立の後に届く）より先に、同期の reset で custom path を失わせる。
    web.webrtc_signaling().context("webrtc")?.reset();
    let lost_at = n0_future::time::Instant::now();
    let fresh = n0_future::time::timeout(
        Duration::from_secs(2),
        web.endpoint()
            .connect(native_addr.clone(), iroh_blobs::ALPN),
    )
    .await
    .context("a new connection to the peer stalls")??;
    web.blobs().remote().fetch(fresh, hash).await?;
    let fetched = lost_at.elapsed();
    assert!(
        fetched <= Duration::from_secs(2),
        "fetched after {fetched:?}"
    );
    assert_eq!(web.blobs().blobs().get_bytes(hash).await?, data);
    n0_future::time::timeout(
        Duration::from_secs(4).saturating_sub(lost_at.elapsed()),
        stuck.closed(),
    )
    .await
    .context("the stuck connection stays open")?;
    Ok(())
}

/// J4・J5（T4・T5・S3）: 需要の表は相手 16 件まで。需要の終わった相手が増えても表に残らない。`reset` で両端の
/// session と backend が 0 に戻り、再開の入力が来るまで交渉しない。`resume` は生きた需要の相手とだけ交渉し直す
/// （需要の終わった 40 件とは交渉しない）。reset で閉じた custom path は iroh がすぐ閉じる（#1482）。満杯の表の
/// 相手（交渉に応じない）との交渉は reset の後も期限まで枠を持つので、resume の前に期限の分だけ待つ。
#[tokio::test]
async fn resume_renegotiates_only_the_live_demand() -> Result<()> {
    let (relay, _relay_server) = signaling_fixture::spawn_relay().await?;
    let (web, web_transport) = node(&relay, true).await?;
    let (live, live_transport) = node(&relay, false).await?;
    let signaling = web.webrtc_signaling().context("webrtc")?;
    let _demand = web_e2e::demand(&web, via_relay(&live, &relay)).await?;
    eventually("the live demand opens a session", async || {
        (
            web_transport.stats().sessions,
            live_transport.stats().sessions,
        ) == (1, 1)
            && custom_is_active(&web, &live).await
    })
    .await?;

    let relay_map = iroh::RelayMap::from_iter([relay.clone()]);
    let mut others = Vec::new();
    for _ in 0..40 {
        let other = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
            .relay_mode(iroh::RelayMode::Custom(relay_map.clone()))
            .bind()
            .await?;
        let connection = other
            .connect(via_relay(&web, &relay), iroh_blobs::ALPN)
            .await?;
        others.push((other, connection));
    }
    eventually("the table is full", async || {
        signaling.stats().demand_peers == kukuri_webrtc_transport::MAX_SESSIONS
    })
    .await?;
    for (_, connection) in &others {
        connection.close(0u32.into(), b"done");
    }
    eventually("only the live demand stays in the table", async || {
        signaling.stats().demand_peers == 1
    })
    .await?;

    signaling.reset();
    eventually("reset closes every session and backend", async || {
        (
            web_transport.stats().sessions,
            live_transport.stats().sessions,
            web_transport.stats().running_backends,
        ) == (0, 0, 0)
    })
    .await?;
    eventually("iroh drops the closed custom path", async || {
        !custom_is_active(&web, &live).await
    })
    .await?;
    let attempts = signaling.stats().attempts;
    n0_future::time::sleep(kukuri_webrtc_transport::NEGOTIATION_DEADLINE).await;
    assert_eq!(
        (web_transport.stats().sessions, signaling.stats().attempts),
        (0, attempts)
    );
    signaling.resume();
    eventually("resume renegotiates the live demand", async || {
        (
            web_transport.stats().sessions,
            live_transport.stats().sessions,
        ) == (1, 1)
            && custom_is_active(&web, &live).await
    })
    .await?;
    n0_future::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(signaling.stats().attempts, attempts + 1);
    Ok(())
}

/// #1214 AC-3: node の停止で WebRTC の交渉の世代も終わり、交渉で開いた session が閉じる。
#[tokio::test]
async fn shutting_down_the_node_closes_its_webrtc_sessions() -> Result<()> {
    let (relay, _relay_server) = signaling_fixture::spawn_relay().await?;
    let (web, web_transport) = node(&relay, true).await?;
    let (native, native_transport) = node(&relay, false).await?;
    let _demand = web_e2e::demand(&web, via_relay(&native, &relay)).await?;
    eventually("one session per side", async || {
        (
            web_transport.stats().sessions,
            native_transport.stats().sessions,
        ) == (1, 1)
    })
    .await?;
    web.shutdown().await?;
    assert_eq!(web_transport.stats().sessions, 0);
    native.shutdown().await?;
    Ok(())
}

/// J8: native の node 同士は交渉しない（需要の表に載らず、session を作らない。UDP を維持する）。
#[tokio::test]
async fn native_nodes_do_not_negotiate_with_each_other() -> Result<()> {
    let (relay, _relay_server) = signaling_fixture::spawn_relay().await?;
    let (a, a_transport) = node(&relay, false).await?;
    let (b, b_transport) = node(&relay, false).await?;
    let _demand = web_e2e::demand(&a, b.endpoint().addr()).await?;
    let signaling = a.webrtc_signaling().context("webrtc")?;
    assert_eq!(signaling.stats(), Default::default());
    assert_eq!(
        (a_transport.stats().sessions, b_transport.stats().sessions),
        (0, 0)
    );
    Ok(())
}

/// 監査 B-1: 交渉が始まった後、session を登録する前に需要の接続（1 回の reader 等）が閉じても、開いた session を残さない。
/// 登録の前の待ちを、応答しない STUN（relay の host の 3478 番。応答を 500 ms 待つ）で作る。
#[tokio::test]
async fn a_demand_that_ends_during_the_negotiation_leaves_no_session() -> Result<()> {
    let _silent_stun =
        std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, kukuri_webrtc_transport::STUN_PORT));
    let (relay, _relay_server) = signaling_fixture::spawn_relay().await?;
    let (web, web_transport) = node(&relay, true).await?;
    let (native, native_transport) = node(&relay, false).await?;
    let signaling = web.webrtc_signaling().context("webrtc")?;
    let demand = web_e2e::demand(&web, via_relay(&native, &relay)).await?;
    eventually("the negotiation starts", async || {
        signaling.stats().attempts == 1
    })
    .await?;
    demand.close(0u32.into(), b"done");
    eventually("the demand ends", async || {
        signaling.stats().demand_peers == 0
    })
    .await?;
    eventually("no session is left on either side", async || {
        (
            web_transport.stats().sessions,
            native_transport.stats().sessions,
        ) == (0, 0)
    })
    .await?;
    n0_future::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(
        (
            web_transport.stats().sessions,
            native_transport.stats().sessions
        ),
        (0, 0)
    );
    Ok(())
}
