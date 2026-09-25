//! #404 ユーザー向け index query endpoint（search / discovery / recommendation）の contract test。
//!
//! `GET /v1/index/search` / `/v1/index/discovery` / `/v1/index/recommendations` は
//! 認証済み + consent 済み user にのみ、fail-closed query gate を通った `allow` verdict の entry を
//! 返す。機能未構成（既定。`CommunityIndex` = `Availability::Planned`）の node は 404。
//!
//! Postgres + Redis を要するため `KUKURI_CN_RUN_INTEGRATION_TESTS=1` で gate する。
//! query 境界は in-memory 実装（`MemoryIndexProjection` + `MemoryIndexEntryStore`）を
//! `with_index_query` で注入する（gate のセマンティクスは ArcadeDB 実装と共通の
//! `FailClosedIndexQuery`）。

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use kukuri_cn_core::{
    ChannelSecretCipher, IndexEntryStore, IndexScopeKind, JwtConfig, MemoryIndexEntryStore,
    NewIndexEntry, TestDatabase, add_supported_topic, connect_postgres, register_channel_secret,
};
use kukuri_cn_indexer::projection::{IndexProjection, IndexedEntry, MemoryIndexProjection};
use kukuri_cn_indexer::query::FailClosedIndexQuery;
use kukuri_cn_protocol::{CHANNEL_MEMBERSHIP_SECRET_HEADER, build_auth_envelope_json};
use kukuri_cn_safety::provider::SubjectKind;
use kukuri_cn_safety::{
    AdvisorySubjectKind, Basis, ContentAdvisory, ReasonCode, SafetyAction, SafetyCategory,
    SafetyLabel, SafetyVerdict,
};
use kukuri_cn_safety_runtime::{
    MemorySafetyArtifactStore, SafetyArtifactStore, VerdictPersistMeta,
};
use kukuri_cn_trust::{EdgeFeatures, FEATURE_SHARED_TOPICS, MemoryRelationStore, RelationStore};
use kukuri_cn_user_api::{RelationVisibilityState, UserApiConfig, app_router, build_state};
use kukuri_core::{KukuriKeys, generate_keys};
use reqwest::{Client, StatusCode};

mod support;
use support::{
    accept_required_consents, integration_test_admin_database_url,
    integration_test_rendezvous_redis_url,
};

/// テスト用の in-memory query 境界一式（seed 用の書き込みハンドルつき）。
struct MemoryIndex {
    store: Arc<MemorySafetyArtifactStore>,
    entries: Arc<MemoryIndexEntryStore>,
    projection: Arc<MemoryIndexProjection>,
    query: Arc<FailClosedIndexQuery>,
}

fn memory_index() -> MemoryIndex {
    let store = Arc::new(MemorySafetyArtifactStore::new());
    let entries = Arc::new(MemoryIndexEntryStore::new(store.clone()));
    let projection = Arc::new(MemoryIndexProjection::new());
    let query = Arc::new(FailClosedIndexQuery::new(
        projection.clone(),
        entries.clone(),
    ));
    MemoryIndex {
        store,
        entries,
        projection,
        query,
    }
}

fn verdict(action: SafetyAction, critical: bool) -> SafetyVerdict {
    SafetyVerdict {
        action,
        labels: Vec::new(),
        advisory_labels: Vec::new(),
        critical,
        reason_code: if action == SafetyAction::Allow {
            ReasonCode::NoKnownMatch
        } else {
            ReasonCode::CsamConfirmed
        },
        confidence: None,
        provider: Some("mock-known-csam".to_string()),
        provider_capability: None,
        policy_version: "policy-v1-test".to_string(),
        scanned_at: "2026-07-02T09:00:00Z".to_string(),
    }
}

impl MemoryIndex {
    /// allow verdict つきの entry を真実源 + 投影の両方へ seed する（ingest の allow 経路と同型）。
    async fn seed_allow(
        &self,
        scope_id: &str,
        object_id: &str,
        author_pubkey: &str,
        text: &str,
    ) -> Result<()> {
        self.seed_allow_in(
            IndexScopeKind::PublicTopic,
            scope_id,
            object_id,
            author_pubkey,
            text,
        )
        .await
    }

    async fn seed_allow_in(
        &self,
        scope_kind: IndexScopeKind,
        scope_id: &str,
        object_id: &str,
        author_pubkey: &str,
        text: &str,
    ) -> Result<()> {
        let source_replica_id = match scope_kind {
            IndexScopeKind::PublicTopic => format!("topic::{scope_id}"),
            IndexScopeKind::PrivateChannel => format!("channel::{scope_id}::epoch::e1"),
        };
        let verdict_id = self
            .store
            .persist_verdict(
                SubjectKind::Post,
                object_id,
                &verdict(SafetyAction::Allow, false),
                &VerdictPersistMeta::default(),
            )
            .await?;
        self.entries
            .upsert_entry(&NewIndexEntry {
                scope_kind,
                scope_id: scope_id.to_string(),
                object_id: object_id.to_string(),
                author_pubkey: author_pubkey.to_string(),
                created_at: 1_700_000_000,
                source_replica_id: source_replica_id.clone(),
                verdict_id,
                verdict_action: "allow".to_string(),
                critical: false,
            })
            .await?;
        self.projection
            .upsert_entry(&IndexedEntry {
                scope_kind,
                scope_id: scope_id.to_string(),
                object_id: object_id.to_string(),
                author_pubkey: author_pubkey.to_string(),
                text: text.to_string(),
                created_at: 1_700_000_000,
                source_replica_id,
                content_advisories: Vec::new(),
            })
            .await?;
        Ok(())
    }

    /// ラベル付き allow（content advisory 付き）の entry を seed する（ADR 0028 §8.1）。
    async fn seed_labeled_allow(
        &self,
        scope_id: &str,
        object_id: &str,
        author_pubkey: &str,
        text: &str,
        advisories: Vec<ContentAdvisory>,
    ) -> Result<()> {
        let mut labeled = verdict(SafetyAction::Allow, false);
        labeled.reason_code = ReasonCode::GeneralModeration;
        labeled.advisory_labels = advisories
            .iter()
            .filter(|advisory| advisory.subject_kind == AdvisorySubjectKind::PostId)
            .map(|advisory| {
                let mut label = SafetyLabel::new(advisory.category);
                if let Some(confidence) = advisory.confidence {
                    label = label.with_confidence(confidence);
                }
                label
            })
            .collect();
        let verdict_id = self
            .store
            .persist_verdict(
                SubjectKind::Post,
                object_id,
                &labeled,
                &VerdictPersistMeta {
                    advisories,
                    ..VerdictPersistMeta::default()
                },
            )
            .await?;
        self.entries
            .upsert_entry(&NewIndexEntry {
                scope_kind: IndexScopeKind::PublicTopic,
                scope_id: scope_id.to_string(),
                object_id: object_id.to_string(),
                author_pubkey: author_pubkey.to_string(),
                created_at: 1_700_000_000,
                source_replica_id: format!("topic::{scope_id}"),
                verdict_id,
                verdict_action: "allow".to_string(),
                critical: false,
            })
            .await?;
        self.projection
            .upsert_entry(&IndexedEntry {
                scope_kind: IndexScopeKind::PublicTopic,
                scope_id: scope_id.to_string(),
                object_id: object_id.to_string(),
                author_pubkey: author_pubkey.to_string(),
                text: text.to_string(),
                created_at: 1_700_000_000,
                source_replica_id: format!("topic::{scope_id}"),
                content_advisories: Vec::new(),
            })
            .await?;
        Ok(())
    }

    /// index 済み entry の verdict を非 allow / critical に更新する（de-index 前の状態を模す）。
    async fn flip_to_excluded(&self, object_id: &str) -> Result<()> {
        self.store
            .persist_verdict(
                SubjectKind::Post,
                object_id,
                &verdict(SafetyAction::Exclude, true),
                &VerdictPersistMeta::default(),
            )
            .await?;
        Ok(())
    }
}

struct TestServer {
    task: tokio::task::JoinHandle<()>,
    database: TestDatabase,
    base_url: String,
    relation: Arc<MemoryRelationStore>,
}

impl TestServer {
    /// index query（in-memory）を注入した user-api server を起動する。
    /// `index` が None の場合は機能未構成の node（`/v1/index/*` は 404）。
    async fn spawn(
        admin_database_url: &str,
        prefix: &str,
        index: Option<Arc<FailClosedIndexQuery>>,
    ) -> Result<Self> {
        Self::spawn_with_channel_secret_key(admin_database_url, prefix, index, None).await
    }

    /// channel secret の at-rest 暗号鍵つきで起動する(#711 の所属証明検証用)。
    async fn spawn_with_channel_secret_key(
        admin_database_url: &str,
        prefix: &str,
        index: Option<Arc<FailClosedIndexQuery>>,
        channel_secret_key: Option<&str>,
    ) -> Result<Self> {
        let database = TestDatabase::create(admin_database_url, prefix).await?;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .context("failed to bind test index query listener")?;
        let addr = listener.local_addr()?;
        let base_url = format!("http://{addr}");
        let relation = Arc::new(MemoryRelationStore::new());
        let mut state = build_state(&UserApiConfig {
            bind_addr: addr,
            database_url: database.database_url.clone(),
            rendezvous_redis_url: integration_test_rendezvous_redis_url(),
            rendezvous_key_prefix: format!("cn:test:{prefix}"),
            base_url: base_url.clone(),
            public_base_url: base_url.clone(),
            connectivity_urls: vec!["http://127.0.0.1:13340".to_string()],
            jwt_config: JwtConfig::new("kukuri-cn-tests", "test-secret", 3600),
            operator_config_path: None,
            channel_secret_key: channel_secret_key.map(str::to_string),
            legal_data_key: None,
            index_query_enabled: false,
            trust_read_enabled: false,
            relation_distance_optout_min_proximity: None,
            deployment_revision: "test-deployment-v1".to_string(),
            readiness_activation_max_age_secs: 3600,
            expected_issuer_node_id: None,
        })
        .await?;
        if let Some(index) = index {
            state = state
                .with_index_query(index)
                .with_relation_visibility(Arc::new(RelationVisibilityState::new(
                    relation.clone(),
                    0.5,
                )?));
        }
        let app = app_router(state);
        let task = tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .expect("index query server");
        });
        Ok(Self {
            task,
            database,
            base_url,
            relation,
        })
    }

    async fn shutdown(self) -> Result<()> {
        self.task.abort();
        self.database.cleanup().await
    }
}

/// 認証 + consent を通し、bearer access token を返す。
async fn authenticate_and_consent(
    client: &Client,
    base_url: &str,
    keys: &KukuriKeys,
) -> Result<String> {
    let pubkey = keys.public_key_hex();
    let challenge = client
        .post(format!("{base_url}/v1/auth/challenge"))
        .json(&serde_json::json!({ "pubkey": pubkey }))
        .send()
        .await?
        .error_for_status()?
        .json::<kukuri_cn_protocol::AuthChallengeResponse>()
        .await?;
    let auth_envelope_json =
        build_auth_envelope_json(keys, challenge.challenge.as_str(), base_url)?;
    let verify = client
        .post(format!("{base_url}/v1/auth/verify"))
        .json(&serde_json::json!({
            "auth_envelope_json": auth_envelope_json,
            "endpoint_id": "peer-a",
        }))
        .send()
        .await?
        .error_for_status()?
        .json::<kukuri_cn_protocol::AuthVerifyResponse>()
        .await?;
    accept_required_consents(client, base_url, verify.access_token.as_str()).await?;
    Ok(verify.access_token)
}

fn entry_ids(body: &serde_json::Value) -> Vec<String> {
    body["entries"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| entry["object_id"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn index_query_is_not_found_when_not_configured() -> Result<()> {
    // 既定（機能無効 = `CommunityIndex` Planned 相当）の node は index query を公開しない。
    let Some(admin_database_url) = integration_test_admin_database_url() else {
        eprintln!("skipping cn-user-api index query test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let server = TestServer::spawn(admin_database_url.as_str(), "cn_index_query_off", None).await?;
    let client = Client::new();
    for path in [
        "/v1/index/search?q=hello",
        "/v1/index/discovery",
        "/v1/index/recommendations",
    ] {
        let response = client
            .get(format!("{}{path}", server.base_url))
            .send()
            .await?;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        // 安定コードは通信契約(#712)。名前の変更はこの試験で検知する。
        let body: serde_json::Value = response.json().await?;
        assert_eq!(body["code"], "INDEX_QUERY_NOT_CONFIGURED", "{path}");
    }
    server.shutdown().await
}

#[tokio::test]
async fn index_query_requires_auth_and_consent() -> Result<()> {
    let Some(admin_database_url) = integration_test_admin_database_url() else {
        eprintln!("skipping cn-user-api index query test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let index = memory_index();
    let server = TestServer::spawn(
        admin_database_url.as_str(),
        "cn_index_query_auth",
        Some(index.query.clone()),
    )
    .await?;
    let client = Client::new();

    let pool = connect_postgres(&server.database.database_url).await?;
    add_supported_topic(&pool, IndexScopeKind::PublicTopic, "rust").await?;

    let unauthenticated = client
        .get(format!(
            "{}/v1/index/search?scope_kind=public_topic&scope_id=rust&q=hello",
            server.base_url
        ))
        .send()
        .await?;
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);
    let demand: Option<String> = sqlx::query_scalar(
        "SELECT last_index_demand_at::text FROM cn_index.supported_topics WHERE kind='public_topic' AND id='rust'",
    )
    .fetch_one(&pool)
    .await?;
    assert!(
        demand.is_none(),
        "unauthenticated query must not prioritize a scope"
    );

    let token = authenticate_and_consent(&client, &server.base_url, &generate_keys()).await?;
    let authorized = client
        .get(format!(
            "{}/v1/index/discovery?scope_kind=public_topic&scope_id=rust",
            server.base_url
        ))
        .bearer_auth(token)
        .send()
        .await?;
    assert_eq!(authorized.status(), StatusCode::OK);
    let demand: Option<String> = sqlx::query_scalar(
        "SELECT last_index_demand_at::text FROM cn_index.supported_topics WHERE kind='public_topic' AND id='rust'",
    )
    .fetch_one(&pool)
    .await?;
    assert!(
        demand.is_some(),
        "authorized scoped demand must be selected first"
    );

    pool.close().await;
    server.shutdown().await
}

#[tokio::test]
async fn search_discovery_recommendation_return_gated_allow_entries_only() -> Result<()> {
    let Some(admin_database_url) = integration_test_admin_database_url() else {
        eprintln!("skipping cn-user-api index query test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let index = memory_index();
    let author = generate_keys().public_key_hex();
    // allow entry / verdict が後から exclude+critical に変わった entry / 投影残留（真実源なし）。
    index
        .seed_allow("rust", "post-kept", author.as_str(), "tokio async runtime")
        .await?;
    index
        .seed_allow("rust", "post-flipped", author.as_str(), "tokio flips later")
        .await?;
    index.flip_to_excluded("post-flipped").await?;
    index
        .projection
        .upsert_entry(&IndexedEntry {
            scope_kind: IndexScopeKind::PublicTopic,
            scope_id: "rust".to_string(),
            object_id: "post-ghost".to_string(),
            author_pubkey: author.clone(),
            text: "tokio ghost residue".to_string(),
            created_at: 1_700_000_001,
            source_replica_id: "topic::rust".to_string(),
            content_advisories: Vec::new(),
        })
        .await?;

    let server = TestServer::spawn(
        admin_database_url.as_str(),
        "cn_index_query_gate",
        Some(index.query.clone()),
    )
    .await?;
    let client = Client::new();
    let keys = generate_keys();
    let token = authenticate_and_consent(&client, &server.base_url, &keys).await?;

    // topic 内検索: allow のみ（非 allow / critical / 投影残留は出ない）。
    let search = client
        .get(format!(
            "{}/v1/index/search?scope_kind=public_topic&scope_id=rust&q=tokio",
            server.base_url
        ))
        .bearer_auth(token.as_str())
        .send()
        .await?;
    assert_eq!(search.status(), StatusCode::OK);
    let body = search.json::<serde_json::Value>().await?;
    assert_eq!(entry_ids(&body), vec!["post-kept".to_string()]);

    // 横断検索も同じ gate を通る。
    let search_all = client
        .get(format!("{}/v1/index/search?q=tokio", server.base_url))
        .bearer_auth(token.as_str())
        .send()
        .await?;
    let body = search_all.json::<serde_json::Value>().await?;
    assert_eq!(entry_ids(&body), vec!["post-kept".to_string()]);

    // discovery / recommendation（新着列挙）にも非 allow / critical は入らない。
    for path in [
        "/v1/index/discovery?scope_kind=public_topic&scope_id=rust",
        "/v1/index/discovery",
        "/v1/index/recommendations",
    ] {
        let response = client
            .get(format!("{}{path}", server.base_url))
            .bearer_auth(token.as_str())
            .send()
            .await?;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        let body = response.json::<serde_json::Value>().await?;
        assert_eq!(entry_ids(&body), vec!["post-kept".to_string()], "{path}");
    }

    server.shutdown().await
}

#[tokio::test]
async fn distance_optout_filters_posts_on_every_index_surface() -> Result<()> {
    let Some(admin_database_url) = integration_test_admin_database_url() else {
        eprintln!("skipping cn-user-api index query test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let index = memory_index();
    let viewer_keys = generate_keys();
    let far_author_keys = generate_keys();
    let close_author_keys = generate_keys();
    let viewer = viewer_keys.public_key_hex();
    let far_author = far_author_keys.public_key_hex();
    let close_author = close_author_keys.public_key_hex();
    index
        .seed_allow(
            "rust",
            "post-far",
            far_author.as_str(),
            "tokio distance sample",
        )
        .await?;
    index
        .seed_allow(
            "rust",
            "post-close",
            close_author.as_str(),
            "tokio distance sample",
        )
        .await?;

    let server = TestServer::spawn(
        admin_database_url.as_str(),
        "cn_index_query_distance_optout",
        Some(index.query.clone()),
    )
    .await?;
    server
        .relation
        .upsert_edge(
            viewer.as_str(),
            far_author.as_str(),
            &EdgeFeatures::new().with(FEATURE_SHARED_TOPICS, 0.1),
        )
        .await?;
    server
        .relation
        .upsert_edge(
            viewer.as_str(),
            close_author.as_str(),
            &EdgeFeatures::new().with(FEATURE_SHARED_TOPICS, 3.0),
        )
        .await?;
    let client = Client::new();
    let viewer_token =
        authenticate_and_consent(&client, server.base_url.as_str(), &viewer_keys).await?;
    let far_author_token =
        authenticate_and_consent(&client, server.base_url.as_str(), &far_author_keys).await?;
    let paths = [
        "/v1/index/search?q=tokio",
        "/v1/index/discovery",
        "/v1/index/recommendations",
    ];

    // 双方未選択なら、遠距離の投稿も自動では抑制しない。
    for path in paths {
        let body: serde_json::Value = client
            .get(format!("{}{path}", server.base_url))
            .bearer_auth(viewer_token.as_str())
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let mut ids = entry_ids(&body);
        ids.sort();
        assert_eq!(ids, vec!["post-close".to_string(), "post-far".to_string()]);
    }

    // author側の選択で、遠距離author本人の投稿だけを全surfaceから除外する。
    client
        .put(format!("{}/v1/relation/optout", server.base_url))
        .bearer_auth(far_author_token.as_str())
        .send()
        .await?
        .error_for_status()?;
    for path in paths {
        let body: serde_json::Value = client
            .get(format!("{}{path}", server.base_url))
            .bearer_auth(viewer_token.as_str())
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        assert_eq!(entry_ids(&body), vec!["post-close".to_string()], "{path}");
    }

    // 解除後は復帰し、viewer側の選択でも同じfar pairだけが再び抑制される。
    client
        .delete(format!("{}/v1/relation/optout", server.base_url))
        .bearer_auth(far_author_token.as_str())
        .send()
        .await?
        .error_for_status()?;
    client
        .put(format!("{}/v1/relation/optout", server.base_url))
        .bearer_auth(viewer_token.as_str())
        .send()
        .await?
        .error_for_status()?;
    let body: serde_json::Value = client
        .get(format!("{}/v1/index/recommendations", server.base_url))
        .bearer_auth(viewer_token.as_str())
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(entry_ids(&body), vec!["post-close".to_string()]);

    server.shutdown().await
}

#[tokio::test]
async fn index_query_validates_parameters() -> Result<()> {
    let Some(admin_database_url) = integration_test_admin_database_url() else {
        eprintln!("skipping cn-user-api index query test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let index = memory_index();
    let server = TestServer::spawn(
        admin_database_url.as_str(),
        "cn_index_query_params",
        Some(index.query.clone()),
    )
    .await?;
    let client = Client::new();
    let keys = generate_keys();
    let token = authenticate_and_consent(&client, &server.base_url, &keys).await?;

    // q 無しの検索は 400。
    let missing_q = client
        .get(format!("{}/v1/index/search", server.base_url))
        .bearer_auth(token.as_str())
        .send()
        .await?;
    assert_eq!(missing_q.status(), StatusCode::BAD_REQUEST);

    // scope_kind / scope_id は組で指定する（片方のみは 400）。
    let half_scope = client
        .get(format!(
            "{}/v1/index/search?q=hello&scope_kind=public_topic",
            server.base_url
        ))
        .bearer_auth(token.as_str())
        .send()
        .await?;
    assert_eq!(half_scope.status(), StatusCode::BAD_REQUEST);

    // 未知の scope_kind は 400。
    let bad_kind = client
        .get(format!(
            "{}/v1/index/discovery?scope_kind=unknown&scope_id=rust",
            server.base_url
        ))
        .bearer_auth(token.as_str())
        .send()
        .await?;
    assert_eq!(bad_kind.status(), StatusCode::BAD_REQUEST);

    server.shutdown().await
}

#[tokio::test]
async fn private_channel_reads_are_limited_to_members_with_secret_proof() -> Result<()> {
    // #711 / ADR 0025 §6.3: 非公開チャンネルの索引は参加者に閉じる。
    // - 横断読み(search_all / discovery / recommendations)には項目も scope_id も出ない。
    // - 範囲指定読みは channel secret の提示(所属証明)を検証し、未提示・不一致・未登録は
    //   同一の安定コード 403 CHANNEL_MEMBERSHIP_REQUIRED で拒否する(存在の秘匿)。
    let Some(admin_database_url) = integration_test_admin_database_url() else {
        eprintln!("skipping cn-user-api index query test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let index = memory_index();
    let author = generate_keys().public_key_hex();
    index
        .seed_allow("rust", "post-public", author.as_str(), "tokio public post")
        .await?;
    index
        .seed_allow_in(
            IndexScopeKind::PrivateChannel,
            "secret-room",
            "post-private",
            author.as_str(),
            "tokio private post",
        )
        .await?;

    let key_material = "index-membership-test-key-material-0123456789";
    let server = TestServer::spawn_with_channel_secret_key(
        admin_database_url.as_str(),
        "cn_index_query_membership",
        Some(index.query.clone()),
        Some(key_material),
    )
    .await?;
    let namespace_secret = "2".repeat(64);
    {
        // サーバと同じ鍵 material から cipher を組み、所属者の申請済み capability を再現する。
        let pool = connect_postgres(server.database.database_url.as_str()).await?;
        let cipher = ChannelSecretCipher::from_key_material(key_material)?;
        add_supported_topic(&pool, IndexScopeKind::PrivateChannel, "secret-room").await?;
        register_channel_secret(&pool, &cipher, "secret-room", namespace_secret.as_str()).await?;
    }
    let client = Client::new();
    let keys = generate_keys();
    let token = authenticate_and_consent(&client, &server.base_url, &keys).await?;

    // 横断読みには非公開チャンネルの項目も識別子も出ない。
    for path in [
        "/v1/index/search?q=tokio",
        "/v1/index/discovery",
        "/v1/index/recommendations",
    ] {
        let response = client
            .get(format!("{}{path}", server.base_url))
            .bearer_auth(token.as_str())
            .send()
            .await?;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        let body = response.json::<serde_json::Value>().await?;
        assert_eq!(entry_ids(&body), vec!["post-public".to_string()], "{path}");
        assert!(
            !body.to_string().contains("secret-room"),
            "{path}: 非公開チャンネルの識別子を漏らさない"
        );
    }

    // 範囲指定読み: 未提示・不一致・未登録チャンネルは同一コードで拒否する。
    let scoped_search =
        "/v1/index/search?scope_kind=private_channel&scope_id=secret-room&q=tokio".to_string();
    let scoped_discovery =
        "/v1/index/discovery?scope_kind=private_channel&scope_id=secret-room".to_string();
    let unregistered =
        "/v1/index/search?scope_kind=private_channel&scope_id=no-such-room&q=tokio".to_string();
    let wrong_secret = "3".repeat(64);
    for (path, secret) in [
        (scoped_search.as_str(), None),
        (scoped_search.as_str(), Some(wrong_secret.as_str())),
        (scoped_discovery.as_str(), None),
        (unregistered.as_str(), Some(namespace_secret.as_str())),
    ] {
        let mut request = client
            .get(format!("{}{path}", server.base_url))
            .bearer_auth(token.as_str());
        if let Some(secret) = secret {
            request = request.header(CHANNEL_MEMBERSHIP_SECRET_HEADER, secret);
        }
        let response = request.send().await?;
        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "{path} secret={secret:?}"
        );
        let body = response.json::<serde_json::Value>().await?;
        assert_eq!(body["code"], "CHANNEL_MEMBERSHIP_REQUIRED");
    }
    let pool = connect_postgres(server.database.database_url.as_str()).await?;
    let denied_demand: Option<String> = sqlx::query_scalar(
        "SELECT last_index_demand_at::text FROM cn_index.supported_topics
         WHERE kind = 'private_channel' AND id = 'secret-room'",
    )
    .fetch_one(&pool)
    .await?;
    assert!(
        denied_demand.is_none(),
        "denied private reads cannot raise priority"
    );

    // 正しい secret を提示した所属者は従来どおり範囲指定で読める。
    for path in [scoped_search.as_str(), scoped_discovery.as_str()] {
        let response = client
            .get(format!("{}{path}", server.base_url))
            .bearer_auth(token.as_str())
            .header(CHANNEL_MEMBERSHIP_SECRET_HEADER, namespace_secret.as_str())
            .send()
            .await?;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        let body = response.json::<serde_json::Value>().await?;
        assert_eq!(entry_ids(&body), vec!["post-private".to_string()], "{path}");
        assert_eq!(
            body["entries"][0]["source_replica_id"],
            "channel::secret-room::epoch::e1"
        );
    }
    let authorized_demand: Option<String> = sqlx::query_scalar(
        "SELECT last_index_demand_at::text FROM cn_index.supported_topics
         WHERE kind = 'private_channel' AND id = 'secret-room'",
    )
    .fetch_one(&pool)
    .await?;
    assert!(
        authorized_demand.is_some(),
        "authorized private reads raise priority"
    );
    pool.close().await;

    server.shutdown().await
}

/// ADR 0025 §7.2 / ADR 0028 §8.6 contract: `content_advisories_are_separate_from_signed_content_labels`。
///
/// index query の応答は最新 verdict 由来の `content_advisories` を別欄で返し、署名済み
/// `content_labels` を生成・改変しない（応答に `content_labels` は現れない）。advisory の無い
/// entry は空配列。
#[tokio::test]
async fn content_advisories_are_separate_from_signed_content_labels() -> Result<()> {
    let Some(admin_database_url) = integration_test_admin_database_url() else {
        eprintln!("skipping cn-user-api index query test; set KUKURI_CN_RUN_INTEGRATION_TESTS=1");
        return Ok(());
    };
    let index = memory_index();
    let author = generate_keys().public_key_hex();
    index
        .seed_allow("rust", "post-plain", author.as_str(), "tokio plain")
        .await?;
    let advisories = vec![
        ContentAdvisory {
            issuer_node_id: "issuer-node".to_string(),
            subject_kind: AdvisorySubjectKind::PostId,
            subject_id: "post-labeled".to_string(),
            category: SafetyCategory::Nsfw,
            label: "adult".to_string(),
            confidence: Some(84),
            signal_id: "signal-text".to_string(),
            basis: Basis::ClassifierScore,
        },
        ContentAdvisory {
            issuer_node_id: "issuer-node".to_string(),
            subject_kind: AdvisorySubjectKind::BlobCid,
            subject_id: "blob-1".to_string(),
            category: SafetyCategory::Objectionable,
            label: "sensitive".to_string(),
            confidence: Some(80),
            signal_id: "signal-blob".to_string(),
            basis: Basis::ClassifierScore,
        },
    ];
    index
        .seed_labeled_allow(
            "rust",
            "post-labeled",
            author.as_str(),
            "tokio labeled",
            advisories.clone(),
        )
        .await?;

    let server = TestServer::spawn(
        admin_database_url.as_str(),
        "cn_index_query_advisory",
        Some(index.query.clone()),
    )
    .await?;
    let client = Client::new();
    let keys = generate_keys();
    let token = authenticate_and_consent(&client, &server.base_url, &keys).await?;

    for path in [
        "/v1/index/search?scope_kind=public_topic&scope_id=rust&q=tokio",
        "/v1/index/discovery?scope_kind=public_topic&scope_id=rust",
        "/v1/index/recommendations",
    ] {
        let response = client
            .get(format!("{}{path}", server.base_url))
            .bearer_auth(token.as_str())
            .send()
            .await?;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        let body = response.json::<serde_json::Value>().await?;
        let entries = body["entries"].as_array().expect("entries");
        assert_eq!(entries.len(), 2, "{path}");
        for entry in entries {
            assert!(
                entry.get("content_labels").is_none(),
                "署名済み content_labels は index が生成しない: {path}"
            );
            let object_id = entry["object_id"].as_str().unwrap();
            let got: Vec<ContentAdvisory> =
                serde_json::from_value(entry["content_advisories"].clone())?;
            match object_id {
                "post-labeled" => assert_eq!(got, advisories, "{path}"),
                "post-plain" => assert!(got.is_empty(), "{path}"),
                other => panic!("unexpected entry {other}"),
            }
        }
        let labeled = entries
            .iter()
            .find(|entry| entry["object_id"] == "post-labeled")
            .unwrap();
        assert_eq!(labeled["content_advisories"][0]["label"], "adult");
        assert_eq!(
            labeled["content_advisories"][0]["basis"],
            "classifier_score"
        );
        assert_eq!(labeled["content_advisories"][1]["subject_kind"], "blob_cid");
        assert_eq!(labeled["content_advisories"][1]["label"], "sensitive");
    }

    // verdict が exclude へ変われば advisory 付き entry ごと落ちる（INVAR-1）。
    index.flip_to_excluded("post-labeled").await?;
    let body = client
        .get(format!(
            "{}/v1/index/search?scope_kind=public_topic&scope_id=rust&q=tokio",
            server.base_url
        ))
        .bearer_auth(token.as_str())
        .send()
        .await?
        .json::<serde_json::Value>()
        .await?;
    assert_eq!(entry_ids(&body), vec!["post-plain".to_string()]);

    server.shutdown().await
}
