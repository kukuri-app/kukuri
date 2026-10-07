//! Web クライアントの実ブラウザ試験の相手（#1220 W8 AC-2a）。
//!
//! 同じ process で、in-process の Community Node（user-api と iroh relay。Web の配信 origin に CORS で応答する）と STUN、
//! Web の build（`dist-web`。その `_headers` の header を付ける）の配信、Community Node に同意した native の相手を
//! 起動する。試験の driver（WebdriverIO）は `/fixture/*` で native を操作し、relay が中継した bytes で実データの経路を
//! 判定する（ADR 0060 §4・§5）。`index.html` には、試験の page の script（`page-init.js`）を同じ origin の script として
//! 足す（driver の機能に頼らず、どのブラウザでも page の script より先に動く。#1220 AC-5b）。
//!
//! Community Node は公開 blob の保持端末の検索を提供する（#1632 AC-6）。DHT は loopback の Testnet、補助 index は同じ
//! process の server。Web の知らない保持端末（Community Node を使わない native）は、scenario が `/fixture/public-blob-holder`
//! で作る。
//!
//! 環境変数:
//! - `KUKURI_WEB_E2E_DIST`: 配信する `dist-web`（必須）
//! - `KUKURI_WEB_E2E_WEB_ADDR`: 配信の listen（既定 `127.0.0.1:4180`）
//! - `KUKURI_WEB_E2E_CN_ADDR`: user-api の listen（既定 `127.0.0.1:4181`。Web の build の Community Node の URL と同じ）
//! - Community Node の Postgres・Redis は harness と同じ（`COMMUNITY_NODE_DATABASE_URL` 等）
//!
//! 準備ができたら標準出力に `KUKURI_WEB_E2E_READY=<配信の origin>` を出す。

use axum::extract::{DefaultBodyLimit, Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::routing::{get, post};
use axum::{Json, Router};
use kukuri_app_api::LinkPreviewRecordInput;
use kukuri_desktop_runtime::{
    ClientGate, ClientHost, ClientStartupState, ClientStartupStatus, CommandError,
    CommunityNodeConsentDocumentRef, DispatchContext, FetchCommunityNodePoliciesRequest,
    SetCommunityNodeConfigNode, dispatch_command,
};
use kukuri_store::AccountSyncStore;
use kukuri_transport::{DhtDiscoveryOptions, PublicBlobIndex};
use n0_mainline::{DhtBuilder, Testnet};
use serde_json::{Value, json};
use tower_http::services::ServeDir;

use crate::*;

/// 経路の判定に使う画像の 1 辺（乱数の画素の PNG は圧縮されず、約 1.7 MiB になる）。
const PAYLOAD_IMAGE_SIDE: u32 = 768;
/// リンクプレビューの record の画像の 1 辺（約 12 KiB。record の画像の上限 1 MiB に収まる）。
const PREVIEW_IMAGE_SIDE: u32 = 64;
/// page の script より先に動く試験の script。
const PAGE_INIT: &str = include_str!("../../../apps/desktop/tests/web-e2e/page-init.js");

struct NativeGate {
    host: Arc<ClientHost>,
    startup: ClientStartupState,
    lock: tokio::sync::Mutex<()>,
}

impl ClientGate for NativeGate {
    fn host(&self) -> Option<Arc<ClientHost>> {
        Some(self.host.clone())
    }

    fn startup(&self) -> &ClientStartupState {
        &self.startup
    }

    fn operation_lock(&self) -> &tokio::sync::Mutex<()> {
        &self.lock
    }

    fn require_running(&self) -> Result<(), CommandError> {
        Ok(())
    }
}

struct Fixture {
    gate: NativeGate,
    stack: CommunityNodeStack,
    /// native の DB（同期の中身の検査で読む）。
    db: PathBuf,
    public_blobs: PublicBlobNet,
    /// Web の知らない公開 blob の保持端末（#1632 AC-6）。
    holder: tokio::sync::Mutex<Option<DesktopRuntime>>,
}

/// 公開 blob の保持端末の検索の DHT（loopback の Testnet）と補助 index の server。値を落とすと止まる。
struct PublicBlobNet {
    testnet: Testnet,
    index: std::net::SocketAddrV4,
    _server: udp_addr_index::UdpHandle,
}

impl PublicBlobNet {
    async fn start() -> Result<Self> {
        let testnet = Testnet::new(3).await?;
        let dht = testnet_dht(&testnet).build()?;
        let index = std::net::SocketAddrV4::new(
            std::net::Ipv4Addr::LOCALHOST,
            dht.info().await?.local_addr().port(),
        );
        let server = udp_addr_index::Server::new(udp_addr_index::Limits::for_tests())
            .attach_with_rendezvous(dht, None)
            .await?;
        Ok(Self {
            testnet,
            index,
            _server: server,
        })
    }

    fn dht(&self) -> DhtBuilder {
        testnet_dht(&self.testnet)
    }

    fn index(&self) -> PublicBlobIndex {
        PublicBlobIndex::Servers(vec![self.index])
    }
}

fn testnet_dht(testnet: &Testnet) -> DhtBuilder {
    let mut builder = DhtBuilder::default();
    builder
        .bootstrap(&testnet.bootstrap)
        .port(0)
        .public_ip(std::net::Ipv4Addr::LOCALHOST);
    builder
}

type Shared = Arc<Fixture>;

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
}

pub async fn run_web_e2e_fixture() -> Result<()> {
    unsafe { std::env::set_var("KUKURI_DISABLE_KEYRING", "1") };
    let dist = PathBuf::from(std::env::var("KUKURI_WEB_E2E_DIST").context("KUKURI_WEB_E2E_DIST")?);
    let web_addr: SocketAddr = env_or("KUKURI_WEB_E2E_WEB_ADDR", "127.0.0.1:4180").parse()?;
    let cn_addr = env_or("KUKURI_WEB_E2E_CN_ADDR", "127.0.0.1:4181");
    let web_origin = format!("http://{web_addr}");
    let rules = Arc::new(artifact_headers(&dist)?);
    let html = axum::response::Html(std::fs::read_to_string(dist.join("index.html"))?.replacen(
        "<head>",
        r#"<head><script src="/fixture/page-init.js"></script>"#,
        1,
    ));
    let index = move || std::future::ready(html.clone());

    let public_blobs = PublicBlobNet::start().await?;
    let search =
        kukuri_cn_user_api::BlobProviderSearch::start(&public_blobs.dht(), public_blobs.index())?;
    let stack = CommunityNodeStack::spawn_with(
        "web_e2e",
        &cn_addr,
        std::slice::from_ref(&web_origin),
        Some(Arc::new(search)),
    )
    .await?;
    // 本番と同じく relay の host の 3478 番で STUN に応答する（ADR 0060 §5）。応答が無いと、browser の offer は候補集めの
    // 上限（3 秒）まで待つ（#1590）。
    let stun = tokio::net::UdpSocket::bind((std::net::Ipv4Addr::UNSPECIFIED, kukuri_cn_stun::PORT))
        .await
        .context("failed to bind the stun socket")?;
    tokio::spawn(async move { kukuri_cn_stun::serve(&stun).await });
    let dir = tempfile::tempdir()?;
    let db = dir.path().join("native.db");
    // 未指定の bind では、WebRTC の候補は既定の経路の IP になる。Firefox は loopback の候補と組を作らない（#1220 AC-5a）。
    let runtime = DesktopRuntime::new_with_config(&db, TransportNetworkConfig::default()).await?;
    consent_to_community_node(&runtime, &stack.base_url).await?;
    let host = ClientHost::from_runtime(dir.path().to_path_buf(), Arc::new(runtime))
        .await
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    let fixture = Arc::new(Fixture {
        gate: NativeGate {
            host,
            startup: ClientStartupState::new(ClientStartupStatus::Ready),
            lock: tokio::sync::Mutex::new(()),
        },
        stack,
        db: db.clone(),
        public_blobs,
        holder: tokio::sync::Mutex::new(None),
    });

    let app = Router::new()
        .route("/fixture/info", get(info))
        .route("/fixture/invoke", post(invoke))
        .route("/fixture/relay-bytes", get(relay_bytes))
        .route("/fixture/payload.png", get(payload_png))
        .route("/fixture/link-preview", post(link_preview))
        .route("/fixture/shutdown", post(shutdown))
        .route("/fixture/account-sync-items", get(account_sync_items))
        .route("/fixture/public-blob-holder", post(public_blob_holder))
        .route(
            "/fixture/public-blob-holder/stop",
            post(stop_public_blob_holder),
        )
        .route(
            "/fixture/page-init.js",
            get(|| async { ([("content-type", "text/javascript")], PAGE_INIT) }),
        )
        .route("/", get(index.clone()))
        // 経路の判定の画像（約 1.7 MiB）を base64 で添えた command を受ける。
        .layer(DefaultBodyLimit::max(8 << 20))
        .with_state(fixture.clone())
        .fallback_service(ServeDir::new(&dist).not_found_service(get(index)))
        .layer(axum::middleware::from_fn(
            move |request: Request, next: Next| {
                let rules = rules.clone();
                async move {
                    let path = request.uri().path().to_owned();
                    let mut response = next.run(request).await;
                    for (pattern, headers) in rules.iter() {
                        if splat_matches(pattern, &path) {
                            response.headers_mut().extend(headers.clone());
                        }
                    }
                    response
                }
            },
        ));
    let listener = tokio::net::TcpListener::bind(web_addr).await?;
    println!("KUKURI_WEB_E2E_READY={web_origin}");
    axum::serve(listener, app).await?;
    drop(dir);
    Ok(())
}

/// 配信の artifact の `_headers`（Cloudflare Pages の形式。ADR 0060 §2、#1220 AC-6）の規則（path の pattern と header）を
/// 読み、本番の header の下で試す。試験の Community Node と relay は http・ws の 127.0.0.1 なので、CSP の connect-src にだけ
/// 足す。
fn artifact_headers(dist: &Path) -> Result<Vec<(String, HeaderMap)>> {
    let mut rules: Vec<(String, HeaderMap)> = Vec::new();
    for line in std::fs::read_to_string(dist.join("_headers"))?.lines() {
        let rule = line.trim();
        if rule.is_empty() || rule.starts_with('#') {
            continue;
        }
        if !line.starts_with(char::is_whitespace) {
            // fixture が当てられるのは、`*` が 1 つまでの path だけ（placeholder や URL の規則は扱わない）。
            anyhow::ensure!(
                rule.starts_with('/') && rule.matches('*').count() <= 1 && !rule.contains(':'),
                "the fixture cannot apply this rule of _headers: {rule}"
            );
            rules.push((rule.to_string(), HeaderMap::new()));
            continue;
        }
        let (name, value) = rule.split_once(':').context("a header of _headers")?;
        let mut value = value.trim().to_string();
        if name.eq_ignore_ascii_case("content-security-policy") {
            value = value.replacen(
                "connect-src",
                "connect-src http://127.0.0.1:* ws://127.0.0.1:*",
                1,
            );
        }
        rules
            .last_mut()
            .context("a header before the path of _headers")?
            .1
            .insert(HeaderName::try_from(name)?, HeaderValue::try_from(value)?);
    }
    Ok(rules)
}

/// `_headers` の path の pattern に一致するか（`*` は任意の文字列に一致する）。
fn splat_matches(pattern: &str, path: &str) -> bool {
    match pattern.split_once('*') {
        Some((prefix, suffix)) => {
            path.len() >= prefix.len() + suffix.len()
                && path.starts_with(prefix)
                && path.ends_with(suffix)
        }
        None => pattern == path,
    }
}

/// harness の接続の scenario と同じ手順で、Community Node の文書に同意して session を張る。
async fn consent_to_community_node(runtime: &DesktopRuntime, base_url: &str) -> Result<()> {
    runtime
        .set_community_node_config(SetCommunityNodeConfigRequest {
            trust_node_priority: None,
            nodes: vec![SetCommunityNodeConfigNode::new(base_url.to_string())],
        })
        .await?;
    let catalog = runtime
        .fetch_community_node_policies(FetchCommunityNodePoliciesRequest {
            base_url: base_url.to_string(),
            language: Some("ja".to_string()),
        })
        .await?;
    let documents = catalog
        .policies
        .iter()
        .map(|policy| CommunityNodeConsentDocumentRef {
            policy_slug: policy.policy_slug.clone(),
            policy_version: policy.policy_version,
            policy_snapshot_revision: policy.policy_snapshot_revision.clone(),
        })
        .collect();
    let accepted = runtime
        .accept_community_node_consents(
            AcceptCommunityNodeConsentsRequest {
                base_url: base_url.to_string(),
                documents,
                language: "ja".to_string(),
            },
            "web-e2e",
        )
        .await?;
    anyhow::ensure!(
        accepted.auth_state.authenticated,
        "native is not authenticated"
    );
    Ok(())
}

fn failed(error: impl std::fmt::Display) -> (StatusCode, Json<Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "message": error.to_string() })),
    )
}

async fn info(State(fixture): State<Shared>) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let status = fixture
        .gate
        .host
        .runtime()
        .get_sync_status()
        .await
        .map_err(failed)?;
    Ok(Json(json!({
        "pubkey": status.local_author_pubkey,
        "endpoint_id": status.discovery.local_endpoint_id,
        // Web が TCP でつなぐ先（Android の emulator は adb の port の転送で届かせる。#1220 AC-5c）。
        "community_node": fixture.stack.base_url,
        "relay_port": fixture.stack.iroh_relay.as_ref().map(|relay| relay.http_addr().port()),
    })))
}

#[derive(Deserialize)]
struct InvokeBody {
    command: String,
    #[serde(default)]
    args: Value,
}

/// native の command を、Tauri・Web と同じ dispatch 表で呼ぶ。
async fn invoke(
    State(fixture): State<Shared>,
    Json(body): Json<InvokeBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    dispatch_command(
        &fixture.gate,
        &DispatchContext::default(),
        &body.command,
        body.args,
    )
    .await
    .map(Json)
    .map_err(|error| failed(format!("{}: {}", error.code, error.message)))
}

async fn relay_bytes(State(fixture): State<Shared>) -> Json<Value> {
    let relayed = fixture
        .stack
        .iroh_relay
        .as_ref()
        .map(SpawnedIrohRelay::relayed_bytes)
        .unwrap_or_default();
    Json(json!({ "relayed_bytes": relayed }))
}

/// native の account 同期の item（採用済みの行の key と値。tombstone は除く）。同期の中身の検査に使う（#1220 AC-3b）。
/// 試験のアカウントの行は少ないので、全部を返す。
async fn account_sync_items(
    State(fixture): State<Shared>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let store = SqliteStore::connect_file(&fixture.db)
        .await
        .map_err(failed)?;
    let mut items = Vec::new();
    for key in store.list_account_sync_keys("").await.map_err(failed)? {
        if let Some(row) = store.get_account_sync_row(&key).await.map_err(failed)? {
            items.push(json!({ "key": row.key, "value": row.value }));
        }
    }
    Ok(Json(Value::Array(items)))
}

/// 投稿・DM に添える画像（driver が Web の投稿欄から添えるか、native の command に base64 で添える）。
async fn payload_png() -> Result<Vec<u8>, (StatusCode, Json<Value>)> {
    random_png(PAYLOAD_IMAGE_SIDE).map_err(failed)
}

#[derive(Deserialize)]
struct LinkPreviewBody {
    object_id: String,
    url: String,
    title: String,
}

/// native が自分の公開投稿のリンクプレビューの record を書く（#1220 AC-2g）。試験の site は ADR 0051 §3 の宛先の制限で
/// 取得できないので OGP は取得せず、表示のときの取得の結果の代わりに試験の題と画像を書く。返り値は画像の data URL（card の
/// 画像との照合用）。
async fn link_preview(
    State(fixture): State<Shared>,
    Json(body): Json<LinkPreviewBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let png = random_png(PREVIEW_IMAGE_SIDE).map_err(failed)?;
    let image_data_url = format!("data:image/png;base64,{}", BASE64_STANDARD.encode(&png));
    fixture
        .gate
        .host
        .runtime()
        .record_link_preview(
            &body.object_id,
            LinkPreviewRecordInput {
                url: body.url,
                title: body.title,
                description: None,
                site_name: "web-e2e".to_string(),
                image: Some(png),
            },
        )
        .await
        .map_err(failed)?;
    Ok(Json(json!({ "image_data_url": image_data_url })))
}

/// native を止める（#1220 AC-2g。投稿者の native が居ないときの中継を確かめる。以後は native を操作できない）。
async fn shutdown(State(fixture): State<Shared>) -> Json<Value> {
    fixture.gate.host.shutdown().await;
    Json(json!({}))
}

#[derive(Deserialize)]
struct PublicBlobHolderBody {
    data_base64: String,
}

/// Web の知らない保持端末（Community Node を使わず、DHT と補助 index と relay だけを使う native）が、画像を添えた公開投稿を
/// 書き、その blob の告知を終えるまで待つ（#1632 AC-6）。返り値は blob の hash。
async fn public_blob_holder(
    State(fixture): State<Shared>,
    Json(body): Json<PublicBlobHolderBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let png = BASE64_STANDARD.decode(&body.data_base64).map_err(failed)?;
    let hash = blake3::hash(&png).to_hex().to_string();
    let db = fixture.db.with_file_name("holder.db");
    let relay = fixture.stack.iroh_relay.as_ref();
    let runtime = DesktopRuntime::new_with_public_blob_discovery(
        &db,
        DhtDiscoveryOptions {
            enabled: true,
            dht_builder: Some(fixture.public_blobs.dht()),
            public_blob_index: Some(fixture.public_blobs.index()),
            ..DhtDiscoveryOptions::default()
        },
        relay
            .map(|relay| format!("http://{}", relay.http_addr()))
            .into_iter()
            .collect(),
    )
    .await
    .map_err(failed)?;
    runtime
        .create_post(CreatePostRequest {
            topic: "kukuri:topic:public-blob-holder".to_string(),
            content: "public blob holder".to_string(),
            reply_to: None,
            channel_ref: ChannelRef::Public,
            attachments: vec![CreateAttachmentRequest {
                file_name: Some("payload.png".to_string()),
                mime: "image/png".to_string(),
                byte_size: png.len() as u64,
                data_base64: body.data_base64,
                role: Some("image_original".to_string()),
            }],
            content_labels: Vec::new(),
        })
        .await
        .map_err(failed)?;
    *fixture.holder.lock().await = Some(runtime);
    // 告知に成功すると、次の更新は 10 分後へ移る。
    let store = SqliteStore::connect_file(&db).await.map_err(failed)?;
    timeout(Duration::from_secs(120), async {
        loop {
            let soon = i64::try_from(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_millis(),
            )? + 9 * 60 * 1000;
            let (due, next_at) = store.due_public_blob_announcements(soon, 1).await?;
            if due.is_empty() && next_at.is_some() {
                return anyhow::Ok(());
            }
            sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .map_err(failed)?
    .map_err(failed)?;
    Ok(Json(json!({ "hash": hash })))
}

async fn stop_public_blob_holder(State(fixture): State<Shared>) -> Json<Value> {
    if let Some(runtime) = fixture.holder.lock().await.take() {
        runtime.shutdown().await;
    }
    Json(json!({}))
}

/// 毎回違う内容（違う blob の hash）にする。同じ内容だと、受け手が前に取得した blob を使い、経路を判定できない。
fn random_png(side: u32) -> Result<Vec<u8>> {
    let mut state = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos() as u64
        | 1;
    let pixels = (0..side * side * 3)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect::<Vec<_>>();
    let image = image::RgbImage::from_raw(side, side, pixels).context("payload image")?;
    let mut png = Vec::new();
    image.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)?;
    Ok(png)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_artifact_headers_apply_by_their_paths() {
        let dist = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps/desktop/web-public");
        let rules = artifact_headers(&dist).expect("the _headers of the web build");
        let applied = |path: &str| {
            rules
                .iter()
                .filter(|(pattern, _)| splat_matches(pattern, path))
                .flat_map(|(_, headers)| headers.keys().map(|name| name.as_str().to_owned()))
                .collect::<Vec<_>>()
        };
        assert_eq!(applied("/"), ["content-security-policy"]);
        assert_eq!(
            applied("/assets/kukuri_web_runtime_bg-x.wasm"),
            ["content-security-policy", "content-type"]
        );
        let csp = rules[0].1["content-security-policy"]
            .to_str()
            .expect("an ascii header");
        assert!(
            csp.contains("connect-src http://127.0.0.1:* ws://127.0.0.1:* 'self' https: wss:;")
        );
    }
}
