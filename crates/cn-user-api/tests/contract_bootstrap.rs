//! bootstrap / topic rendezvous の contract テスト(WP-H4 で contract.rs から分割)。

mod support;

use anyhow::{Context, Result};
use kukuri_cn_core::USER_API_BEARER_CHALLENGE;
use kukuri_core::{
    TopicId, generate_keys, private_topic_rendezvous_key_hex_secret, public_topic_rendezvous_key,
};
use reqwest::{Client, StatusCode};
use sqlx::postgres::PgPool;

use support::*;

// #857: Node 同意の提示は認証前に成立させる必要があるため、公開 policy カタログは
// bearer なしで文書一覧・本文・版を返す。
#[tokio::test]
async fn public_policies_are_served_without_auth() -> Result<()> {
    let Some(admin_database_url) = integration_test_admin_database_url() else {
        eprintln!("skipping cn-user-api integration test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let server = TestServer::spawn(admin_database_url.as_str(), "cn_public_policies").await?;
    let client = Client::new();

    let response = client
        .get(format!("{}/v1/policies", server.base_url))
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let catalog = response
        .json::<kukuri_cn_protocol::CommunityNodePoliciesResponse>()
        .await?;
    assert!(!catalog.policies.is_empty());
    assert!(
        catalog
            .policies
            .iter()
            .all(|policy| !policy.policy_slug.is_empty()
                && policy.policy_version >= 1
                && !policy.title.trim().is_empty()
                && !policy.body_markdown.trim().is_empty()
                && policy.effective_date.as_deref() == Some("2026-09-02")
                && policy.language.as_deref() == Some("ja"))
    );
    assert!(catalog.policies.iter().any(|policy| policy.required));
    assert!(catalog.policies.iter().all(|policy| {
        policy.is_current
            && policy.publication_status.as_deref() == Some("current")
            && policy.published_at.is_some()
            && policy.retired_at.is_none()
    }));
    assert!(catalog.policies.iter().all(|policy| {
        !policy
            .body_markdown
            .contains("You must acknowledge the community node")
            && !policy
                .body_markdown
                .contains("You must follow the community node")
    }));

    // #1192: slug は operator が自由に決めるため、client が文書の役割を判別できるよう
    // operator config の kind を付けて返す。DB には保存しない。
    let kind_of = |slug: &str| {
        catalog
            .policies
            .iter()
            .find(|policy| policy.policy_slug == slug)
            .and_then(|policy| policy.policy_kind.clone())
    };
    assert_eq!(kind_of("terms_of_service").as_deref(), Some("terms"));
    assert_eq!(kind_of("privacy_policy").as_deref(), Some("privacy"));
    assert_eq!(
        kind_of("rights_infringement").as_deref(),
        Some("rights_infringement")
    );
    assert!(
        catalog
            .policies
            .iter()
            .all(|policy| policy.policy_kind.is_some()),
        "every policy from the current operator config carries its kind"
    );

    let current = catalog.policies.first().expect("current policy");
    let snapshot = current
        .policy_snapshot_revision
        .as_deref()
        .expect("snapshot revision");
    let exact = client
        .get(format!(
            "{}/v1/policies/{}/snapshots/{}",
            server.base_url, current.policy_slug, snapshot
        ))
        .send()
        .await?
        .error_for_status()?
        .json::<kukuri_cn_protocol::CommunityNodePolicyDocument>()
        .await?;
    assert_eq!(exact.policy_snapshot_revision.as_deref(), Some(snapshot));
    assert_eq!(exact.publication_status.as_deref(), Some("current"));
    assert_eq!(exact.policy_kind, current.policy_kind);

    Ok(())
}

#[tokio::test]
async fn public_policy_contract_is_operator_and_authoritative_language_independent() -> Result<()> {
    let Some(admin_database_url) = integration_test_admin_database_url() else {
        eprintln!("skipping cn-user-api integration test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let config = kukuri_cn_operator::SAMPLE_CONFIG
        .replace("example-kukuri.net", "community.example.org")
        .replace("Example Operator", "Independent Operator")
        .replace("country: JP", "country: IE")
        .replace("cloud_provider: AWS", "cloud_provider: ExampleCloud")
        .replace("region: ap-northeast-1", "region: eu-west-1")
        .replace("language: ja", "language: en")
        .replace("connection_logs_days: 30", "connection_logs_days: 14");
    let server = TestServer::spawn_with_operator_config(
        admin_database_url.as_str(),
        "cn_public_policies_independent_operator",
        &config,
    )
    .await?;
    let catalog = Client::new()
        .get(format!("{}/v1/policies", server.base_url))
        .send()
        .await?
        .error_for_status()?
        .json::<kukuri_cn_protocol::CommunityNodePoliciesResponse>()
        .await?;
    assert_eq!(catalog.policies.len(), 7);
    assert!(catalog.policies.iter().all(|policy| {
        policy.authoritative_language.as_deref() == Some("en")
            && policy.body_markdown.contains("Independent Operator")
            && policy
                .body_markdown
                .contains("Capability-derived policy facts")
    }));
    server.shutdown().await
}

#[tokio::test]
async fn bootstrap_requires_bearer_then_consents() -> Result<()> {
    let Some(admin_database_url) = integration_test_admin_database_url() else {
        eprintln!("skipping cn-user-api integration test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let server = TestServer::spawn(admin_database_url.as_str(), "cn_user_api_contract").await?;
    let client = Client::new();

    let unauthenticated = client
        .get(format!("{}/v1/bootstrap/nodes", server.base_url))
        .send()
        .await?;
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        unauthenticated
            .headers()
            .get("www-authenticate")
            .and_then(|value| value.to_str().ok()),
        Some(USER_API_BEARER_CHALLENGE)
    );
    let unauthenticated_body = unauthenticated.json::<serde_json::Value>().await?;
    assert_eq!(unauthenticated_body["code"], "AUTH_REQUIRED");

    let keys = generate_keys();
    let (access_token, auth_envelope_json) =
        authenticate(&client, &server.base_url, &keys, "peer-a", None).await?;

    let reused = client
        .post(format!("{}/v1/auth/verify", server.base_url))
        .json(&serde_json::json!({ "auth_envelope_json": auth_envelope_json }))
        .send()
        .await?;
    assert_eq!(reused.status(), StatusCode::UNAUTHORIZED);

    let consent_required = client
        .get(format!("{}/v1/bootstrap/nodes", server.base_url))
        .bearer_auth(access_token.as_str())
        .send()
        .await?;
    assert_eq!(consent_required.status(), StatusCode::FORBIDDEN);
    let consent_required_body = consent_required.json::<serde_json::Value>().await?;
    assert_eq!(consent_required_body["code"], "CONSENT_REQUIRED");

    let consent_status = client
        .get(format!("{}/v1/consents/status", server.base_url))
        .bearer_auth(access_token.as_str())
        .send()
        .await?
        .error_for_status()?
        .json::<kukuri_cn_protocol::CommunityNodeConsentStatus>()
        .await?;
    assert!(!consent_status.all_required_accepted);
    assert!(
        consent_status
            .items
            .iter()
            .all(|item| item.accepted_at.is_none())
    );
    // #384: client が規約本文を表示できるよう body が返ること。
    assert!(
        consent_status
            .items
            .iter()
            .all(|item| !item.body.trim().is_empty())
    );
    assert!(consent_status.items.iter().all(|item| {
        item.effective_date.as_deref() == Some("2026-09-02")
            && item.language.as_deref() == Some("ja")
    }));
    // 初回（過去同意なし）は previously_accepted_version が None であること。
    assert!(
        consent_status
            .items
            .iter()
            .all(|item| item.previously_accepted_version.is_none())
    );

    let accepted =
        accept_required_consents(&client, &server.base_url, access_token.as_str()).await?;
    assert!(accepted.all_required_accepted);
    // required 文書の受諾後は accepted_at と previously_accepted_version が設定されること。
    assert!(
        accepted
            .items
            .iter()
            .filter(|item| item.required)
            .all(|item| item.accepted_at.is_some() && item.previously_accepted_version.is_some())
    );

    let bootstrap = client
        .get(format!("{}/v1/bootstrap/nodes", server.base_url))
        .bearer_auth(access_token.as_str())
        .send()
        .await?
        .error_for_status()?
        .json::<serde_json::Value>()
        .await?;
    assert_eq!(bootstrap["nodes"][0]["base_url"], server.base_url);
    assert_eq!(
        bootstrap["nodes"][0]["resolved_urls"]["connectivity_urls"][0],
        "http://127.0.0.1:13340"
    );
    assert_eq!(
        bootstrap["nodes"][0]["resolved_urls"]["seed_peers"]
            .as_array()
            .map(Vec::len),
        Some(0)
    );

    server.shutdown().await
}

#[tokio::test]
async fn bootstrap_heartbeat_requires_current_consents_before_mutation() -> Result<()> {
    let Some(admin_database_url) = integration_test_admin_database_url() else {
        eprintln!("skipping cn-user-api integration test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let server = TestServer::spawn(
        admin_database_url.as_str(),
        "cn_bootstrap_heartbeat_consent",
    )
    .await?;
    let client = Client::new();
    let pool = PgPool::connect(server.database.database_url.as_str()).await?;
    let keys = generate_keys();
    let pubkey = keys.public_key_hex();
    let (access_token, _) = authenticate(
        &client,
        &server.base_url,
        &keys,
        "peer-consent",
        Some("127.0.0.1:47100"),
    )
    .await?;

    let registration = || {
        sqlx::query_as::<_, (Option<String>, chrono::DateTime<chrono::Utc>)>(
            "SELECT addr_hint, expires_at FROM cn_bootstrap.peer_registrations
             WHERE subscriber_pubkey = $1 AND endpoint_id = 'peer-consent'",
        )
        .bind(pubkey.clone())
        .fetch_optional(&pool)
    };
    let before_no_consent = registration().await?;
    let no_consent = client
        .post(format!("{}/v1/bootstrap/heartbeat", server.base_url))
        .bearer_auth(access_token.as_str())
        .json(&serde_json::json!({
            "endpoint_id": "peer-consent",
            "addr_hint": "127.0.0.1:47101",
        }))
        .send()
        .await?;
    assert_eq!(no_consent.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        no_consent.json::<serde_json::Value>().await?["code"],
        "CONSENT_REQUIRED"
    );
    assert_eq!(registration().await?, before_no_consent, "拒否時はDB不変");

    accept_required_consents(&client, &server.base_url, access_token.as_str()).await?;
    send_bootstrap_heartbeat(
        &client,
        &server.base_url,
        access_token.as_str(),
        "peer-consent",
        Some("127.0.0.1:47102"),
    )
    .await?;
    let current_registration = registration().await?.context("peer registration missing")?;
    assert_eq!(current_registration.0.as_deref(), Some("127.0.0.1:47102"));

    advance_policy_snapshot(&pool, "issue-860-next-snapshot").await?;
    let before_old_snapshot = registration().await?;
    let old_snapshot = client
        .post(format!("{}/v1/bootstrap/heartbeat", server.base_url))
        .bearer_auth(access_token.as_str())
        .json(&serde_json::json!({
            "endpoint_id": "peer-consent",
            "addr_hint": "127.0.0.1:47103",
        }))
        .send()
        .await?;
    assert_eq!(old_snapshot.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        old_snapshot.json::<serde_json::Value>().await?["code"],
        "CONSENT_REQUIRED"
    );
    assert_eq!(
        registration().await?,
        before_old_snapshot,
        "旧snapshot拒否時はDB不変"
    );

    server.shutdown().await
}

#[tokio::test]
async fn bootstrap_exposes_other_registered_seed_peers() -> Result<()> {
    let Some(admin_database_url) = integration_test_admin_database_url() else {
        eprintln!("skipping cn-user-api integration test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let server = TestServer::spawn(admin_database_url.as_str(), "cn_user_api_seed_peers").await?;
    let client = Client::new();

    let keys_a = generate_keys();
    let keys_b = generate_keys();
    let (access_token_a, _) = authenticate(
        &client,
        &server.base_url,
        &keys_a,
        "peer-a",
        Some("127.0.0.1:44001"),
    )
    .await?;
    let (access_token_b, _) = authenticate(
        &client,
        &server.base_url,
        &keys_b,
        "peer-b",
        Some("127.0.0.1:44002"),
    )
    .await?;

    for access_token in [&access_token_a, &access_token_b] {
        let accepted =
            accept_required_consents(&client, &server.base_url, access_token.as_str()).await?;
        assert!(accepted.all_required_accepted);
    }
    send_bootstrap_heartbeat(
        &client,
        &server.base_url,
        access_token_a.as_str(),
        "peer-a",
        Some("127.0.0.1:44001"),
    )
    .await?;
    send_bootstrap_heartbeat(
        &client,
        &server.base_url,
        access_token_b.as_str(),
        "peer-b",
        Some("127.0.0.1:44002"),
    )
    .await?;

    let bootstrap_a = client
        .get(format!("{}/v1/bootstrap/nodes", server.base_url))
        .bearer_auth(access_token_a.as_str())
        .send()
        .await?
        .error_for_status()?
        .json::<serde_json::Value>()
        .await?;
    assert_eq!(
        bootstrap_a["nodes"][0]["resolved_urls"]["seed_peers"][0]["endpoint_id"],
        "peer-b"
    );
    assert_eq!(
        bootstrap_a["nodes"][0]["resolved_urls"]["seed_peers"][0]["addr_hint"],
        "127.0.0.1:44002"
    );

    let bootstrap_b = client
        .get(format!("{}/v1/bootstrap/nodes", server.base_url))
        .bearer_auth(access_token_b.as_str())
        .send()
        .await?
        .error_for_status()?
        .json::<serde_json::Value>()
        .await?;
    assert_eq!(
        bootstrap_b["nodes"][0]["resolved_urls"]["seed_peers"][0]["endpoint_id"],
        "peer-a"
    );
    assert_eq!(
        bootstrap_b["nodes"][0]["resolved_urls"]["seed_peers"][0]["addr_hint"],
        "127.0.0.1:44001"
    );

    server.shutdown().await
}

#[tokio::test]
async fn bootstrap_exposes_other_endpoints_for_same_subscriber() -> Result<()> {
    let Some(admin_database_url) = integration_test_admin_database_url() else {
        eprintln!("skipping cn-user-api integration test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let server =
        TestServer::spawn(admin_database_url.as_str(), "cn_user_api_same_subscriber").await?;
    let client = Client::new();

    let keys = generate_keys();
    let (access_token_a1, _) =
        authenticate(&client, &server.base_url, &keys, "peer-a-1", None).await?;
    let (access_token_a2, _) =
        authenticate(&client, &server.base_url, &keys, "peer-a-2", None).await?;

    let accepted =
        accept_required_consents(&client, &server.base_url, access_token_a1.as_str()).await?;
    assert!(accepted.all_required_accepted);
    send_bootstrap_heartbeat(
        &client,
        &server.base_url,
        access_token_a1.as_str(),
        "peer-a-1",
        None,
    )
    .await?;
    send_bootstrap_heartbeat(
        &client,
        &server.base_url,
        access_token_a2.as_str(),
        "peer-a-2",
        None,
    )
    .await?;

    let bootstrap_a1 = client
        .get(format!("{}/v1/bootstrap/nodes", server.base_url))
        .bearer_auth(access_token_a1.as_str())
        .send()
        .await?
        .error_for_status()?
        .json::<serde_json::Value>()
        .await?;
    let seed_peers_a1 = bootstrap_a1["nodes"][0]["resolved_urls"]["seed_peers"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(seed_peers_a1.len(), 1);
    assert_eq!(seed_peers_a1[0]["endpoint_id"], "peer-a-2");

    let bootstrap_a2 = client
        .get(format!("{}/v1/bootstrap/nodes", server.base_url))
        .bearer_auth(access_token_a2.as_str())
        .send()
        .await?
        .error_for_status()?
        .json::<serde_json::Value>()
        .await?;
    let seed_peers_a2 = bootstrap_a2["nodes"][0]["resolved_urls"]["seed_peers"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(seed_peers_a2.len(), 1);
    assert_eq!(seed_peers_a2[0]["endpoint_id"], "peer-a-1");

    server.shutdown().await
}

#[tokio::test]
async fn bootstrap_filters_expired_peer_registrations_and_heartbeat_restores_them() -> Result<()> {
    let Some(admin_database_url) = integration_test_admin_database_url() else {
        eprintln!("skipping cn-user-api integration test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let server = TestServer::spawn(
        admin_database_url.as_str(),
        "cn_user_api_peer_registration_ttl",
    )
    .await?;
    let client = Client::new();
    let pool = PgPool::connect(server.database.database_url.as_str()).await?;

    let keys_a = generate_keys();
    let keys_b = generate_keys();
    let (token_a_initial, _) = authenticate(
        &client,
        &server.base_url,
        &keys_a,
        "peer-a-1",
        Some("127.0.0.1:45001"),
    )
    .await?;
    let (token_a, _) = authenticate(
        &client,
        &server.base_url,
        &keys_a,
        "peer-a-2",
        Some("127.0.0.1:45002"),
    )
    .await?;
    let (token_b, _) = authenticate(
        &client,
        &server.base_url,
        &keys_b,
        "peer-b",
        Some("127.0.0.1:45003"),
    )
    .await?;

    for access_token in [&token_a, &token_b] {
        let accepted =
            accept_required_consents(&client, &server.base_url, access_token.as_str()).await?;
        assert!(accepted.all_required_accepted);
    }
    send_bootstrap_heartbeat(
        &client,
        &server.base_url,
        token_a_initial.as_str(),
        "peer-a-1",
        Some("127.0.0.1:45001"),
    )
    .await?;
    send_bootstrap_heartbeat(
        &client,
        &server.base_url,
        token_a.as_str(),
        "peer-a-2",
        Some("127.0.0.1:45002"),
    )
    .await?;
    send_bootstrap_heartbeat(
        &client,
        &server.base_url,
        token_b.as_str(),
        "peer-b",
        Some("127.0.0.1:45003"),
    )
    .await?;

    sqlx::query(
        "UPDATE cn_bootstrap.peer_registrations
         SET expires_at = NOW() - INTERVAL '1 second'
         WHERE subscriber_pubkey = $1
           AND endpoint_id = 'peer-a-1'",
    )
    .bind(keys_a.public_key_hex())
    .execute(&pool)
    .await?;

    let bootstrap_before = client
        .get(format!("{}/v1/bootstrap/nodes", server.base_url))
        .bearer_auth(token_b.as_str())
        .send()
        .await?
        .error_for_status()?
        .json::<serde_json::Value>()
        .await?;
    let seed_peers_before = bootstrap_before["nodes"][0]["resolved_urls"]["seed_peers"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(seed_peers_before.len(), 1);
    assert_eq!(seed_peers_before[0]["endpoint_id"], "peer-a-2");

    client
        .post(format!("{}/v1/bootstrap/heartbeat", server.base_url))
        .bearer_auth(token_a_initial.as_str())
        .json(&serde_json::json!({
            "endpoint_id": "peer-a-1",
            "addr_hint": "127.0.0.1:45011",
        }))
        .send()
        .await?
        .error_for_status()?;

    let bootstrap_after = client
        .get(format!("{}/v1/bootstrap/nodes", server.base_url))
        .bearer_auth(token_b.as_str())
        .send()
        .await?
        .error_for_status()?
        .json::<serde_json::Value>()
        .await?;
    let seed_peers_after = bootstrap_after["nodes"][0]["resolved_urls"]["seed_peers"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(seed_peers_after.len(), 2);
    let endpoint_ids = seed_peers_after
        .iter()
        .filter_map(|peer| peer["endpoint_id"].as_str())
        .collect::<Vec<_>>();
    assert!(endpoint_ids.contains(&"peer-a-1"));
    assert!(endpoint_ids.contains(&"peer-a-2"));
    let peer_a1 = seed_peers_after
        .iter()
        .find(|peer| peer["endpoint_id"] == "peer-a-1")
        .context("peer-a-1 restored seed peer missing")?;
    assert_eq!(peer_a1["addr_hint"], "127.0.0.1:45011");

    server.shutdown().await
}

#[tokio::test]
async fn topic_rendezvous_batch_heartbeat_returns_fresh_peer_candidates() -> Result<()> {
    let Some(admin_database_url) = integration_test_admin_database_url() else {
        eprintln!("skipping cn-user-api integration test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let server = TestServer::spawn(admin_database_url.as_str(), "cn_user_api_rendezvous").await?;
    let client = Client::new();

    let keys_a = generate_keys();
    let keys_b = generate_keys();
    let (token_a, _) = authenticate(&client, &server.base_url, &keys_a, "peer-a", None).await?;
    let (token_b, _) = authenticate(
        &client,
        &server.base_url,
        &keys_b,
        "peer-b",
        Some("127.0.0.1:46002"),
    )
    .await?;
    accept_required_consents(&client, &server.base_url, token_a.as_str()).await?;
    accept_required_consents(&client, &server.base_url, token_b.as_str()).await?;

    let raw_topic = TopicId::new("kukuri:topic:rendezvous-public");
    let topic_key = public_topic_rendezvous_key(&raw_topic);

    let first = client
        .post(format!(
            "{}/v1/rendezvous/topics/heartbeat",
            server.base_url
        ))
        .bearer_auth(token_a.as_str())
        .json(&serde_json::json!({
            "endpoint_id": "peer-a",
            "addr_hint": null,
            "joins": [topic_key],
            "refreshes": [],
            "leaves": []
        }))
        .send()
        .await?
        .error_for_status()?
        .json::<serde_json::Value>()
        .await?;
    assert_eq!(
        first["topics"][0]["peers"].as_array().map(Vec::len),
        Some(0)
    );

    let second = client
        .post(format!(
            "{}/v1/rendezvous/topics/heartbeat",
            server.base_url
        ))
        .bearer_auth(token_b.as_str())
        .json(&serde_json::json!({
            "endpoint_id": "peer-b",
            "addr_hint": "127.0.0.1:46002",
            "joins": [topic_key],
            "refreshes": [],
            "leaves": []
        }))
        .send()
        .await?
        .error_for_status()?
        .json::<serde_json::Value>()
        .await?;
    assert_eq!(second["expires_in_seconds"], 45);
    assert_eq!(second["topics"][0]["topic_key"], topic_key);
    assert_eq!(second["topics"][0]["peers"][0]["endpoint_id"], "peer-a");
    assert_eq!(
        second["topics"][0]["peers"][0]["addr_hint"],
        serde_json::Value::Null
    );
    assert_eq!(
        second["topics"][0]["peers"][0]["relay_urls"][0],
        "http://127.0.0.1:13340"
    );

    let refreshed = client
        .post(format!(
            "{}/v1/rendezvous/topics/heartbeat",
            server.base_url
        ))
        .bearer_auth(token_a.as_str())
        .json(&serde_json::json!({
            "endpoint_id": "peer-a",
            "addr_hint": null,
            "joins": [],
            "refreshes": [topic_key],
            "leaves": []
        }))
        .send()
        .await?
        .error_for_status()?
        .json::<serde_json::Value>()
        .await?;
    assert_eq!(refreshed["topics"][0]["peers"][0]["endpoint_id"], "peer-b");
    assert_eq!(
        refreshed["topics"][0]["peers"][0]["addr_hint"],
        "127.0.0.1:46002"
    );

    client
        .post(format!(
            "{}/v1/rendezvous/topics/heartbeat",
            server.base_url
        ))
        .bearer_auth(token_b.as_str())
        .json(&serde_json::json!({
            "endpoint_id": "peer-b",
            "addr_hint": "127.0.0.1:46002",
            "joins": [],
            "refreshes": [],
            "leaves": [topic_key]
        }))
        .send()
        .await?
        .error_for_status()?;

    let after_leave = client
        .post(format!(
            "{}/v1/rendezvous/topics/heartbeat",
            server.base_url
        ))
        .bearer_auth(token_a.as_str())
        .json(&serde_json::json!({
            "endpoint_id": "peer-a",
            "addr_hint": null,
            "joins": [],
            "refreshes": [topic_key],
            "leaves": []
        }))
        .send()
        .await?
        .error_for_status()?
        .json::<serde_json::Value>()
        .await?;
    assert_eq!(
        after_leave["topics"][0]["peers"].as_array().map(Vec::len),
        Some(0)
    );

    server.shutdown().await
}

#[tokio::test]
async fn topic_rendezvous_keys_do_not_expose_raw_topic_ids() -> Result<()> {
    let Some(admin_database_url) = integration_test_admin_database_url() else {
        eprintln!("skipping cn-user-api integration test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let server = TestServer::spawn(
        admin_database_url.as_str(),
        "cn_user_api_rendezvous_privacy",
    )
    .await?;
    let client = Client::new();

    let keys = generate_keys();
    let (token, _) = authenticate(&client, &server.base_url, &keys, "peer-a", None).await?;
    accept_required_consents(&client, &server.base_url, token.as_str()).await?;

    let raw_public_topic = TopicId::new("kukuri:topic:dictionary-visible");
    let raw_private_topic = TopicId::new("kukuri:private:super-secret-channel");
    let public_key = public_topic_rendezvous_key(&raw_public_topic);
    let private_key = private_topic_rendezvous_key_hex_secret(
        "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
        &raw_private_topic,
    )?;

    assert!(!public_key.contains(raw_public_topic.as_str()));
    assert!(!private_key.contains(raw_private_topic.as_str()));
    assert_ne!(public_key, private_key);

    client
        .post(format!(
            "{}/v1/rendezvous/topics/heartbeat",
            server.base_url
        ))
        .bearer_auth(token.as_str())
        .json(&serde_json::json!({
            "endpoint_id": "peer-a",
            "addr_hint": null,
            "joins": [public_key, private_key],
            "refreshes": [],
            "leaves": []
        }))
        .send()
        .await?
        .error_for_status()?;

    let keys = redis_keys(
        server.rendezvous_redis_url.as_str(),
        format!("{}*", server.rendezvous_key_prefix).as_str(),
    )
    .await?;
    assert!(!keys.is_empty());
    let serialized_keys = keys.join("\n");
    assert!(!serialized_keys.contains(raw_public_topic.as_str()));
    assert!(!serialized_keys.contains(raw_private_topic.as_str()));

    server.shutdown().await
}
