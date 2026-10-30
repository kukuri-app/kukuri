//! SDK のゲームの action session の記録(ADR 0070 §5.2、#1978 AC-2・#1979 AC-1)。host の端末で受け付けた session と、
//! 参加者の端末で参加中の session を持つ。開いた session だけが packet を送受信でき、閉じたら記録を消して
//! `game_session_closed` を知らせる。

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex, Weak};
use std::time::Duration;

use anyhow::{Result, anyhow};
use kukuri_app_api::{AppService, GAME_SESSION_ROOM_FULL, GameSessionRejected};
use kukuri_iroh_node::{
    GameDelivery, GamePacket, GameSendRefused, GameSessionAuthorizer, GameSessionChannel,
    GameSessionDenied, GameSessionGrant, GameSessionLink, GameSessionPath, HostedGameSession,
    IrohDocsNode,
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::RuntimeEvent;

/// 参加者の端末が同時に参加する session の上限。
const MAX_JOINED_SESSIONS: usize = 8;
const SESSION_LIMIT: &str = "session_limit";
const HOST_UNREACHABLE: &str = "host_unreachable";
/// 経路と RTT を確かめる間隔。変わったときだけ知らせる。
const PATH_CHECK_INTERVAL: Duration = Duration::from_secs(1);

struct HostedEntry {
    room_id: String,
    /// 参加の応答を返して送受信を始めるまでは None。
    channel: Option<Arc<GameSessionChannel>>,
}

#[derive(Default)]
struct Ledger {
    /// host の端末で受け付けた session。
    hosted: HashMap<String, HostedEntry>,
    /// 参加者の端末で参加中の session(session ID → room ID と接続、参加の枠)。
    joined: HashMap<String, (String, Arc<GameSessionLink>, OwnedSemaphorePermit)>,
}

impl Ledger {
    fn channel(&self, session_id: &str) -> Option<Arc<GameSessionChannel>> {
        match self.joined.get(session_id) {
            Some((_, link, _)) => Some(link.channel().clone()),
            None => self.hosted.get(session_id)?.channel.clone(),
        }
    }
}

pub(crate) struct GameSessions {
    ledger: Arc<StdMutex<Ledger>>,
    /// 参加の枠。参加を始める前に取り、記録が消えるまで持つ。
    join_slots: Arc<Semaphore>,
    /// 取り出しを始める session の位置(取り出しの上限で後ろの session が待たされ続けないように回す)。
    next_take: AtomicUsize,
    events: tokio::sync::broadcast::Sender<RuntimeEvent>,
}

/// host の端末で受け付けた session。閉じたら(または応答の前に落ちたら)記録を消す。
struct Hosted {
    ledger: Weak<StdMutex<Ledger>>,
    events: tokio::sync::broadcast::Sender<RuntimeEvent>,
    session_id: String,
    room_id: String,
    participant_pubkey: String,
}

impl HostedGameSession for Hosted {
    fn opened(&mut self, channel: Arc<GameSessionChannel>) {
        if let Some(ledger) = self.ledger.upgrade()
            && let Some(entry) = ledger
                .lock()
                .expect("game session ledger")
                .hosted
                .get_mut(&self.session_id)
        {
            entry.channel = Some(channel.clone());
        }
        let _ = self.events.send(RuntimeEvent::GameSessionOpened {
            session_id: self.session_id.clone(),
            room_id: self.room_id.clone(),
            participant_pubkey: self.participant_pubkey.clone(),
        });
        watch_path(&channel, self.session_id.clone(), self.events.clone());
    }

    /// 記録を消してから知らせる(閉じた知らせの後に、その session の packet を取り出させない)。
    fn closed(self: Box<Self>, reason: &str) {
        self.forget();
        let _ = self.events.send(RuntimeEvent::GameSessionClosed {
            session_id: self.session_id.clone(),
            room_id: self.room_id.clone(),
            reason: reason.into(),
        });
    }
}

impl Hosted {
    fn forget(&self) {
        if let Some(ledger) = self.ledger.upgrade() {
            ledger
                .lock()
                .expect("game session ledger")
                .hosted
                .remove(&self.session_id);
        }
    }
}

impl Drop for Hosted {
    fn drop(&mut self) {
        self.forget();
    }
}

/// 経路が変わった・RTT が大きく(5 ms 以上かつ 2 割以上)変わったときに、送受信量とともに知らせる(最初は開いたとき)。
fn watch_path(
    channel: &Arc<GameSessionChannel>,
    session_id: String,
    events: tokio::sync::broadcast::Sender<RuntimeEvent>,
) {
    let channel = Arc::downgrade(channel);
    n0_future::task::spawn(async move {
        let mut last: Option<GameSessionPath> = None;
        loop {
            let Some(open) = channel.upgrade().filter(|channel| channel.is_open()) else {
                return;
            };
            // 選ばれた経路が無い間(開いた直後・切り替えの途中)は知らせない。
            let current = open.path();
            drop(open);
            let Some(path) = current else {
                n0_future::time::sleep(PATH_CHECK_INTERVAL).await;
                continue;
            };
            let changed = last.as_ref().is_none_or(|last| {
                let moved = path.rtt_ms.abs_diff(last.rtt_ms);
                last.path != path.path || (moved >= 5 && moved * 5 >= last.rtt_ms)
            });
            if changed {
                let _ = events.send(RuntimeEvent::GameSessionPath {
                    session_id: session_id.clone(),
                    path: path.path.into(),
                    rtt_ms: path.rtt_ms,
                    sent_bytes: path.sent_bytes,
                    received_bytes: path.received_bytes,
                });
                last = Some(path);
            }
            n0_future::time::sleep(PATH_CHECK_INTERVAL).await;
        }
    });
}

fn new_session_id() -> String {
    format!("gs-{}", uuid::Uuid::new_v4().simple())
}

impl GameSessions {
    pub(crate) fn new(events: tokio::sync::broadcast::Sender<RuntimeEvent>) -> Arc<Self> {
        Arc::new(Self {
            ledger: Arc::default(),
            join_slots: Arc::new(Semaphore::new(MAX_JOINED_SESSIONS)),
            next_take: AtomicUsize::new(0),
            events,
        })
    }

    /// host の受け口の認可。公開範囲と room の状態は app が、人数の上限(host を含めて最大人数)はこの記録が確かめる。
    pub(crate) fn authorizer(self: &Arc<Self>, app: Weak<AppService>) -> GameSessionAuthorizer {
        let ledger = Arc::downgrade(&self.ledger);
        let events = self.events.clone();
        Arc::new(move |_remote, request| {
            let (app, ledger, events) = (app.clone(), ledger.clone(), events.clone());
            Box::pin(async move {
                let unavailable = || kukuri_app_api::GAME_SESSION_ROOM_NOT_FOUND.to_string();
                let app_strong = app.upgrade().ok_or_else(unavailable)?;
                let admitted = app_strong
                    .authorize_game_session(&request)
                    .await
                    .map_err(str::to_string)?;
                let ledger_strong = ledger.upgrade().ok_or_else(unavailable)?;
                let session_id = new_session_id();
                {
                    let mut guard = ledger_strong.lock().expect("game session ledger");
                    let players = guard
                        .hosted
                        .values()
                        .filter(|entry| entry.room_id == admitted.room_id)
                        .count();
                    if players + 1 >= admitted.max_players as usize {
                        return Err(GAME_SESSION_ROOM_FULL.to_string());
                    }
                    guard.hosted.insert(
                        session_id.clone(),
                        HostedEntry {
                            room_id: admitted.room_id.clone(),
                            channel: None,
                        },
                    );
                }
                let session = Box::new(Hosted {
                    ledger,
                    events,
                    session_id: session_id.clone(),
                    room_id: admitted.room_id.clone(),
                    participant_pubkey: admitted.participant.as_str().to_string(),
                });
                Ok(GameSessionGrant {
                    session_id,
                    revalidate: Arc::new(move || {
                        let (app, admitted) = (app.clone(), admitted.clone());
                        Box::pin(async move {
                            match app.upgrade() {
                                Some(app) => app
                                    .game_session_close_reason(&admitted)
                                    .await
                                    .map(str::to_string),
                                None => Some(kukuri_app_api::GAME_SESSION_ROOM_ENDED.to_string()),
                            }
                        })
                    }),
                    session,
                })
            })
        })
    }

    /// room の host へ参加する。断られたら・接続できなければ理由の code の [`GameSessionRejected`]。
    pub(crate) async fn join(
        &self,
        app: &AppService,
        node: &IrohDocsNode,
        topic_id: &str,
        scope: kukuri_core::TimelineScope,
        room_id: &str,
    ) -> Result<String> {
        let slot = self
            .join_slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| anyhow!(GameSessionRejected(SESSION_LIMIT.into())))?;
        let join = app
            .game_session_join_request(topic_id, scope, room_id)
            .await?;
        let link = node
            .game_session_join(&join.host_endpoint_id, &join.request)
            .await
            .map_err(|error| {
                let code = error
                    .downcast_ref::<GameSessionDenied>()
                    .map_or(HOST_UNREACHABLE.to_string(), |denied| denied.0.clone());
                anyhow!(GameSessionRejected(code))
            })?;
        let link = Arc::new(link);
        let session_id = link.session_id().to_string();
        self.ledger
            .lock()
            .expect("game session ledger")
            .joined
            .insert(
                session_id.clone(),
                (room_id.to_string(), link.clone(), slot),
            );
        watch_path(link.channel(), session_id.clone(), self.events.clone());
        // 閉じるまで待ち、記録を消して知らせる(上限つきの参加中の session ごとに 1 つ。退出でも閉じる)。
        let (ledger, events, room_id) = (
            Arc::downgrade(&self.ledger),
            self.events.clone(),
            room_id.to_string(),
        );
        n0_future::task::spawn(async move {
            let reason = link.closed().await;
            if let Some(ledger) = ledger.upgrade() {
                ledger
                    .lock()
                    .expect("game session ledger")
                    .joined
                    .remove(link.session_id());
            }
            let _ = events.send(RuntimeEvent::GameSessionClosed {
                session_id: link.session_id().to_string(),
                room_id,
                reason,
            });
        });
        Ok(session_id)
    }

    /// 参加中の session から退出する(知らせは閉じた後に `left` で届く)。
    pub(crate) fn leave(&self, session_id: &str) {
        let link = self
            .ledger
            .lock()
            .expect("game session ledger")
            .joined
            .get(session_id)
            .map(|(_, link, _)| link.clone());
        if let Some(link) = link {
            link.leave();
        }
    }

    /// 開いている session に packet を送る。記録に無い(閉じた・知らない)session は `Closed`。
    pub(crate) fn send(
        &self,
        session_id: &str,
        delivery: GameDelivery,
        bytes: &[u8],
    ) -> std::result::Result<(), GameSendRefused> {
        let channel = self
            .ledger
            .lock()
            .expect("game session ledger")
            .channel(session_id);
        channel
            .ok_or(GameSendRefused::Closed)?
            .send(delivery, bytes)
    }

    /// 開いている session の受け取った packet を、`fits` が許す間だけ取り出す。`accept` は [`GameSessionChannel::take`]。
    pub(crate) fn take(
        &self,
        accept: bool,
        mut fits: impl FnMut(usize) -> bool,
    ) -> Vec<(String, GamePacket)> {
        let mut channels: Vec<(String, Arc<GameSessionChannel>)> = {
            let ledger = self.ledger.lock().expect("game session ledger");
            let joined = ledger
                .joined
                .iter()
                .map(|(id, (_, link, _))| (id.clone(), link.channel().clone()));
            let hosted = ledger
                .hosted
                .iter()
                .filter_map(|(id, entry)| Some((id.clone(), entry.channel.as_ref()?.clone())));
            joined.chain(hosted).collect()
        };
        if !channels.is_empty() {
            let start = self.next_take.fetch_add(1, Ordering::Relaxed) % channels.len();
            channels.rotate_left(start);
        }
        let mut packets = Vec::new();
        for (session_id, channel) in channels {
            for packet in channel.take(accept, &mut fits) {
                packets.push((session_id.clone(), packet));
            }
        }
        packets
    }

    /// 送受信を認可してよい session か(参加者の端末で参加中、または host の端末で受け付け中)。
    #[cfg(test)]
    pub(crate) fn is_open(&self, session_id: &str) -> bool {
        let ledger = self.ledger.lock().expect("game session ledger");
        ledger.hosted.contains_key(session_id)
            || ledger
                .joined
                .get(session_id)
                .is_some_and(|(_, link, _)| link.is_open())
    }
}
