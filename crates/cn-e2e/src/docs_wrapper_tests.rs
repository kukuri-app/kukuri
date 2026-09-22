use super::*;

#[tokio::test]
async fn query_fault_wrapper_forwards_replica_close() -> Result<()> {
    let directory = TempDir::new()?;
    let node = IrohDocsNode::persistent_with_discovery_config(
        directory.path(),
        TransportNetworkConfig::loopback(),
        DhtDiscoveryOptions::disabled(),
        TransportRelayConfig::default(),
    )
    .await?;
    let docs = Arc::new(IrohDocsSync::new(node.clone()));
    let wrapper = FaultInjectingDocsSync {
        inner: docs.clone(),
        fail_queries: Arc::new(AtomicBool::new(true)),
    };
    let replica = kukuri_docs_sync::topic_replica_id("wrapper-close");
    wrapper.open_replica(&replica).await?;
    let closed = wrapper.close_replica(&replica).await;
    docs.shutdown().await;
    node.shutdown().await?;
    closed?;
    Ok(())
}
