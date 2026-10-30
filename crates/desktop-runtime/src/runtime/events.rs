//! runtime の event と、画面・CLI・SDK への転送(通知・成人向けの印・関係の変化・リアクション・Live の Push・ゲームの
//! アセット・投稿)。

use std::sync::Arc;

use crate::community_node::CommunityNodeNodeStatus;
use kukuri_app_api::{AppService, PostReceivedView, ReactionReceivedView, SyncStatus};
use kukuri_store::ReactionProjectionRow;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RuntimeEvent {
    NotificationStatusChanged,
    /// 起動の状態が変わった（Web で別の tab に runtime を引き継がれたとき。ADR 0059 §4）。画面は状態を読み直す。
    StartupStatusChanged,
    AdultMediaLabelEvicted {
        hash: Option<String>,
    },
    /// 通信状態の差分(#1221 R2-D)。`sync_status` は件数と、変わった topic だけの `topic_diagnostics`。
    /// CN の node は変わった node だけ。抜けた topic と外した node は `removed_*`。
    SyncStatusChanged {
        sync_status: Option<Box<SyncStatus>>,
        removed_topics: Vec<String>,
        community_node_statuses: Vec<CommunityNodeNodeStatus>,
        removed_community_nodes: Vec<String>,
    },
    /// 自分を指す相手の follow の edge を新しく保存した(#1521 AC-1b)。画面はその相手の開いている列の関係を読み直す。
    /// 自分の pubkey なら、本人の別の端末で変えた自分の profile を採った(#1220 AC-3b。画面は自分の profile を読み直す)。
    /// 知らせが溢れたときは `None`(開いている列と自分の profile を読み直す)。
    AuthorRelationshipChanged {
        pubkey: Option<String>,
    },
    /// 手元へ反映したリアクションの新しい版(#1841)。Live の表示が新しく届いたものを浮かべる。外した版と非表示の作者のものは
    /// 送らない。溢れた分は送り直さない。中身は大きいので box に置く(event を持つ受け手の大きさを増やさない)。
    ReactionReceived {
        reaction: Box<ReactionReceivedView>,
    },
    LiveOverlayPushReceived {
        push: Option<Box<kukuri_app_api::LiveOverlayPushView>>,
    },
    /// 手元へ新しく保存した投稿・返信(#1981。SDK の `post_received`)。古いものと非表示の作者のものは送らない。知らせが溢れた
    /// ときは `None`(受け手は表示中の timeline・thread を読み直す)。
    PostReceived {
        post: Option<Box<PostReceivedView>>,
    },
    /// SDK のゲームの session が閉じた(#1978)。理由は `left`・`room_ended`・`access_revoked`・`host_unavailable`。
    GameSessionClosed {
        session_id: String,
        room_id: String,
        reason: String,
    },
    /// host の端末で SDK のゲームの session を受け付け、送受信を始めた(#1979)。
    GameSessionOpened {
        session_id: String,
        room_id: String,
        participant_pubkey: String,
    },
    /// 検証できるゲームのアセットの新しい release が届いた(#1980)。この端末で適用中のアセットは取得・適用した後に `path`
    /// 付きで、まだ適用していないアセットは `path` 無しで届く。
    GameAssetUpdated {
        publisher_pubkey: String,
        application_id: String,
        asset_id: String,
        revision: u64,
        path: Option<String>,
    },
    /// session の実データの経路(`direct_p2p`・`relay_supported_p2p`・`relay_fallback`)か RTT が変わった(#1979)。
    /// 送受信量はその時点の接続の UDP の bytes。
    GameSessionPath {
        session_id: String,
        path: String,
        rtt_ms: u64,
        sent_bytes: u64,
        received_bytes: u64,
    },
}

/// #1841: 画面へ送るリアクションの新着の上限(1 秒あたり)。画面が浮かべる同時の数と待ちの数の和で、表示できる分は減らさない。
/// event の channel は全種類で共有し、溢れると画面へ読み直し(画像の解放・関係の読み直し)を促すので、照合で多くのリアクションを
/// まとめて反映しても溢れさせない。
pub(crate) const REACTION_EVENTS_PER_SECOND: u32 = 36;

/// 1 秒の区切りは単調な時計で数える(端末の時計を戻しても止まらない)。
#[derive(Default)]
pub(crate) struct ReactionEventBudget {
    window_started_at: Option<n0_future::time::Instant>,
    sent: u32,
}

impl ReactionEventBudget {
    pub(crate) fn take(&mut self, now: n0_future::time::Instant) -> bool {
        if self
            .window_started_at
            .is_none_or(|started| now.saturating_duration_since(started).as_millis() >= 1_000)
        {
            self.window_started_at = Some(now);
            self.sent = 0;
        }
        if self.sent >= REACTION_EVENTS_PER_SECOND {
            return false;
        }
        self.sent += 1;
        true
    }
}

/// runtime の event を画面へ送る(通知・成人向けの印・関係の変化・#1841 のリアクション)。
///
/// 通知の Notify(`notify_waiters`)は待っている相手がいないと知らせが残らないので、待っている相手として登録し続ける。リアクションの
/// 分岐は `reaction_view`(非表示の作者を store から読む)を待つので、その間に来た通知も受けられるようにする。broadcast の 3 つは
/// 溜まる(#1521 AC-1b)。リアクションは溢れた分を送り直さない(浮かべる演出だけが使い、数は投稿の読み直しで揃う)。
pub(crate) async fn forward_runtime_events<V, F>(
    notify: Arc<tokio::sync::Notify>,
    mut label_evictions: tokio::sync::broadcast::Receiver<String>,
    mut relationship_changes: tokio::sync::broadcast::Receiver<String>,
    mut reaction_changes: tokio::sync::broadcast::Receiver<ReactionProjectionRow>,
    reaction_view: V,
    sender: tokio::sync::broadcast::Sender<RuntimeEvent>,
) where
    V: Fn(ReactionProjectionRow) -> F,
    F: std::future::Future<Output = Option<ReactionReceivedView>>,
{
    use tokio::sync::broadcast::error::RecvError;
    let mut reaction_budget = ReactionEventBudget::default();
    let notified = notify.notified();
    tokio::pin!(notified);
    notified.as_mut().enable();
    loop {
        tokio::select! {
            biased;
            _ = &mut notified => {
                notified.set(notify.notified());
                notified.as_mut().enable();
                let _ = sender.send(RuntimeEvent::NotificationStatusChanged);
            }
            label = label_evictions.recv() => match label {
                Ok(hash) => {
                    let _ = sender.send(RuntimeEvent::AdultMediaLabelEvicted { hash: Some(hash) });
                }
                Err(RecvError::Lagged(_)) => {
                    let _ = sender.send(RuntimeEvent::AdultMediaLabelEvicted { hash: None });
                }
                Err(RecvError::Closed) => break,
            },
            change = relationship_changes.recv() => match change {
                Ok(pubkey) => {
                    let _ = sender.send(RuntimeEvent::AuthorRelationshipChanged { pubkey: Some(pubkey) });
                }
                Err(RecvError::Lagged(_)) => {
                    let _ = sender.send(RuntimeEvent::AuthorRelationshipChanged { pubkey: None });
                }
                Err(RecvError::Closed) => break,
            },
            change = reaction_changes.recv() => match change {
                Ok(row) => {
                    if let Some(reaction) = reaction_view(row).await
                        && reaction_budget.take(n0_future::time::Instant::now())
                    {
                        let _ = sender.send(RuntimeEvent::ReactionReceived { reaction: Box::new(reaction) });
                    }
                }
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => break,
            },
        }
    }
}

/// runtime の event を `sender` へ転送する task を起こす。
pub(crate) fn spawn_event_forwarding(
    app_service: &Arc<AppService>,
    store: &dyn kukuri_store::AccountStore,
    event_sender: tokio::sync::broadcast::Sender<RuntimeEvent>,
) -> n0_future::task::JoinHandle<()> {
    let reactions_app = Arc::downgrade(app_service);
    let notifications = forward_runtime_events(
        app_service.notification_inserted_notify(),
        store.subscribe_adult_label_evictions(),
        app_service.subscribe_author_relationship_changes(),
        app_service.subscribe_reaction_changes(),
        move |row| {
            let app = reactions_app.clone();
            async move {
                app.upgrade()?
                    .reaction_received_view(&row)
                    .await
                    .ok()
                    .flatten()
            }
        },
        event_sender.clone(),
    );
    let mut pushes = app_service.subscribe_live_overlay_pushes();
    let push_sender = event_sender.clone();
    let mut assets = app_service.subscribe_game_asset_updates();
    let asset_sender = event_sender.clone();
    let mut posts = app_service.subscribe_post_changes();
    let post_sender = event_sender.clone();
    let posts_app = Arc::downgrade(app_service);
    n0_future::task::spawn(async move {
        // reactionの表示データの読みを待つ間も、短命のPushを画面へ渡す。
        let forward_pushes = async move {
            loop {
                let push = match pushes.recv().await {
                    Ok(push) => push.map(Box::new),
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => None,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                };
                let _ = push_sender.send(RuntimeEvent::LiveOverlayPushReceived { push });
            }
        };
        // ゲームのアセットの知らせは溢れた分を送り直さない(受け手は一覧を読み直せる。ADR 0070 §3)。
        let forward_assets = async move {
            loop {
                match assets.recv().await {
                    Ok(update) => {
                        let _ = asset_sender.send(RuntimeEvent::GameAssetUpdated {
                            publisher_pubkey: update.publisher_pubkey,
                            application_id: update.application_id,
                            asset_id: update.asset_id,
                            revision: update.revision,
                            path: update.path,
                        });
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        };
        let forward_posts = async move {
            loop {
                let post = match posts.recv().await {
                    Ok(row) => {
                        let Some(app) = posts_app.upgrade() else {
                            break;
                        };
                        match app.post_received_view(&row).await {
                            Ok(Some(post)) => Some(Box::new(post)),
                            _ => continue,
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => None,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                };
                let _ = post_sender.send(RuntimeEvent::PostReceived { post });
            }
        };
        tokio::select! {
            _ = notifications => {}, _ = forward_pushes => {}, _ = forward_assets => {}, _ = forward_posts => {}
        }
    })
}
