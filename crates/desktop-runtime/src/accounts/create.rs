use super::*;
use crate::CreateAccountRequest;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PreparedAccountCreation {
    pub committed: bool,
    pub operation_id: String,
    pub previous: String,
    pub next: AccountRecord,
}

pub(crate) async fn prepare_account_creation(
    dir: &Path,
    mode: IdentityStorageMode,
    request: &CreateAccountRequest,
) -> Result<PreparedAccountCreation> {
    let operation = uuid::Uuid::parse_str(&request.operation_id)?.to_string();
    let mut registry = load_registry(dir)
        .await?
        .ok_or_else(|| anyhow!("accounts not initialized"))?;
    if let Some(completed) = registry.completed_creations.get(&operation) {
        if completed.previous != request.account_id
            || registry.active_account_id != completed.next.id
            || !registry
                .accounts
                .iter()
                .any(|a| a.id == completed.next.id && a.pubkey == completed.next.pubkey)
        {
            bail!("account creation operation was already completed");
        }
        verify_persisted_identity(
            &account_db_path(dir, &completed.next.id),
            mode,
            &completed.next.pubkey,
        )
        .await?;
        return Ok(completed.clone());
    }
    if let Some(pending) = &registry.pending_creation
        && pending.operation_id == operation
        && pending.previous == request.account_id
    {
        if pending.committed && registry.active_account_id != pending.next.id {
            bail!("account creation was already completed");
        }
        if registry.active_account_id != pending.previous
            && registry.active_account_id != pending.next.id
        {
            bail!("account creation request is stale");
        }
        verify_persisted_identity(
            &account_db_path(dir, &pending.next.id),
            mode,
            &pending.next.pubkey,
        )
        .await?;
        return Ok(pending.clone());
    }
    if registry.active_account_id != request.account_id {
        bail!("account creation target is no longer active");
    }
    let staging = dir
        .join("account-transitions")
        .join(format!("create-{operation}"))
        .join(DB_FILE_NAME);
    let next = lifecycle::generate_account(dir, mode, &staging).await?;
    if registry
        .accounts
        .iter()
        .any(|a| a.id == next.id || a.pubkey == next.pubkey)
    {
        bail!("account creation operation already registered");
    }
    let pending = PreparedAccountCreation {
        committed: false,
        operation_id: operation,
        previous: request.account_id.clone(),
        next,
    };
    registry.pending_creation = Some(pending.clone());
    save_registry(dir, &registry).await?;
    Ok(pending)
}

pub(crate) async fn commit_account_creation(
    dir: &Path,
    pending: &PreparedAccountCreation,
) -> Result<()> {
    let mut registry = load_registry(dir)
        .await?
        .ok_or_else(|| anyhow!("accounts not initialized"))?;
    if registry.pending_creation.as_ref() != Some(pending)
        || registry.active_account_id != pending.previous
    {
        bail!("account creation reservation is stale");
    }
    if registry.accounts.iter().any(|a| a.id == pending.next.id) {
        bail!("account already registered");
    }
    registry.accounts.push(pending.next.clone());
    registry.profile_setup.push(pending.next.id.clone());
    registry
        .history
        .retain(|id| id != &pending.previous && id != &pending.next.id);
    registry.history.insert(0, pending.previous.clone());
    registry.active_account_id = pending.next.id.clone();
    registry.completed_creations.insert(
        pending.operation_id.clone(),
        PreparedAccountCreation {
            committed: true,
            ..pending.clone()
        },
    );
    registry.pending_creation = None;
    save_registry(dir, &registry).await
}
