//! SDK のゲームの action session の記録(ADR 0070 §5.2、#1978 AC-2)。host の端末で受け付けた session と、参加者の
//! 端末で参加中の session を持ち、閉じたら記録を消して `game_session_closed` を知らせる。

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex, Weak};

use anyhow::{Result, anyhow};
use kukuri_app_api::{AppService, GAME_SESSION_ROOM_FULL, GameSessionRejected};
use kukuri_iroh_node::{
    GameSessionAuthorizer, GameSessionDenied, GameSessionGrant, GameSessionLink, IrohDocsNode,
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::RuntimeEvent;

/// 参加者の端末が同時に参加する session の上限。
const MAX_JOINED_SESSIONS: usize = 8;
const SESSION_LIMIT: &str = "session_limit";
const HOST_UNREACHABLE: &str = "host_unreachable";

#[derive(Default)]
struct Ledger {
    /// host の端末で受け付けた session(session ID → room ID)。
    hosted: HashMap<String, String>,
    /// 参加者の端末で参加中の session(session ID → room ID と接続、参加の枠)。
    joined: HashMap<String, (String, Arc<GameSessionLink>, OwnedSemaphorePermit)>,
}

pub(crate) struct GameSessions {
    ledger: Arc<StdMutex<Ledger>>,
    /// 参加の枠。参加を始める前に取り、記録が消えるまで持つ。
    join_slots: Arc<Semaphore>,
    events: tokio::sync::broadcast::Sender<RuntimeEvent>,
}

/// 落とすと(host の接続が閉じると)受け付けた session の記録を消す。
struct Hosted {
    ledger: Weak<StdMutex<Ledger>>,
    session_id: String,
}

impl Drop for Hosted {
    fn drop(&mut self) {
        if let Some(ledger) = self.ledger.upgrade() {
            ledger
                .lock()
                .expect("game session ledger")
                .hosted
                .remove(&self.session_id);
        }
    }
}

fn new_session_id() -> String {
    format!("gs-{}", uuid::Uuid::new_v4().simple())
}

impl GameSessions {
    pub(crate) fn new(events: tokio::sync::broadcast::Sender<RuntimeEvent>) -> Arc<Self> {
        Arc::new(Self {
            ledger: Arc::default(),
            join_slots: Arc::new(Semaphore::new(MAX_JOINED_SESSIONS)),
            events,
        })
    }

    /// host の受け口の認可。公開範囲と room の状態は app が、人数の上限(host を含めて最大人数)はこの記録が確かめる。
    pub(crate) fn authorizer(self: &Arc<Self>, app: Weak<AppService>) -> GameSessionAuthorizer {
        let ledger = Arc::downgrade(&self.ledger);
        Arc::new(move |_remote, request| {
            let (app, ledger) = (app.clone(), ledger.clone());
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
                        .filter(|room| **room == admitted.room_id)
                        .count();
                    if players + 1 >= admitted.max_players as usize {
                        return Err(GAME_SESSION_ROOM_FULL.to_string());
                    }
                    guard
                        .hosted
                        .insert(session_id.clone(), admitted.room_id.clone());
                }
                Ok(GameSessionGrant {
                    session_id: session_id.clone(),
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
                    guard: Box::new(Hosted { ledger, session_id }),
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

    /// 送受信を認可してよい session か(参加者の端末で参加中、または host の端末で受け付け中)。#1979 が使う。
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
