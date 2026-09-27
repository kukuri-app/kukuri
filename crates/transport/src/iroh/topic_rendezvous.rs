//! #1221 R2-B: CN の rendezvous の peer を topic ごとに覚えて join し、neighbor の無い topic を候補の窓で再 join する。
use super::peer_state::BootstrapCandidates;
use super::*;

/// topic ごとに覚える rendezvous の候補の上限(1 回の join の窓と同じ。#1221 R2-B)。
pub(crate) const MAX_TOPIC_RENDEZVOUS_PEERS: usize = 4;

/// neighbor の無い topic を再 join する間隔。topic ごとに倍へ伸ばし(上限 64 秒)、neighbor の成立で最初へ戻す
/// (#1221 R2-B)。
pub(super) fn topic_rejoin_delay(step: u32) -> Duration {
    Duration::from_secs(1 << step.min(6))
}

/// 再 join の窓: この topic の rendezvous の候補と、seed・ticket の窓から最大 4 件。
pub(super) async fn topic_rejoin_window(
    candidates: &BootstrapCandidates,
    rendezvous: &Mutex<Vec<(String, EndpointAddr)>>,
    step: u32,
) -> Vec<EndpointAddr> {
    let mut peers = rendezvous
        .lock()
        .await
        .iter()
        .map(|(_, peer)| peer.clone())
        .collect::<Vec<_>>();
    for peer in candidates.window().await.unwrap_or_default() {
        if !peers.iter().any(|known| known.id == peer.id) {
            peers.push(peer);
        }
    }
    if !peers.is_empty() {
        let start = step as usize % peers.len();
        peers.rotate_left(start);
    }
    peers.truncate(4);
    peers
}

impl IrohGossipTransport {
    /// CN の rendezvous が返した peer を、その topic だけへ join する(#1221 R2-B)。`topic` は購読している
    /// gossip の topic(`hint/...`)。まだ知らない候補だけを join し、最大 [`MAX_TOPIC_RENDEZVOUS_PEERS`] 件を覚える。
    pub(crate) async fn join_topic_peers_impl(
        &self,
        source: &str,
        topic: &TopicId,
        peers: Vec<SeedPeer>,
    ) -> Result<()> {
        let relay_urls = self
            .relay_urls
            .read()
            .expect("transport relay urls poisoned")
            .clone();
        let mut added = Vec::new();
        if let Some(rendezvous) = self
            .topic_states
            .lock()
            .await
            .get(topic.as_str())
            .map(|state| Arc::clone(&state.rendezvous))
        {
            let mut rendezvous = rendezvous.lock().await;
            for peer in peers.into_iter().take(MAX_TOPIC_RENDEZVOUS_PEERS) {
                let peer = peer.to_endpoint_addr_with_relays(&relay_urls)?;
                if rendezvous.iter().any(|(_, known)| known.id == peer.id) {
                    continue;
                }
                if rendezvous.len() == MAX_TOPIC_RENDEZVOUS_PEERS {
                    rendezvous.remove(0);
                }
                rendezvous.push((source.to_string(), peer.clone()));
                added.push(peer);
            }
        }
        for peer in &added {
            self.remember_hot_endpoint(peer.clone()).await;
        }
        self.extend_topic_peers(Some(topic.as_str()), added, "rendezvous")
            .await;
        Ok(())
    }

    /// 取得元の CN(`None` は全部)の rendezvous の候補を忘れる。
    pub(crate) async fn clear_topic_rendezvous(&self, source: Option<&str>) {
        let states = self
            .topic_states
            .lock()
            .await
            .values()
            .map(|state| Arc::clone(&state.rendezvous))
            .collect::<Vec<_>>();
        for rendezvous in states {
            rendezvous
                .lock()
                .await
                .retain(|(from, _)| source.is_some_and(|source| source != from));
        }
    }

    /// topic(`hint/...`)へ `join_peers` した回数(候補の更新による分、再 join の分)。#1221 R2-B の観測用。
    pub async fn topic_join_counts(&self, topic: &str) -> (u64, u64) {
        self.topic_states
            .lock()
            .await
            .get(topic)
            .map(|state| {
                (
                    state.joins[0].load(Ordering::Relaxed),
                    state.joins[1].load(Ordering::Relaxed),
                )
            })
            .unwrap_or_default()
    }
}
