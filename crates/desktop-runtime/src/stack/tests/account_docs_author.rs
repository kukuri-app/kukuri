use super::*;

// #1258 TR-1: 起動時に設定した docs author は、stack を作り直しても同じで、`ReloadableDocsSync` 越しに
// 照会と「docs author と key の組」の読み出しが内側へ届く。
#[tokio::test]
async fn account_docs_author_survives_a_stack_rebuild() {
    let dir = tempdir().expect("tempdir");
    let discovery_config = DiscoveryConfig::static_peer_default();
    let stack = SharedIrohStack::new(
        &dir.path().join("stack-docs-author"),
        TransportNetworkConfig::loopback(),
        &discovery_config,
        &[],
        DhtDiscoveryOptions::disabled(),
        TransportRelayConfig::default(),
        None,
    )
    .await
    .expect("stack");
    assert_eq!(
        stack.docs_sync.local_docs_author().await.expect("before"),
        None
    );
    let keys = kukuri_core::generate_keys();
    let id = stack
        .use_account_docs_author(keys.derive_docs_author_seed())
        .await
        .expect("use the account docs author");
    let replica = kukuri_docs_sync::topic_replica_id("kukuri:topic:stack-docs-author");
    let write = |key: &'static str| {
        stack.docs_sync.apply_doc_op(
            &replica,
            DocOp::SetBytes {
                key: key.into(),
                value: key.as_bytes().to_vec(),
            },
        )
    };
    write("objects/before/envelope").await.expect("write");

    stack
        .rebuild(&discovery_config, &[], TransportRelayConfig::default())
        .await
        .expect("rebuild");
    assert_eq!(
        stack.docs_sync.local_docs_author().await.expect("after"),
        Some(id.clone()),
        "a rebuilt stack must keep writing as the account docs author"
    );
    write("objects/after/envelope").await.expect("write");
    for key in ["objects/before/envelope", "objects/after/envelope"] {
        let record = stack
            .docs_sync
            .query_replica_by_author(&replica, id.as_str(), key, DocFetchPolicy::LocalOnly)
            .await
            .expect("read by docs author")
            .expect("record");
        assert_eq!(record.docs_author.as_deref(), Some(id.as_str()));
    }
    stack.shutdown_checked().await.expect("shutdown");
}
