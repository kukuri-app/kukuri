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

pub(crate) async fn ensure_private_replica_access(
    replica_id: &ReplicaId,
    private_replica_secrets: &Arc<Mutex<HashMap<String, NamespaceSecret>>>,
) -> Result<()> {
    if replica_id.as_str().starts_with("bucket::") {
        crate::BucketReplica::parse(replica_id)?;
    }
    if public_replica_secret(replica_id).is_some() {
        return Ok(());
    }
    if registered_private_secret(replica_id, &*private_replica_secrets.lock().await).is_some() {
        return Ok(());
    }
    bail!("private replica capability is not registered");
}
