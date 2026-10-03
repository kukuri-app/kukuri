//! Web の node（メモリの store と WebRTC の transport）も、作り直せる（#1220 W8 AC-2a）。Web の runtime は Community Node
//! の relay が届いたときに stack を作り直す。WebRTC の transport は 1 回しか bind できないので、開くたびに作る。

use std::sync::Arc;

use kukuri_store::SqliteStore;
use kukuri_transport::{DhtDiscoveryOptions, TransportNetworkConfig, TransportRelayConfig};

use crate::DiscoveryConfig;
use crate::stack::{NodeSource, SharedIrohStack};

#[tokio::test]
async fn a_memory_node_with_webrtc_can_be_rebuilt() {
    let discovery = DiscoveryConfig::static_peer_default();
    let stack = SharedIrohStack::open(
        NodeSource::Memory {
            secret_key: Box::new(iroh::SecretKey::generate()),
            webrtc: true,
        },
        TransportNetworkConfig::loopback(),
        &discovery,
        &[],
        DhtDiscoveryOptions::disabled(),
        TransportRelayConfig::default(),
        Arc::new(SqliteStore::connect_memory().await.expect("store")),
    )
    .await
    .expect("open");
    for _ in 0..2 {
        stack
            .rebuild(&discovery, &[], TransportRelayConfig::default())
            .await
            .expect("rebuild");
    }
}
