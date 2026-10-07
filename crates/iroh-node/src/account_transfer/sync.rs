//! 本人の端末どうしの和集合の同期（#1650、ADR 0062 §9）。移行と同じ枠（端末ごとに 1 つ）で待ち受け、本人の端末の
//! 候補へつなぐ。アカウント鍵から導出した証明で互いを確かめたら、両端末が同じ接続で、鍵を除く必須 bundle と投稿の
//! 記録すべてを移行と同じ frame で送り合い、受けたものを取り込む。

use kukuri_core::ACCOUNT_TRANSFER_INVITE_TTL_MS;

use super::*;

impl AccountTransfer {
    /// 本人の端末どうしの和集合の同期（#1650）を待ち受け、`peers`（本人の端末の候補）へつなぐ。前の移行・同期は取り
    /// 消す。同じアカウント鍵を持つ端末とだけつながり、両端末が `source` の必須 bundle（鍵を除く）と投稿の記録すべてを
    /// 送り合って、受けたものを `sink` へ取り込む。
    pub fn sync(
        &self,
        keys: AccountSyncKeys,
        source: Arc<dyn AccountBundleSource>,
        sink: Arc<dyn AccountBundleSink>,
        peers: Vec<EndpointAddr>,
    ) {
        let status = Status::Waiting {
            expires_at_ms: now_ms() + ACCOUNT_TRANSFER_INVITE_TTL_MS,
        };
        let mut session = Session::new(
            Role::Sync,
            Pairing::Account(keys),
            status,
            Some(source),
            None,
        );
        Arc::get_mut(&mut session)
            .expect("the new session is not shared")
            .sink = Some(sink);
        self.replace(Some(session), None);
        self.sync_peers(peers);
    }

    /// 同期を待ち受けている間に、本人の端末の候補へつなぐ（開始と、rendezvous の応答のたび）。同じ相手へは 1 つずつ。
    pub fn sync_peers(&self, peers: Vec<EndpointAddr>) {
        let Some(session) = self.waiting_sync() else {
            return;
        };
        let mut attempts = session
            .attempts
            .lock()
            .expect("account sync attempts poisoned");
        attempts.retain(|_, attempt| !attempt.is_finished());
        for peer in peers {
            if peer.id == self.endpoint.id() || attempts.contains_key(&peer.id) {
                continue;
            }
            let id = peer.id;
            let task = n0_future::task::spawn({
                let (endpoint, session) = (self.endpoint.clone(), session.clone());
                async move {
                    // 取消・期限・完了で状態が終わったら、途中でもやめる。
                    let mut status = session.status.subscribe();
                    let ended = status.wait_for(Status::is_terminal);
                    tokio::select! {
                        _ = run_sync(&endpoint, peer, &session) => {}
                        _ = ended => {}
                    }
                }
            });
            attempts.insert(id, AbortOnDropHandle::new(task));
        }
    }

    /// 相手を待ち受けている同期（期限内で、まだ相手とつながっていない）。
    pub(super) fn waiting_sync(&self) -> Option<Arc<Session>> {
        let slot = self.slot.lock().expect("account transfer slot poisoned");
        slot.session
            .clone()
            .filter(|session| session.role == Role::Sync && session.waiting())
    }

    /// 同期の相手からの接続（#1650）。待ち受けていて、証明が正しいときだけ受けて、同じ接続で送り合う。こちらからも同じ
    /// 相手へつないでいる最中で、こちらの endpoint id が小さいときは受けない（両端末が互いへつないでも、使う接続を 1 本
    /// にする）。
    pub(super) async fn serve_sync(&self, connection: &Connection) {
        let Some(session) = self.waiting_sync() else {
            return;
        };
        let Pairing::Account(keys) = &session.pairing else {
            return;
        };
        let (initiator, responder) = (connection.remote_id(), self.endpoint.id());
        let hello = timeout(HELLO_TIMEOUT, async {
            let (send, mut recv) = connection.accept_bi().await.ok()?;
            let mut proof = [0; 32];
            recv.read_exact(&mut proof).await.ok()?;
            Some((send, proof))
        });
        let Ok(Some((mut send, proof))) = hello.await else {
            return;
        };
        let (from, to) = (initiator.as_bytes(), responder.as_bytes());
        let racing = from > to && session.attempting(initiator);
        if !keys.verify_pairing(from, to, true, &proof) || racing || !session.claim() {
            let _ = send.write_all(&[REJECTED]).await;
            let _ = send.finish();
            let _ = timeout(HELLO_TIMEOUT, send.stopped()).await;
            return;
        }
        let accepted = async {
            send.write_all(&[ACCEPTED]).await.ok()?;
            send.write_all(&keys.pairing_proof(from, to, false))
                .await
                .ok()?;
            send.finish().ok()
        };
        if accepted.await.is_none() {
            session.fail(Failure::Interrupted);
            return;
        }
        sync_both(&session, connection).await;
    }
}

impl Session {
    /// 同期: 期限内で、まだ相手とつながっていない。
    fn waiting(&self) -> bool {
        !*self.consumed.lock().expect("account transfer poisoned") && self.before_deadline()
    }

    fn before_deadline(&self) -> bool {
        matches!(*self.status.borrow(), Status::Waiting { expires_at_ms } if now_ms() < expires_at_ms)
    }

    /// 同期: 待ち受けを 1 回だけ使う（最初に確かめた相手とだけ送り合う）。
    fn claim(&self) -> bool {
        let mut consumed = self.consumed.lock().expect("account transfer poisoned");
        let waiting = !*consumed && self.before_deadline();
        *consumed |= waiting;
        waiting
    }

    /// 同期: こちらから `peer` へつないでいる最中か。
    fn attempting(&self, peer: EndpointId) -> bool {
        self.attempts
            .lock()
            .expect("account sync attempts poisoned")
            .get(&peer)
            .is_some_and(|attempt| !attempt.is_finished())
    }
}

/// 同期: 本人の端末の候補へつなぎ、証明を送る。相手が受けたら、相手の証明を確かめて同じ接続で送り合う。受けなければ
/// 待ち受けを続ける（相手がまだ待ち受けていない、または相手からの接続を使う）。
async fn run_sync(endpoint: &Endpoint, peer: EndpointAddr, session: &Session) {
    let Pairing::Account(keys) = &session.pairing else {
        return;
    };
    let responder = peer.id;
    let Ok(Ok(connection)) =
        timeout(CONNECT_TIMEOUT, endpoint.connect(peer, ACCOUNT_SYNC_ALPN)).await
    else {
        return;
    };
    let (from, to) = (endpoint.id(), responder);
    let accepted = async {
        if connection.remote_id() != responder {
            return None;
        }
        let (mut send, mut recv) = connection.open_bi().await.ok()?;
        send.write_all(&keys.pairing_proof(from.as_bytes(), to.as_bytes(), true))
            .await
            .ok()?;
        send.finish().ok()?;
        let mut verdict = [0];
        timeout(HELLO_TIMEOUT, recv.read_exact(&mut verdict))
            .await
            .ok()?
            .ok()?;
        let mut proof = [0; 32];
        if verdict == [ACCEPTED] {
            timeout(HELLO_TIMEOUT, recv.read_exact(&mut proof))
                .await
                .ok()?
                .ok()?;
        }
        keys.verify_pairing(from.as_bytes(), to.as_bytes(), false, &proof)
            .then_some(())
    };
    if accepted.await.is_none() || !session.claim() {
        connection.close(0u32.into(), b"account sync declined");
        return;
    }
    sync_both(session, &connection).await;
}

/// 同期: 両端末が同じ接続で、自分の必須 bundle（鍵を除く）と投稿の記録すべてを送り、相手のものを受けて取り込む。結果は
/// 受けた向きで示す（送る向きの失敗は、相手の端末が受けた向きの失敗として相手に示される）。取消で状態が終わったら、
/// 途中でもやめる。
async fn sync_both(session: &Session, connection: &Connection) {
    let exchange = async {
        let source = session.source.as_deref().ok_or(Failure::Invalid)?;
        let sink = session.sink.as_deref().ok_or(Failure::Invalid)?;
        session.set(Status::Transferring {
            role: Role::Sync,
            items: 0,
        });
        let (_, received) = futures_util::future::join(
            send_bundle(session, connection, source),
            receive_bundle(session, connection, sink),
        )
        .await;
        let account_id = received?;
        let (_, history) = futures_util::future::join(
            serve_history(session, connection, source),
            receive_history(
                session,
                connection,
                sink,
                &account_id,
                AccountTransferHistory::All,
            ),
        )
        .await;
        Ok(history)
    };
    let mut status = session.status.subscribe();
    let ended = status.wait_for(Status::is_terminal);
    tokio::select! {
        result = exchange => match result {
            Ok(history) => session.set(Status::Completed {
                role: Role::Sync,
                account_id: None,
                history: Some(history),
            }),
            Err(reason) => session.fail(reason),
        },
        _ = ended => {}
    }
    connection.close(0u32.into(), b"account sync ended");
}
