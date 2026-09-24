use super::*;
use crate::discovery::DiscoveryConfig;
use crate::stack::SharedIrohStack;
use kukuri_iroh_node::IrohDocsNode;
use kukuri_transport::{TransportRelayConfig, fetch_receive_endpoint_binding};
use tokio::time::Instant;

#[tokio::test]
async fn desktop_runtime_installs_binding_for_loaded_account() {
    let _resource = lock_test_resource(TestResource::IdentityStorage).await;
    let dir = tempdir().unwrap();
    let runtime = DesktopRuntime::new_with_config_and_identity(
        dir.path().join("kukuri.db"),
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .unwrap();
    let client = IrohDocsNode::memory().await.unwrap();
    let candidate = {
        let guard = runtime.iroh_stack.current.lock().await;
        guard.as_ref().unwrap().node.endpoint().addr()
    };
    let verified = fetch_receive_endpoint_binding(
        client.endpoint(),
        candidate,
        &runtime.author_keys.public_key(),
        Instant::now() + Duration::from_secs(5),
    )
    .await
    .unwrap();
    assert_eq!(verified.account(), &runtime.author_keys.public_key());
    runtime.shutdown().await;
    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn account_binding_is_live_only_after_identity_load_and_survives_stack_rebuild() {
    let dir = tempdir().unwrap();
    let discovery = DiscoveryConfig::static_peer_default();
    let stack = SharedIrohStack::new(
        &dir.path().join("receive-binding-stack"),
        TransportNetworkConfig::loopback(),
        &discovery,
        &[],
        DhtDiscoveryOptions::disabled(),
        TransportRelayConfig::default(),
        None,
    )
    .await
    .unwrap();
    let client = IrohDocsNode::memory().await.unwrap();
    let keys = Arc::new(KukuriKeys::generate());
    let candidate = {
        let guard = stack.current.lock().await;
        guard.as_ref().unwrap().node.endpoint().addr()
    };
    assert!(
        fetch_receive_endpoint_binding(
            client.endpoint(),
            candidate,
            &keys.public_key(),
            Instant::now() + Duration::from_secs(5),
        )
        .await
        .is_err()
    );

    stack
        .use_account_receive_binding(keys.clone())
        .await
        .unwrap();
    for rebuild in [false, true] {
        if rebuild {
            stack
                .rebuild(&discovery, &[], TransportRelayConfig::default())
                .await
                .unwrap();
        }
        let candidate = {
            let guard = stack.current.lock().await;
            guard.as_ref().unwrap().node.endpoint().addr()
        };
        let verified = fetch_receive_endpoint_binding(
            client.endpoint(),
            candidate.clone(),
            &keys.public_key(),
            Instant::now() + Duration::from_secs(5),
        )
        .await
        .unwrap();
        assert_eq!(verified.account(), &keys.public_key());
        assert_eq!(verified.endpoint_id(), candidate.id.to_string());
        assert!(
            fetch_receive_endpoint_binding(
                client.endpoint(),
                candidate,
                &KukuriKeys::generate().public_key(),
                Instant::now() + Duration::from_secs(5),
            )
            .await
            .is_err()
        );
    }
    stack.shutdown_checked().await.unwrap();
    client.shutdown().await.unwrap();
}
