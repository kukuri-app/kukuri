//! #1061 ブロック / ミュート観測の CN 提供（ADR 0026 §8.3 / §8.5、ADR 0022 追補）。
//!
//! mock CN の受信記録で、同意が無い・削除要求が未完了・session が準備できていない間は
//! 観測の HTTP を 1 件も送らないこと、送信待ちの集約と restart 後の再開、無効化・同意取消・
//! CN 削除での削除要求を確認する。

use super::super::*;
use axum::response::{IntoResponse, Response};
use kukuri_cn_protocol::{
    ApiErrorBody, CommunityNodeConsentStatus, CommunityNodePoliciesResponse,
    CommunityNodePolicyDocument, TRUST_OBSERVATION_SHARING_POLICY_SLUG,
    TrustObservationsRevokeResponse, TrustObservationsSubmitResponse,
};
use kukuri_core::{
    KukuriEnvelope, TrustObservation, TrustObservationKind, parse_trust_observation,
};

use crate::{
    AcceptCommunityNodeConsentsRequest, CommunityNodeConsentDocumentRef,
    EnableCommunityNodeObservationSharingRequest,
};

const SHARING_POLICY_VERSION: i32 = 3;

#[derive(Clone)]
struct MockObservationState {
    managed: Arc<MockManagedCommunityNodeState>,
    /// POST /v1/trust/observations の受信（認証を通ったもの）。
    submissions: Arc<Mutex<Vec<Vec<TrustObservation>>>>,
    /// DELETE /v1/trust/observations の受信（認証を通ったもの）。
    revocations: Arc<AtomicUsize>,
    /// 同意受諾で指定された slug。
    accepted_slugs: Arc<Mutex<Vec<Vec<String>>>>,
    submit_error: Arc<Mutex<Option<(StatusCode, String)>>>,
    revoke_error: Arc<Mutex<Option<StatusCode>>>,
    offer_sharing: Arc<AtomicBool>,
}

fn policy(slug: &str, version: i32, required: bool) -> CommunityNodePolicyDocument {
    CommunityNodePolicyDocument {
        policy_kind: None,
        policy_slug: slug.to_string(),
        policy_version: version,
        title: slug.to_string(),
        body_markdown: format!("{slug} body"),
        required,
        effective_date: Some("2026-09-18".to_string()),
        language: Some("ja".to_string()),
        policy_snapshot_revision: None,
        authoritative_language: Some("ja".to_string()),
        reference_translation: false,
        translation_revision: None,
        translation_of_version: None,
        fallback: false,
        requested_language: None,
        material_change: false,
        requires_reconsent: false,
        is_current: true,
        publication_status: Some("current".to_string()),
        published_at: None,
        retired_at: None,
        previous_policy_version: None,
        previous_policy_snapshot_revision: None,
        next_policy_version: None,
        next_policy_snapshot_revision: None,
    }
}

async fn mock_policies(
    State(state): State<MockObservationState>,
) -> Json<CommunityNodePoliciesResponse> {
    let mut policies = vec![policy(MOCK_MANAGED_POLICY_SLUG, 1, true)];
    if state.offer_sharing.load(Ordering::SeqCst) {
        policies.push(policy(
            TRUST_OBSERVATION_SHARING_POLICY_SLUG,
            SHARING_POLICY_VERSION,
            false,
        ));
    }
    Json(CommunityNodePoliciesResponse {
        policies,
        policy_snapshot_revision: None,
    })
}

async fn mock_accept_consents(
    State(state): State<MockObservationState>,
    headers: HeaderMap,
    Json(request): Json<serde_json::Value>,
) -> std::result::Result<Json<CommunityNodeConsentStatus>, StatusCode> {
    authorize_managed_community_node_request(&headers, state.managed.as_ref()).await?;
    let slugs = request["policy_slugs"]
        .as_array()
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    state.accepted_slugs.lock().await.push(slugs);
    state.managed.consent_accepted.store(true, Ordering::SeqCst);
    Ok(Json(managed_community_node_consent_status(true)))
}

fn api_error(status: StatusCode, code: &str) -> Response {
    (
        status,
        Json(ApiErrorBody {
            code: code.to_string(),
            message: code.to_string(),
        }),
    )
        .into_response()
}

async fn mock_submit(
    State(state): State<MockObservationState>,
    headers: HeaderMap,
    Json(request): Json<serde_json::Value>,
) -> Response {
    if authorize_managed_community_node_request(&headers, state.managed.as_ref())
        .await
        .is_err()
    {
        return api_error(StatusCode::UNAUTHORIZED, "AUTH_REQUIRED");
    }
    if let Some((status, code)) = state.submit_error.lock().await.clone() {
        return api_error(status, code.as_str());
    }
    let envelopes: Vec<KukuriEnvelope> =
        serde_json::from_value(request["envelopes"].clone()).expect("envelopes");
    let observations: Vec<TrustObservation> = envelopes
        .iter()
        .map(|envelope| {
            parse_trust_observation(envelope)
                .expect("valid observation")
                .expect("observation kind")
        })
        .collect();
    let stored = u32::try_from(observations.len()).expect("count");
    state.submissions.lock().await.push(observations);
    Json(TrustObservationsSubmitResponse { stored, ignored: 0 }).into_response()
}

async fn mock_revoke(State(state): State<MockObservationState>, headers: HeaderMap) -> Response {
    if authorize_managed_community_node_request(&headers, state.managed.as_ref())
        .await
        .is_err()
    {
        return api_error(StatusCode::UNAUTHORIZED, "AUTH_REQUIRED");
    }
    if let Some(status) = *state.revoke_error.lock().await {
        return api_error(status, "UNAVAILABLE");
    }
    state.revocations.fetch_add(1, Ordering::SeqCst);
    Json(TrustObservationsRevokeResponse { deleted: 0 }).into_response()
}

async fn mock_rendezvous() -> Json<kukuri_cn_protocol::TopicRendezvousHeartbeatResponse> {
    Json(kukuri_cn_protocol::TopicRendezvousHeartbeatResponse {
        expires_in_seconds: 45,
        topics: Vec::new(),
    })
}

struct Harness {
    runtime: DesktopRuntime,
    base_url: String,
    state: MockObservationState,
    server: tokio::task::JoinHandle<()>,
    dir: tempfile::TempDir,
}

async fn spawn_server(
    state: MockObservationState,
    listener: TcpListener,
) -> tokio::task::JoinHandle<()> {
    let managed_router = Router::new()
        .route("/v1/auth/challenge", post(mock_managed_auth_challenge))
        .route("/v1/auth/verify", post(mock_managed_auth_verify))
        .route("/v1/consents/status", get(mock_managed_consent_status))
        .route(
            "/v1/bootstrap/heartbeat",
            post(mock_managed_bootstrap_heartbeat),
        )
        .route("/v1/bootstrap/nodes", get(mock_managed_bootstrap_nodes))
        .with_state(Arc::clone(&state.managed));
    let observation_router = Router::new()
        .route("/v1/policies", get(mock_policies))
        .route("/v1/consents", post(mock_accept_consents))
        .route(
            "/v1/trust/observations",
            post(mock_submit).delete(mock_revoke),
        )
        .route("/v1/rendezvous/topics/heartbeat", post(mock_rendezvous))
        .with_state(state);
    tokio::spawn(async move {
        axum::serve(listener, managed_router.merge(observation_router))
            .await
            .expect("server");
    })
}

async fn open_runtime(db_path: &std::path::Path, base_url: &str) -> DesktopRuntime {
    let runtime = DesktopRuntime::new_with_config_and_identity(
        db_path,
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime");
    *runtime.community_node_config.lock().await = CommunityNodeConfig {
        trust_node_priority: Vec::new(),
        nodes: vec![CommunityNodeNodeConfig {
            content_advisory_enabled: true,
            base_url: base_url.to_string(),
            resolved_urls: Some(
                CommunityNodeResolvedUrls::new(base_url.to_string(), Vec::new(), Vec::new())
                    .expect("resolved urls"),
            ),
        }],
    };
    runtime
}

async fn harness() -> Harness {
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("community-trust-observations.db");
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
    let base_url = format!("http://{}", listener.local_addr().expect("local addr"));
    let managed = Arc::new(MockManagedCommunityNodeState::new(
        base_url.clone(),
        Vec::new(),
        true,
        Arc::new(Mutex::new("observation-token".to_string())),
    ));
    let state = MockObservationState {
        managed,
        submissions: Arc::new(Mutex::new(Vec::new())),
        revocations: Arc::new(AtomicUsize::new(0)),
        accepted_slugs: Arc::new(Mutex::new(Vec::new())),
        submit_error: Arc::new(Mutex::new(None)),
        revoke_error: Arc::new(Mutex::new(None)),
        offer_sharing: Arc::new(AtomicBool::new(true)),
    };
    let server = spawn_server(state.clone(), listener).await;
    persist_community_node_token(
        &db_path,
        IdentityStorageMode::FileOnly,
        base_url.as_str(),
        &StoredCommunityNodeToken {
            access_token: "observation-token".to_string(),
            expires_at: Utc::now().timestamp() + 3600,
        },
    )
    .expect("persist token");
    let runtime = open_runtime(&db_path, base_url.as_str()).await;
    seed_local_community_node_consents(&runtime, base_url.as_str(), 1);
    Harness {
        runtime,
        base_url,
        state,
        server,
        dir,
    }
}

impl Harness {
    async fn enable(&self, include_existing: bool) -> crate::CommunityNodeObservationSharingStatus {
        self.runtime
            .enable_community_node_observation_sharing(
                EnableCommunityNodeObservationSharingRequest {
                    base_url: self.base_url.clone(),
                    policy_version: SHARING_POLICY_VERSION,
                    policy_snapshot_revision: None,
                    language: "ja".to_string(),
                    include_existing,
                },
                "test-app",
            )
            .await
            .expect("enable observation sharing")
    }

    async fn status(&self) -> crate::CommunityNodeObservationSharingStatus {
        self.runtime
            .get_community_node_observation_sharing(CommunityNodeTargetRequest {
                base_url: self.base_url.clone(),
            })
            .await
            .expect("observation sharing status")
    }

    async fn submitted(&self) -> Vec<TrustObservation> {
        self.state
            .submissions
            .lock()
            .await
            .iter()
            .flatten()
            .cloned()
            .collect()
    }

    async fn submission_count(&self) -> usize {
        self.state.submissions.lock().await.len()
    }

    async fn mute(&self, pubkey: &str) {
        self.runtime
            .mute_author(AuthorRequest {
                pubkey: pubkey.to_string(),
            })
            .await
            .expect("mute author");
    }

    async fn finish(self) {
        self.runtime.shutdown().await;
        self.server.abort();
        drop(self.dir);
    }
}

/// テスト中は同じ seed に同じ有効な公開鍵を返す。
fn author(seed: char) -> String {
    static AUTHORS: std::sync::OnceLock<[String; 3]> = std::sync::OnceLock::new();
    let authors = AUTHORS.get_or_init(|| {
        [
            kukuri_core::generate_keys().public_key_hex(),
            kukuri_core::generate_keys().public_key_hex(),
            kukuri_core::generate_keys().public_key_hex(),
        ]
    });
    match seed {
        '1' => authors[0].clone(),
        '2' => authors[1].clone(),
        _ => authors[2].clone(),
    }
}

#[tokio::test]
async fn observation_not_sent_without_sharing_consent() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let harness = harness().await;

    harness.mute(author('1').as_str()).await;
    harness
        .runtime
        .flush_community_node_trust_observations_once()
        .await;

    assert_eq!(harness.submission_count().await, 0);
    assert_eq!(harness.state.revocations.load(Ordering::SeqCst), 0);
    let status = harness.status().await;
    assert!(status.offered);
    assert!(!status.enabled);
    assert_eq!(status.pending_count, 0);
    assert_eq!(
        status.policy.map(|policy| policy.policy_version),
        Some(SHARING_POLICY_VERSION)
    );
    harness.finish().await;
}

#[tokio::test]
async fn existing_mutes_are_sent_only_when_opted_in() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let harness = harness().await;
    harness.mute(author('1').as_str()).await;

    // 既存分を選ばなければ、有効化前の mute は送らない。
    let status = harness.enable(false).await;
    assert!(status.enabled);
    assert_eq!(harness.submission_count().await, 0);
    assert_eq!(
        harness.state.accepted_slugs.lock().await.clone(),
        vec![vec![TRUST_OBSERVATION_SHARING_POLICY_SLUG.to_string()]]
    );

    // 有効化後の操作だけを送る。
    harness.mute(author('2').as_str()).await;
    harness
        .runtime
        .flush_community_node_trust_observations_once()
        .await;
    let submitted = harness.submitted().await;
    assert_eq!(submitted.len(), 1);
    assert_eq!(submitted[0].target_pubkey.as_str(), author('2'));
    assert_eq!(submitted[0].kind, TrustObservationKind::Mute);
    assert!(submitted[0].active);
    assert_eq!(harness.status().await.pending_count, 0);

    // 無効化して既存分つきで再度有効化すると、現在の mute をすべて送る。
    harness
        .runtime
        .disable_community_node_observation_sharing(CommunityNodeTargetRequest {
            base_url: harness.base_url.clone(),
        })
        .await
        .expect("disable");
    harness.enable(true).await;
    let mut targets: Vec<String> = harness
        .submitted()
        .await
        .into_iter()
        .skip(1)
        .map(|observation| observation.target_pubkey.0)
        .collect();
    targets.sort();
    let mut expected = vec![author('1'), author('2')];
    expected.sort();
    assert_eq!(targets, expected);
    harness.finish().await;
}

#[tokio::test]
async fn outbox_coalesces_and_resumes_after_restart() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let harness = harness().await;
    harness.enable(false).await;
    *harness.state.submit_error.lock().await =
        Some((StatusCode::SERVICE_UNAVAILABLE, "UNAVAILABLE".to_string()));

    let target = author('3');
    harness.mute(target.as_str()).await;
    harness
        .runtime
        .unmute_author(AuthorRequest {
            pubkey: target.clone(),
        })
        .await
        .expect("unmute");
    harness.mute(target.as_str()).await;
    harness
        .runtime
        .flush_community_node_trust_observations_once()
        .await;
    // 送信に失敗しても、対象 × 種別ごとに最新 1 件だけが残る。
    assert_eq!(harness.status().await.pending_count, 1);
    assert_eq!(harness.submission_count().await, 0);

    // restart 後に再開し、最新の状態（active）だけを送る。
    let db_path = harness.dir.path().join("community-trust-observations.db");
    harness.runtime.shutdown().await;
    *harness.state.submit_error.lock().await = None;
    let restarted = open_runtime(&db_path, harness.base_url.as_str()).await;
    assert!(
        restarted
            .get_community_node_observation_sharing(CommunityNodeTargetRequest {
                base_url: harness.base_url.clone(),
            })
            .await
            .expect("status")
            .enabled
    );
    restarted
        .flush_community_node_trust_observations_once()
        .await;
    let submitted = harness.submitted().await;
    assert_eq!(submitted.len(), 1);
    assert_eq!(submitted[0].target_pubkey.as_str(), target);
    assert!(submitted[0].active);
    restarted.shutdown().await;
    harness.server.abort();
}

#[tokio::test]
async fn revocation_pending_blocks_new_posts() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let harness = harness().await;
    harness.enable(false).await;
    *harness.state.revoke_error.lock().await = Some(StatusCode::SERVICE_UNAVAILABLE);

    let status = harness
        .runtime
        .disable_community_node_observation_sharing(CommunityNodeTargetRequest {
            base_url: harness.base_url.clone(),
        })
        .await
        .expect("disable");
    assert!(!status.enabled);
    assert!(status.revocation_pending);

    // 削除要求が終わるまで、新しい操作は積まず送らない。
    harness.mute(author('1').as_str()).await;
    harness
        .runtime
        .flush_community_node_trust_observations_once()
        .await;
    assert_eq!(harness.submission_count().await, 0);
    assert_eq!(harness.status().await.pending_count, 0);
    assert!(harness.status().await.revocation_pending);

    // 再送が成功すると印が外れる。
    *harness.state.revoke_error.lock().await = None;
    harness
        .runtime
        .flush_community_node_trust_observations_once()
        .await;
    assert_eq!(harness.state.revocations.load(Ordering::SeqCst), 1);
    assert!(!harness.status().await.revocation_pending);
    assert_eq!(harness.submission_count().await, 0);
    harness.finish().await;
}

#[tokio::test]
async fn reauth_once_on_401() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let harness = harness().await;
    harness.enable(false).await;
    harness.mute(author('1').as_str()).await;

    // 保存済み token が失効している。
    *harness.state.managed.current_token.lock().await = "rotated-token".to_string();
    harness
        .runtime
        .flush_community_node_trust_observations_once()
        .await;

    assert!(harness.state.managed.verify_hits.load(Ordering::SeqCst) >= 1);
    assert_eq!(harness.submitted().await.len(), 1);
    assert_eq!(harness.status().await.pending_count, 0);
    harness.finish().await;
}

#[tokio::test]
async fn local_mute_succeeds_when_cn_unreachable() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let harness = harness().await;
    harness.enable(false).await;
    harness.server.abort();
    tokio::task::yield_now().await;

    let view = harness
        .runtime
        .mute_author(AuthorRequest {
            pubkey: author('1'),
        })
        .await
        .expect("local mute succeeds without the community node");
    assert!(view.muted);
    harness
        .runtime
        .flush_community_node_trust_observations_once()
        .await;
    let state = crate::community_node::load_trust_observation_pending_count(
        &harness.runtime,
        harness.base_url.as_str(),
    )
    .await;
    assert_eq!(state, 1);
    harness.runtime.shutdown().await;
}

#[tokio::test]
async fn sharing_consent_required_revokes_and_stops_sending_until_reenabled() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let harness = harness().await;
    harness.enable(false).await;
    *harness.state.submit_error.lock().await = Some((
        StatusCode::FORBIDDEN,
        "TRUST_OBSERVATION_SHARING_CONSENT_REQUIRED".to_string(),
    ));
    harness.mute(author('1').as_str()).await;
    harness
        .runtime
        .flush_community_node_trust_observations_once()
        .await;
    // 同意が失効したら、送信済みの観測も残さず削除を要求する（要求は次の tick で送る）。
    let status = harness.status().await;
    assert!(!status.enabled);
    assert!(status.needs_reconsent);
    assert_eq!(status.pending_count, 0);
    assert!(status.revocation_pending);
    harness
        .runtime
        .flush_community_node_trust_observations_once()
        .await;
    assert_eq!(harness.state.revocations.load(Ordering::SeqCst), 1);
    assert!(!harness.status().await.revocation_pending);

    // 再同意まで、操作は積まず送らない。
    *harness.state.submit_error.lock().await = None;
    harness.mute(author('2').as_str()).await;
    harness
        .runtime
        .flush_community_node_trust_observations_once()
        .await;
    assert_eq!(harness.submission_count().await, 0);

    // 再同意すると提供を再開する。既存分を選ばなければ、以後の操作だけを送る。
    let status = harness.enable(false).await;
    assert!(status.enabled);
    assert!(!status.needs_reconsent);
    assert!(!status.revocation_pending);
    assert_eq!(harness.submission_count().await, 0);
    harness.mute(author('3').as_str()).await;
    harness
        .runtime
        .flush_community_node_trust_observations_once()
        .await;
    let submitted = harness.submitted().await;
    assert_eq!(submitted.len(), 1);
    assert_eq!(submitted[0].target_pubkey.as_str(), author('3'));
    harness.finish().await;
}

#[tokio::test]
async fn reconsent_does_not_send_observations_that_no_longer_match_local_state() {
    // 提供が止まっている間の解除は積まれない。止まる前の送信待ちが残っていても、
    // 端末の現在の状態と合わないものは再同意時に捨てる。
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let harness = harness().await;
    harness.enable(false).await;
    *harness.state.submit_error.lock().await =
        Some((StatusCode::SERVICE_UNAVAILABLE, "UNAVAILABLE".to_string()));
    harness.mute(author('1').as_str()).await;
    harness.mute(author('2').as_str()).await;
    harness
        .runtime
        .flush_community_node_trust_observations_once()
        .await;
    assert_eq!(harness.status().await.pending_count, 2);

    // 送信できていない間に author 1 のミュートを解除する（提供は有効なので解除も積まれる）。
    harness
        .runtime
        .unmute_author(AuthorRequest {
            pubkey: author('1'),
        })
        .await
        .expect("unmute");
    *harness.state.submit_error.lock().await = None;
    harness.enable(false).await;
    let submitted = harness.submitted().await;
    let targets: Vec<&str> = submitted
        .iter()
        .map(|observation| observation.target_pubkey.as_str())
        .collect();
    assert!(targets.contains(&author('2').as_str()), "{targets:?}");
    // 解除は解除として送られ、古い「有効」は送られない。
    let author_one = author('1');
    let one: Vec<&TrustObservation> = submitted
        .iter()
        .filter(|observation| observation.target_pubkey.as_str() == author_one)
        .collect();
    assert!(one.iter().all(|observation| !observation.active), "{one:?}");
    harness.finish().await;
}

#[tokio::test]
async fn not_offered_node_cannot_be_enabled() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let harness = harness().await;
    harness.state.offer_sharing.store(false, Ordering::SeqCst);

    let status = harness.status().await;
    assert!(!status.offered);
    let error = harness
        .runtime
        .enable_community_node_observation_sharing(
            EnableCommunityNodeObservationSharingRequest {
                base_url: harness.base_url.clone(),
                policy_version: SHARING_POLICY_VERSION,
                policy_snapshot_revision: None,
                language: "ja".to_string(),
                include_existing: true,
            },
            "test-app",
        )
        .await
        .expect_err("node without the sharing document");
    assert!(error.to_string().contains("does not offer"), "{error}");
    // 版が違う文書への同意も受け付けない。
    harness.state.offer_sharing.store(true, Ordering::SeqCst);
    let error = harness
        .runtime
        .enable_community_node_observation_sharing(
            EnableCommunityNodeObservationSharingRequest {
                base_url: harness.base_url.clone(),
                policy_version: SHARING_POLICY_VERSION - 1,
                policy_snapshot_revision: None,
                language: "ja".to_string(),
                include_existing: false,
            },
            "test-app",
        )
        .await
        .expect_err("stale document version");
    assert!(error.to_string().contains("policy changed"), "{error}");
    assert!(harness.state.accepted_slugs.lock().await.is_empty());
    assert_eq!(harness.submission_count().await, 0);
    harness.finish().await;
}

#[tokio::test]
async fn consent_withdrawal_and_node_removal_request_observation_deletion() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let harness = harness().await;
    harness.enable(false).await;

    // 同意取消は token を消す前に削除を要求する。
    harness
        .runtime
        .withdraw_community_node_consents(CommunityNodeTargetRequest {
            base_url: harness.base_url.clone(),
        })
        .await
        .expect("withdraw");
    assert_eq!(harness.state.revocations.load(Ordering::SeqCst), 1);
    let status = harness.status().await;
    assert!(!status.enabled);
    assert!(!status.revocation_pending);

    // 再同意 → 提供を再開 → CN を削除すると、削除を要求してから状態を破棄する。
    seed_local_community_node_consents(&harness.runtime, harness.base_url.as_str(), 1);
    persist_community_node_token(
        &harness.runtime.db_path,
        IdentityStorageMode::FileOnly,
        harness.base_url.as_str(),
        &StoredCommunityNodeToken {
            access_token: harness.state.managed.current_token.lock().await.clone(),
            expires_at: Utc::now().timestamp() + 3600,
        },
    )
    .expect("persist token");
    harness.enable(false).await;
    harness
        .runtime
        .set_community_node_config(SetCommunityNodeConfigRequest {
            trust_node_priority: None,
            nodes: Vec::new(),
        })
        .await
        .expect("remove node");
    assert_eq!(harness.state.revocations.load(Ordering::SeqCst), 2);
    assert_eq!(
        crate::community_node::load_trust_observation_pending_count(
            &harness.runtime,
            harness.base_url.as_str(),
        )
        .await,
        0
    );
    harness.finish().await;
}

#[tokio::test]
async fn general_consent_acceptance_excludes_sharing_document() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let harness = harness().await;
    harness
        .runtime
        .accept_community_node_consents(
            AcceptCommunityNodeConsentsRequest {
                base_url: harness.base_url.clone(),
                documents: vec![
                    CommunityNodeConsentDocumentRef {
                        policy_slug: MOCK_MANAGED_POLICY_SLUG.to_string(),
                        policy_version: 1,
                        policy_snapshot_revision: None,
                    },
                    CommunityNodeConsentDocumentRef {
                        policy_slug: TRUST_OBSERVATION_SHARING_POLICY_SLUG.to_string(),
                        policy_version: SHARING_POLICY_VERSION,
                        policy_snapshot_revision: None,
                    },
                ],
                language: "ja".to_string(),
            },
            "test-app",
        )
        .await
        .expect("accept consents");
    let consents = crate::community_node::load_community_node_local_consents(
        &harness.runtime.db_path,
        IdentityStorageMode::FileOnly,
        harness.base_url.as_str(),
    )
    .expect("local consents");
    assert!(
        consents
            .records
            .iter()
            .all(|record| record.policy_slug != TRUST_OBSERVATION_SHARING_POLICY_SLUG)
    );
    assert!(!harness.status().await.enabled);
    harness.finish().await;
}
