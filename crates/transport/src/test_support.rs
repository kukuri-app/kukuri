//! crate 内テスト共有ユーティリティ。
//!
//! `fake.rs` のユニットテストと `iroh/tests/`(relay_connectivity 含む)が共用する
//! hint roundtrip 待機と peer snapshot 整形を置く。実装は 1 系統のみで、
//! source peer 検証が不要な呼び出し側は `expected_source_peer: None` を渡す。

use std::time::Duration;

use futures_util::StreamExt;
use kukuri_core::{GossipHint, HintObjectRef, TopicId};
use tokio::time::timeout;

use crate::traits::{HintStream, HintTransport, PeerSnapshot, Transport};

pub(crate) struct HintRoundtripParticipant<'a, T> {
    pub(crate) transport: &'a T,
    pub(crate) stream: &'a mut HintStream,
    pub(crate) expected_source_peer: Option<&'a str>,
}

pub(crate) async fn wait_for_hint_roundtrip<T>(
    participant_a: HintRoundtripParticipant<'_, T>,
    participant_b: HintRoundtripParticipant<'_, T>,
    topic: &TopicId,
    step_timeout: Duration,
    label: &str,
) where
    T: Transport + HintTransport + Sync,
{
    let hint_from_a = GossipHint::TopicObjectsChanged {
        topic_id: topic.clone(),
        objects: vec![HintObjectRef {
            object_id: format!("{label}-from-a"),
            object_kind: "post".into(),
            docs_author: None,
        }],
    };
    let hint_from_b = GossipHint::TopicObjectsChanged {
        topic_id: topic.clone(),
        objects: vec![HintObjectRef {
            object_id: format!("{label}-from-b"),
            object_kind: "post".into(),
            docs_author: None,
        }],
    };
    match timeout(step_timeout, async {
        let mut received_on_a = false;
        let mut received_on_b = false;
        loop {
            if !received_on_a {
                participant_b
                    .transport
                    .publish_hint(topic, hint_from_b.clone())
                    .await
                    .expect("publish hint from b");
            }
            if !received_on_b {
                participant_a
                    .transport
                    .publish_hint(topic, hint_from_a.clone())
                    .await
                    .expect("publish hint from a");
            }
            if !received_on_a
                && let Ok(Some(envelope)) =
                    timeout(Duration::from_millis(500), participant_a.stream.next()).await
            {
                received_on_a = envelope.hint == hint_from_b
                    && participant_b
                        .expected_source_peer
                        .is_none_or(|peer_id| envelope.source_peer == peer_id);
            }
            if !received_on_b
                && let Ok(Some(envelope)) =
                    timeout(Duration::from_millis(500), participant_b.stream.next()).await
            {
                received_on_b = envelope.hint == hint_from_a
                    && participant_a
                        .expected_source_peer
                        .is_none_or(|peer_id| envelope.source_peer == peer_id);
            }
            if received_on_a && received_on_b {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    {
        Ok(()) => {}
        Err(_) => {
            let peers_a = participant_a.transport.peers().await.expect("peers a");
            let peers_b = participant_b.transport.peers().await.expect("peers b");
            panic!(
                "{label} hint roundtrip timeout: a={} b={}",
                format_peer_snapshot(&peers_a),
                format_peer_snapshot(&peers_b)
            );
        }
    }
}

pub(crate) fn format_peer_snapshot(snapshot: &PeerSnapshot) -> String {
    let topics = snapshot
        .topic_diagnostics
        .iter()
        .map(|topic| {
            format!(
                "{}: joined={}, peer_count={}, connected_peers={:?}, missing_peer_ids={:?}, status_detail={}, last_error={:?}",
                topic.topic,
                topic.joined,
                topic.peer_count,
                topic.connected_peers,
                topic.missing_peer_ids,
                topic.status_detail,
                topic.last_error
            )
        })
        .collect::<Vec<_>>();
    format!(
        "connected={}, peer_count={}, connected_peers={:?}, configured_peers={:?}, status_detail={}, last_error={:?}, topics={topics:?}",
        snapshot.connected,
        snapshot.peer_count,
        snapshot.connected_peers,
        snapshot.configured_peers,
        snapshot.status_detail,
        snapshot.last_error
    )
}
