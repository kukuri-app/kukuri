//! QR・専用リンクの移行の接続と、両端末の確認（#1211 W7 AC-1）。
//!
//! 移行先は招待の endpoint id の端末だけへ接続する（QUIC の認証で接続先を束縛する）。移行元は招待の秘密による
//! 証明・期限・未使用を確かめて招待を消費し、両端末が同じ確認コードを出す。両方の承認を交換したときだけ
//! 「確認済み」になる。確認済みが鍵・設定の転送（AC-2）の唯一の入口で、それ以外の終わり方では何も送らない。

use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use iroh::endpoint::{Connection, RecvStream, SendStream};
use iroh::protocol::{AcceptError, ProtocolHandler};
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayUrl, TransportAddr};
use kukuri_core::{
    AccountTransferFailure as Failure, AccountTransferInvite, AccountTransferRole as Role,
    AccountTransferStatus as Status,
};
use n0_future::task::AbortOnDropHandle;
use n0_future::time::timeout;
use tokio::sync::{Semaphore, watch};

pub const ACCOUNT_TRANSFER_ALPN: &[u8] = b"/kukuri/account-transfer/1";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
const CONFIRM_TIMEOUT: Duration = Duration::from_secs(120);
/// 同時に応じる接続の数。超えた接続は待たせずに閉じる。
const CONCURRENT_REQUESTS: usize = 2;
const ACCEPTED: u8 = 1;
const REJECTED: u8 = 0;

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

    /// 移行元: 招待を出す。
    pub fn issue(&self) -> Result<AccountTransferInvite> {
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
            Some(Session::new(Role::Source, invite.clone(), status)),
            None,
        );
        Ok(invite)
    }

    /// 移行先: リンクの移行元へ接続し、確認を始める。リンクの形式・期限の誤りはここで返す。
    pub fn open(&self, link: &str) -> Result<()> {
        let invite = AccountTransferInvite::parse_link(link, now_ms())?;
        let addr = issuer_addr(&invite)?;
        let session = Session::new(Role::Target, invite, Status::Connecting);
        let task = n0_future::task::spawn({
            let endpoint = self.endpoint.clone();
            let session = session.clone();
            async move {
                if let Err(reason) = run_target(&endpoint, addr, &session).await {
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
    fn new(role: Role, invite: AccountTransferInvite, status: Status) -> Arc<Self> {
        Arc::new(Self {
            role,
            invite,
            consumed: StdMutex::new(false),
            status: watch::channel(status).0,
            decision: watch::channel(None).0,
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
        if let Err(reason) = serve_source(&connection, &session).await {
            session.fail(reason);
        }
        Ok(())
    }
}

/// 移行元: 証明を確かめ、受けた接続だけで確認へ進む。証明の誤りは待っている招待を終わらせない。
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
    confirm(session, connection, send, recv, &target).await
}

/// 移行先: 招待の endpoint id へ接続し、証明を送る。
async fn run_target(
    endpoint: &Endpoint,
    addr: EndpointAddr,
    session: &Session,
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
    confirm(session, &connection, send, recv, &target).await
}

/// 両端末: 確認コードを出し、自分の承認を送り、相手の承認を受ける。両方そろったときだけ確認済みにする。
async fn confirm(
    session: &Session,
    connection: &Connection,
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
    session.set(Status::Confirmed { role: session.role });
    connection.close(0u32.into(), b"account transfer confirmed");
    Ok(())
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
