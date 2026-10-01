//! Local-only logout reservation. The account registry is the commit point.
use super::*;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PreparedLogout {
    pub target: String,
    pub next: AccountRecord,
    pub generated: bool,
}

pub(crate) async fn prepare_logout(
    dir: &Path,
    mode: IdentityStorageMode,
    target: &str,
) -> Result<PreparedLogout> {
    let mut registry = load_registry(dir)
        .await?
        .ok_or_else(|| anyhow!("accounts not initialized"))?;
    if registry.active_account_id != target || !registry.accounts.iter().any(|a| a.id == target) {
        bail!("logout target is no longer active");
    }
    let previous = registry
        .history
        .iter()
        .find_map(|id| {
            registry
                .accounts
                .iter()
                .find(|a| &a.id == id && a.id != target)
                .cloned()
        })
        .or_else(|| {
            let mut others: Vec<_> = registry
                .accounts
                .iter()
                .filter(|a| a.id != target)
                .cloned()
                .collect();
            others.sort_by(|a, b| b.last_used_at.cmp(&a.last_used_at).then(a.id.cmp(&b.id)));
            others.into_iter().next()
        });
    let generated = previous.is_none();
    if let Some(pending) = &registry.pending_logout {
        let same_destination = previous.as_ref().map_or(pending.generated, |a| {
            !pending.generated && a.id == pending.next.id
        });
        if pending.target == target && same_destination {
            verify_persisted_identity(
                &account_db_path(dir, &pending.next.id),
                mode,
                &pending.next.pubkey,
            )
            .await?;
            return Ok(pending.clone());
        }
    }
    let next = if let Some(previous) = previous {
        previous
    } else {
        // Stable staging identity also survives a crash before the reservation write.
        let staging = dir
            .join("account-transitions")
            .join(format!("{}-{target}", registry.logout_sequence))
            .join(DB_FILE_NAME);
        generate_account(dir, mode, &staging).await?
    };
    let prepared = PreparedLogout {
        target: target.to_owned(),
        next,
        generated,
    };
    verify_persisted_identity(
        &account_db_path(dir, &prepared.next.id),
        mode,
        &prepared.next.pubkey,
    )
    .await?;
    registry.pending_logout = Some(prepared.clone());
    save_registry(dir, &registry).await?;
    Ok(prepared)
}

pub(crate) async fn commit_logout(dir: &Path, prepared: &PreparedLogout) -> Result<()> {
    let mut registry = load_registry(dir)
        .await?
        .ok_or_else(|| anyhow!("accounts not initialized"))?;
    if registry.active_account_id != prepared.target
        || registry.pending_logout.as_ref() != Some(prepared)
    {
        bail!("logout reservation is stale");
    }
    registry.accounts.retain(|a| a.id != prepared.target);
    registry
        .history
        .retain(|id| id != &prepared.target && id != &prepared.next.id);
    if prepared.generated {
        registry.accounts.push(prepared.next.clone());
        registry.profile_setup.push(prepared.next.id.clone());
    }
    let next = registry
        .accounts
        .iter_mut()
        .find(|a| a.id == prepared.next.id)
        .ok_or_else(|| anyhow!("logout destination is missing"))?;
    next.last_used_at = now_millis();
    registry.active_account_id = next.id.clone();
    registry.pending_logout = None;
    registry.logout_sequence = registry
        .logout_sequence
        .checked_add(1)
        .ok_or_else(|| anyhow!("logout sequence overflow"))?;
    save_registry(dir, &registry).await
}

pub async fn profile_setup_required(dir: &Path, account_id: &str) -> Result<bool> {
    let registry = load_registry(dir)
        .await?
        .ok_or_else(|| anyhow!("accounts not initialized"))?;
    if registry.active_account_id != account_id {
        bail!("account is no longer active");
    }
    Ok(registry.profile_setup.iter().any(|id| id == account_id))
}

pub(crate) async fn complete_profile_setup(dir: &Path, account_id: &str) -> Result<()> {
    let mut registry = load_registry(dir)
        .await?
        .ok_or_else(|| anyhow!("accounts not initialized"))?;
    if registry.active_account_id != account_id {
        bail!("account is no longer active");
    }
    registry.profile_setup.retain(|id| id != account_id);
    save_registry(dir, &registry).await
}

/// A private-file write may fail after rename. Only a confirmed old registry
/// permits a runtime rollback; an applied write is flushed again, never undone.
pub(crate) async fn reconcile_account_commit(
    dir: &Path,
    previous: &str,
    next: &str,
    logout: bool,
) -> Result<bool> {
    let registry = load_registry(dir)
        .await?
        .ok_or_else(|| anyhow!("registry missing after commit"))?;
    if registry.active_account_id == next
        && (!logout || !registry.accounts.iter().any(|a| a.id == previous))
    {
        save_registry(dir, &registry).await?;
        return Ok(true);
    }
    if registry.active_account_id == previous && registry.accounts.iter().any(|a| a.id == previous)
    {
        return Ok(false);
    }
    bail!("account commit outcome is unknown")
}

pub(crate) async fn generate_account(
    dir: &Path,
    mode: IdentityStorageMode,
    staging: &Path,
) -> Result<AccountRecord> {
    create_account_dir(staging)?;
    let keys = load_or_create_keys(staging, mode).await?;
    let pubkey = keys.public_key_hex();
    let id = account_id_for_pubkey(&pubkey)?;
    let db = account_db_path(dir, &id);
    create_account_dir(&db)?;
    if let Some(existing) = load_existing_keys(&db, mode).await? {
        if existing.public_key_hex() != pubkey {
            bail!("generated account identity collision");
        }
    } else {
        if db.exists() {
            bail!("generated account path already contains data");
        }
        persist_keys(&db, mode, &keys).await?;
    }
    verify_persisted_identity(&db, mode, &pubkey).await?;
    Ok(AccountRecord {
        id,
        pubkey,
        label: None,
        created_at: now_millis(),
        last_used_at: now_millis(),
    })
}
