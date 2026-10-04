//! 信頼評価の観測の提供の状態と送信待ち（#1510）。期待値は SQLite の実装の結果で、store の試験が SQLite、Web の browser
//! 試験が IndexedDB の実装で同じ操作列を確かめる。

use kukuri_core::KukuriEnvelope;

use crate::{TrustObservationNode, TrustObservationStore};

const SHARING: TrustObservationNode = TrustObservationNode {
    enabled: true,
    needs_reconsent: false,
    revocation_pending: false,
};

fn observation(id: &str, created_at: i64) -> KukuriEnvelope {
    super::parity_envelope("kukuri:topic:trust", id, created_at, None, None)
}

async fn queued(store: &dyn TrustObservationStore, base_url: &str) -> Vec<(String, String)> {
    store
        .queued_trust_observations(base_url, usize::MAX)
        .await
        .expect("queued")
        .into_iter()
        .map(|(key, envelope)| (key, envelope.id.0))
        .collect()
}

pub async fn check_trust_observation_store(store: &dyn TrustObservationStore) {
    let pairs = |rows: &[(&str, &str)]| -> Vec<(String, String)> {
        rows.iter()
            .map(|(key, id)| (key.to_string(), id.to_string()))
            .collect()
    };
    assert!(!store.trust_observation_sharing().await.expect("sharing"));
    assert_eq!(store.trust_observation_node("a").await.expect("node"), None);

    // 提供中の a・b、削除要求が未完了の c、再同意を待つ d。
    let revoking = TrustObservationNode {
        enabled: false,
        needs_reconsent: true,
        revocation_pending: true,
    };
    let waiting = TrustObservationNode {
        needs_reconsent: true,
        ..TrustObservationNode::default()
    };
    for (base_url, node) in [
        ("a", SHARING),
        ("b", SHARING),
        ("c", revoking),
        ("d", waiting),
    ] {
        store
            .save_trust_observation_node(base_url, Some(node))
            .await
            .expect("save");
    }
    assert!(store.trust_observation_sharing().await.expect("sharing"));
    assert_eq!(
        store.trust_observation_node("c").await.expect("node"),
        Some(revoking)
    );

    // 提供中の node だけへ積み、同じ key は新しいもので置き換える。
    store
        .queue_trust_observation(None, "x|mute", &observation("x1", 10))
        .await
        .expect("queue");
    store
        .queue_trust_observation(Some("a"), "w|block", &observation("w1", 20))
        .await
        .expect("queue");
    store
        .queue_trust_observation(Some("c"), "w|block", &observation("w1", 20))
        .await
        .expect("queue");
    store
        .queue_trust_observation(Some("a"), "x|mute", &observation("x2", 30))
        .await
        .expect("queue");
    assert_eq!(
        queued(store, "a").await,
        pairs(&[("w|block", "w1"), ("x|mute", "x2")])
    );
    assert_eq!(queued(store, "b").await, pairs(&[("x|mute", "x1")]));
    assert!(queued(store, "c").await.is_empty() && queued(store, "d").await.is_empty());
    assert_eq!(
        store
            .queued_trust_observations("a", 1)
            .await
            .expect("queued")
            .len(),
        1
    );
    assert_eq!(
        store
            .count_queued_trust_observations("a")
            .await
            .expect("count"),
        2
    );
    assert_eq!(
        store
            .latest_queued_trust_observation_at("x|mute")
            .await
            .expect("latest"),
        Some(30)
    );

    // 送信中に置き換わった key は残す。
    store
        .dequeue_trust_observation("a", "x|mute", "x1")
        .await
        .expect("dequeue");
    store
        .dequeue_trust_observation("a", "w|block", "w1")
        .await
        .expect("dequeue");
    assert_eq!(queued(store, "a").await, pairs(&[("x|mute", "x2")]));

    // 提供をやめた node と忘れた node の送信待ちは消える。
    store
        .save_trust_observation_node("b", Some(waiting))
        .await
        .expect("save");
    store
        .save_trust_observation_node("a", None)
        .await
        .expect("forget");
    assert_eq!(store.trust_observation_node("a").await.expect("node"), None);
    assert!(queued(store, "a").await.is_empty() && queued(store, "b").await.is_empty());
    assert_eq!(
        store
            .latest_queued_trust_observation_at("x|mute")
            .await
            .expect("latest"),
        None
    );
    assert!(!store.trust_observation_sharing().await.expect("sharing"));
}
