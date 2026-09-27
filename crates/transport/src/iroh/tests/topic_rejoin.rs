//! #1221 R2-B: neighbor の無い topic だけを候補の窓で再 join し、CN の rendezvous の peer はその topic だけへ join する。
use super::*;

async fn wait_for_neighbor(transport: &IrohGossipTransport, topic: &str, expected: bool) {
    timeout(Duration::from_secs(30), async {
        while transport
            .peers()
            .await
            .expect("peers")
            .topic_diagnostics
            .iter()
            .any(|diag| diag.topic == topic && !diag.connected_peers.is_empty())
            != expected
        {
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{topic}: neighbor did not become {expected}"));
}

/// ADR 0055 NW-6。neighbor を失った topic だけを再 join し、seed の適用が成功しても回復とはみなさない。
/// 回復は neighbor の成立で判定し、成立した後は再 join しない。neighbor のある別の topic は触らない。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recovery_requires_protocol_observation() {
    let transport_a = IrohGossipTransport::bind_local().await.expect("a");
    let transport_b = IrohGossipTransport::bind_local().await.expect("b");
    let ticket_b = transport_b
        .export_ticket()
        .await
        .expect("ticket b")
        .expect("ticket b value");
    let seeds = vec![seed_peer_from_ticket(&ticket_b)];
    transport_a
        .configure_discovery(DiscoveryMode::StaticPeer, false, seeds.clone(), Vec::new())
        .await
        .expect("seed b");
    let lost = TopicId::new("kukuri:topic:rejoin-lost");
    let kept = TopicId::new("kukuri:topic:rejoin-kept");
    let (lost_hint, kept_hint) = (
        "hint/kukuri:topic:rejoin-lost",
        "hint/kukuri:topic:rejoin-kept",
    );
    let _lost_b = transport_b.subscribe_hints(&lost).await.expect("b lost");
    let _kept_b = transport_b.subscribe_hints(&kept).await.expect("b kept");
    let _lost_a = transport_a.subscribe_hints(&lost).await.expect("a lost");
    let _kept_a = transport_a.subscribe_hints(&kept).await.expect("a kept");
    wait_for_neighbor(&transport_a, lost_hint, true).await;
    wait_for_neighbor(&transport_a, kept_hint, true).await;
    let kept_before = transport_a.topic_join_counts(kept_hint).await;
    let lost_before = transport_a.topic_join_counts(lost_hint).await;

    transport_b
        .unsubscribe_hints(&lost)
        .await
        .expect("b leaves");
    wait_for_neighbor(&transport_a, lost_hint, false).await;
    // seed の適用の成功は回復ではない。neighbor が無い間は再 join を続ける。
    transport_a
        .configure_discovery(DiscoveryMode::StaticPeer, false, seeds, Vec::new())
        .await
        .expect("apply the same seed");
    timeout(Duration::from_secs(30), async {
        while transport_a.topic_join_counts(lost_hint).await.1 < lost_before.1 + 2 {
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the topic without a neighbor rejoins with backoff");
    assert_eq!(
        transport_a.topic_join_counts(kept_hint).await,
        kept_before,
        "a topic that keeps its neighbor is not joined again"
    );

    let _lost_b = transport_b.subscribe_hints(&lost).await.expect("b again");
    wait_for_neighbor(&transport_a, lost_hint, true).await;
    let recovered = transport_a.topic_join_counts(lost_hint).await;
    sleep(Duration::from_secs(3)).await;
    assert_eq!(
        transport_a.topic_join_counts(lost_hint).await,
        recovered,
        "an established neighbor ends the rejoin"
    );
    assert_eq!(transport_a.topic_join_counts(kept_hint).await, kept_before);
    transport_a.shutdown().await;
    transport_b.shutdown().await;
}

/// CN の rendezvous の peer は、その topic だけへ、まだ知らない peer だけを join する。CN の候補を消すと忘れる。
#[tokio::test]
async fn rendezvous_peers_join_only_their_topic_once() {
    let transport = IrohGossipTransport::bind_local().await.expect("transport");
    let one = TopicId::new("kukuri:topic:rendezvous-one");
    let other = TopicId::new("kukuri:topic:rendezvous-other");
    let _one = transport.subscribe_hints(&one).await.expect("one");
    let _other = transport.subscribe_hints(&other).await.expect("other");
    let (one_hint, other_hint) = (
        TopicId::new("hint/kukuri:topic:rendezvous-one"),
        "hint/kukuri:topic:rendezvous-other",
    );
    let peer = SeedPeer {
        endpoint_id: iroh::SecretKey::from_bytes(&[61; 32]).public().to_string(),
        addr_hint: Some("127.0.0.1:9".into()),
    };
    for _ in 0..3 {
        transport
            .join_topic_peers("https://cn-a.example", &one_hint, vec![peer.clone()])
            .await
            .expect("rendezvous peers");
    }
    assert_eq!(transport.topic_join_counts(one_hint.as_str()).await.0, 1);
    assert_eq!(transport.topic_join_counts(other_hint).await.0, 0);
    let rendezvous = |topic: &'static str| {
        let transport = &transport;
        async move {
            transport.topic_states.lock().await[topic]
                .rendezvous
                .lock()
                .await
                .len()
        }
    };
    assert_eq!(rendezvous("hint/kukuri:topic:rendezvous-one").await, 1);
    transport
        .clear_receive_candidates(Some("https://cn-b.example"))
        .await
        .expect("clear another cn");
    assert_eq!(rendezvous("hint/kukuri:topic:rendezvous-one").await, 1);
    transport
        .clear_receive_candidates(Some("https://cn-a.example"))
        .await
        .expect("clear cn a");
    assert_eq!(rendezvous("hint/kukuri:topic:rendezvous-one").await, 0);
    transport.shutdown().await;
}
