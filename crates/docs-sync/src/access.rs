use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
use iroh_docs::NamespaceSecret;
use kukuri_core::ReplicaId;
use tokio::sync::Mutex;

use crate::replicas::public_replica_secret;

pub(crate) fn parse_namespace_secret_hex(value: &str) -> Result<NamespaceSecret> {
    let decoded = hex::decode(value.trim()).context("invalid namespace secret hex")?;
    let bytes: [u8; 32] = decoded
        .try_into()
        .map_err(|_| anyhow!("namespace secret must be 32 bytes"))?;
    Ok(NamespaceSecret::from_bytes(&bytes))
}

/// 登録済みの private capability。private bucket は、登録した epoch の capability から導出する
/// (ADR 0054 §1。epoch の capability を外せば、その epoch の bucket も読み書きできなくなる)。
pub(crate) fn registered_private_secret(
    replica_id: &ReplicaId,
    secrets: &HashMap<String, NamespaceSecret>,
) -> Option<NamespaceSecret> {
    if let Some(secret) = secrets.get(replica_id.as_str()) {
        return Some(secret.clone());
    }
    let bucket = crate::BucketReplica::parse(replica_id).ok()?;
    let crate::BucketScope::PrivateChannel {
        channel_id,
        epoch_id,
    } = bucket.scope()
    else {
        return None;
    };
    let epoch =
        secrets.get(crate::private_channel_replica_for_epoch(channel_id, epoch_id).as_str())?;
    let derived = bucket.derive_private_secret(&epoch.to_bytes()).ok()?;
    Some(NamespaceSecret::from_bytes(&derived))
}

/// 参加中の private channel の世代の秘密の参照(ADR 0061 §9)。app-api が store の世代の鍵の行から引く。
#[async_trait::async_trait]
pub trait PrivateEpochSecrets: Send + Sync {
    /// (channel, epoch) の namespace の秘密の hex。行が無ければ `None`。
    async fn epoch_secret_hex(&self, channel_id: &str, epoch_id: &str) -> Option<String>;
}

/// private の capability。登録簿(account 同期の replica と一時の秘密)と、参加中の channel の世代の秘密の参照。
/// 手元で replica を開くときも、相手からの private の読取りに応えるときも、これで引く。
#[derive(Default)]
pub(crate) struct PrivateSecrets {
    pub(crate) registered: Mutex<HashMap<String, NamespaceSecret>>,
    epochs: std::sync::RwLock<Option<Arc<dyn PrivateEpochSecrets>>>,
}

impl PrivateSecrets {
    pub(crate) fn install_epoch_secrets(&self, source: Arc<dyn PrivateEpochSecrets>) {
        if let Ok(mut epochs) = self.epochs.write() {
            *epochs = Some(source);
        }
    }

    pub(crate) async fn secret(&self, replica_id: &ReplicaId) -> Option<NamespaceSecret> {
        if let Some(secret) = registered_private_secret(replica_id, &*self.registered.lock().await)
        {
            return Some(secret);
        }
        let (channel_id, epoch_id) = crate::private_channel_epoch_of(replica_id)?;
        let source = self.epochs.read().ok()?.clone()?;
        let epoch =
            parse_namespace_secret_hex(&source.epoch_secret_hex(&channel_id, &epoch_id).await?)
                .ok()?;
        if !replica_id.as_str().starts_with("bucket::") {
            return Some(epoch);
        }
        let derived = crate::BucketReplica::parse(replica_id)
            .ok()?
            .derive_private_secret(&epoch.to_bytes())
            .ok()?;
        Some(NamespaceSecret::from_bytes(&derived))
    }
}

#[async_trait::async_trait]
impl kukuri_iroh_node::PrivateSecretLookup for PrivateSecrets {
    async fn private_secret(&self, replica: &ReplicaId) -> Option<NamespaceSecret> {
        self.secret(replica).await
    }
}

pub(crate) async fn ensure_private_replica_access(
    replica_id: &ReplicaId,
    private_replica_secrets: &PrivateSecrets,
) -> Result<()> {
    if replica_id.as_str().starts_with("bucket::") {
        crate::BucketReplica::parse(replica_id)?;
    }
    if public_replica_secret(replica_id).is_some() {
        return Ok(());
    }
    if private_replica_secrets.secret(replica_id).await.is_some() {
        return Ok(());
    }
    bail!("private replica capability is not registered");
}
