//! metaverse の空間音声と live の media の搬送(ADR 0067)。
//!
//! 受け口は、最初の双方向 stream で受けた認可要求を登録された認可へ渡し、許されたときだけ、認可が返した
//! broadcast だけを公開範囲(origin)に置いた moq-lite の session を始める。送る側は 5 秒ごとに認可を
//! 評価し直し、拒否なら接続を閉じる(拒否は理由の code だけを返し、media と他の参加者の識別子を返さない)。
//! 受ける側は endpoint id へ Direct P2P → Relay Supported P2P の順に接続する。

use std::sync::{Arc, RwLock as StdRwLock};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use iroh::EndpointId;
use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler};
use moq_net::bytes::Bytes;
use moq_net::web_transport_trait as wt;
use moq_net::{broadcast, origin};
use n0_future::task::AbortOnDropHandle;
use n0_future::time::timeout;
use tokio::sync::Semaphore;

use crate::IrohDocsNode;

pub(crate) const MEDIA_ALPN: &[u8] = b"/kukuri/media/1";
/// 受け口の同時接続(metaverse の話者が同時に送る相手の上限 64。ADR 0067 §4)。
pub const MEDIA_MAX_INBOUND_SESSIONS: usize = 64;
/// 認可要求の上限(private channel の proof の envelope 2 つを収める)。
pub const MEDIA_ACCESS_REQUEST_MAX_BYTES: usize = 64 * 1024;
/// 認可の応答(`ok` か拒否の code に、改行と添え書きが続く)の上限。live の転送の祖先と子の一覧(ADR 0067 §8)を収める。
const ACCESS_RESPONSE_MAX_BYTES: usize = 4 * 1024;
/// 接続の後の、認可の往復と moq-lite の handshake のそれぞれの上限。
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// 拒否の code を相手が読むまで接続を残す上限。
const DENIAL_LINGER: Duration = Duration::from_secs(2);
/// 送る側が access を評価し直す間隔(ADR 0067 §3)。
pub const MEDIA_REVALIDATE_INTERVAL: Duration = Duration::from_secs(5);
/// 読み書きの無い group を cache から外すまでの時間。判定は半分の間隔ごとなので、残るのは長くても約 2 倍
/// (4 秒)で、media の保持を 10 秒以下にする(ADR 0067 §2)。各 track の最新の group は残る。
const MEDIA_CACHE_IDLE: Duration = Duration::from_secs(2);
const GRANTED: &[u8] = b"ok";
const CLOSE_DONE: u32 = 0;
const CLOSE_BUSY: u32 = 1;
const CLOSE_DENIED: u32 = 2;
/// moq-net の app の close code(`Error::App`)。access が失われた。
const APP_REVOKED: u16 = 1;

/// 認可の future。ブラウザの moq-lite の型は Send でない(受け口は main thread の task で動かす)。
#[cfg(not(target_family = "wasm"))]
pub type MediaFuture<T> = futures_util::future::BoxFuture<'static, T>;
#[cfg(target_family = "wasm")]
pub type MediaFuture<T> = futures_util::future::LocalBoxFuture<'static, T>;

/// 認可を評価し直す。偽なら接続を閉じる。
#[cfg(not(target_family = "wasm"))]
pub type MediaRevalidate = Arc<dyn Fn() -> MediaFuture<bool> + Send + Sync>;
#[cfg(target_family = "wasm")]
pub type MediaRevalidate = Arc<dyn Fn() -> MediaFuture<bool>>;

/// 認可の結果。`publish` には、この相手に見せてよい broadcast だけが載る。`detail` は許可の応答に添える(空なら付けない)。
pub struct MediaGrant {
    pub publish: origin::Consumer,
    pub revalidate: MediaRevalidate,
    pub detail: String,
}

/// 相手の endpoint id と認可要求の bytes から、許可(`MediaGrant`)か拒否の code を返す。
pub type MediaAuthorizer = Arc<
    dyn Fn(EndpointId, Vec<u8>) -> MediaFuture<std::result::Result<MediaGrant, String>>
        + Send
        + Sync,
>;

/// 送る側が拒否した(拒否の理由の code。改行の後に添え書きが続くことがある。接続できない失敗とは区別する)。
#[derive(Debug)]
pub struct MediaDenied(pub String);

impl std::fmt::Display for MediaDenied {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "media access denied: {}", self.0)
    }
}

impl std::error::Error for MediaDenied {}

/// moq-net は `protocol()`(ALPN)から moq-lite の版を選ぶ。kukuri の ALPN の上で、この build の moq-net の
/// moq-lite の版を固定して使う(ADR 0067 §1)。
#[derive(Clone)]
struct LiteSession(web_transport_iroh::Session);

impl wt::Session for LiteSession {
    type SendStream = web_transport_iroh::SendStream;
    type RecvStream = web_transport_iroh::RecvStream;
    type Error = web_transport_iroh::SessionError;

    async fn accept_uni(&self) -> std::result::Result<Self::RecvStream, Self::Error> {
        self.0.accept_uni().await
    }

    async fn accept_bi(
        &self,
    ) -> std::result::Result<(Self::SendStream, Self::RecvStream), Self::Error> {
        self.0.accept_bi().await
    }

    async fn open_bi(
        &self,
    ) -> std::result::Result<(Self::SendStream, Self::RecvStream), Self::Error> {
        self.0.open_bi().await
    }

    async fn open_uni(&self) -> std::result::Result<Self::SendStream, Self::Error> {
        self.0.open_uni().await
    }

    fn send_datagram(&self, payload: Bytes) -> std::result::Result<(), Self::Error> {
        self.0.send_datagram(payload)
    }

    async fn recv_datagram(&self) -> std::result::Result<Bytes, Self::Error> {
        self.0.read_datagram().await
    }

    fn max_datagram_size(&self) -> usize {
        self.0.max_datagram_size()
    }

    fn protocol(&self) -> Option<&str> {
        Some(moq_net::ALPNS[0])
    }

    fn close(&self, code: u32, reason: &str) {
        self.0.close(code, reason.as_bytes());
    }

    async fn closed(&self) -> Self::Error {
        self.0.closed().await
    }
}

/// moq-net の poll 型の transport(web-transport-iroh は async 型だけを実装する)。
type Transport = moq_tokio::transport::Session<LiteSession>;

fn transport(connection: &Connection) -> Transport {
    Transport::new(LiteSession(web_transport_iroh::Session::raw(
        connection.clone(),
    )))
}

/// media の origin の設定(話者の公開と聞き手の購読で共用)。moq-net の既定(30 秒)より早く cache から外す。
pub fn media_origin_config() -> origin::Config {
    let mut config = origin::Config::default();
    config.pool =
        moq_net::cache::Pool::new(moq_net::cache::Config::default().with_expiry(MEDIA_CACHE_IDLE));
    config
}

/// moq-net の driver は tokio の時計(Web は `performance.now()` の時計)で poll される(`moq_net::time::run`)ので、
/// 始点も同じ時計から取る。
fn driver_now() -> moq_net::time::Instant {
    #[cfg(not(target_family = "wasm"))]
    return tokio::time::Instant::now().into_std();
    #[cfg(target_family = "wasm")]
    moq_net::time::Instant::now()
}

#[derive(Clone)]
pub(crate) struct MediaSlot {
    authorizer: Arc<StdRwLock<Option<MediaAuthorizer>>>,
    sessions: Arc<Semaphore>,
}

impl Default for MediaSlot {
    fn default() -> Self {
        Self {
            authorizer: Arc::default(),
            sessions: Arc::new(Semaphore::new(MEDIA_MAX_INBOUND_SESSIONS)),
        }
    }
}

impl std::fmt::Debug for MediaSlot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("MediaSlot").finish_non_exhaustive()
    }
}

impl MediaSlot {
    pub(crate) fn install(&self, authorizer: Option<MediaAuthorizer>) {
        *self.authorizer.write().expect("media authorizer poisoned") = authorizer;
    }

    fn authorizer(&self) -> Option<MediaAuthorizer> {
        self.authorizer
            .read()
            .expect("media authorizer poisoned")
            .clone()
    }
}

impl ProtocolHandler for MediaSlot {
    async fn accept(&self, connection: Connection) -> std::result::Result<(), AcceptError> {
        // ブラウザの moq-lite の session は Send でないので、main thread の task で動かす(ADR 0056 §4)。
        crate::confine_local(self.clone().serve_connection(connection)).await
    }
}

impl MediaSlot {
    async fn serve_connection(
        self,
        connection: Connection,
    ) -> std::result::Result<(), AcceptError> {
        let Ok(_session) = self.sessions.clone().try_acquire_owned() else {
            connection.close(CLOSE_BUSY.into(), b"busy");
            return Ok(());
        };
        let Some(authorizer) = self.authorizer() else {
            connection.close(CLOSE_DENIED.into(), b"unavailable");
            return Ok(());
        };
        let grant = match timeout(HANDSHAKE_TIMEOUT, authorize(&connection, &authorizer)).await {
            Ok(Ok(Some(grant))) => grant,
            Ok(Ok(None)) => {
                // 拒否の code を相手が読んで閉じるまで待つ(先に閉じると応答が届かない)。
                let _ = timeout(DENIAL_LINGER, connection.closed()).await;
                connection.close(CLOSE_DENIED.into(), b"denied");
                return Ok(());
            }
            _ => {
                connection.close(CLOSE_DENIED.into(), b"invalid");
                return Ok(());
            }
        };
        serve(&connection, grant).await;
        connection.close(CLOSE_DONE.into(), b"done");
        Ok(())
    }
}

/// 認可要求を読み、許可なら `ok` を返す。拒否なら理由の code を返して `None`。
async fn authorize(
    connection: &Connection,
    authorizer: &MediaAuthorizer,
) -> Result<Option<MediaGrant>> {
    let (mut send, mut recv) = connection.accept_bi().await?;
    let request = recv.read_to_end(MEDIA_ACCESS_REQUEST_MAX_BYTES).await?;
    match authorizer(connection.remote_id(), request).await {
        Ok(grant) => {
            send.write_all(GRANTED).await?;
            if !grant.detail.is_empty() {
                send.write_all(format!("\n{}", grant.detail).as_bytes())
                    .await?;
            }
            send.finish()?;
            Ok(Some(grant))
        }
        Err(code) => {
            let code = code.into_bytes();
            send.write_all(&code[..code.len().min(ACCESS_RESPONSE_MAX_BYTES)])
                .await?;
            send.finish()?;
            Ok(None)
        }
    }
}

/// 認可した broadcast だけを公開する session を、閉じられるか access が失われるまで動かす。
async fn serve(connection: &Connection, grant: MediaGrant) {
    let accepted = timeout(
        HANDSHAKE_TIMEOUT,
        moq_net::Server::new()
            .with_publisher(grant.publish)
            .accept_lite(driver_now(), transport(connection)),
    )
    .await;
    let Ok(Ok((session, driver))) = accepted else {
        return;
    };
    let run = moq_net::time::run(driver);
    let revoked = async {
        let mut revalidate = n0_future::time::interval(MEDIA_REVALIDATE_INTERVAL);
        revalidate.tick().await;
        loop {
            revalidate.tick().await;
            if !(grant.revalidate)().await {
                session.abort(moq_net::Error::App(APP_REVOKED));
                return;
            }
        }
    };
    // 状態の照会中も、認可済みの搬送と制御を進める。拒否が確定したら直ちに閉じる。
    tokio::select! {
        _ = run => {},
        _ = revoked => {},
    }
}

/// 送る側の端末への購読の接続。落とすと接続を閉じる。
pub struct MediaSubscription {
    connection: Connection,
    announced: origin::Consumer,
    detail: String,
    _session: moq_net::Session,
    _driver: AbortOnDropHandle<()>,
}

impl MediaSubscription {
    /// 許可の応答の添え書き(無ければ空)。
    pub fn detail(&self) -> &str {
        &self.detail
    }

    /// 相手が公開した `path` の broadcast。公開を待ち、session が閉じたら失敗する。
    pub async fn broadcast(&self, path: &str) -> Result<broadcast::Consumer> {
        self.announced
            .routed_broadcast(path)
            .await
            .map_err(|error| anyhow!("media broadcast is not available: {error}"))
    }

    /// 接続が閉じるまで待つ(相手の拒否・停止・切断)。
    pub async fn closed(&self) {
        self.connection.closed().await;
    }

    pub fn remote_id(&self) -> EndpointId {
        self.connection.remote_id()
    }
}

impl Drop for MediaSubscription {
    fn drop(&mut self) {
        self.connection.close(CLOSE_DONE.into(), b"done");
    }
}

impl IrohDocsNode {
    /// media の受け口の認可を付け替える(`None` で外す)。iroh の stack を作り直したときも付け直す。
    pub fn install_media_authorizer(&self, authorizer: Option<MediaAuthorizer>) {
        self.media.install(authorizer);
    }

    /// `endpoint_id` の端末へ接続して認可要求を送り、許されたら moq-lite の session を始める。
    /// 拒否は [`MediaDenied`]、接続できない・期限切れはそれ以外の失敗になる。
    pub async fn media_subscribe(
        &self,
        endpoint_id: &str,
        request: &[u8],
    ) -> Result<MediaSubscription> {
        let id = endpoint_id
            .parse::<EndpointId>()
            .context("invalid media endpoint id")?;
        let connection = self.connect_direct_first(id, MEDIA_ALPN, "media").await?;
        let response = timeout(HANDSHAKE_TIMEOUT, async {
            let (mut send, mut recv) = connection.open_bi().await?;
            send.write_all(request).await?;
            send.finish()?;
            anyhow::Ok(recv.read_to_end(ACCESS_RESPONSE_MAX_BYTES).await?)
        })
        .await
        .context("the media peer did not answer in time")
        .and_then(|response| response);
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                connection.close(CLOSE_DONE.into(), b"done");
                return Err(error);
            }
        };
        let response = String::from_utf8_lossy(&response).into_owned();
        let (code, detail) = response.split_once('\n').unwrap_or((&response, ""));
        if code.as_bytes() != GRANTED {
            connection.close(CLOSE_DONE.into(), b"done");
            return Err(MediaDenied(response).into());
        }
        let detail = detail.to_string();
        let (subscribe, announced_driver) = origin::Producer::new(media_origin_config());
        let connected = timeout(
            HANDSHAKE_TIMEOUT,
            moq_net::Client::new()
                .with_subscriber(subscribe.clone())
                .connect_lite(driver_now(), transport(&connection)),
        )
        .await;
        let (session, driver) = match connected {
            Ok(Ok(connected)) => connected,
            Ok(Err(error)) => {
                connection.close(CLOSE_DONE.into(), b"done");
                return Err(anyhow!("media session handshake failed: {error}"));
            }
            Err(_) => {
                connection.close(CLOSE_DONE.into(), b"done");
                return Err(anyhow!("media session handshake timed out"));
            }
        };
        let task = n0_future::task::spawn(async move {
            let session = moq_net::time::run(driver);
            let announced = moq_net::time::run(announced_driver);
            tokio::pin!(session, announced);
            tokio::select! {
                _ = &mut session => return,
                _ = &mut announced => {}
            }
            session.await;
        });
        Ok(MediaSubscription {
            connection,
            announced: subscribe.consume(),
            detail,
            _session: session,
            _driver: AbortOnDropHandle::new(task),
        })
    }
}
