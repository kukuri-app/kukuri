//! Web クライアントの実ブラウザ試験の相手（#1220 W8 AC-2a）。
//!
//! 同じ process で、in-process の Community Node（user-api と iroh relay。Web の配信 origin に CORS で応答する）、
//! Web の build（`dist-web`。その `_headers` の header を付ける）の配信、Community Node に同意した native の相手を
//! 起動する。試験の driver（WebdriverIO）は `/fixture/*` で native を操作し、relay が中継した bytes で実データの経路を
//! 判定する（ADR 0060 §4・§5）。
//!
//! 環境変数:
//! - `KUKURI_WEB_E2E_DIST`: 配信する `dist-web`（必須）
//! - `KUKURI_WEB_E2E_WEB_ADDR`: 配信の listen（既定 `127.0.0.1:4180`）
//! - `KUKURI_WEB_E2E_CN_ADDR`: user-api の listen（既定 `127.0.0.1:4181`。Web の build の Community Node の URL と同じ）
//! - Community Node の Postgres・Redis は harness と同じ（`COMMUNITY_NODE_DATABASE_URL` 等）
//!
//! 準備ができたら標準出力に `KUKURI_WEB_E2E_READY=<配信の origin>` を出す。

use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use kukuri_app_api::LinkPreviewRecordInput;
use kukuri_desktop_runtime::{
    ClientGate, ClientHost, ClientStartupState, ClientStartupStatus, CommandError,
    CommunityNodeConsentDocumentRef, DispatchContext, FetchCommunityNodePoliciesRequest,
    SetCommunityNodeConfigNode, dispatch_command,
};
use serde_json::{Value, json};
use tower_http::services::{ServeDir, ServeFile};

use crate::*;

/// 経路の判定に使う画像の 1 辺（乱数の画素の PNG は圧縮されず、約 1.7 MiB になる）。
const PAYLOAD_IMAGE_SIDE: u32 = 768;
/// リンクプレビューの record の画像の 1 辺（約 12 KiB。record の画像の上限 1 MiB に収まる）。
const PREVIEW_IMAGE_SIDE: u32 = 64;

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
    let headers = artifact_headers(&dist)?;

    let stack =
        CommunityNodeStack::spawn_with("web_e2e", &cn_addr, std::slice::from_ref(&web_origin))
            .await?;
    let dir = tempfile::tempdir()?;
    let db = dir.path().join("native.db");
    let runtime = DesktopRuntime::new_with_config(&db, TransportNetworkConfig::loopback()).await?;
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
    });

    let app = Router::new()
        .route("/fixture/info", get(info))
        .route("/fixture/invoke", post(invoke))
        .route("/fixture/relay-bytes", get(relay_bytes))
        .route("/fixture/payload.png", get(payload_png))
        .route("/fixture/link-preview", post(link_preview))
        .route("/fixture/shutdown", post(shutdown))
        // 経路の判定の画像（約 1.7 MiB）を base64 で添えた command を受ける。
        .layer(DefaultBodyLimit::max(8 << 20))
        .with_state(fixture.clone())
        .fallback_service(
            ServeDir::new(&dist).not_found_service(ServeFile::new(dist.join("index.html"))),
        )
        .layer(axum::middleware::map_response(
            move |mut response: Response| {
                let headers = headers.clone();
                async move {
                    response.headers_mut().extend(headers);
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

/// 配信の artifact の `_headers`（Cloudflare Pages の形式。ADR 0060 §2、#1220 AC-6）の全体（`/*`）の header を読み、
/// 本番の CSP の下で試す。試験の Community Node と relay は http・ws の 127.0.0.1 なので、CSP の connect-src にだけ足す。
fn artifact_headers(dist: &Path) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    for line in std::fs::read_to_string(dist.join("_headers"))?.lines() {
        let rule = line.trim();
        if rule.is_empty() || rule.starts_with('#') {
            continue;
        }
        if !line.starts_with(char::is_whitespace) {
            anyhow::ensure!(
                rule == "/*",
                "the fixture applies only the `/*` rule: {rule}"
            );
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
        headers.insert(HeaderName::try_from(name)?, HeaderValue::try_from(value)?);
    }
    Ok(headers)
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
