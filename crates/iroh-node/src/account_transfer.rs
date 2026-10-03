//! QR・専用リンクの移行の接続と両端末の確認（#1211 W7 AC-1）、必須 bundle の転送（AC-2）。
//!
//! 移行先は招待の endpoint id の端末だけへ接続する（QUIC の認証で接続先を束縛する）。移行元は招待の秘密による
//! 証明・期限・未使用を確かめて招待を消費し、両端末が同じ確認コードを出す。両方の承認を交換したときだけ、同じ
//! 接続の新しい stream で移行元が必須 bundle（鍵・item の chunk・総数）を送る。それ以外の終わり方では何も送らない。
//! 移行先は chunk ごとに保存し、すべて受けて確定したら ACK を返す。移行元は ACK を受けたときだけ完了にする。

use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use iroh::endpoint::{Connection, ConnectionError, RecvStream, SendStream};
use iroh::protocol::{AcceptError, ProtocolHandler};
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayUrl, TransportAddr};
use kukuri_core::{
    AccountTransferFailure as Failure, AccountTransferFrame as Frame, AccountTransferInvite,
    AccountTransferItem, AccountTransferRole as Role, AccountTransferStatus as Status,
    MAX_ACCOUNT_TRANSFER_FRAME_BYTES,
};
use n0_future::task::AbortOnDropHandle;
use n0_future::time::timeout;
use tokio::sync::{Semaphore, watch};

pub const ACCOUNT_TRANSFER_ALPN: &[u8] = b"/kukuri/account-transfer/1";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
const CONFIRM_TIMEOUT: Duration = Duration::from_secs(120);
/// 必須 bundle の frame 1 つの待ち。
const FRAME_TIMEOUT: Duration = Duration::from_secs(30);
/// 移行先が保存を確定して ACK を返すまでの待ち（登録簿の更新はアカウントの操作の排他を待つ）。
const COMMIT_TIMEOUT: Duration = Duration::from_secs(60);
/// 同時に応じる接続の数。超えた接続は待たせずに閉じる。
const CONCURRENT_REQUESTS: usize = 2;
const ACCEPTED: u8 = 1;
const REJECTED: u8 = 0;
/// 移行先が保存に失敗して接続を閉じるときの code。移行元は保存の失敗として示す。
const STORAGE_FAILED: u32 = 1;

/// 移行元の必須 bundle（#1211 AC-2）。runtime が手元の account の replica から読む。
#[async_trait::async_trait]
pub trait AccountBundleSource: Send + Sync {
    /// アカウントの秘密鍵（hex）。
    fn secret_hex(&self) -> String;
    /// `cursor`（最初は `None`）からの 1 page と、次の cursor（尽きたら `None`）。
    async fn page(
        &self,
        cursor: Option<String>,
    ) -> Result<(Vec<AccountTransferItem>, Option<String>)>;
}

/// 移行先の保存（#1211 AC-2）。鍵を受けて保存を始める。
#[async_trait::async_trait]
pub trait AccountBundleSink: Send + Sync {
    async fn begin(&self, secret_hex: &str) -> Result<Box<dyn AccountBundleStaging>, Failure>;
}

/// 1 回の移行の保存。chunk ごとに保存し、すべて受けたら確定する。確定しなければ消す。
#[async_trait::async_trait]
pub trait AccountBundleStaging: Send {
    async fn stage(&mut self, items: Vec<AccountTransferItem>) -> Result<(), Failure>;
    /// 確定して、受けたアカウントの ID を返す。
    async fn commit(&mut self) -> Result<String, Failure>;
    async fn abort(&mut self);
}

/// 端末ごとに 1 つだけの移行（移行元・移行先のどちらか）。新しい移行は前の移行を取り消す。
#[derive(Clone)]
pub struct AccountTransfer {
    endpoint: Endpoint,
    slot: Arc<StdMutex<Slot>>,
    permits: Arc<Semaphore>,
}

#[derive(Default)]
struct Slot {
    session: Option<Arc<Session>>,
    _task: Option<AbortOnDropHandle<()>>,
}

struct Session {
    role: Role,
    invite: AccountTransferInvite,
    consumed: StdMutex<bool>,
    status: watch::Sender<Status>,
    decision: watch::Sender<Option<bool>>,
    /// 移行元が送る必須 bundle（移行先は `None`）。
    source: Option<Arc<dyn AccountBundleSource>>,
}

impl std::fmt::Debug for AccountTransfer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccountTransfer").finish_non_exhaustive()
    }
}

impl AccountTransfer {
    pub(crate) fn new(endpoint: Endpoint) -> Self {
        Self {
            endpoint,
            slot: Arc::default(),
            permits: Arc::new(Semaphore::new(CONCURRENT_REQUESTS)),
        }
    }

    /// 移行元: 招待を出す。確認の後に `source` の必須 bundle を送る。
    pub fn issue(&self, source: Arc<dyn AccountBundleSource>) -> Result<AccountTransferInvite> {
        let addr = self.endpoint.addr();
        let invite = AccountTransferInvite::issue(
            addr.id.to_string(),
            addr.relay_urls().next().map(ToString::to_string),
            addr.ip_addrs().copied(),
            now_ms(),
        )?;
        let status = Status::Waiting {
            expires_at_ms: invite.expires_at_ms,
        };
        self.replace(
            Some(Session::new(
                Role::Source,
                invite.clone(),
                status,
                Some(source),
            )),
            None,
        );
        Ok(invite)
    }

    /// 移行先: リンクの移行元へ接続し、確認を始める。確認の後に受けた必須 bundle を `sink` へ保存する。リンクの
    /// 形式・期限の誤りはここで返す。
    pub fn open(&self, link: &str, sink: Arc<dyn AccountBundleSink>) -> Result<()> {
        let invite = AccountTransferInvite::parse_link(link, now_ms())?;
        let addr = issuer_addr(&invite)?;
        let session = Session::new(Role::Target, invite, Status::Connecting, None);
        let task = n0_future::task::spawn({
            let endpoint = self.endpoint.clone();
            let session = session.clone();
            async move {
                if let Err(reason) = run_target(&endpoint, addr, &session, sink.as_ref()).await {
                    session.fail(reason);
                }
            }
        });
        self.replace(Some(session), Some(AbortOnDropHandle::new(task)));
        Ok(())
    }

    pub fn status(&self) -> Status {
        let slot = self.slot.lock().expect("account transfer slot poisoned");
        let Some(session) = slot.session.as_ref() else {
            return Status::Idle;
        };
        match session.status.borrow().clone() {
            Status::Waiting { expires_at_ms } if now_ms() >= expires_at_ms => Status::Failed {
                role: Role::Source,
                reason: Failure::Expired,
            },
            status => status,
        }
    }

    /// 確認コードの一致・不一致を伝える。確認の途中だけ受け付ける。
    pub fn decide(&self, accept: bool) -> Result<()> {
        let slot = self.slot.lock().expect("account transfer slot poisoned");
        let session = slot.session.as_ref().context("no account transfer")?;
        ensure!(
            matches!(*session.status.borrow(), Status::Confirming { .. }),
            "account transfer is not awaiting confirmation"
        );
        session.decision.send_if_modified(|decision| {
            let first = decision.is_none();
            if first {
                *decision = Some(accept);
            }
            first
        });
        Ok(())
    }

    pub fn cancel(&self) {
        self.replace(None, None);
    }

    fn replace(&self, session: Option<Arc<Session>>, task: Option<AbortOnDropHandle<()>>) {
        let previous = std::mem::replace(
            &mut *self.slot.lock().expect("account transfer slot poisoned"),
            Slot {
                session,
                _task: task,
            },
        );
        if let Some(session) = previous.session {
            session.fail(Failure::Cancelled);
            session.decision.send_replace(Some(false));
        }
    }

    fn source_session(&self) -> Option<Arc<Session>> {
        let slot = self.slot.lock().expect("account transfer slot poisoned");
        slot.session
            .clone()
            .filter(|session| session.role == Role::Source)
    }
}

impl Session {
    fn new(
        role: Role,
        invite: AccountTransferInvite,
        status: Status,
        source: Option<Arc<dyn AccountBundleSource>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            role,
            invite,
            consumed: StdMutex::new(false),
            status: watch::channel(status).0,
            decision: watch::channel(None).0,
            source,
        })
    }

    /// 終わった移行の状態は変えない。
    fn set(&self, status: Status) {
        self.status.send_if_modified(|current| {
            let open = !current.is_terminal();
            if open {
                *current = status;
            }
            open
        });
    }

    fn fail(&self, reason: Failure) {
        self.set(Status::Failed {
            role: self.role,
            reason,
        });
    }

    /// 移行元: 期限内・未使用・証明が正しいときだけ、招待を 1 回だけ消費する。
    fn consume(&self, target: &[u8; 32], proof: &[u8; 32]) -> bool {
        let mut consumed = self.consumed.lock().expect("account transfer poisoned");
        let valid = !*consumed
            && !self.status.borrow().is_terminal()
            && now_ms() < self.invite.expires_at_ms
            && self.invite.verify_hello(target, proof);
        *consumed |= valid;
        valid
    }
}

impl ProtocolHandler for AccountTransfer {
    async fn accept(&self, connection: Connection) -> std::result::Result<(), AcceptError> {
        let Ok(_permit) = self.permits.try_acquire() else {
            return Ok(());
        };
        let Some(session) = self.source_session() else {
            return Ok(());
        };
        // 取消（新しい移行・cancel）で状態が終わったら、確認・転送の途中でもやめる。
        let mut status = session.status.subscribe();
        let ended = status.wait_for(Status::is_terminal);
        let result = tokio::select! {
            result = serve_source(&connection, &session) => result,
            _ = ended => Err(Failure::Cancelled),
        };
        if let Err(reason) = result {
            session.fail(reason);
            connection.close(0u32.into(), b"account transfer ended");
        }
        Ok(())
    }
}

/// 移行元: 証明を確かめ、受けた接続だけで確認へ進み、両端末の承認の後に必須 bundle を送る。証明の誤りは待って
/// いる招待を終わらせない。
async fn serve_source(connection: &Connection, session: &Session) -> Result<(), Failure> {
    let target = *connection.remote_id().as_bytes();
    let hello = timeout(HELLO_TIMEOUT, async {
        let (send, mut recv) = connection.accept_bi().await.ok()?;
        let mut proof = [0; 32];
        recv.read_exact(&mut proof).await.ok()?;
        Some((send, recv, proof))
    });
    let Ok(Some((mut send, recv, proof))) = hello.await else {
        return Ok(());
    };
    if !session.consume(&target, &proof) {
        let _ = send.write_all(&[REJECTED]).await;
        let _ = send.finish();
        let _ = timeout(HELLO_TIMEOUT, send.stopped()).await;
        return Ok(());
    }
    send.write_all(&[ACCEPTED])
        .await
        .map_err(|_| Failure::Interrupted)?;
    confirm(session, send, recv, &target).await?;
    let source = session.source.as_deref().ok_or(Failure::Invalid)?;
    send_bundle(session, connection, source).await
}

/// 移行先: 招待の endpoint id へ接続し、証明を送る。両端末の承認の後に必須 bundle を受けて保存する。
async fn run_target(
    endpoint: &Endpoint,
    addr: EndpointAddr,
    session: &Session,
    sink: &dyn AccountBundleSink,
) -> Result<(), Failure> {
    let issuer = addr.id;
    let connection = timeout(
        CONNECT_TIMEOUT,
        endpoint.connect(addr, ACCOUNT_TRANSFER_ALPN),
    )
    .await
    .map_err(|_| Failure::Unreachable)?
    .map_err(|_| Failure::Unreachable)?;
    if connection.remote_id() != issuer {
        return Err(Failure::Invalid);
    }
    let target = *endpoint.id().as_bytes();
    let (mut send, mut recv) = connection
        .open_bi()
        .await
        .map_err(|_| Failure::Interrupted)?;
    send.write_all(&session.invite.hello_proof(&target))
        .await
        .map_err(|_| Failure::Interrupted)?;
    let mut verdict = [0];
    match timeout(HELLO_TIMEOUT, recv.read_exact(&mut verdict)).await {
        Ok(Ok(())) if verdict[0] == ACCEPTED => {}
        _ => return Err(Failure::Invalid),
    }
    confirm(session, send, recv, &target).await?;
    receive_bundle(session, &connection, sink).await
}

/// 両端末: 確認コードを出し、自分の承認を送り、相手の承認を受ける。両方そろったときだけ転送へ進む。
async fn confirm(
    session: &Session,
    mut send: SendStream,
    mut recv: RecvStream,
    target: &[u8; 32],
) -> Result<(), Failure> {
    let code = session.invite.verification_code(target);
    let confirming = |local_accepted| Status::Confirming {
        role: session.role,
        code: code.clone(),
        local_accepted,
    };
    session.set(confirming(false));
    let local = async {
        let accepted = session
            .decision
            .subscribe()
            .wait_for(Option::is_some)
            .await
            .map_err(|_| Failure::Cancelled)?
            .unwrap_or(false);
        if accepted {
            session.set(confirming(true));
        }
        send.write_all(&[if accepted { ACCEPTED } else { REJECTED }])
            .await
            .map_err(|_| Failure::Interrupted)?;
        send.finish().map_err(|_| Failure::Interrupted)?;
        let delivered = send.stopped().await.is_ok();
        match (accepted, delivered) {
            (false, _) => Err(Failure::Rejected),
            (true, false) => Err(Failure::Interrupted),
            (true, true) => Ok(()),
        }
    };
    let remote = async {
        let decision = recv
            .read_to_end(1)
            .await
            .map_err(|_| Failure::Interrupted)?;
        if decision == [ACCEPTED] {
            Ok(())
        } else {
            Err(Failure::Rejected)
        }
    };
    timeout(
        CONFIRM_TIMEOUT,
        futures_util::future::try_join(local, remote),
    )
    .await
    .map_err(|_| Failure::Expired)??;
    session.set(Status::Transferring {
        role: session.role,
        items: 0,
    });
    Ok(())
}

/// 移行元: 同じ接続の新しい stream で、鍵・item の chunk・総数を送り、移行先の保存の ACK を受けたら完了にする。
/// 移行先が保存の失敗で閉じた接続は、送る途中でも保存の失敗として示す。
async fn send_bundle(
    session: &Session,
    connection: &Connection,
    source: &dyn AccountBundleSource,
) -> Result<(), Failure> {
    let sent = async {
        let (mut send, mut recv) = connection
            .open_bi()
            .await
            .map_err(|_| Failure::Interrupted)?;
        write_frame(
            &mut send,
            &Frame::Key {
                secret: source.secret_hex(),
            },
        )
        .await?;
        let (mut cursor, mut count) = (None, 0);
        loop {
            let (items, next) = source.page(cursor).await.map_err(|_| Failure::Storage)?;
            for frame in Frame::chunks(items).map_err(|_| Failure::Invalid)? {
                if let Frame::Items { items } = &frame {
                    count += items.len() as u64;
                }
                write_frame(&mut send, &frame).await?;
                session.set(Status::Transferring {
                    role: Role::Source,
                    items: count,
                });
            }
            let Some(next) = next else { break };
            cursor = Some(next);
        }
        write_frame(&mut send, &Frame::End { count }).await?;
        send.finish().map_err(|_| Failure::Interrupted)?;
        match timeout(COMMIT_TIMEOUT, recv.read_to_end(1)).await {
            Ok(Ok(ack)) if ack == [ACCEPTED] => Ok(()),
            _ => Err(Failure::Interrupted),
        }
    }
    .await;
    if let Err(reason) = sent {
        let storage = matches!(
            connection.close_reason(),
            Some(ConnectionError::ApplicationClosed(close))
                if close.error_code == STORAGE_FAILED.into()
        );
        return Err(if storage { Failure::Storage } else { reason });
    }
    session.set(Status::Completed {
        role: Role::Source,
        account_id: None,
    });
    connection.close(0u32.into(), b"account transfer completed");
    Ok(())
}

/// 移行先: 鍵を受けて保存を始め、chunk ごとに保存し、総数が合えば確定して ACK を返す。確定しなければ保存を消す。
async fn receive_bundle(
    session: &Session,
    connection: &Connection,
    sink: &dyn AccountBundleSink,
) -> Result<(), Failure> {
    let (mut send, mut recv) = timeout(FRAME_TIMEOUT, connection.accept_bi())
        .await
        .map_err(|_| Failure::Interrupted)?
        .map_err(|_| Failure::Interrupted)?;
    let Frame::Key { secret } = read_frame(&mut recv).await? else {
        return Err(Failure::Invalid);
    };
    let storage_failed = |reason| {
        if reason == Failure::Storage {
            connection.close(STORAGE_FAILED.into(), b"account transfer storage failed");
        }
        reason
    };
    let mut staging = sink.begin(&secret).await.map_err(&storage_failed)?;
    let staged = async {
        let mut count = 0;
        loop {
            match read_frame(&mut recv).await? {
                Frame::Items { items } => {
                    count += items.len() as u64;
                    staging.stage(items).await?;
                    session.set(Status::Transferring {
                        role: Role::Target,
                        items: count,
                    });
                }
                Frame::End { count: sent } if sent == count => return staging.commit().await,
                _ => return Err(Failure::Invalid),
            }
        }
    }
    .await;
    let account_id = match staged {
        Ok(account_id) => account_id,
        Err(reason) => {
            staging.abort().await;
            return Err(storage_failed(reason));
        }
    };
    // 確定した後の ACK は、届かなくても保存を戻さない（移行元は完了にならず、やり直しは同じアカウントへの merge）。
    if send.write_all(&[ACCEPTED]).await.is_ok() && send.finish().is_ok() {
        let _ = timeout(FRAME_TIMEOUT, send.stopped()).await;
    }
    session.set(Status::Completed {
        role: Role::Target,
        account_id: Some(account_id),
    });
    Ok(())
}

/// frame を 1 つ読む（長さ 4 byte の後に JSON）。大きさと形を確かめる。
async fn read_frame(recv: &mut RecvStream) -> Result<Frame, Failure> {
    let read = async {
        let mut len = [0; 4];
        recv.read_exact(&mut len)
            .await
            .map_err(|_| Failure::Interrupted)?;
        let len = u32::from_be_bytes(len) as usize;
        if len > MAX_ACCOUNT_TRANSFER_FRAME_BYTES {
            return Err(Failure::Invalid);
        }
        let mut bytes = vec![0; len];
        recv.read_exact(&mut bytes)
            .await
            .map_err(|_| Failure::Interrupted)?;
        Frame::decode(&bytes).map_err(|_| Failure::Invalid)
    };
    timeout(FRAME_TIMEOUT, read)
        .await
        .map_err(|_| Failure::Interrupted)?
}

async fn write_frame(send: &mut SendStream, frame: &Frame) -> Result<(), Failure> {
    let bytes = frame.encode().map_err(|_| Failure::Invalid)?;
    let len = u32::try_from(bytes.len()).map_err(|_| Failure::Invalid)?;
    send.write_all(&len.to_be_bytes())
        .await
        .map_err(|_| Failure::Interrupted)?;
    send.write_all(&bytes)
        .await
        .map_err(|_| Failure::Interrupted)
}

fn issuer_addr(invite: &AccountTransferInvite) -> Result<EndpointAddr> {
    let id: EndpointId = invite
        .issuer
        .parse()
        .context("invalid account transfer issuer")?;
    let relay = invite
        .relay_url
        .as_deref()
        .map(str::parse::<RelayUrl>)
        .transpose()
        .context("invalid account transfer relay url")?;
    Ok(EndpointAddr::from_parts(
        id,
        invite
            .direct_addrs
            .iter()
            .copied()
            .map(TransportAddr::Ip)
            .chain(relay.map(TransportAddr::Relay)),
    ))
}

fn now_ms() -> i64 {
    web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

#[cfg(test)]
#[cfg(not(target_family = "wasm"))]
#[path = "tests/account_transfer.rs"]
mod tests;
