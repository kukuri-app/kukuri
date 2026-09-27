use anyhow::Result;
use iroh::RelayUrl;

use kukuri_iroh_node::IrohDocsNode;

use crate::IrohDocsSync;
use kukuri_transport::TransportRelayConfig;

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
