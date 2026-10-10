//! #616 有効化の関門の contract test。
//!
//! 環境変数（COMMUNITY_NODE_INDEX_QUERY_ENABLED / COMMUNITY_NODE_TRUST_READ_ENABLED）を
//! 真にしただけでは索引・信頼の読み取り面は公開されず、`cn-cli readiness` の全項目合格
//! 記録（cn_admin.readiness_activations）が存在するときにのみ有効化される。
//! 判定項目集合が現行と一致しない古い記録は無効（安全側）。
//!
//! Postgres + Redis を要するため `KUKURI_CN_RUN_INTEGRATION_TESTS=1` で gate する。
//! ArcadeDB へは接続しない（有効時の確認は「404 でない」ことに留める。実際の応答内容は
//! 全構成 E2E が担う）。

use std::net::SocketAddr;

use anyhow::{Context, Result};
use chrono::Utc;
use kukuri_cn_core::{
    JwtConfig, TestDatabase, connect_postgres, readiness_context_fingerprint,
    record_readiness_activation, record_readiness_revocation,
};
use kukuri_cn_operator::READINESS_CHECK_IDS;
use kukuri_cn_user_api::{UserApiConfig, app_router, build_state};
use reqwest::{Client, StatusCode};

mod support;
use support::{integration_test_admin_database_url, integration_test_rendezvous_redis_url};

/// 各 read surface と、有効化が無効なときに返るべき安定コード(#712)。
///
/// コード名は通信契約(クライアントが縮退判別に使う)なので、変更はこの契約試験の
/// 失敗として検知させる。ここでは意図的にリテラルで固定する。
const SURFACES: [(&str, &str); 5] = [
    ("/v1/index/search?q=test", "INDEX_QUERY_NOT_ACTIVATED"),
    ("/v1/indexing/status", "INDEXING_REQUEST_NOT_ACTIVATED"),
    (
        "/v1/trust/users/0000000000000000000000000000000000000000000000000000000000000001",
        "TRUST_READ_NOT_ACTIVATED",
    ),
    (
        "/v1/relation/users/0000000000000000000000000000000000000000000000000000000000000001",
        "TRUST_READ_NOT_ACTIVATED",
    ),
    ("/v1/relation/optout", "RELATION_VISIBILITY_NOT_ACTIVATED"),
];

/// 404 応答の body から安定コードを取り出す。
fn body_code(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|value| value["code"].as_str().map(str::to_string))
        .unwrap_or_default()
}

fn activation_context(revision: &str) -> String {
    readiness_context_fingerprint("public-node", revision, b"")
}

async fn spawn_api(
    database_url: &str,
    prefix: &str,
) -> Result<(String, tokio::task::JoinHandle<()>)> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .context("failed to bind test listener")?;
    let addr = listener.local_addr()?;
    let base_url = format!("http://{addr}");
    let indexer_dir = tempfile::tempdir()?;
    let state = build_state(&UserApiConfig {
        bind_addr: addr,
        database_url: database_url.to_string(),
        rendezvous_redis_url: integration_test_rendezvous_redis_url(),
        rendezvous_key_prefix: format!("cn:test:{prefix}"),
        base_url: base_url.clone(),
        public_base_url: base_url.clone(),
        connectivity_urls: vec!["http://127.0.0.1:13340".to_string()],
        jwt_config: JwtConfig::new("kukuri-cn-tests", "test-secret", 3600),
        operator_config_path: None,
        channel_secret_key: None,
        legal_data_key: None,
        index_query_enabled: true,
        indexer_data_dir: indexer_dir.path().to_path_buf(),
        trust_read_enabled: true,
        relation_distance_optout_min_proximity: Some(0.5),
        deployment_revision: "test-deployment-v1".to_string(),
        readiness_activation_max_age_secs: 3600,
        expected_issuer_node_id: None,
    })
    .await?;
    let app = app_router(state);
    let task = tokio::spawn(async move {
        let _indexer_dir = indexer_dir;
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .expect("test user-api server");
    });
    Ok((base_url, task))
}

async fn surface_statuses(base_url: &str) -> Result<Vec<(String, String, StatusCode, String)>> {
    let client = Client::new();
    let mut statuses = Vec::new();
    for (path, expected_code) in SURFACES {
        let response = client.get(format!("{base_url}{path}")).send().await?;
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        statuses.push((path.to_string(), expected_code.to_string(), status, body));
    }
    Ok(statuses)
}

#[tokio::test]
async fn flags_without_activation_record_keep_surfaces_hidden() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping activation gate test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_activation_none").await?;

    // 記録なし + 環境変数真 → 全 surface が 404 のまま。
    let (base_url, task) = spawn_api(database.database_url.as_str(), "act-none").await?;
    for (path, expected_code, status, body) in surface_statuses(&base_url).await? {
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}: {body}");
        assert_eq!(body_code(&body), expected_code, "{path}: {body}");
    }
    task.abort();
    Ok(())
}

#[tokio::test]
async fn stale_check_id_set_keeps_surfaces_hidden() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping activation gate test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_activation_stale").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    kukuri_cn_core::initialize_database(&pool).await?;

    // 判定項目集合が現行と一致しない記録（判定基準の変更前の合格）は無効。
    record_readiness_activation(
        &pool,
        Utc::now(),
        "public-node",
        &["old_check_only"],
        &activation_context("test-deployment-v1"),
        &serde_json::json!([]),
    )
    .await?;
    let (base_url, task) = spawn_api(database.database_url.as_str(), "act-stale").await?;
    for (path, expected_code, status, body) in surface_statuses(&base_url).await? {
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}: {body}");
        assert_eq!(body_code(&body), expected_code, "{path}: {body}");
    }
    task.abort();
    Ok(())
}

#[tokio::test]
async fn valid_activation_record_enables_surfaces() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping activation gate test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_activation_ok").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    kukuri_cn_core::initialize_database(&pool).await?;

    record_readiness_activation(
        &pool,
        Utc::now(),
        "public-node",
        &READINESS_CHECK_IDS,
        &activation_context("test-deployment-v1"),
        &serde_json::json!([]),
    )
    .await?;
    let (base_url, task) = spawn_api(database.database_url.as_str(), "act-ok").await?;
    // 有効化済みなら 404 ではなくなる（実際の応答内容は認証・ArcadeDB を要するため
    // 全構成 E2E が担う。ここでは「隠されていない」ことだけを固定する）。
    for (path, _expected_code, status, body) in surface_statuses(&base_url).await? {
        assert_ne!(
            status,
            StatusCode::NOT_FOUND,
            "surface が 404 のまま: {path}: {body}"
        );
    }
    task.abort();
    Ok(())
}

#[tokio::test]
async fn activation_record_created_after_startup_enables_surfaces() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping activation gate test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_activation_live").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    kukuri_cn_core::initialize_database(&pool).await?;

    let (base_url, task) = spawn_api(database.database_url.as_str(), "act-live").await?;
    for (path, expected_code, status, body) in surface_statuses(&base_url).await? {
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}: {body}");
        assert_eq!(body_code(&body), expected_code, "{path}: {body}");
    }

    record_readiness_activation(
        &pool,
        Utc::now(),
        "public-node",
        &READINESS_CHECK_IDS,
        &activation_context("test-deployment-v1"),
        &serde_json::json!([]),
    )
    .await?;
    for (path, _expected_code, status, body) in surface_statuses(&base_url).await? {
        assert_ne!(
            status,
            StatusCode::NOT_FOUND,
            "startup後のactivationでsurfaceが開かない: {path}: {body}"
        );
    }
    task.abort();
    Ok(())
}

#[tokio::test]
async fn different_deployment_context_keeps_surfaces_hidden() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping activation gate test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_activation_context").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    kukuri_cn_core::initialize_database(&pool).await?;
    record_readiness_activation(
        &pool,
        Utc::now(),
        "public-node",
        &READINESS_CHECK_IDS,
        &activation_context("different-deployment"),
        &serde_json::json!([]),
    )
    .await?;
    let (base_url, task) = spawn_api(database.database_url.as_str(), "act-context").await?;
    for (path, expected_code, status, body) in surface_statuses(&base_url).await? {
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}: {body}");
        assert_eq!(body_code(&body), expected_code, "{path}: {body}");
    }
    task.abort();
    Ok(())
}

#[tokio::test]
async fn expired_activation_keeps_surfaces_hidden() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping activation gate test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_activation_expired").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    kukuri_cn_core::initialize_database(&pool).await?;
    record_readiness_activation(
        &pool,
        Utc::now() - chrono::Duration::hours(2),
        "public-node",
        &READINESS_CHECK_IDS,
        &activation_context("test-deployment-v1"),
        &serde_json::json!([]),
    )
    .await?;
    let (base_url, task) = spawn_api(database.database_url.as_str(), "act-expired").await?;
    for (path, expected_code, status, body) in surface_statuses(&base_url).await? {
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}: {body}");
        assert_eq!(body_code(&body), expected_code, "{path}: {body}");
    }
    task.abort();
    Ok(())
}

#[tokio::test]
async fn revocation_hides_already_started_surfaces() -> Result<()> {
    let Some(admin_url) = integration_test_admin_database_url() else {
        eprintln!("skipping activation gate test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let database = TestDatabase::create(admin_url.as_str(), "cn_activation_revoked").await?;
    let pool = connect_postgres(database.database_url.as_str()).await?;
    kukuri_cn_core::initialize_database(&pool).await?;
    let context = activation_context("test-deployment-v1");
    record_readiness_activation(
        &pool,
        Utc::now(),
        "public-node",
        &READINESS_CHECK_IDS,
        &context,
        &serde_json::json!([]),
    )
    .await?;
    let (base_url, task) = spawn_api(database.database_url.as_str(), "act-revoked").await?;
    for (path, _expected_code, status, body) in surface_statuses(&base_url).await? {
        assert_ne!(status, StatusCode::NOT_FOUND, "{path}: {body}");
    }

    record_readiness_revocation(&pool, Utc::now(), "public-node", &context, "test_failure").await?;
    for (path, expected_code, status, body) in surface_statuses(&base_url).await? {
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}: {body}");
        assert_eq!(body_code(&body), expected_code, "{path}: {body}");
    }
    task.abort();
    Ok(())
}
