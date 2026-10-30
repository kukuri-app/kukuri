//! #1978 AC-2: SDK のゲームの action session の host への接続(実 iroh の 2 node)。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::Result;
use tokio::sync::Mutex;

use crate::{
    GAME_SESSION_HOST_UNAVAILABLE, GAME_SESSION_LEFT, GameSessionAuthorizer, GameSessionChannel,
    GameSessionDenied, GameSessionGrant, HostedGameSession, IrohDocsNode,
};

/// host の端末で受け付けた session の送受信。
pub(super) type HostChannel = Arc<std::sync::Mutex<Option<Arc<GameSessionChannel>>>>;

/// 落とされた(host が session を記録から消した)回数を数え、送受信を `channel` に置く。
struct Recorded(Arc<AtomicUsize>, HostChannel);

impl HostedGameSession for Recorded {
    fn opened(&mut self, channel: Arc<GameSessionChannel>) {
        *self.1.lock().expect("host channel") = Some(channel);
    }

    fn closed(self: Box<Self>, _reason: &str) {}
}

impl Drop for Recorded {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

/// `join` は受け付け、他は `access_denied` で断る。受け付けた session は `revoked` に理由が入るまで許す。
fn authorizer(
    released: Arc<AtomicUsize>,
    revoked: Arc<Mutex<Option<String>>>,
) -> GameSessionAuthorizer {
    authorizer_with(released, revoked, HostChannel::default())
}

pub(super) fn authorizer_with(
    released: Arc<AtomicUsize>,
    revoked: Arc<Mutex<Option<String>>>,
    channel: HostChannel,
) -> GameSessionAuthorizer {
    Arc::new(move |_remote, request| {
        let released = released.clone();
        let revoked = revoked.clone();
        let channel = channel.clone();
        Box::pin(async move {
            if request != b"join" {
                return Err("access_denied".to_string());
            }
            Ok(GameSessionGrant {
                session_id: "session-1".into(),
                revalidate: Arc::new(move || {
                    let revoked = revoked.clone();
                    Box::pin(async move { revoked.lock().await.clone() })
                }),
                session: Box::new(Recorded(released, channel)),
            })
        })
    })
}

pub(super) async fn pair() -> Result<(Arc<IrohDocsNode>, Arc<IrohDocsNode>, String)> {
    let host = IrohDocsNode::memory().await?;
    let participant = IrohDocsNode::memory().await?;
    participant
        .discovery()
        .add_endpoint_info(host.endpoint().addr());
    let host_id = host.endpoint().id().to_string();
    Ok((host, participant, host_id))
}

pub(super) async fn eventually(released: &AtomicUsize, expected: usize) {
    for _ in 0..100 {
        if released.load(Ordering::SeqCst) == expected {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(released.load(Ordering::SeqCst), expected);
}

/// 受け付けた参加は session ID を受け取り、退出すると host の記録が消える。断られた参加は理由の code だけを受け取る。
#[tokio::test]
async fn a_granted_join_lasts_until_the_participant_leaves_and_a_denial_carries_its_code()
-> Result<()> {
    let (host, participant, host_id) = pair().await?;
    let released = Arc::new(AtomicUsize::new(0));
    host.install_game_session_authorizer(Some(authorizer(released.clone(), Arc::default())));

    let denied = participant
        .game_session_join(&host_id, b"other")
        .await
        .err()
        .expect("denied");
    assert_eq!(
        denied
            .downcast_ref::<GameSessionDenied>()
            .map(|denied| denied.0.as_str()),
        Some("access_denied")
    );

    let link = participant.game_session_join(&host_id, b"join").await?;
    assert_eq!(link.session_id(), "session-1");
    assert!(link.is_open());
    assert_eq!(
        released.load(Ordering::SeqCst),
        0,
        "the host keeps the session"
    );
    link.leave();
    assert_eq!(link.closed().await, GAME_SESSION_LEFT);
    assert!(!link.is_open());
    eventually(&released, 1).await;

    participant.shutdown().await?;
    host.shutdown().await?;
    Ok(())
}

/// host が評価し直して許さなくなったら、理由の code を付けて閉じ、参加者はその code を受け取る。
#[tokio::test]
async fn the_host_closes_a_session_with_the_reason_once_it_is_no_longer_allowed() -> Result<()> {
    let (host, participant, host_id) = pair().await?;
    let released = Arc::new(AtomicUsize::new(0));
    let revoked = Arc::new(Mutex::new(None));
    host.install_game_session_authorizer(Some(authorizer(released.clone(), revoked.clone())));
    let link = participant.game_session_join(&host_id, b"join").await?;
    *revoked.lock().await = Some("room_ended".into());
    let reason = tokio::time::timeout(Duration::from_secs(15), link.closed()).await?;
    assert_eq!(reason, "room_ended");
    eventually(&released, 1).await;

    participant.shutdown().await?;
    host.shutdown().await?;
    Ok(())
}

/// host の端末が止まったら、参加者は `host_unavailable` を受け取る。
#[tokio::test]
async fn a_stopped_host_ends_the_session_as_unavailable() -> Result<()> {
    let (host, participant, host_id) = pair().await?;
    host.install_game_session_authorizer(Some(authorizer(Arc::default(), Arc::default())));
    let link = participant.game_session_join(&host_id, b"join").await?;
    host.shutdown().await?;
    let reason = tokio::time::timeout(Duration::from_secs(60), link.closed()).await?;
    assert_eq!(reason, GAME_SESSION_HOST_UNAVAILABLE);
    participant.shutdown().await?;
    Ok(())
}
