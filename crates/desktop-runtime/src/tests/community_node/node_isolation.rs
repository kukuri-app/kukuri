//! #1221 R2-B: 正常な CN と失敗する CN の同居。失敗する CN のどの出来事も、正常な CN への HTTP・
//! endpoint の作り直し・topic への join を増やさない。期限の来た node だけを処理する。
use super::super::*;
use axum::body::Bytes;
use axum::http::{Method, Uri};
use axum::response::{IntoResponse, Response};
use kukuri_cn_protocol::TopicRendezvousCandidate;
use kukuri_transport::Transport;
use std::sync::Mutex as StdMutex;

const A_SEED: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const B_SEED: &str = "2222222222222222222222222222222222222222222222222222222222222222";
const RENDEZVOUS_PEER: &str = "3333333333333333333333333333333333333333333333333333333333333333";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fault {
    None,
    /// 全要求へ 503。
    ServerError,
    /// 認証つき要求へ 401。再認証が `reauth` なら成功し、そうでなければ challenge が 503。
    Unauthorized {
        reauth: bool,
    },
}

/// 受け付けた接続を、`unreachable` の間は応答せずに閉じる(通信失敗)。
struct FlakyListener {
    inner: TcpListener,
    unreachable: Arc<AtomicBool>,
}

impl axum::serve::Listener for FlakyListener {
    type Io = tokio::net::TcpStream;
    type Addr = std::net::SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            let Ok((io, addr)) = self.inner.accept().await else {
                continue;
            };
            if !self.unreachable.load(Ordering::SeqCst) {
                return (io, addr);
            }
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        self.inner.local_addr()
    }
}

struct FlakyNode {
    base_url: String,
    relay_urls: Vec<String>,
    seed: String,
    fault: StdMutex<Fault>,
    unreachable: Arc<AtomicBool>,
    token: StdMutex<String>,
    hits: AtomicUsize,
    /// topic の rendezvous 鍵 → 返す peer。
    rendezvous: StdMutex<Vec<(String, String)>>,
    /// rendezvous の要求ごとの refresh の件数。
    rendezvous_refreshes: StdMutex<Vec<usize>>,
    server: StdMutex<Option<tokio::task::JoinHandle<()>>>,
}

impl FlakyNode {
    async fn spawn(relay_urls: Vec<String>, seed: &str) -> Arc<Self> {
        let inner = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let unreachable = Arc::new(AtomicBool::new(false));
        let node = Arc::new(Self {
            base_url: format!("http://{}", inner.local_addr().expect("addr")),
            relay_urls,
            seed: seed.to_string(),
            fault: StdMutex::new(Fault::None),
            unreachable: unreachable.clone(),
            token: StdMutex::new(String::new()),
            hits: AtomicUsize::new(0),
            rendezvous: StdMutex::new(Vec::new()),
            rendezvous_refreshes: StdMutex::new(Vec::new()),
            server: StdMutex::new(None),
        });
        let app = Router::new()
            .fallback(flaky_node_request)
            .with_state(node.clone());
        let listener = FlakyListener { inner, unreachable };
        *node.server.lock().unwrap() = Some(tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        }));
        node
    }

    fn set_fault(&self, fault: Fault) {
        *self.fault.lock().unwrap() = fault;
        if matches!(fault, Fault::Unauthorized { .. }) {
            *self.token.lock().unwrap() = "revoked".into();
        }
    }

    fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }
}

impl Drop for FlakyNode {
    fn drop(&mut self) {
        if let Some(server) = self.server.lock().unwrap().take() {
            server.abort();
        }
    }
}

async fn flaky_node_request(
    State(node): State<Arc<FlakyNode>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    node.hits.fetch_add(1, Ordering::SeqCst);
    let fault = *node.fault.lock().unwrap();
    if fault == Fault::ServerError {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let path = uri.path();
    match (method, path) {
        (Method::GET, "/v1/policies") => return mock_current_policies().await.into_response(),
        (Method::POST, "/v1/auth/challenge") => {
            if fault == (Fault::Unauthorized { reauth: false }) {
                return StatusCode::SERVICE_UNAVAILABLE.into_response();
            }
            return Json(kukuri_cn_protocol::AuthChallengeResponse {
                challenge: "challenge".into(),
                expires_at: Utc::now().timestamp() + 300,
            })
            .into_response();
        }
        (Method::POST, "/v1/auth/verify") => {
            let token = format!("token-{}", node.hits());
            *node.token.lock().unwrap() = token.clone();
            *node.fault.lock().unwrap() = Fault::None;
            return Json(kukuri_cn_protocol::AuthVerifyResponse {
                access_token: token,
                token_type: "Bearer".into(),
                expires_at: Utc::now().timestamp() + 3600,
                pubkey: "f".repeat(64),
            })
            .into_response();
        }
        _ => {}
    }
    let authorized = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|token| token == node.token.lock().unwrap().as_str());
    if !authorized {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    match path {
        "/v1/consents/status" | "/v1/consents" => {
            Json(managed_community_node_consent_status(true)).into_response()
        }
        "/v1/bootstrap/heartbeat" => Json(BootstrapHeartbeatResponse {
            expires_at: Utc::now().timestamp() + 300,
        })
        .into_response(),
        "/v1/bootstrap/nodes" => Json(BootstrapNodesResponse {
            nodes: vec![kukuri_cn_protocol::CommunityNodeBootstrapNode {
                base_url: node.base_url.clone(),
                resolved_urls: CommunityNodeResolvedUrls::new(
                    node.base_url.clone(),
                    node.relay_urls.clone(),
                    vec![CommunityNodeSeedPeer::new(node.seed.clone(), None).expect("seed")],
                )
                .expect("resolved urls"),
            }],
        })
        .into_response(),
        "/v1/rendezvous/topics/heartbeat" => {
            let request: kukuri_cn_protocol::TopicRendezvousHeartbeat =
                serde_json::from_slice(&body).expect("rendezvous request");
            node.rendezvous_refreshes
                .lock()
                .unwrap()
                .push(request.refreshes.len());
            let topics = node
                .rendezvous
                .lock()
                .unwrap()
                .iter()
                .filter(|(key, _)| request.refreshes.contains(key))
                .map(
                    |(key, peer)| kukuri_cn_protocol::TopicRendezvousTopicResponse {
                        topic_key: key.clone(),
                        peers: vec![TopicRendezvousCandidate {
                            endpoint_id: peer.clone(),
                            addr_hint: Some("127.0.0.1:9".into()),
                            relay_urls: Vec::new(),
                        }],
                    },
                )
                .collect();
            Json(kukuri_cn_protocol::TopicRendezvousHeartbeatResponse {
                expires_in_seconds: 300,
                topics,
            })
            .into_response()
        }
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn isolation_runtime(dir: &Path, nodes: &[&Arc<FlakyNode>]) -> DesktopRuntime {
    let runtime = DesktopRuntime::new_with_config_and_identity(
        dir.join("isolation.db"),
        TransportNetworkConfig::loopback(),
        IdentityStorageMode::FileOnly,
    )
    .await
    .expect("runtime");
    for node in nodes {
        seed_local_community_node_consents(&runtime, &node.base_url, 1);
    }
    timeout(
        Duration::from_secs(60),
        runtime.set_community_node_config(SetCommunityNodeConfigRequest {
            nodes: nodes
                .iter()
                .map(|node| SetCommunityNodeConfigNode::new(node.base_url.clone()))
                .collect(),
            trust_node_priority: None,
        }),
    )
    .await
    .expect("config timeout")
    .expect("config");
    for node in nodes {
        assert_eq!(
            session_phase(&runtime, node).await,
            CommunityNodeSessionPhase::Ready
        );
    }
    runtime
}

async fn session_phase(runtime: &DesktopRuntime, node: &FlakyNode) -> CommunityNodeSessionPhase {
    runtime.community_node_sessions.lock().await[node.base_url.as_str()].session_phase
}

/// その node の期限を過ぎたことにして、scheduler の 1 回分を動かす。
async fn run_due(runtime: &DesktopRuntime, node: &FlakyNode) {
    if let Some(session) = runtime
        .community_node_sessions
        .lock()
        .await
        .get_mut(node.base_url.as_str())
    {
        session.heartbeat_deadline = 0;
        session.rendezvous_refresh_deadline = 0;
        session.session_retry_deadline = 0;
    }
    timeout(
        Duration::from_secs(60),
        runtime.run_community_node_session_maintenance_once(),
    )
    .await
    .expect("maintenance timeout");
}

async fn relay_urls(runtime: &DesktopRuntime) -> Vec<String> {
    let node = runtime
        .iroh_stack
        .current
        .lock()
        .await
        .as_ref()
        .expect("stack")
        .node
        .clone();
    node.relay_urls()
        .await
        .into_iter()
        .map(|url| url.to_string())
        .collect()
}

async fn bootstrap_seeds(runtime: &DesktopRuntime) -> Vec<String> {
    runtime
        .iroh_stack
        .transport
        .discovery()
        .await
        .expect("discovery")
        .bootstrap_seed_peer_ids
}

async fn update_joins(runtime: &DesktopRuntime, topic: &str) -> u64 {
    let hint = kukuri_core::wire::hint_topic_id(&kukuri_core::TopicId::new(topic));
    runtime
        .iroh_stack
        .transport
        .current()
        .await
        .topic_join_counts(hint.as_str())
        .await
        .0
}

async fn run_relay() -> (String, impl Sized) {
    let (_map, url, guard) = iroh::test_utils::run_relay_server()
        .await
        .expect("relay server");
    (url.to_string(), guard)
}

/// 正常な A と、失敗を起こす B。B の 401(再認証の成功・失敗)・5xx・通信失敗・同意の解除のどれでも、
/// A への HTTP、endpoint の作り直し(と lease の task の作り直し)、A の topic への join は増えない。
/// 一時的な失敗の間は B の relay と seed を使い続け、認証の失効と同意の解除では外す。期限前は HTTP を送らない。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failing_node_never_touches_the_healthy_node() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let (relay_a, _relay_a_guard) = run_relay().await;
    let (relay_b, _relay_b_guard) = run_relay().await;
    let a = FlakyNode::spawn(vec![relay_a.clone()], A_SEED).await;
    let b = FlakyNode::spawn(vec![relay_b.clone()], B_SEED).await;
    let dir = tempdir().expect("tempdir");
    let runtime = isolation_runtime(dir.path(), &[&a, &b]).await;
    let topic = "kukuri:topic:node-isolation";
    open_topic_column(&runtime, topic, TimelineScope::Public)
        .await
        .expect("column");
    assert_eq!(relay_urls(&runtime).await.len(), 2);
    let a_hits = a.hits();
    let generation = runtime.iroh_stack.generation();
    let joins = update_joins(&runtime, topic).await;
    let b_kept = |label: &'static str| {
        let runtime = &runtime;
        let relay_b = relay_b.clone();
        async move {
            assert!(
                relay_urls(runtime).await.contains(&relay_b),
                "{label}: B relay"
            );
            assert!(
                bootstrap_seeds(runtime).await.contains(&B_SEED.to_string()),
                "{label}: B seed"
            );
        }
    };
    let b_removed = |label: &'static str| {
        let runtime = &runtime;
        let relay_b = relay_b.clone();
        async move {
            assert!(
                !relay_urls(runtime).await.contains(&relay_b),
                "{label}: B relay"
            );
            assert!(
                !bootstrap_seeds(runtime).await.contains(&B_SEED.to_string()),
                "{label}: B seed"
            );
        }
    };

    // 期限前は、どの node にも HTTP を送らない(同意と policy の確認を含む)。
    let b_hits = b.hits();
    runtime.run_community_node_session_maintenance_once().await;
    assert_eq!((a.hits(), b.hits()), (a_hits, b_hits), "no HTTP before due");

    // 401 のあと再認証に成功: そのまま使い続ける。
    b.set_fault(Fault::Unauthorized { reauth: true });
    run_due(&runtime, &b).await;
    assert_eq!(
        session_phase(&runtime, &b).await,
        CommunityNodeSessionPhase::Ready
    );
    b_kept("reauthenticated").await;

    // 401 のあと再認証に失敗: 認証の失効として B の relay と seed を外す。
    b.set_fault(Fault::Unauthorized { reauth: false });
    run_due(&runtime, &b).await;
    assert_eq!(
        session_phase(&runtime, &b).await,
        CommunityNodeSessionPhase::Retrying
    );
    b_removed("authentication revoked").await;

    // 回復すると戻る。
    b.set_fault(Fault::None);
    run_due(&runtime, &b).await;
    assert_eq!(
        session_phase(&runtime, &b).await,
        CommunityNodeSessionPhase::Ready
    );
    b_kept("recovered").await;

    // 5xx と通信失敗は一時的な失敗: B の phase と backoff だけが変わり、relay と seed は残る。
    b.set_fault(Fault::ServerError);
    run_due(&runtime, &b).await;
    assert_eq!(
        session_phase(&runtime, &b).await,
        CommunityNodeSessionPhase::Retrying
    );
    b_kept("server error").await;
    b.set_fault(Fault::None);
    b.unreachable.store(true, Ordering::SeqCst);
    run_due(&runtime, &b).await;
    assert_eq!(
        session_phase(&runtime, &b).await,
        CommunityNodeSessionPhase::Retrying
    );
    b_kept("unreachable").await;
    b.unreachable.store(false, Ordering::SeqCst);

    // 同意の解除で外す。
    runtime
        .withdraw_community_node_consents(CommunityNodeTargetRequest {
            base_url: b.base_url.clone(),
        })
        .await
        .expect("withdraw");
    b_removed("consent withdrawn").await;
    assert!(relay_urls(&runtime).await.contains(&relay_a));
    assert!(
        bootstrap_seeds(&runtime)
            .await
            .contains(&A_SEED.to_string())
    );

    assert_eq!(a.hits(), a_hits, "HTTP to the healthy node");
    assert_eq!(
        runtime.iroh_stack.generation(),
        generation,
        "endpoint rebuild (lease tasks are recreated only on a rebuild)"
    );
    assert_eq!(
        update_joins(&runtime, topic).await,
        joins,
        "joins on the topic"
    );
    assert_eq!(
        session_phase(&runtime, &a).await,
        CommunityNodeSessionPhase::Ready
    );
    runtime.shutdown().await;
}

/// ADR 0055 NW-5。rendezvous の peer は、その鍵の topic だけへ join する。同じ peer をもう一度受け取っても
/// join しない。無関係な topic は触らない。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn peer_delta_is_scoped() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let a = FlakyNode::spawn(Vec::new(), A_SEED).await;
    let (one, other) = (
        "kukuri:topic:rendezvous-one",
        "kukuri:topic:rendezvous-other",
    );
    let key = kukuri_core::public_topic_rendezvous_key(&kukuri_core::wire::hint_topic_id(
        &kukuri_core::TopicId::new(one),
    ));
    a.rendezvous
        .lock()
        .unwrap()
        .push((key, RENDEZVOUS_PEER.to_string()));
    let dir = tempdir().expect("tempdir");
    let runtime = isolation_runtime(dir.path(), &[&a]).await;
    open_topic_column(&runtime, one, TimelineScope::Public)
        .await
        .expect("one");
    open_topic_column(&runtime, other, TimelineScope::Public)
        .await
        .expect("other");
    for _ in 0..2 {
        run_due(&runtime, &a).await;
    }
    assert_eq!(update_joins(&runtime, one).await, 1);
    assert_eq!(update_joins(&runtime, other).await, 0);
    assert!(
        !bootstrap_seeds(&runtime)
            .await
            .contains(&RENDEZVOUS_PEER.to_string()),
        "a rendezvous peer is not a seed of every topic"
    );
    runtime.shutdown().await;
}

/// 1 回の tick の仕事(期限の来た job の数、node ごとの HTTP、rendezvous の鍵の数)は、B の失敗の履歴、
/// 休止した topic、設定から外した node の履歴を 10 倍にしても変わらない(V2・V3)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tick_work_does_not_grow_with_failure_or_dormant_history() {
    let _resource = lock_test_resource(TestResource::CommunityNodeServer).await;
    let a = FlakyNode::spawn(Vec::new(), A_SEED).await;
    let b = FlakyNode::spawn(Vec::new(), B_SEED).await;
    let dir = tempdir().expect("tempdir");
    let runtime = isolation_runtime(dir.path(), &[&a, &b]).await;
    open_topic_column(&runtime, "kukuri:topic:tick-leased", TimelineScope::Public)
        .await
        .expect("leased");
    b.set_fault(Fault::ServerError);
    let mut dormant = 0;
    let mut add_history = async |runtime: &DesktopRuntime, count: usize| {
        for _ in 0..count {
            run_due(runtime, &b).await;
            let topic = format!("kukuri:topic:tick-dormant-{dormant}");
            open_topic_column(runtime, &topic, TimelineScope::Public)
                .await
                .expect("dormant");
            runtime
                .set_scope_display(crate::ScopeDisplayRequest {
                    observer: format!("test-column:{topic}:{:?}", TimelineScope::Public),
                    target: crate::ScopeDisplayTarget::Timeline {
                        topic,
                        scope: TimelineScope::Public,
                    },
                    visible: false,
                })
                .await
                .expect("close dormant");
            let removed = format!("http://127.0.0.1:9/removed-{dormant}");
            for nodes in [
                vec![a.base_url.clone(), b.base_url.clone(), removed],
                vec![a.base_url.clone(), b.base_url.clone()],
            ] {
                runtime
                    .set_community_node_config(SetCommunityNodeConfigRequest {
                        nodes: nodes
                            .into_iter()
                            .map(SetCommunityNodeConfigNode::new)
                            .collect(),
                        trust_node_priority: None,
                    })
                    .await
                    .expect("config history");
            }
            dormant += 1;
        }
    };
    let measure = async |runtime: &DesktopRuntime| {
        let idle_jobs = runtime.community_node_maintenance_jobs().await;
        let hits = (a.hits(), b.hits());
        runtime.run_community_node_session_maintenance_once().await;
        let idle_http = (a.hits() - hits.0, b.hits() - hits.1);
        a.rendezvous_refreshes.lock().unwrap().clear();
        let before = a.hits();
        run_due(runtime, &a).await;
        let due_http = a.hits() - before;
        let mut refreshes = a.rendezvous_refreshes.lock().unwrap().clone();
        refreshes.sort();
        (idle_jobs, idle_http, due_http, refreshes)
    };
    add_history(&runtime, 1).await;
    let once = measure(&runtime).await;
    add_history(&runtime, 10).await;
    let tenfold = measure(&runtime).await;
    assert_eq!(once.1, (0, 0), "no HTTP before due");
    assert_eq!(once, tenfold);
    runtime.shutdown().await;
}
