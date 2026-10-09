use kukuri_cli::{
    dispatcher::{DispatchReply, Dispatcher},
    protocol::{PROTOCOL_VERSION, RequestEnvelope, ResponseEnvelope, SecretInput},
};
use kukuri_desktop_runtime::{ClientHost, DesktopRuntime};
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

async fn call(
    dispatcher: &Dispatcher,
    host: &Arc<ClientHost>,
    command: &str,
    payload: Value,
    secret: Option<&[u8]>,
) -> ResponseEnvelope {
    let request = RequestEnvelope {
        protocol_version: PROTOCOL_VERSION,
        request_id: "node-contract".into(),
        command: command.into(),
        profile: "test".into(),
        payload,
        timeout_ms: None,
        secret_bytes: secret.map(|bytes| bytes.len() as u64),
        accepts_secret_output: false,
    };
    let DispatchReply::Unary(response, output) = dispatcher
        .dispatch(
            request,
            secret.map(|bytes| SecretInput::new(bytes.to_vec())),
            "test",
            Some(host),
        )
        .await
    else {
        panic!("JSON")
    };
    assert!(output.is_none());
    response
}

#[tokio::test]
async fn node_commands_do_not_create_consent_or_send_private_requests_before_consent() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let base_url = format!("http://{}", listener.local_addr().expect("address"));
    let root = tempfile::tempdir().expect("tempdir");
    let runtime = Arc::new(
        DesktopRuntime::new(root.path().join("kukuri.db"))
            .await
            .expect("runtime"),
    );
    let host = ClientHost::from_runtime(root.path().to_path_buf(), runtime)
        .await
        .expect("host");
    let dispatcher = Dispatcher::builtin();
    let configured = call(
        &dispatcher,
        &host,
        "set_community_node_config",
        json!({"nodes": [{"base_url": base_url}]}),
        None,
    )
    .await;
    assert!(configured.ok, "{:?}", configured.error);
    let sentinel = b"invite-code-secret-sentinel";
    for (command, secret) in [
        ("authenticate_community_node", None),
        ("set_community_node_invite_code", Some(sentinel.as_slice())),
    ] {
        let response = call(
            &dispatcher,
            &host,
            command,
            json!({"base_url": base_url}),
            secret,
        )
        .await;
        assert_eq!(
            response.error_code(),
            Some("consent_required"),
            "{command}: {:?}",
            response.error
        );
        assert!(
            !serde_json::to_string(&response)
                .expect("error JSON")
                .contains("sentinel")
        );
        let status = call(
            &dispatcher,
            &host,
            "get_community_node_statuses",
            json!({}),
            None,
        )
        .await;
        assert!(status.ok, "{:?}", status.error);
        let statuses = status.data.expect("statuses");
        let data = &statuses[0];
        assert!(
            data["local_consent"]["records"]
                .as_array()
                .expect("consents")
                .is_empty()
        );
        assert_eq!(data["auth_state"]["authenticated"], false);
        assert_eq!(data["invite_code_saved"], secret.is_some());
        assert!(!data.to_string().contains("sentinel"));
    }
    for (command, payload) in [
        (
            "search_community_node_index",
            json!({"base_url": base_url, "query": "private-body"}),
        ),
        (
            "submit_community_node_tester_feedback",
            json!({"base_url": base_url, "what_attempted": "private-body", "what_happened": "test", "what_seemed_wrong": "test"}),
        ),
    ] {
        let response = call(&dispatcher, &host, command, payload, None).await;
        assert_eq!(
            response.error_code(),
            Some("consent_required"),
            "{command}: {:?}",
            response.error
        );
    }
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), listener.accept())
            .await
            .is_err(),
        "同意前のHTTP送信は0件"
    );
    let cleared = call(
        &dispatcher,
        &host,
        "set_community_node_invite_code",
        json!({"base_url": base_url}),
        Some(b""),
    )
    .await;
    assert_eq!(cleared.error_code(), Some("consent_required"));
    let status = call(
        &dispatcher,
        &host,
        "get_community_node_statuses",
        json!({}),
        None,
    )
    .await;
    assert!(status.ok, "{:?}", status.error);
    assert_eq!(
        status.data.expect("statuses")[0]["invite_code_saved"],
        false
    );
    dispatcher.finish_operations().await;
    host.shutdown().await;
}

#[path = "support/community_node_mock.rs"]
mod community_node_mock;
use community_node_mock::{MockNode, TOKEN, mock_node};

#[tokio::test]
async fn explicit_node_consent_enables_requests_and_policy_change_blocks_them_again() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let node = Arc::new(MockNode {
        base_url: format!("http://{}", listener.local_addr().expect("address")),
        policy_version: AtomicUsize::new(1),
        reject_index_once: AtomicBool::new(true),
        index_hits: AtomicUsize::new(0),
        verify_hits: AtomicUsize::new(0),
    });
    let router = axum::Router::new()
        .fallback(mock_node)
        .with_state(node.clone());
    let server =
        tokio::spawn(async move { axum::serve(listener, router).await.expect("mock server") });
    let root = tempfile::tempdir().expect("tempdir");
    let runtime = Arc::new(
        DesktopRuntime::new(root.path().join("kukuri.db"))
            .await
            .expect("runtime"),
    );
    let host = ClientHost::from_runtime(root.path().to_path_buf(), runtime)
        .await
        .expect("host");
    let dispatcher = Dispatcher::builtin();
    let base_url = &node.base_url;
    let configured = call(
        &dispatcher,
        &host,
        "set_community_node_config",
        json!({"nodes": [{"base_url": base_url}]}),
        None,
    )
    .await;
    assert!(configured.ok, "{:?}", configured.error);
    let policies = call(
        &dispatcher,
        &host,
        "fetch_community_node_policies",
        json!({"base_url": base_url, "language": "ja"}),
        None,
    )
    .await;
    assert!(policies.ok, "{:?}", policies.error);
    assert_eq!(
        policies.data.expect("policies")["policies"][0]["body_markdown"],
        "テスト文書の本文"
    );
    let manifest = call(
        &dispatcher,
        &host,
        "fetch_community_node_manifest",
        json!({"base_url": base_url}),
        None,
    )
    .await;
    assert!(manifest.ok, "{:?}", manifest.error);
    assert_eq!(manifest.data.expect("manifest")["status"], "absent");
    assert_eq!(node.verify_hits.load(Ordering::SeqCst), 0);
    let accepted = call(&dispatcher, &host, "accept_community_node_consents", json!({"base_url": base_url, "language": "ja", "documents": [{"policy_slug": "builder-preview", "policy_version": 1}]}), None).await;
    assert!(accepted.ok, "{:?}", accepted.error);
    assert!(
        !serde_json::to_string(&accepted)
            .expect("JSON")
            .contains(TOKEN)
    );
    let accepted = accepted.data.expect("status");
    assert_eq!(
        accepted["auth_state"]["authenticated"],
        true,
        "状態: {accepted}, 認証回数: {}",
        node.verify_hits.load(Ordering::SeqCst)
    );
    assert_eq!(
        accepted["local_consent"]["records"][0]["app_version"],
        env!("CARGO_PKG_VERSION")
    );
    let result = call(
        &dispatcher,
        &host,
        "search_community_node_index",
        json!({"base_url": base_url, "query": "test"}),
        None,
    )
    .await;
    assert!(result.ok, "{:?}", result.error);
    assert_eq!(result.data, Some(json!({"entries": []})));
    assert_eq!(
        node.index_hits.load(Ordering::SeqCst),
        2,
        "共有runtimeの401再認証を維持する"
    );
    // #1702: 信頼値の basis の続きは入力の `cursor` を CN へ渡し、次の `basis_next_cursor` を出力する。
    let trust = call(
        &dispatcher,
        &host,
        "read_community_node_trust_user",
        json!({"base_url": base_url, "target_pubkey": "a".repeat(64), "cursor": "1.1760000000000000.signal-50"}),
        None,
    )
    .await;
    assert!(trust.ok, "{:?}", trust.error);
    assert_eq!(
        trust.data.expect("trust")["basis_next_cursor"],
        "1.1760000000000000.signal-50-next"
    );
    node.policy_version.store(2, Ordering::SeqCst);
    let verifies = node.verify_hits.load(Ordering::SeqCst);
    let rejected = call(
        &dispatcher,
        &host,
        "search_community_node_index",
        json!({"base_url": base_url, "query": "private-body"}),
        None,
    )
    .await;
    assert_eq!(rejected.error_code(), Some("consent_required"));
    assert_eq!(
        node.index_hits.load(Ordering::SeqCst),
        2,
        "同意失効後は検索本文を送らない"
    );
    assert_eq!(
        node.verify_hits.load(Ordering::SeqCst),
        verifies,
        "自動的に再同意・認証しない"
    );
    dispatcher.finish_operations().await;
    host.shutdown().await;
    server.abort();
    let _ = server.await;
}
