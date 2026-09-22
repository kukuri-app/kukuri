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
    if private_replica_secrets
        .lock()
        .await
        .contains_key(replica_id.as_str())
    {
        return Ok(());
    }
    bail!("private replica capability is not registered");
}
