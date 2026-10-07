//! Gossip接続の弱参照を、neighborの診断用の経路購読へ渡す（#1595）。
use super::*;
use iroh::endpoint::{
    AfterHandshakeOutcome, Connection, EndpointHooks, PathEventStream, WeakConnectionHandle,
};

/// 接続を延命せず、追加のtaskも起動しない。履歴は最大1024件の弱参照の窓だけを保持する。
#[derive(Clone, Debug)]
pub struct GossipConnectionPaths {
    recent: Arc<std::sync::Mutex<VecDeque<(EndpointId, WeakConnectionHandle)>>>,
    connected: broadcast::Sender<EndpointId>,
}

impl Default for GossipConnectionPaths {
    fn default() -> Self {
        Self {
            recent: Arc::default(),
            connected: broadcast::channel(256).0,
        }
    }
}

impl GossipConnectionPaths {
    pub(crate) fn subscribe(&self) -> broadcast::Receiver<EndpointId> {
        self.connected.subscribe()
    }

    pub(crate) fn watch_peer(
        &self,
        peer: EndpointId,
        paths: &mut tokio_stream::StreamMap<(EndpointId, usize), PathEventStream>,
    ) {
        for connection in self
            .recent
            .lock()
            .expect("gossip connection paths poisoned")
            .iter()
            .filter(|(id, _)| *id == peer)
            .filter_map(|(_, weak)| weak.upgrade())
            .filter(|connection| connection.close_reason().is_none())
        {
            let key = (peer, connection.stable_id());
            if !paths.contains_key(&key) {
                paths.insert(key, connection.path_events());
            }
        }
    }

    pub(crate) fn clear(&self) {
        self.recent
            .lock()
            .expect("gossip connection paths poisoned")
            .clear();
    }

    #[cfg(test)]
    pub(crate) fn watcher_count(&self) -> usize {
        self.connected.receiver_count()
    }
}

impl EndpointHooks for GossipConnectionPaths {
    async fn after_handshake(&self, connection: &Connection) -> AfterHandshakeOutcome {
        if connection.alpn() == GOSSIP_ALPN {
            let peer = connection.remote_id();
            let mut recent = self
                .recent
                .lock()
                .expect("gossip connection paths poisoned");
            recent.retain(|(_, weak)| {
                weak.upgrade()
                    .is_some_and(|connection| connection.close_reason().is_none())
            });
            if recent.len() == 1024 {
                recent.pop_front();
            }
            recent.push_back((peer, connection.weak_handle()));
            let _ = self.connected.send(peer);
        }
        AfterHandshakeOutcome::accept()
    }
}
