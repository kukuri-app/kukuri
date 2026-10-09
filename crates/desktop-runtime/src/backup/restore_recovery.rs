use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::atomic::{AtomicU8, Ordering};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

use super::{
    ACCOUNTS_DIR_NAME, ACCOUNTS_REGISTRY_FILE_NAME, DB_FILE_NAME, DeviceBackupRestoreResult,
    PreparedDeviceRestore, account_db_path, account_id_for_pubkey, known_optional_secret_locators,
    list_accounts, make_account_file_self_contained, register_restored_account,
    scrub_keyring_optional_secrets, unique_account_work_dir, validate_frontend_state,
    write_private_file_atomically,
};
use crate::identity::IdentityStorageMode;
use crate::storage::{ClientStorage, platform_storage};
use n0_future::future::block_on;

const RESTORE_JOURNAL_FILE: &str = "device-restore-journal.json";
const RESTORE_FRONTEND_STATE_FILE: &str = "device-restore-frontend-state.json";
const RESTORE_ROLLBACK_WORK_PREFIX: &str = ".device-restore-rollback";
const RESTORE_STAGING_ENTRY_PREFIX: &str = ".device-restore-staging-";

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum DeviceRestoreTestFailurePoint {
    AfterInstallingJournal = 1,
    FailExistingDirectoryMove = 2,
    AfterExistingDirectoryMove = 3,
    AfterStagingDirectoryMove = 4,
    FailInstalledJournalWrite = 5,
    RollbackAfterFinalDirectoryRemoval = 6,
    RollbackAfterOriginalDirectoryRestore = 7,
    AfterRegistryCommit = 8,
    AfterInstalledJournal = 9,
    AfterFrontendStateMarker = 10,
}

#[cfg(test)]
static DEVICE_RESTORE_TEST_FAILURE_POINT: AtomicU8 = AtomicU8::new(0);

#[cfg(test)]
pub(crate) struct DeviceRestoreFailureGuard;

#[cfg(test)]
impl Drop for DeviceRestoreFailureGuard {
    fn drop(&mut self) {
        DEVICE_RESTORE_TEST_FAILURE_POINT.store(0, Ordering::Release);
    }
}

#[cfg(test)]
pub(crate) fn fail_device_restore_at(
    point: DeviceRestoreTestFailurePoint,
) -> DeviceRestoreFailureGuard {
    let previous = DEVICE_RESTORE_TEST_FAILURE_POINT.swap(point as u8, Ordering::AcqRel);
    assert_eq!(previous, 0, "device restore failure is already active");
    DeviceRestoreFailureGuard
}

#[cfg(test)]
fn take_device_restore_failure(point: DeviceRestoreTestFailurePoint) -> bool {
    DEVICE_RESTORE_TEST_FAILURE_POINT
        .compare_exchange(point as u8, 0, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
}

pub struct InstalledDeviceRestore {
    app_data_dir: PathBuf,
    public_key: String,
    account_label: Option<String>,
    frontend_state: BTreeMap<String, String>,
    final_dir: PathBuf,
}

impl InstalledDeviceRestore {
    pub fn db_path(&self) -> PathBuf {
        self.final_dir.join(DB_FILE_NAME)
    }

    pub fn frontend_state(&self) -> &BTreeMap<String, String> {
        &self.frontend_state
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct RestoreJournal {
    version: u32,
    phase: DeviceRestorePhase,
    final_dir: PathBuf,
    staging_dir: PathBuf,
    rollback_dir: Option<PathBuf>,
    registry_before: String,
    #[serde(default)]
    frontend_state: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct PendingRestoreFrontendState {
    version: u32,
    frontend_state: BTreeMap<String, String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceRestorePhase {
    Installing,
    Installed,
    Committed,
    AwaitingConsent,
    Activated,
}

pub fn install_prepared_device_restore(
    app_data_dir: &Path,
    prepared: PreparedDeviceRestore,
) -> Result<InstalledDeviceRestore> {
    install_prepared_device_restore_with_storage(
        app_data_dir,
        prepared,
        IdentityStorageMode::from_env(),
        platform_storage(),
    )
}

pub(crate) fn install_prepared_device_restore_with_storage(
    app_data_dir: &Path,
    prepared: PreparedDeviceRestore,
    identity_mode: IdentityStorageMode,
    storage: &dyn ClientStorage,
) -> Result<InstalledDeviceRestore> {
    recover_interrupted_restore_inner(app_data_dir, Some(&prepared.staging_dir))?;
    if let Some(phase) = pending_device_restore_phase(app_data_dir)? {
        bail!("device restore transaction is already pending in phase {phase:?}");
    }
    let id = account_id_for_pubkey(&prepared.public_key)?;
    let final_db = account_db_path(app_data_dir, &id);
    let final_dir = final_db
        .parent()
        .ok_or_else(|| anyhow!("invalid restored account path"))?
        .to_path_buf();
    let snapshot = block_on(list_accounts(app_data_dir))?;
    let existing = snapshot
        .accounts
        .iter()
        .find(|account| account.pubkey == prepared.public_key);
    if existing.is_some() && !prepared.replace_existing {
        bail!(
            "device backup account already exists; explicit replacement confirmation is required"
        );
    }
    if existing.is_none() && final_dir.exists() {
        bail!("restored account directory already exists outside the registry");
    }

    let registry_path = app_data_dir.join(ACCOUNTS_REGISTRY_FILE_NAME);
    let registry_before = fs::read_to_string(&registry_path)
        .context("failed to snapshot accounts registry before restore")?;
    let staging_db = prepared.staging_db_path();
    let mut secret_locators = known_optional_secret_locators(&staging_db)?;
    let rollback_dir = if final_dir.exists() {
        secret_locators.extend(known_optional_secret_locators(&final_db)?);
        make_account_file_self_contained(&final_db, identity_mode, storage, &secret_locators)?;
        Some(unique_account_work_dir(
            app_data_dir,
            RESTORE_ROLLBACK_WORK_PREFIX,
        )?)
    } else {
        None
    };
    let mut journal = RestoreJournal {
        version: 1,
        phase: DeviceRestorePhase::Installing,
        final_dir: final_dir.clone(),
        staging_dir: prepared.staging_dir.clone(),
        rollback_dir: rollback_dir.clone(),
        registry_before,
        frontend_state: prepared.frontend_state.clone(),
    };
    write_restore_journal(app_data_dir, &journal)?;

    #[cfg(test)]
    if take_device_restore_failure(DeviceRestoreTestFailurePoint::AfterInstallingJournal) {
        std::mem::forget(prepared);
        bail!("simulated process stop after installing journal");
    }

    if let Some(rollback_dir) = &rollback_dir {
        #[cfg(test)]
        let move_existing_result = if take_device_restore_failure(
            DeviceRestoreTestFailurePoint::FailExistingDirectoryMove,
        ) {
            Err(std::io::Error::other(
                "simulated existing account directory move failure",
            ))
        } else {
            fs::rename(&final_dir, rollback_dir)
        };
        #[cfg(not(test))]
        let move_existing_result = fs::rename(&final_dir, rollback_dir);
        if let Err(error) = move_existing_result {
            return Err(error_with_restore_rollback(
                app_data_dir,
                &journal,
                error.into(),
                "failed to stage existing account rollback",
            ));
        }

        #[cfg(test)]
        if take_device_restore_failure(DeviceRestoreTestFailurePoint::AfterExistingDirectoryMove) {
            std::mem::forget(prepared);
            bail!("simulated process stop after existing account directory move");
        }
    }
    if let Err(error) = fs::rename(&prepared.staging_dir, &final_dir) {
        return Err(error_with_restore_rollback(
            app_data_dir,
            &journal,
            error.into(),
            "failed to install restored account directory",
        ));
    }

    #[cfg(test)]
    if take_device_restore_failure(DeviceRestoreTestFailurePoint::AfterStagingDirectoryMove) {
        std::mem::forget(prepared);
        bail!("simulated process stop after staging directory move");
    }

    // optional secretのkeyring accountはcanonical DB pathに紐づく。Windowsでも過去の
    // entryと同じpathを解決でき、かつ復元fileを消さないよう、final DB配置後に
    // keyring entryだけを削除する。runtimeはこのinstall完了前には構築されない。
    if let Err(error) = scrub_keyring_optional_secrets(&final_db, &secret_locators, storage) {
        return Err(error_with_restore_rollback(
            app_data_dir,
            &journal,
            error,
            "failed to clear pre-existing account secrets before restore",
        ));
    }

    journal.phase = DeviceRestorePhase::Installed;
    #[cfg(test)]
    let installed_journal_result =
        if take_device_restore_failure(DeviceRestoreTestFailurePoint::FailInstalledJournalWrite) {
            Err(anyhow!("simulated installed journal write failure"))
        } else {
            write_restore_journal(app_data_dir, &journal)
        };
    #[cfg(not(test))]
    let installed_journal_result = write_restore_journal(app_data_dir, &journal);
    if let Err(error) = installed_journal_result {
        return Err(error_with_restore_rollback(
            app_data_dir,
            &journal,
            error,
            "failed to persist installed device restore",
        ));
    }
    #[cfg(test)]
    if take_device_restore_failure(DeviceRestoreTestFailurePoint::AfterInstalledJournal) {
        std::mem::forget(prepared);
        bail!("simulated process stop after installed journal");
    }

    Ok(InstalledDeviceRestore {
        app_data_dir: app_data_dir.to_path_buf(),
        public_key: prepared.public_key.clone(),
        account_label: prepared.account_label.clone(),
        frontend_state: prepared.frontend_state.clone(),
        final_dir,
    })
}

pub fn commit_device_restore(
    installed: &InstalledDeviceRestore,
) -> Result<DeviceBackupRestoreResult> {
    let mut journal = load_restore_journal(&installed.app_data_dir)?
        .ok_or_else(|| anyhow!("device restore journal is missing"))?;
    validate_journal_paths(&installed.app_data_dir, &journal)?;
    if journal.phase != DeviceRestorePhase::Installed {
        bail!(
            "device restore registry cannot be committed from phase {:?}",
            journal.phase
        );
    }

    let account = match block_on(register_restored_account(
        &installed.app_data_dir,
        &installed.public_key,
        installed.account_label.clone(),
    )) {
        Ok(account) => account,
        Err(error) => {
            return Err(error_with_restore_rollback(
                &installed.app_data_dir,
                &journal,
                error,
                "failed to update restored account registry",
            ));
        }
    };
    #[cfg(test)]
    if take_device_restore_failure(DeviceRestoreTestFailurePoint::AfterRegistryCommit) {
        bail!("simulated process stop after restored account registry commit");
    }
    journal.phase = DeviceRestorePhase::Committed;
    if let Err(error) = write_restore_journal(&installed.app_data_dir, &journal) {
        return Err(error_with_restore_rollback(
            &installed.app_data_dir,
            &journal,
            error,
            "failed to persist committed device restore",
        ));
    }
    Ok(DeviceBackupRestoreResult {
        account,
        frontend_state: installed.frontend_state.clone(),
    })
}

pub fn finalize_device_restore(installed: InstalledDeviceRestore) -> Result<()> {
    finalize_pending_device_restore(&installed.app_data_dir)
}

pub fn rollback_device_restore(installed: &InstalledDeviceRestore) -> Result<()> {
    rollback_pending_device_restore(&installed.app_data_dir)
}

pub fn recover_interrupted_restore(app_data_dir: &Path) -> Result<()> {
    recover_interrupted_restore_inner(app_data_dir, None)
}

/// 現在の復元transaction phaseを返す。journalが無ければ復元待ちは無い。
pub fn pending_device_restore_phase(app_data_dir: &Path) -> Result<Option<DeviceRestorePhase>> {
    let Some(journal) = load_restore_journal(app_data_dir)? else {
        return Ok(None);
    };
    validate_journal_paths(app_data_dir, &journal)?;
    Ok(Some(journal.phase))
}

/// activation済みだがfrontendが未反映のportable stateを返す。
pub fn pending_device_restore_frontend_state(
    app_data_dir: &Path,
) -> Result<Option<BTreeMap<String, String>>> {
    Ok(load_pending_restore_frontend_state(app_data_dir)?.map(|pending| pending.frontend_state))
}

/// frontend state反映済みmarkerを削除する。同じackのretryは成功として扱う。
pub fn acknowledge_pending_device_restore_frontend_state(app_data_dir: &Path) -> Result<()> {
    if load_pending_restore_frontend_state(app_data_dir)?.is_none() {
        return Ok(());
    }
    match fs::remove_file(restore_frontend_state_path(app_data_dir)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("failed to acknowledge restored frontend state"),
    }
}

/// registry commit後にapp-level同意をresetし終えたことを記録する。
///
/// 同じ遷移のretryは成功として扱い、process停止後も再実行可能にする。
pub fn mark_device_restore_awaiting_consent(app_data_dir: &Path) -> Result<()> {
    transition_device_restore_phase(
        app_data_dir,
        DeviceRestorePhase::Committed,
        DeviceRestorePhase::AwaitingConsent,
    )
}

/// 明示的な再同意後に復元runtimeの構築が成功したことを記録する。
///
/// cleanupより先にこのphaseをdurableにし、cleanup途中の停止はstartupでfinish-forwardする。
pub fn mark_device_restore_activated(app_data_dir: &Path) -> Result<()> {
    transition_device_restore_phase(
        app_data_dir,
        DeviceRestorePhase::AwaitingConsent,
        DeviceRestorePhase::Activated,
    )
}

/// process再起動側から、未完了の復元を旧accountへ戻す。
pub fn rollback_pending_device_restore(app_data_dir: &Path) -> Result<()> {
    rollback_pending_device_restore_with_storage(app_data_dir, platform_storage())
}

pub(crate) fn rollback_pending_device_restore_with_storage(
    app_data_dir: &Path,
    storage: &dyn ClientStorage,
) -> Result<()> {
    let Some(journal) = load_restore_journal(app_data_dir)? else {
        cleanup_orphan_restore_staging_dirs(app_data_dir, None)?;
        return Ok(());
    };
    validate_journal_paths(app_data_dir, &journal)?;
    if journal.phase == DeviceRestorePhase::Activated {
        bail!("activated device restore cannot be rolled back");
    }
    rollback_from_journal(app_data_dir, &journal, storage)?;
    cleanup_orphan_restore_staging_dirs(app_data_dir, None)
}

/// activation済み復元のrollback directoryとjournalを削除する。
pub fn finalize_pending_device_restore(app_data_dir: &Path) -> Result<()> {
    let Some(journal) = load_restore_journal(app_data_dir)? else {
        cleanup_orphan_restore_staging_dirs(app_data_dir, None)?;
        return Ok(());
    };
    validate_journal_paths(app_data_dir, &journal)?;
    if journal.phase != DeviceRestorePhase::Activated {
        bail!(
            "device restore cannot be finalized from phase {:?}",
            journal.phase
        );
    }
    finalize_from_journal(app_data_dir, &journal)?;
    cleanup_orphan_restore_staging_dirs(app_data_dir, None)
}

fn recover_interrupted_restore_inner(
    app_data_dir: &Path,
    preserved_staging_dir: Option<&Path>,
) -> Result<()> {
    let journal = load_restore_journal(app_data_dir)?;
    if let Some(journal) = &journal {
        validate_journal_paths(app_data_dir, journal)?;
        match journal.phase {
            DeviceRestorePhase::Installing | DeviceRestorePhase::Installed => {
                rollback_from_journal(app_data_dir, journal, platform_storage())?;
            }
            DeviceRestorePhase::Committed | DeviceRestorePhase::AwaitingConsent => {
                // app-level同意のresetまたは明示的な再同意を待つ。通常recoveryは
                // どちらにも決め打ちせず、startup orchestrationへ判断を返す。
            }
            DeviceRestorePhase::Activated => finalize_from_journal(app_data_dir, journal)?,
        }
    }

    let journal_staging_dir =
        load_restore_journal(app_data_dir)?.map(|journal| journal.staging_dir);
    let preserved = journal_staging_dir.as_deref().or(preserved_staging_dir);
    cleanup_orphan_restore_staging_dirs(app_data_dir, preserved)
}

fn rollback_from_journal(
    app_data_dir: &Path,
    journal: &RestoreJournal,
    storage: &dyn ClientStorage,
) -> Result<()> {
    validate_journal_paths(app_data_dir, journal)?;
    // activation 前に復元した値が keyring へ移っていても、元の file と「未保存」が正本になるよう片付ける（#1195 AC-2）。
    let final_db = journal.final_dir.join(DB_FILE_NAME);
    let mut locators = known_optional_secret_locators(&final_db)?;
    if let Some(rollback_dir) = &journal.rollback_dir {
        locators.extend(known_optional_secret_locators(
            &rollback_dir.join(DB_FILE_NAME),
        )?);
    }
    scrub_keyring_optional_secrets(&final_db, &locators, storage)?;
    match &journal.rollback_dir {
        Some(rollback_dir) if rollback_dir.exists() => {
            if journal.final_dir.exists() {
                fs::remove_dir_all(&journal.final_dir)
                    .context("failed to remove incomplete restored account")?;
                #[cfg(test)]
                if take_device_restore_failure(
                    DeviceRestoreTestFailurePoint::RollbackAfterFinalDirectoryRemoval,
                ) {
                    bail!("simulated process stop after restored directory removal");
                }
            }
            fs::rename(rollback_dir, &journal.final_dir)
                .context("failed to restore previous account directory")?;
            #[cfg(test)]
            if take_device_restore_failure(
                DeviceRestoreTestFailurePoint::RollbackAfterOriginalDirectoryRestore,
            ) {
                bail!("simulated process stop after original directory restore");
            }
        }
        Some(_) => {
            // journal保存直後で旧directoryをまだ移動していない場合と、recoveryが
            // 既にrollback directoryを戻した後に停止した場合は同じ配置になる。
            // どちらもfinal directoryが旧状態なので削除してはならない。
            if !journal.final_dir.exists() {
                bail!("device restore rollback and original account directories are both missing");
            }
        }
        None => {
            // 新規accountのtransactionには戻すdirectoryが無い。finalが存在すれば
            // staging rename済みの未commit accountなので削除する。
            if journal.final_dir.exists() {
                fs::remove_dir_all(&journal.final_dir)
                    .context("failed to remove incomplete restored account")?;
                #[cfg(test)]
                if take_device_restore_failure(
                    DeviceRestoreTestFailurePoint::RollbackAfterFinalDirectoryRemoval,
                ) {
                    bail!("simulated process stop after restored directory removal");
                }
            }
        }
    }
    if journal.staging_dir.exists() {
        fs::remove_dir_all(&journal.staging_dir)
            .context("failed to remove device restore staging")?;
    }
    write_private_file_atomically(
        &app_data_dir.join(ACCOUNTS_REGISTRY_FILE_NAME),
        journal.registry_before.as_bytes(),
    )?;
    let journal_path = restore_journal_path(app_data_dir);
    if journal_path.exists() {
        fs::remove_file(journal_path).context("failed to clear device restore journal")?;
    }
    Ok(())
}

fn finalize_from_journal(app_data_dir: &Path, journal: &RestoreJournal) -> Result<()> {
    validate_journal_paths(app_data_dir, journal)?;
    persist_pending_restore_frontend_state(app_data_dir, &journal.frontend_state)?;
    #[cfg(test)]
    if take_device_restore_failure(DeviceRestoreTestFailurePoint::AfterFrontendStateMarker) {
        bail!("simulated process stop after restored frontend state marker");
    }
    if let Some(rollback_dir) = &journal.rollback_dir
        && rollback_dir.exists()
    {
        fs::remove_dir_all(rollback_dir)
            .context("failed to remove completed device restore rollback")?;
    }
    if journal.staging_dir.exists() {
        fs::remove_dir_all(&journal.staging_dir)
            .context("failed to remove completed device restore staging")?;
    }
    let journal_path = restore_journal_path(app_data_dir);
    if journal_path.exists() {
        fs::remove_file(journal_path).context("failed to clear device restore journal")?;
    }
    Ok(())
}

fn transition_device_restore_phase(
    app_data_dir: &Path,
    expected: DeviceRestorePhase,
    next: DeviceRestorePhase,
) -> Result<()> {
    let mut journal = load_restore_journal(app_data_dir)?
        .ok_or_else(|| anyhow!("device restore journal is missing"))?;
    validate_journal_paths(app_data_dir, &journal)?;
    if journal.phase == next {
        return Ok(());
    }
    if journal.phase != expected {
        bail!(
            "device restore phase transition requires {expected:?}, found {:?}",
            journal.phase
        );
    }
    journal.phase = next;
    write_restore_journal(app_data_dir, &journal)
}

fn cleanup_orphan_restore_staging_dirs(
    app_data_dir: &Path,
    preserved_staging_dir: Option<&Path>,
) -> Result<()> {
    let accounts_root = app_data_dir.join(ACCOUNTS_DIR_NAME);
    if !accounts_root.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(&accounts_root).context("failed to list accounts directory")? {
        let entry = entry.context("failed to inspect accounts directory entry")?;
        let path = entry.path();
        if preserved_staging_dir == Some(path.as_path()) {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.starts_with(RESTORE_STAGING_ENTRY_PREFIX) {
            continue;
        }
        if entry
            .file_type()
            .context("failed to inspect device restore staging entry")?
            .is_dir()
        {
            fs::remove_dir_all(&path).with_context(|| {
                format!(
                    "failed to remove orphan device restore staging `{}`",
                    path.display()
                )
            })?;
        }
    }
    Ok(())
}

fn error_with_restore_rollback(
    app_data_dir: &Path,
    journal: &RestoreJournal,
    operation_error: anyhow::Error,
    context: &str,
) -> anyhow::Error {
    match rollback_from_journal(app_data_dir, journal, platform_storage()) {
        Ok(()) => operation_error.context(context.to_string()),
        Err(rollback_error) => {
            anyhow!("{context}: {operation_error:#}; rollback failed: {rollback_error:#}")
        }
    }
}

fn restore_journal_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join(RESTORE_JOURNAL_FILE)
}

fn restore_frontend_state_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join(RESTORE_FRONTEND_STATE_FILE)
}

fn persist_pending_restore_frontend_state(
    app_data_dir: &Path,
    frontend_state: &BTreeMap<String, String>,
) -> Result<()> {
    validate_frontend_state(frontend_state)?;
    let pending = PendingRestoreFrontendState {
        version: 1,
        frontend_state: frontend_state.clone(),
    };
    if let Some(existing) = load_pending_restore_frontend_state(app_data_dir)? {
        if existing == pending {
            return Ok(());
        }
        bail!("another restored frontend state is awaiting acknowledgement");
    }
    let bytes = serde_json::to_vec(&pending).context("failed to encode restored frontend state")?;
    write_private_file_atomically(&restore_frontend_state_path(app_data_dir), &bytes)
        .context("failed to persist restored frontend state")
}

fn load_pending_restore_frontend_state(
    app_data_dir: &Path,
) -> Result<Option<PendingRestoreFrontendState>> {
    let path = restore_frontend_state_path(app_data_dir);
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(&path).context("failed to read restored frontend state")?;
    let pending: PendingRestoreFrontendState =
        serde_json::from_slice(&bytes).context("invalid restored frontend state")?;
    if pending.version != 1 {
        bail!("unsupported restored frontend state version");
    }
    validate_frontend_state(&pending.frontend_state)?;
    Ok(Some(pending))
}

fn write_restore_journal(app_data_dir: &Path, journal: &RestoreJournal) -> Result<()> {
    validate_journal_paths(app_data_dir, journal)?;
    let bytes = serde_json::to_vec(journal).context("failed to encode device restore journal")?;
    write_private_file_atomically(&restore_journal_path(app_data_dir), &bytes)
}

fn load_restore_journal(app_data_dir: &Path) -> Result<Option<RestoreJournal>> {
    let path = restore_journal_path(app_data_dir);
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(&path).context("failed to read device restore journal")?;
    let journal: RestoreJournal =
        serde_json::from_slice(&bytes).context("invalid device restore journal")?;
    if journal.version != 1 {
        bail!("unsupported device restore journal version");
    }
    Ok(Some(journal))
}

fn validate_journal_paths(app_data_dir: &Path, journal: &RestoreJournal) -> Result<()> {
    let accounts_root = app_data_dir.join(ACCOUNTS_DIR_NAME);
    for path in [
        Some(&journal.final_dir),
        Some(&journal.staging_dir),
        journal.rollback_dir.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        if path.parent() != Some(accounts_root.as_path()) {
            bail!("device restore journal contains a path outside the accounts directory");
        }
    }
    Ok(())
}
