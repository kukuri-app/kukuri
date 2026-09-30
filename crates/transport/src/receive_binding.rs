//! 公開のaccount/endpoint対応を、実際に接続したendpointに照合する。
//! このprotocolだけでは投稿のscope権限や通知の配送完了を認定しない。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler};
use iroh::{Endpoint, EndpointAddr, EndpointId};
use kukuri_core::{
    KukuriKeys, Pubkey, RECEIVE_ENDPOINT_BINDING_MAX_BYTES,
    RECEIVE_ENDPOINT_BINDING_MAX_LIFETIME_MS, ReceiveEndpointBindingV1,
    VerifiedReceiveEndpointBinding, receive_route_for_account,
};
use tokio::sync::{RwLock, Semaphore};
use tokio::time::{Instant, timeout, timeout_at};

pub const RECEIVE_BINDING_ALPN: &[u8] = b"/kukuri/receive-binding/1";
/// 1 endpoint が同時に応じる binding 要求の数。超えた要求は待たせずに接続を閉じる。
pub const RECEIVE_BINDING_CONCURRENT_REQUESTS: usize = 2;
const RECEIVE_BINDING_SERVE_TIMEOUT: Duration = Duration::from_secs(2);

/// account runtimeに組み込む際はownerがbinding更新とRouter終了を所有する。
/// 新たな待機taskを作らず、満杯時は接続を閉じる。
#[derive(Debug, Clone)]
pub struct ReceiveBindingProtocol {
    local_endpoint: EndpointId,
    account: Pubkey,
    binding: Arc<RwLock<ReceiveEndpointBindingV1>>,
    permits: Arc<Semaphore>,
}

/// Router起動時はaccount未確定なので、署名鍵の導入後だけbindingを返す。
/// 固定のbindingを保持せず、要求時に短命の署名を作るため更新timerを持たない。
#[derive(Clone)]
pub struct ReceiveBindingSlot {
    local_endpoint: EndpointId,
    signer: Arc<RwLock<Option<Arc<KukuriKeys>>>>,
    permits: Arc<Semaphore>,
    closed: Arc<AtomicBool>,
}

impl std::fmt::Debug for ReceiveBindingSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReceiveBindingSlot")
            .field("local_endpoint", &self.local_endpoint)
            .finish_non_exhaustive()
    }
}

impl ReceiveBindingSlot {
    pub fn new(local_endpoint: EndpointId) -> Self {
        Self {
            local_endpoint,
            signer: Arc::new(RwLock::new(None)),
            permits: Arc::new(Semaphore::new(RECEIVE_BINDING_CONCURRENT_REQUESTS)),
            closed: Arc::new(AtomicBool::new(false)),
        }
    }

    /// 既存nodeを別accountへ付け替えない。account切替はnode全体の再生成で行う。
    pub async fn install(&self, keys: Arc<KukuriKeys>) -> Result<()> {
        ensure!(
            !self.closed.load(Ordering::Acquire),
            "receive binding slot is closed"
        );
        let now = now_ms();
        ReceiveEndpointBindingV1::sign(
            &keys,
            &self.local_endpoint.to_string(),
            now,
            now + RECEIVE_ENDPOINT_BINDING_MAX_LIFETIME_MS,
        )?;
        let mut current = self.signer.write().await;
        ensure!(
            !self.closed.load(Ordering::Acquire),
            "receive binding slot is closed"
        );
        if let Some(existing) = current.as_ref() {
            ensure!(
                existing.public_key() == keys.public_key(),
                "receive binding account cannot change on a live node"
            );
        }
        *current = Some(keys);
        Ok(())
    }

    pub async fn clear(&self) {
        self.reject_new_requests();
        *self.signer.write().await = None;
    }

    pub fn reject_new_requests(&self) {
        self.closed.store(true, Ordering::Release);
    }

    async fn serve(&self, connection: &Connection) -> Result<()> {
        let signer = self.signer.read().await;
        let keys = signer
            .as_ref()
            .context("receive binding account is not installed")?;
        let (mut send, mut recv) = connection.accept_bi().await?;
        ensure!(
            recv.read_to_end(1).await? == [1],
            "unsupported binding request"
        );
        let now = now_ms();
        let binding = ReceiveEndpointBindingV1::sign(
            keys.as_ref(),
            &self.local_endpoint.to_string(),
            now,
            now + RECEIVE_ENDPOINT_BINDING_MAX_LIFETIME_MS,
        )?;
        write_binding(&mut send, &binding).await
    }
}

impl ProtocolHandler for ReceiveBindingSlot {
    async fn accept(&self, connection: Connection) -> std::result::Result<(), AcceptError> {
        let connection = CloseBindingConnection(connection);
        if self.closed.load(Ordering::Acquire) {
            return Ok(());
        }
        let Ok(_permit) = self.permits.try_acquire() else {
            return Ok(());
        };
        timeout(RECEIVE_BINDING_SERVE_TIMEOUT, self.serve(&connection.0))
            .await
            .context("receive binding request timed out")
            .and_then(|result| result)
            .map_err(|error| AcceptError::from_boxed(error.into_boxed_dyn_error()))
    }
}

impl ReceiveBindingProtocol {
    pub fn new(local_endpoint: EndpointId, binding: ReceiveEndpointBindingV1) -> Result<Self> {
        binding.verify_for(&binding.account, &local_endpoint.to_string(), now_ms())?;
        Ok(Self {
            local_endpoint,
            account: binding.account.clone(),
            binding: Arc::new(RwLock::new(binding)),
            permits: Arc::new(Semaphore::new(RECEIVE_BINDING_CONCURRENT_REQUESTS)),
        })
    }

    pub async fn replace(&self, binding: ReceiveEndpointBindingV1) -> Result<()> {
        binding.verify_for(&self.account, &self.local_endpoint.to_string(), now_ms())?;
        let mut current = self.binding.write().await;
        ensure!(
            binding.issued_at_ms >= current.issued_at_ms,
            "stale receive binding update"
        );
        *current = binding;
        Ok(())
    }

    async fn serve(&self, connection: &Connection) -> Result<()> {
        let (mut send, mut recv) = connection.accept_bi().await?;
        ensure!(
            recv.read_to_end(1).await? == [1],
            "unsupported binding request"
        );
        let binding = self.binding.read().await.clone();
        binding.verify_for(&self.account, &self.local_endpoint.to_string(), now_ms())?;
        write_binding(&mut send, &binding).await
    }
}

async fn write_binding(
    send: &mut iroh::endpoint::SendStream,
    binding: &ReceiveEndpointBindingV1,
) -> Result<()> {
    let bytes = serde_json::to_vec(binding)?;
    ensure!(
        bytes.len() <= RECEIVE_ENDPOINT_BINDING_MAX_BYTES,
        "binding response is too large"
    );
    send.write_all(&bytes).await?;
    send.finish()?;
    send.stopped().await?;
    Ok(())
}

impl ProtocolHandler for ReceiveBindingProtocol {
    async fn accept(&self, connection: Connection) -> std::result::Result<(), AcceptError> {
        let connection = CloseBindingConnection(connection);
        let Ok(_permit) = self.permits.try_acquire() else {
            return Ok(());
        };
        timeout(RECEIVE_BINDING_SERVE_TIMEOUT, self.serve(&connection.0))
            .await
            .context("receive binding request timed out")
            .and_then(|result| result)
            .map_err(|error| AcceptError::from_boxed(error.into_boxed_dyn_error()))
    }
}

/// ownerが選んだ候補1件を照合する。deadlineはqueue待機前の受付時点で決定し、
/// このfutureの取消時は接続も閉じる。独自retryやpeer探索を起動しない。
pub async fn fetch_receive_endpoint_binding(
    endpoint: &Endpoint,
    candidate: EndpointAddr,
    expected_account: &Pubkey,
    deadline: Instant,
) -> Result<VerifiedReceiveEndpointBinding> {
    receive_route_for_account(expected_account)?;
    ensure!(deadline > Instant::now(), "receive binding request expired");
    timeout_at(deadline, async {
        let connection =
            CloseBindingConnection(endpoint.connect(candidate, RECEIVE_BINDING_ALPN).await?);
        let (mut send, mut recv) = connection.0.open_bi().await?;
        send.write_all(&[1]).await?;
        send.finish()?;
        let bytes = recv.read_to_end(RECEIVE_ENDPOINT_BINDING_MAX_BYTES).await?;
        let binding = ReceiveEndpointBindingV1::decode(&bytes)?;
        binding.verify_for(
            expected_account,
            &connection.0.remote_id().to_string(),
            now_ms(),
        )
    })
    .await
    .context("receive binding lookup timed out")?
}

struct CloseBindingConnection(Connection);

impl Drop for CloseBindingConnection {
    fn drop(&mut self) {
        self.0
            .close(0u32.into(), b"receive binding exchange complete");
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[cfg(test)]
#[path = "receive_binding_tests.rs"]
mod tests;
