//! SDK のゲームの action session の、host の端末への接続(ADR 0070 §5.2、#1978 AC-2)。
//!
//! 参加者は最初の双方向 stream で参加の要求(proof を含む)を送り、host の認可の応答を読む。認可された接続は session
//! の間ずっと保ち、host は 5 秒ごとに評価し直して、許されなくなったら理由の code を付けて閉じる。参加者が閉じれば
//! 退出。どちらの側も、接続が閉じたことで session の終わりを知る。受け付けた後の送受信は [`GameSessionChannel`]。

use std::sync::{Arc, RwLock as StdRwLock};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use futures_util::future::BoxFuture;
use iroh::EndpointId;
use iroh::endpoint::{Connection, ConnectionError};
use iroh::protocol::{AcceptError, ProtocolHandler};
use n0_future::time::timeout;
use tokio::sync::Semaphore;

use crate::{GameSessionChannel, IrohDocsNode};

pub(crate) const GAME_SESSION_ALPN: &[u8] = b"/kukuri/game-session/1";
/// 参加の要求の上限(private channel の proof の envelope を含む)。
pub const GAME_SESSION_REQUEST_MAX_BYTES: usize = 64 * 1024;
/// host が許可を評価し直す間隔(media と同じ。ADR 0070 §5.2)。
pub const GAME_SESSION_REVALIDATE_INTERVAL: Duration = Duration::from_secs(5);
/// host が同時に受け付ける session の上限。room ごとの上限(最大人数)は認可が持つ。
const MAX_HOST_SESSIONS: usize = 256;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const RESPONSE_MAX_BYTES: usize = 256;
const GRANTED: &str = "ok";
/// 参加者が退出したときの理由。
pub const GAME_SESSION_LEFT: &str = "left";
/// 理由の code を受け取れないまま接続が切れた(host の端末が止まった・経路が失われた)。
pub const GAME_SESSION_HOST_UNAVAILABLE: &str = "host_unavailable";
/// host が参加者を外した(#1979 AC-2。NGO の `DisconnectRemoteClient`)。
pub const GAME_SESSION_REMOVED: &str = "removed";
const CLOSE_DONE: u32 = 0;
/// 理由の code を付けて閉じる。
pub(crate) const CLOSE_ENDED: u32 = 1;

pub type GameSessionFuture<T> = BoxFuture<'static, T>;
/// 許されている間は `None`、許されなくなったら閉じる理由の code。
pub type GameSessionRevalidate = Arc<dyn Fn() -> GameSessionFuture<Option<String>> + Send + Sync>;

/// host が受け付けた session。
pub struct GameSessionGrant {
    pub session_id: String,
    pub revalidate: GameSessionRevalidate,
    pub session: Box<dyn HostedGameSession>,
}

/// host の端末の、受け付けた session の記録。送受信を始めたら `opened`、閉じたら理由とともに `closed` が呼ばれる。
/// どちらも呼ばれずに落とされたら(応答を返す前に切れた)、記録を消す。
pub trait HostedGameSession: Send + Sync {
    fn opened(&mut self, channel: Arc<GameSessionChannel>);
    fn closed(self: Box<Self>, reason: &str);
}

/// 参加の要求の bytes と相手の endpoint から、受け付けた session か拒否の理由の code を返す。
pub type GameSessionAuthorizer = Arc<
    dyn Fn(EndpointId, Vec<u8>) -> GameSessionFuture<std::result::Result<GameSessionGrant, String>>
        + Send
        + Sync,
>;

/// host が参加を断った(理由の code)。接続できないときは別の error。
#[derive(Debug)]
pub struct GameSessionDenied(pub String);

impl std::fmt::Display for GameSessionDenied {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "the game session host denied the join: {}",
            self.0
        )
    }
}

impl std::error::Error for GameSessionDenied {}

#[derive(Clone)]
pub(crate) struct GameSessionSlot {
    authorizer: Arc<StdRwLock<Option<GameSessionAuthorizer>>>,
    sessions: Arc<Semaphore>,
}

impl Default for GameSessionSlot {
    fn default() -> Self {
        Self {
            authorizer: Arc::default(),
            sessions: Arc::new(Semaphore::new(MAX_HOST_SESSIONS)),
        }
    }
}

impl std::fmt::Debug for GameSessionSlot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GameSessionSlot")
            .finish_non_exhaustive()
    }
}

impl GameSessionSlot {
    pub(crate) fn install(&self, authorizer: Option<GameSessionAuthorizer>) {
        *self
            .authorizer
            .write()
            .expect("game session authorizer poisoned") = authorizer;
    }

    fn authorizer(&self) -> Option<GameSessionAuthorizer> {
        self.authorizer
            .read()
            .expect("game session authorizer poisoned")
            .clone()
    }
}

impl ProtocolHandler for GameSessionSlot {
    async fn accept(&self, connection: Connection) -> std::result::Result<(), AcceptError> {
        let Ok(_permit) = self.sessions.try_acquire() else {
            connection.close(CLOSE_DONE.into(), b"busy");
            return Ok(());
        };
        let Some(authorizer) = self.authorizer() else {
            connection.close(CLOSE_DONE.into(), b"unavailable");
            return Ok(());
        };
        let Ok(Ok(Some(GameSessionGrant {
            revalidate,
            mut session,
            ..
        }))) = timeout(HANDSHAKE_TIMEOUT, admit(&connection, &authorizer)).await
        else {
            // 拒否の code を相手が読んで閉じるまで待つ(先に閉じると応答が届かない)。
            let _ = timeout(HANDSHAKE_TIMEOUT, connection.closed()).await;
            connection.close(CLOSE_DONE.into(), b"denied");
            return Ok(());
        };
        let revoked = async {
            let mut every = n0_future::time::interval(GAME_SESSION_REVALIDATE_INTERVAL);
            every.tick().await;
            loop {
                every.tick().await;
                if let Some(reason) = revalidate().await {
                    return reason;
                }
            }
        };
        session.opened(Arc::new(GameSessionChannel::new(connection.clone())));
        // host が外した([`GameSessionChannel::close`])のでなければ、参加者が閉じた・接続が失われたので退出として扱う。
        let reason = tokio::select! {
            closed = connection.closed() => match closed {
                ConnectionError::LocallyClosed => GAME_SESSION_REMOVED.to_string(),
                _ => GAME_SESSION_LEFT.to_string(),
            },
            reason = revoked => {
                connection.close(CLOSE_ENDED.into(), reason.as_bytes());
                reason
            }
        };
        session.closed(&reason);
        Ok(())
    }
}

/// 参加の要求を読み、受け付けたら `ok\n<session id>` を、断ったら理由の code を返す。
async fn admit(
    connection: &Connection,
    authorizer: &GameSessionAuthorizer,
) -> Result<Option<GameSessionGrant>> {
    let (mut send, mut recv) = connection.accept_bi().await?;
    let request = recv.read_to_end(GAME_SESSION_REQUEST_MAX_BYTES).await?;
    let (answer, grant) = match authorizer(connection.remote_id(), request).await {
        Ok(grant) => (format!("{GRANTED}\n{}", grant.session_id), Some(grant)),
        Err(code) => (code, None),
    };
    send.write_all(answer.as_bytes()).await?;
    send.finish()?;
    Ok(grant)
}

/// 参加者の側の、host への session の接続。落とすと退出として閉じる。
pub struct GameSessionLink {
    connection: Connection,
    session_id: String,
    channel: Arc<GameSessionChannel>,
}

impl GameSessionLink {
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn channel(&self) -> &Arc<GameSessionChannel> {
        &self.channel
    }

    /// session が閉じるまで待ち、理由の code を返す(退出なら `left`)。
    pub async fn closed(&self) -> String {
        match self.connection.closed().await {
            ConnectionError::ApplicationClosed(close) if close.error_code == CLOSE_ENDED.into() => {
                String::from_utf8_lossy(&close.reason).into_owned()
            }
            ConnectionError::LocallyClosed => GAME_SESSION_LEFT.into(),
            _ => GAME_SESSION_HOST_UNAVAILABLE.into(),
        }
    }

    pub fn is_open(&self) -> bool {
        self.connection.close_reason().is_none()
    }

    pub fn leave(&self) {
        self.connection
            .close(CLOSE_DONE.into(), GAME_SESSION_LEFT.as_bytes());
    }
}

impl Drop for GameSessionLink {
    fn drop(&mut self) {
        self.leave();
    }
}

impl IrohDocsNode {
    pub fn install_game_session_authorizer(&self, authorizer: Option<GameSessionAuthorizer>) {
        self.game_session.install(authorizer);
    }

    /// `endpoint_id` の host へ参加の要求を送る。断られたら [`GameSessionDenied`]、接続できなければ別の error。
    pub async fn game_session_join(
        &self,
        endpoint_id: &str,
        request: &[u8],
    ) -> Result<GameSessionLink> {
        let id = endpoint_id
            .parse::<EndpointId>()
            .context("invalid game session host endpoint id")?;
        let connection = self
            .connect_direct_first(id, GAME_SESSION_ALPN, "Game session")
            .await?;
        let answer = timeout(HANDSHAKE_TIMEOUT, async {
            let (mut send, mut recv) = connection.open_bi().await?;
            send.write_all(request).await?;
            send.finish()?;
            anyhow::Ok(recv.read_to_end(RESPONSE_MAX_BYTES).await?)
        })
        .await
        .context("the game session host did not answer in time")
        .and_then(|answer| answer);
        let answer = match answer {
            Ok(answer) => String::from_utf8_lossy(&answer).into_owned(),
            Err(error) => {
                connection.close(CLOSE_DONE.into(), b"unanswered");
                return Err(error);
            }
        };
        match answer.split_once('\n') {
            Some((GRANTED, session_id)) if !session_id.is_empty() => Ok(GameSessionLink {
                channel: Arc::new(GameSessionChannel::new(connection.clone())),
                connection,
                session_id: session_id.to_string(),
            }),
            _ => {
                connection.close(CLOSE_DONE.into(), b"denied");
                Err(anyhow!(GameSessionDenied(answer)))
            }
        }
    }
}
