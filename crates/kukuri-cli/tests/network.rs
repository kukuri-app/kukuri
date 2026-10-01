use kukuri_cli::{
    dispatcher::{DispatchReply, Dispatcher},
    protocol::{PROTOCOL_VERSION, RequestEnvelope, ResponseEnvelope},
};
use kukuri_desktop_runtime::{
    ClientHost, DesiredSubscription, DesiredSubscriptionScope, DesktopRuntime,
};
use serde_json::{Value, json};
use std::sync::Arc;

async fn call(
    dispatcher: &Dispatcher,
    host: &Arc<ClientHost>,
    command: &str,
    payload: Value,
) -> ResponseEnvelope {
    let request = RequestEnvelope {
        protocol_version: PROTOCOL_VERSION,
        request_id: "network-contract".into(),
        command: command.into(),
        profile: "test".into(),
        payload,
        timeout_ms: None,
        secret_bytes: None,
        accepts_secret_output: false,
    };
    let DispatchReply::Unary(response, secret) =
        dispatcher.dispatch(request, None, "test", Some(host)).await
    else {
        panic!("JSON response")
    };
    assert!(secret.is_none());
    response
}

#[tokio::test]
async fn network_commands_preserve_identity_and_subscription_changes_after_restart() {
    let root = tempfile::tempdir().expect("tempdir");
    let db = root.path().join("kukuri.db");
    let runtime = Arc::new(DesktopRuntime::new(&db).await.expect("runtime"));
    let host = ClientHost::from_runtime(root.path().to_path_buf(), runtime)
        .await
        .expect("host");
    let dispatcher = Dispatcher::builtin();
    let topic = "kukuri:topic:cli-network";
    let disabled = "kukuri:topic:cli-disabled";
    let enabled = "kukuri:topic:cli-enabled";
    host.add_desired_subscription(DesiredSubscription {
        topic: topic.into(),
        scope: DesiredSubscriptionScope::Public,
    })
    .await
    .expect("desired subscription");
    let before = call(&dispatcher, &host, "get_sync_status", json!({})).await;
    assert!(before.ok, "{:?}", before.error);
    let before = before.data.expect("sync status");
    assert!(
        before["subscribed_topics"]
            .as_array()
            .expect("topics")
            .contains(&json!(topic))
    );
    // #1221 R2-D: 通常の状態は件数だけを持ち、peer の一覧は詳細のページで読む。
    assert!(before["configured_peer_count"].is_u64());
    assert!(before.get("configured_peers").is_none());
    for (command, payload) in [
        (
            "list_connectivity_peers",
            json!({"kind": "configured_seed", "limit": 64}),
        ),
        ("get_discovery_config", json!({})),
        ("get_local_peer_ticket", json!({})),
        (
            "set_topic_gossip_enabled",
            json!({"topic": disabled, "enabled": false}),
        ),
        ("unsubscribe_topic", json!({"topic": topic})),
        (
            "set_topic_gossip_enabled",
            json!({"topic": enabled, "enabled": true}),
        ),
    ] {
        let result = call(&dispatcher, &host, command, payload).await;
        assert!(result.ok, "{command}: {:?}", result.error);
    }
    assert_eq!(
        host.desired_subscriptions()
            .await
            .expect("desired subscriptions"),
        vec![DesiredSubscription {
            topic: enabled.into(),
            scope: DesiredSubscriptionScope::Public,
        }]
    );
    dispatcher.finish_operations().await;
    host.shutdown().await;

    let runtime = Arc::new(DesktopRuntime::new(&db).await.expect("restarted runtime"));
    let host = ClientHost::from_runtime(root.path().to_path_buf(), runtime)
        .await
        .expect("restarted host");
    let dispatcher = Dispatcher::builtin();
    let after = call(&dispatcher, &host, "get_sync_status", json!({})).await;
    assert!(after.ok, "{:?}", after.error);
    let after = after.data.expect("sync status");
    assert_eq!(after["local_author_pubkey"], before["local_author_pubkey"]);
    assert!(
        after["subscribed_topics"]
            .as_array()
            .expect("topics")
            .contains(&json!(enabled))
    );
    assert!(
        !after["subscribed_topics"]
            .as_array()
            .expect("topics")
            .contains(&json!(topic))
    );
    assert!(
        after["gossip_disabled_topics"]
            .as_array()
            .expect("disabled topics")
            .contains(&json!(disabled))
    );
    dispatcher.finish_operations().await;
    host.shutdown().await;
}
