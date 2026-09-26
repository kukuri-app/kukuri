use std::fs;
use std::path::{Path, PathBuf};

use tempfile::tempdir;

use crate::IrohDocsNode;

#[tokio::test]
async fn runtime_reopen_requires_existing_data_without_initializing_a_profile() {
    let dir = tempdir().expect("tempdir");
    let result = IrohDocsNode::reopen_with_discovery_config(
        dir.path(),
        kukuri_transport::TransportNetworkConfig::loopback(),
        kukuri_transport::DhtDiscoveryOptions::disabled(),
        kukuri_transport::TransportRelayConfig::default(),
    )
    .await;
    assert!(result.is_err());
    assert_eq!(fs::read_dir(dir.path()).expect("unchanged root").count(), 0);
}

#[tokio::test]
async fn cancelled_shutdown_caller_still_waits_for_owned_cleanup_on_retry() {
    let dir = tempdir().unwrap();
    let node = IrohDocsNode::persistent(dir.path()).await.unwrap();
    {
        let shutdown = node.clone().shutdown();
        futures_util::pin_mut!(shutdown);
        assert!(futures_util::poll!(shutdown.as_mut()).is_pending());
        // Drop the first caller while the node-owned shutdown continues.
    }
    tokio::time::timeout(std::time::Duration::from_secs(45), node.clone().shutdown())
        .await
        .unwrap()
        .unwrap();
    drop(node);
    let reopened = IrohDocsNode::persistent(dir.path()).await.unwrap();
    reopened.shutdown().await.unwrap();
}

#[tokio::test]
async fn offline_blob_read_does_not_create_a_node_or_missing_store() {
    let dir = tempdir().unwrap();
    assert!(
        crate::read_offline_blob(dir.path(), &"0".repeat(64))
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

fn recovery_dirs(root: &Path) -> Vec<PathBuf> {
    let mut dirs = fs::read_dir(root)
        .expect("read root")
        .filter_map(|entry| {
            let entry = entry.expect("dir entry");
            let path = entry.path();
            let name = path.file_name()?.to_str()?;
            if path.is_dir() && name.starts_with("iroh-docs-recovery-") {
                Some(path)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    dirs.sort();
    dirs
}

#[tokio::test]
async fn persistent_node_recovers_corrupt_docs_store() {
    let dir = tempdir().expect("tempdir");
    let root = dir.path();
    fs::write(root.join("docs.redb"), b"not a redb database").expect("write corrupt docs");
    fs::write(
        root.join("default-author"),
        "535f0f7a2bf4128ddf726b6dbd5d5eae9a7b9e05440344e63b7568b68ef8abd3",
    )
    .expect("write stale author");
    fs::write(
        root.join("endpoint-secret.json"),
        "{\"version\":1,\"secret_key_hex\":\"0101010101010101010101010101010101010101010101010101010101010101\"}",
    )
    .expect("write endpoint secret");

    let node = IrohDocsNode::persistent(root)
        .await
        .expect("node should recover corrupt docs store");
    node.shutdown().await.expect("shutdown");

    let dirs = recovery_dirs(root);
    assert_eq!(dirs.len(), 1);
    assert!(dirs[0].join("docs.redb").exists());
    assert!(dirs[0].join("default-author").exists());
    assert!(root.join("docs.redb").exists());
    assert!(root.join("default-author").exists());
    assert!(root.join("endpoint-secret.json").exists());
}

#[tokio::test]
async fn persistent_node_recovers_stale_default_author_without_docs_store() {
    let dir = tempdir().expect("tempdir");
    let root = dir.path();
    fs::write(
        root.join("default-author"),
        "535f0f7a2bf4128ddf726b6dbd5d5eae9a7b9e05440344e63b7568b68ef8abd3",
    )
    .expect("write stale author");

    let node = IrohDocsNode::persistent(root)
        .await
        .expect("node should recover stale default author");
    node.shutdown().await.expect("shutdown");

    let dirs = recovery_dirs(root);
    assert_eq!(dirs.len(), 1);
    assert!(dirs[0].join("docs.redb").exists());
    assert!(dirs[0].join("default-author").exists());
    assert!(root.join("docs.redb").exists());
    assert!(root.join("default-author").exists());
}

#[tokio::test]
async fn persistent_node_does_not_recover_healthy_store() {
    let dir = tempdir().expect("tempdir");
    let root = dir.path();
    let node = IrohDocsNode::persistent(root)
        .await
        .expect("initial healthy node");
    node.shutdown().await.expect("initial shutdown");

    let restarted = IrohDocsNode::persistent(root)
        .await
        .expect("healthy node restart");
    restarted.shutdown().await.expect("restart shutdown");

    assert!(recovery_dirs(root).is_empty());
    assert!(root.join("docs.redb").exists());
    assert!(root.join("default-author").exists());
}

// ---- endpoint secret の自前形式(WP-C5) ----
// 固定 secret bytes 1..=32 に対する値。ENDPOINT_ID は SecretKey::from_bytes(...).public() の実出力。
const ENDPOINT_SECRET_HEX: &str =
    "0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20";
const ENDPOINT_SECRET_ENDPOINT_ID: &str =
    "79b5562e8fe654f94078b112e8a98ba7901f853ae695bed7e0e3910bad049664";
// 旧形式 = iroh::SecretKey の serde 素通し表現(ed25519_dalek 由来のバイト配列)。
// iroh 1.0 の実出力から生成したリテラル。V1 導入により「読めない」ことを固定する
// (本リリース前の破壊的変更として fallback は置かない — プラン Decision 2)。
const LEGACY_ENDPOINT_SECRET_JSON: &str = "{\"secret_key\":[1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31,32]}";

fn read_endpoint_secret_json(root: &Path) -> serde_json::Value {
    let bytes = fs::read(root.join("endpoint-secret.json")).expect("read endpoint secret file");
    serde_json::from_slice(&bytes).expect("endpoint secret file must be JSON")
}

#[tokio::test]
async fn persistent_node_reads_v1_endpoint_secret_and_keeps_endpoint_id() {
    let dir = tempdir().expect("tempdir");
    let root = dir.path();
    fs::write(
        root.join("endpoint-secret.json"),
        format!("{{\"version\":1,\"secret_key_hex\":\"{ENDPOINT_SECRET_HEX}\"}}"),
    )
    .expect("write v1 endpoint secret");

    let node = IrohDocsNode::persistent(root)
        .await
        .expect("node should read the v1 endpoint secret");
    assert_eq!(
        node.endpoint().id().to_string(),
        ENDPOINT_SECRET_ENDPOINT_ID,
        "endpoint id must be derived from the stored secret"
    );
    node.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn persistent_node_endpoint_id_survives_restart_and_saves_v1() {
    let dir = tempdir().expect("tempdir");
    let root = dir.path();

    let node = IrohDocsNode::persistent(root).await.expect("initial node");
    let endpoint_id = node.endpoint().id().to_string();
    node.shutdown().await.expect("initial shutdown");

    let stored = read_endpoint_secret_json(root);
    assert_eq!(
        stored.get("version").and_then(serde_json::Value::as_u64),
        Some(1),
        "endpoint secret must be persisted in the v1 schema (got: {stored})"
    );
    let secret_hex = stored
        .get("secret_key_hex")
        .and_then(serde_json::Value::as_str)
        .expect("secret_key_hex field");
    assert_eq!(secret_hex.len(), 64, "secret must be 32 bytes hex");

    let restarted = IrohDocsNode::persistent(root).await.expect("restart node");
    assert_eq!(
        restarted.endpoint().id().to_string(),
        endpoint_id,
        "endpoint id must survive a restart"
    );
    restarted.shutdown().await.expect("restart shutdown");
}

#[tokio::test]
async fn persistent_node_rejects_legacy_endpoint_secret() {
    let dir = tempdir().expect("tempdir");
    let root = dir.path();
    fs::write(
        root.join("endpoint-secret.json"),
        LEGACY_ENDPOINT_SECRET_JSON,
    )
    .expect("write legacy endpoint secret");

    let error = match IrohDocsNode::persistent(root).await {
        Ok(_) => panic!("legacy endpoint secret must fail loudly (no silent key regeneration)"),
        Err(error) => error,
    };
    assert!(
        format!("{error:#}").contains("failed to parse endpoint secret"),
        "unexpected error: {error:#}"
    );
}

#[tokio::test]
async fn persistent_node_rejects_corrupted_endpoint_secret() {
    let dir = tempdir().expect("tempdir");
    let root = dir.path();
    fs::write(root.join("endpoint-secret.json"), b"not-json").expect("write corrupted secret");

    let error = match IrohDocsNode::persistent(root).await {
        Ok(_) => panic!("corrupted endpoint secret must fail loudly"),
        Err(error) => error,
    };
    assert!(
        format!("{error:#}").contains("failed to parse endpoint secret"),
        "unexpected error: {error:#}"
    );
}
