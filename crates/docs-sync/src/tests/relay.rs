use std::time::Duration;

use anyhow::Result;
use futures_util::StreamExt;
use iroh::RelayUrl;
use tempfile::tempdir;
use tokio::time::{sleep, timeout};

use kukuri_cn_iroh_relay::{IrohRelayConfig, SpawnedIrohRelay, spawn_server};
use kukuri_iroh_node::IrohDocsNode;

use crate::{DocOp, DocQuery, DocsSync, IrohDocsSync, stable_key, topic_replica_id};
use kukuri_transport::{
    DhtDiscoveryOptions, SeedPeer, TransportNetworkConfig, TransportRelayConfig,
};

fn relay_seeded_public_replication_timeout() -> Duration {
    if cfg!(target_os = "windows") || std::env::var_os("GITHUB_ACTIONS").is_some() {
        Duration::from_secs(60)
    } else {
        Duration::from_secs(20)
    }
}

fn external_relay_url() -> Option<String> {
    std::env::var("KUKURI_TEST_EXTERNAL_IROH_RELAY_URL")
        .ok()
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())
}

async fn spawn_local_test_relay() -> Result<(String, SpawnedIrohRelay)> {
    let relay = spawn_server(IrohRelayConfig {
        http_bind_addr: "127.0.0.1:0".parse()?,
        tls: None,
        client_rx_limit: None,
    })
    .await?;
    let relay_url = format!("http://{}", relay.http_addr());
    Ok((relay_url, relay))
}

async fn wait_for_relay_seeded_replica_row(
    docs: &IrohDocsSync,
    replica: &str,
    expected_key: &str,
) -> Result<()> {
    let sync_result = timeout(relay_seeded_public_replication_timeout(), async {
        loop {
            let rows = docs
                .query_replica(
                    &topic_replica_id(replica),
                    DocQuery::Prefix("timeline/".into()),
                )
                .await
                .expect("query replica");
            if rows.iter().any(|row| row.key == expected_key) {
                return;
            }
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await;

    if let Err(error) = sync_result {
        anyhow::bail!("relay-seeded public replica sync timeout: {error:?}");
    }
    Ok(())
}

async fn assert_public_replica_syncs_over_relay(relay_url: &str, topic: &str) -> Result<()> {
    let relay_config = TransportRelayConfig {
        iroh_relay_urls: vec![relay_url.to_string()],
    }
    .normalized();
    let dir = tempdir()?;
    let node_a = IrohDocsNode::persistent_with_discovery_config(
        dir.path().join("docs-a"),
        TransportNetworkConfig::default(),
        DhtDiscoveryOptions::disabled(),
        relay_config.clone(),
    )
    .await?;
    let node_b = IrohDocsNode::persistent_with_discovery_config(
        dir.path().join("docs-b"),
        TransportNetworkConfig::default(),
        DhtDiscoveryOptions::disabled(),
        relay_config,
    )
    .await?;
    let docs_a = IrohDocsSync::new(node_a.clone());
    let docs_b = IrohDocsSync::new(node_b.clone());
    let replica = topic_replica_id(topic);
    let key = stable_key("timeline", "0001-external-relay-event");

    docs_a
        .set_seed_peers(vec![SeedPeer {
            endpoint_id: node_b.endpoint().id().to_string(),
            addr_hint: None,
        }])
        .await?;
    docs_b
        .set_seed_peers(vec![SeedPeer {
            endpoint_id: node_a.endpoint().id().to_string(),
            addr_hint: None,
        }])
        .await?;
    docs_a.open_replica(&replica).await?;
    docs_b.open_replica(&replica).await?;
    docs_a
        .apply_doc_op(
            &replica,
            DocOp::SetBytes {
                key: key.clone(),
                value: b"external-relay-doc-entry".repeat(64),
            },
        )
        .await?;

    if let Err(error) = wait_for_relay_seeded_replica_row(&docs_b, topic, key.as_str()).await {
        let diagnostics =
            relay_seeded_timeout_diagnostics(&docs_a, &docs_b, &node_a, &node_b, topic).await;
        docs_a.shutdown().await;
        docs_b.shutdown().await;
        node_a.shutdown().await?;
        node_b.shutdown().await?;
        anyhow::bail!("{error:#}; {diagnostics}");
    }

    docs_a.shutdown().await;
    docs_b.shutdown().await;
    node_a.shutdown().await?;
    node_b.shutdown().await?;
    Ok(())
}

async fn relay_seeded_timeout_diagnostics(
    docs_a: &IrohDocsSync,
    docs_b: &IrohDocsSync,
    node_a: &IrohDocsNode,
    node_b: &IrohDocsNode,
    replica: &str,
) -> String {
    let replica = topic_replica_id(replica);
    let rows_a = docs_a
        .query_replica(&replica, DocQuery::Prefix("timeline/".into()))
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|row| row.key)
        .collect::<Vec<_>>();
    let rows_b = docs_b
        .query_replica(&replica, DocQuery::Prefix("timeline/".into()))
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|row| row.key)
        .collect::<Vec<_>>();
    let remote_info_a = node_a
        .endpoint()
        .remote_info(node_b.endpoint().id())
        .await
        .is_some();
    let remote_info_b = node_b
        .endpoint()
        .remote_info(node_a.endpoint().id())
        .await
        .is_some();
    let seed_peers_a = docs_a.available_sync_peer_ids().await;
    let seed_peers_b = docs_b.available_sync_peer_ids().await;
    format!(
        "rows_a={rows_a:?}; rows_b={rows_b:?}; remote_info_a={remote_info_a}; remote_info_b={remote_info_b}; seed_peers_a={seed_peers_a:?}; seed_peers_b={seed_peers_b:?}"
    )
}

#[tokio::test]
async fn apply_relay_config_tolerates_relay_activation_timeout() -> Result<()> {
    let node = IrohDocsNode::memory().await?;
    let relay_url = "http://127.0.0.1:9".parse::<RelayUrl>()?;

    node.apply_relay_config(TransportRelayConfig {
        iroh_relay_urls: vec![relay_url.to_string()],
    })
    .await?;

    assert_eq!(node.relay_urls().await, vec![relay_url]);

    let docs = IrohDocsSync::new(node.clone());
    docs.shutdown().await;
    node.shutdown().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn public_replica_syncs_over_custom_relay_seed_peers() -> Result<()> {
    let (relay_url, _relay) = spawn_local_test_relay().await?;
    let relay_config = TransportRelayConfig {
        iroh_relay_urls: vec![relay_url],
    }
    .normalized();
    let dir = tempdir()?;
    let node_a = IrohDocsNode::persistent_with_discovery_config(
        dir.path().join("docs-a"),
        TransportNetworkConfig::loopback(),
        DhtDiscoveryOptions::disabled(),
        relay_config.clone(),
    )
    .await?;
    let node_b = IrohDocsNode::persistent_with_discovery_config(
        dir.path().join("docs-b"),
        TransportNetworkConfig::loopback(),
        DhtDiscoveryOptions::disabled(),
        relay_config,
    )
    .await?;
    let docs_a = IrohDocsSync::new(node_a.clone());
    let docs_b = IrohDocsSync::new(node_b.clone());
    let replica = topic_replica_id("kukuri:topic:relay-seeded-docs");

    docs_a
        .set_seed_peers(vec![SeedPeer {
            endpoint_id: node_b.endpoint().id().to_string(),
            addr_hint: None,
        }])
        .await?;
    docs_b
        .set_seed_peers(vec![SeedPeer {
            endpoint_id: node_a.endpoint().id().to_string(),
            addr_hint: None,
        }])
        .await?;
    docs_a.open_replica(&replica).await?;
    docs_b.open_replica(&replica).await?;
    docs_a
        .apply_doc_op(
            &replica,
            DocOp::SetJson {
                key: stable_key("timeline", "0001-relay-event"),
                value: serde_json::json!({
                    "object_id": "relay-event-1",
                    "topic_id": "kukuri:topic:relay-seeded-docs"
                }),
            },
        )
        .await?;
    if let Err(error) = wait_for_relay_seeded_replica_row(
        &docs_b,
        "kukuri:topic:relay-seeded-docs",
        "timeline/0001-relay-event",
    )
    .await
    {
        let diagnostics = relay_seeded_timeout_diagnostics(
            &docs_a,
            &docs_b,
            &node_a,
            &node_b,
            "kukuri:topic:relay-seeded-docs",
        )
        .await;
        anyhow::bail!("{error:#}; {diagnostics}");
    }

    docs_b
        .apply_doc_op(
            &replica,
            DocOp::SetJson {
                key: stable_key("timeline", "0002-relay-event"),
                value: serde_json::json!({
                    "object_id": "relay-event-2",
                    "topic_id": "kukuri:topic:relay-seeded-docs"
                }),
            },
        )
        .await?;
    if let Err(error) = wait_for_relay_seeded_replica_row(
        &docs_a,
        "kukuri:topic:relay-seeded-docs",
        "timeline/0002-relay-event",
    )
    .await
    {
        let diagnostics = relay_seeded_timeout_diagnostics(
            &docs_a,
            &docs_b,
            &node_a,
            &node_b,
            "kukuri:topic:relay-seeded-docs",
        )
        .await;
        anyhow::bail!("{error:#}; {diagnostics}");
    }

    docs_a.shutdown().await;
    docs_b.shutdown().await;
    node_a.shutdown().await?;
    node_b.shutdown().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn public_replica_syncs_over_external_relay_when_configured() -> Result<()> {
    let Some(relay_url) = external_relay_url() else {
        return Ok(());
    };

    assert_public_replica_syncs_over_relay(relay_url.as_str(), "kukuri:topic:external-relay-docs")
        .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn learned_peer_recovers_relay_backfilled_doc_entry_after_seed_peers_are_cleared()
-> Result<()> {
    let (relay_url, _relay) = spawn_local_test_relay().await?;
    let relay_config = TransportRelayConfig {
        iroh_relay_urls: vec![relay_url],
    }
    .normalized();
    let dir = tempdir()?;
    let node_a = IrohDocsNode::persistent_with_discovery_config(
        dir.path().join("docs-learned-a"),
        TransportNetworkConfig::loopback(),
        DhtDiscoveryOptions::disabled(),
        relay_config.clone(),
    )
    .await?;
    let node_b = IrohDocsNode::persistent_with_discovery_config(
        dir.path().join("docs-learned-b"),
        TransportNetworkConfig::loopback(),
        DhtDiscoveryOptions::disabled(),
        relay_config,
    )
    .await?;
    let docs_a = IrohDocsSync::new(node_a.clone());
    let docs_b = IrohDocsSync::new(node_b.clone());
    let replica = topic_replica_id("kukuri:topic:relay-learned-docs");
    let key = stable_key("timeline", "0001-learned-peer");
    let value = b"learned-peer-doc-entry".repeat(128);

    docs_a
        .set_seed_peers(vec![SeedPeer {
            endpoint_id: node_b.endpoint().id().to_string(),
            addr_hint: None,
        }])
        .await?;
    docs_b
        .set_seed_peers(vec![SeedPeer {
            endpoint_id: node_a.endpoint().id().to_string(),
            addr_hint: None,
        }])
        .await?;
    docs_a.open_replica(&replica).await?;
    docs_b.open_replica(&replica).await?;
    let mut events_b = docs_b.subscribe_replica(&replica).await?;

    docs_a
        .apply_doc_op(
            &replica,
            DocOp::SetBytes {
                key: key.clone(),
                value: value.clone(),
            },
        )
        .await?;

    timeout(relay_seeded_public_replication_timeout(), async {
        loop {
            if let Some(Ok(event)) = events_b.next().await
                && event.key == key
                && event.source_peer.is_some()
            {
                return;
            }
        }
    })
    .await?;

    timeout(Duration::from_secs(5), async {
        loop {
            if node_b
                .endpoint()
                .remote_info(node_a.endpoint().id())
                .await
                .is_some()
            {
                return;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await?;

    docs_b
        .learn_peer(&node_a.endpoint().id().to_string())
        .await?;
    docs_b.set_seed_peers(Vec::new()).await?;

    assert_eq!(
        docs_b.available_sync_peer_ids().await,
        vec![node_a.endpoint().id().to_string()]
    );

    let rows = docs_b
        .query_replica(&replica, DocQuery::Exact(key.clone()))
        .await?;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].key, key);
    assert_eq!(rows[0].value, value);

    docs_a.shutdown().await;
    docs_b.shutdown().await;
    node_a.shutdown().await?;
    node_b.shutdown().await?;
    Ok(())
}

// 実際の 2 node の同期で、`SyncFinished`・`ContentReady` が購読側へ届く。通知の数は entry の数に比例しない
// (entry ごとの `ContentReady { hash }` は流さない)。独立監査 PR #1266 の確認 test を恒久化。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sync_boundaries_reach_the_notice_subscriber_and_stay_bounded() -> Result<()> {
    use crate::ReplicaNotice;

    const ENTRIES: usize = 60;
    let (relay_url, _relay) = spawn_local_test_relay().await?;
    let relay_config = TransportRelayConfig {
        iroh_relay_urls: vec![relay_url],
    }
    .normalized();
    let dir = tempdir()?;
    let node_a = IrohDocsNode::persistent_with_discovery_config(
        dir.path().join("notices-a"),
        TransportNetworkConfig::loopback(),
        DhtDiscoveryOptions::disabled(),
        relay_config.clone(),
    )
    .await?;
    let node_b = IrohDocsNode::persistent_with_discovery_config(
        dir.path().join("notices-b"),
        TransportNetworkConfig::loopback(),
        DhtDiscoveryOptions::disabled(),
        relay_config,
    )
    .await?;
    let docs_a = IrohDocsSync::new(node_a.clone());
    let docs_b = IrohDocsSync::new(node_b.clone());
    let replica = topic_replica_id("kukuri:topic:notices-notices");

    // A は同期の前に entry をまとめて書く(B は、1 回の同期でまとめて受け取る)。
    docs_a.open_replica(&replica).await?;
    for index in 0..ENTRIES {
        docs_a
            .apply_doc_op(
                &replica,
                DocOp::SetBytes {
                    key: stable_key("timeline", &format!("{index:04}-entry")),
                    value: format!("notice-entry-{index}").into_bytes().repeat(64),
                },
            )
            .await?;
    }

    let mut notices_b = docs_b.subscribe_replica_notices(&replica).await?;
    docs_a
        .set_seed_peers(vec![SeedPeer {
            endpoint_id: node_b.endpoint().id().to_string(),
            addr_hint: None,
        }])
        .await?;
    docs_b
        .set_seed_peers(vec![SeedPeer {
            endpoint_id: node_a.endpoint().id().to_string(),
            addr_hint: None,
        }])
        .await?;

    let mut remote_entries = 0usize;
    let mut sync_finished = 0usize;
    let mut content_ready = 0usize;
    let mut lagged = 0u64;
    let collected = timeout(relay_seeded_public_replication_timeout(), async {
        while let Some(Ok(notice)) = notices_b.next().await {
            match notice {
                ReplicaNotice::Entry(event) if event.source_peer.is_some() => remote_entries += 1,
                ReplicaNotice::Entry(_) => {}
                ReplicaNotice::SyncFinished => sync_finished += 1,
                ReplicaNotice::ContentReady => content_ready += 1,
                ReplicaNotice::Lagged { missed } => lagged += missed,
            }
            if remote_entries as u64 + lagged >= ENTRIES as u64
                && sync_finished > 0
                && content_ready > 0
            {
                return;
            }
        }
    })
    .await;
    docs_a.shutdown().await;
    docs_b.shutdown().await;
    node_a.shutdown().await?;
    node_b.shutdown().await?;

    collected?;
    assert!(sync_finished > 0, "SyncFinished must reach the subscriber");
    assert!(content_ready > 0, "ContentReady must reach the subscriber");
    // 同期の通知は、entry の数ではなく同期の回数に比例する。
    assert!(
        sync_finished + content_ready <= 16,
        "sync notices must not grow with the entries: finished={sync_finished} ready={content_ready}"
    );
    Ok(())
}
