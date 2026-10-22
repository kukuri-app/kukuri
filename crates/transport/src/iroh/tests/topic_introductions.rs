//! #1910: 近隣に、その topic でのその peer の近隣を紹介してもらい、まだ知らない非近隣の peer を読み先の候補に足す。
use super::*;
use crate::iroh::topic_introductions::request_introductions;

async fn wait_for_neighbor(transport: &IrohGossipTransport, hint_topic: &str) {
    timeout(Duration::from_secs(30), async {
        while !transport
            .peers()
            .await
            .expect("peers")
            .topic_diagnostics
            .iter()
            .any(|diag| diag.topic == hint_topic && diag.peer_count > 0)
        {
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{hint_topic}: neighbor did not become established"));
}

async fn endpoint_addr_of(transport: &IrohGossipTransport) -> EndpointAddr {
    let ticket = transport
        .export_ticket()
        .await
        .expect("ticket")
        .expect("ticket value");
    seed_peer_from_ticket(&ticket)
        .to_endpoint_addr()
        .expect("endpoint addr")
}

/// holder と bridge は gossip の近隣。reader は誰とも合流せず、bridge だけを近隣に置く（R の近隣が非保持者だけの
/// fixture）。bridge の紹介で holder が reader の読み先の候補に入り、到達の手掛かりを持つ。自分と要求先は紹介されない。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_neighbor_introduces_its_topic_peers_as_read_candidates() {
    let holder = IrohGossipTransport::bind_local().await.expect("holder");
    let bridge = IrohGossipTransport::bind_local().await.expect("bridge");
    let reader = IrohGossipTransport::bind_local().await.expect("reader");
    let holder_ticket = holder
        .export_ticket()
        .await
        .expect("ticket")
        .expect("ticket value");
    bridge
        .configure_discovery(
            DiscoveryMode::StaticPeer,
            false,
            vec![seed_peer_from_ticket(&holder_ticket)],
            Vec::new(),
        )
        .await
        .expect("seed the holder");
    let topic = TopicId::new("kukuri:topic:introductions");
    let _holder_hints = holder.subscribe_hints(&topic).await.expect("holder");
    let _bridge_hints = bridge.subscribe_hints(&topic).await.expect("bridge");
    wait_for_neighbor(&bridge, "hint/kukuri:topic:introductions").await;

    let _reader_hints = reader.subscribe_hints(&topic).await.expect("reader");
    assert!(
        reader
            .topic_read_candidates(&topic)
            .await
            .expect("candidates")
            .is_empty()
    );
    let bridge_addr = endpoint_addr_of(&bridge).await;
    reader
        .place_topic_neighbor_for_tests(&topic, bridge_addr.clone())
        .await
        .expect("place the bridge");
    let holder_id = holder.endpoint.id().to_string();
    timeout(Duration::from_secs(30), async {
        while reader.introduced_topic_peers_for_tests(&topic).await != [holder_id.clone()] {
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the bridge introduces the holder");

    let window = reader
        .topic_read_candidates(&topic)
        .await
        .expect("candidates");
    let ids = window
        .iter()
        .map(|peer| peer.endpoint_id.clone())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        ids,
        BTreeSet::from([bridge_addr.id.to_string(), holder_id.clone()])
    );
    let introduced = window
        .iter()
        .find(|peer| peer.endpoint_id == holder_id)
        .expect("the holder is in the window");
    assert!(introduced.addr_hint.is_some(), "{introduced:?}");

    holder.shutdown().await;
    bridge.shutdown().await;
    reader.shutdown().await;
}

/// 答える側は、購読している公開の hint topic の近隣だけを返す。private channel の hint topic と、購読していない
/// topic には答えない（空）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn introductions_cover_only_subscribed_public_topics() {
    let holder = IrohGossipTransport::bind_local().await.expect("holder");
    let bridge = IrohGossipTransport::bind_local().await.expect("bridge");
    let reader = IrohGossipTransport::bind_local().await.expect("reader");
    let holder_ticket = holder
        .export_ticket()
        .await
        .expect("ticket")
        .expect("ticket value");
    bridge
        .configure_discovery(
            DiscoveryMode::StaticPeer,
            false,
            vec![seed_peer_from_ticket(&holder_ticket)],
            Vec::new(),
        )
        .await
        .expect("seed the holder");
    let public = TopicId::new("kukuri:topic:introductions-public");
    let private = TopicId::new("private/introductions-room");
    let _holder_public = holder.subscribe_hints(&public).await.expect("holder");
    let _holder_private = holder.subscribe_hints(&private).await.expect("holder");
    let _bridge_public = bridge.subscribe_hints(&public).await.expect("bridge");
    let _bridge_private = bridge.subscribe_hints(&private).await.expect("bridge");
    wait_for_neighbor(&bridge, "hint/kukuri:topic:introductions-public").await;
    wait_for_neighbor(&bridge, "hint/private/introductions-room").await;

    let bridge_addr = endpoint_addr_of(&bridge).await;
    let ask = |topic: &'static str| {
        let (reader, bridge_addr) = (&reader, bridge_addr.clone());
        async move {
            request_introductions(&reader.endpoint, bridge_addr, topic)
                .await
                .expect("introduction")
                .into_iter()
                .map(|(address, _)| address.id.to_string())
                .collect::<Vec<_>>()
        }
    };
    assert_eq!(
        ask("hint/kukuri:topic:introductions-public").await,
        [holder.endpoint.id().to_string()]
    );
    assert!(ask("hint/private/introductions-room").await.is_empty());
    assert!(ask("hint/kukuri:topic:not-subscribed").await.is_empty());

    holder.shutdown().await;
    bridge.shutdown().await;
    reader.shutdown().await;
}

/// 本番の購読の経路(bootstrap の peer と join し、`joined()` が最初の NeighborUp を消費する)でも、最初の近隣が
/// 成立したときに 1 回だけ紹介を求める(2 人目の近隣を待たない)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_first_neighbor_of_a_bootstrapped_topic_is_asked_once() {
    let bridge = IrohGossipTransport::bind_local().await.expect("bridge");
    let reader = IrohGossipTransport::bind_local().await.expect("reader");
    let bridge_ticket = bridge
        .export_ticket()
        .await
        .expect("ticket")
        .expect("ticket value");
    reader
        .configure_discovery(
            DiscoveryMode::StaticPeer,
            false,
            vec![seed_peer_from_ticket(&bridge_ticket)],
            Vec::new(),
        )
        .await
        .expect("seed the bridge");
    let topic = TopicId::new("kukuri:topic:introductions-bootstrap");
    let _bridge_hints = bridge.subscribe_hints(&topic).await.expect("bridge");
    let _reader_hints = reader.subscribe_hints(&topic).await.expect("reader");
    wait_for_neighbor(&reader, "hint/kukuri:topic:introductions-bootstrap").await;
    timeout(Duration::from_secs(10), async {
        while reader.introduction_request_count_for_tests(&topic).await == 0 {
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the first neighbor is asked for introductions");
    sleep(Duration::from_millis(500)).await;
    assert_eq!(reader.introduction_request_count_for_tests(&topic).await, 1);
    bridge.shutdown().await;
    reader.shutdown().await;
}
