use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use kukuri_core::{DeviceBackupReader, KukuriKeys};
use kukuri_store::SqliteStore;
use serde::{Deserialize, Serialize};

use crate::accounts::{
    ACCOUNTS_DIR_NAME, ACCOUNTS_REGISTRY_FILE_NAME, AccountRecord, account_db_path,
    account_id_for_pubkey, list_accounts, register_restored_account,
};
use crate::community_node::{
    COMMUNITY_NODE_CONSENT_PURPOSE, COMMUNITY_NODE_INVITE_CODE_PURPOSE,
    COMMUNITY_NODE_TOKEN_PURPOSE, load_community_node_config_from_file,
};
use crate::discovery::load_discovery_config_from_file;
use crate::identity::{
    IdentityStorageMode, delete_optional_secret_keyring_entry_with_storage, load_existing_keys,
    load_existing_keys_with_storage, load_optional_secret, load_optional_secret_with_storage,
    persist_keys, persist_keys_with_storage, persist_optional_secret,
    persist_optional_secret_with_storage,
};
use crate::storage::{ClientStorage, write_private_file_atomically};
// backup・restore は native だけの同期の処理。native の保存先の future は I/O を待たずに終わるので block_on で呼ぶ。
use crate::paths::DB_FILE_NAME;
use crate::runtime::{
    GOSSIP_SUBSCRIPTION_STATE_KEY, GOSSIP_SUBSCRIPTION_STATE_PURPOSE,
    PRIVATE_CHANNEL_CAPABILITIES_KEY, PRIVATE_CHANNEL_CAPABILITIES_PURPOSE,
    validate_persisted_runtime_state,
};
use n0_future::future::block_on;

const SECRETS_ENTRY: &str = "secret-bundle.json";
const FRONTEND_STATE_ENTRY: &str = "frontend-state.json";
const FILE_ENTRY_PREFIX: &str = "file/";
const FRONTEND_STATE_MAX_BYTES: usize = 2 * 1024 * 1024;
const RESTORE_STAGING_WORK_PREFIX: &str = ".device-restore-staging";

struct DeviceBackupOutputFile {
    file: File,
}

impl DeviceBackupOutputFile {
    fn create_new(path: &Path) -> std::io::Result<Self> {
        let file = OpenOptions::new().create_new(true).write(true).open(path)?;
        Ok(Self { file })
    }

    fn sync_all(&self) -> std::io::Result<()> {
        self.file.sync_all()
    }
}

impl Write for DeviceBackupOutputFile {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        #[cfg(test)]
        {
            let remaining = DEVICE_BACKUP_TEST_WRITE_BUDGET.load(Ordering::Acquire);
            if remaining != u64::MAX {
                if remaining == 0 {
                    return Err(std::io::Error::other(
                        "simulated device backup storage exhaustion",
                    ));
                }
                let allowed = bytes
                    .len()
                    .min(usize::try_from(remaining).unwrap_or(usize::MAX));
                let written = self.file.write(&bytes[..allowed])?;
                DEVICE_BACKUP_TEST_WRITE_BUDGET.fetch_sub(written as u64, Ordering::AcqRel);
                return Ok(written);
            }
        }
        self.file.write(bytes)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

#[cfg(test)]
static DEVICE_BACKUP_TEST_WRITE_BUDGET: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(u64::MAX);

#[cfg(test)]
pub(crate) struct DeviceBackupWriteFailureGuard;

#[cfg(test)]
impl Drop for DeviceBackupWriteFailureGuard {
    fn drop(&mut self) {
        DEVICE_BACKUP_TEST_WRITE_BUDGET.store(u64::MAX, Ordering::Release);
    }
}

#[cfg(test)]
pub(crate) fn fail_device_backup_writes_after(bytes: u64) -> DeviceBackupWriteFailureGuard {
    let previous = DEVICE_BACKUP_TEST_WRITE_BUDGET.swap(bytes, Ordering::AcqRel);
    assert_eq!(
        previous,
        u64::MAX,
        "device backup write failure is already active"
    );
    DeviceBackupWriteFailureGuard
}

mod create;
mod restore_recovery;

pub use create::create_device_backup;
use create::validate_frontend_state;
pub use restore_recovery::{
    DeviceRestorePhase, InstalledDeviceRestore, acknowledge_pending_device_restore_frontend_state,
    commit_device_restore, finalize_device_restore, finalize_pending_device_restore,
    install_prepared_device_restore, mark_device_restore_activated,
    mark_device_restore_awaiting_consent, pending_device_restore_frontend_state,
    pending_device_restore_phase, recover_interrupted_restore, rollback_device_restore,
    rollback_pending_device_restore,
};
#[cfg(test)]
pub(crate) use restore_recovery::{
    DeviceRestoreTestFailurePoint, fail_device_restore_at,
    install_prepared_device_restore_with_storage,
};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct CreateDeviceBackupRequest {
    pub path: String,
    pub passphrase: String,
    #[serde(default)]
    pub frontend_state: BTreeMap<String, String>,
}

impl std::fmt::Debug for CreateDeviceBackupRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CreateDeviceBackupRequest")
            .field("path", &self.path)
            .field("passphrase", &"<redacted>")
            .field("frontend_state_keys", &self.frontend_state.keys())
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct PreviewDeviceBackupRequest {
    pub path: String,
    pub passphrase: String,
}

impl std::fmt::Debug for PreviewDeviceBackupRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreviewDeviceBackupRequest")
            .field("path", &self.path)
            .field("passphrase", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct RestoreDeviceBackupRequest {
    pub path: String,
    pub passphrase: String,
    #[serde(default)]
    pub replace_existing: bool,
    #[serde(default)]
    pub apply_frontend_state: bool,
}

impl std::fmt::Debug for RestoreDeviceBackupRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RestoreDeviceBackupRequest")
            .field("path", &self.path)
            .field("passphrase", &"<redacted>")
            .field("replace_existing", &self.replace_existing)
            .field("apply_frontend_state", &self.apply_frontend_state)
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct DeviceBackupSummary {
    pub path: String,
    pub public_key: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct DeviceBackupPreview {
    pub public_key: String,
    #[serde(default)]
    pub account_label: Option<String>,
    pub created_at: i64,
    pub app_version: String,
    pub content_bytes: u64,
    pub existing_account_id: Option<String>,
    pub included: Vec<String>,
    pub requires_reconsent: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct DeviceBackupRestoreResult {
    pub account: AccountRecord,
    #[serde(default)]
    pub frontend_state: BTreeMap<String, String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum DeviceBackupPhase {
    Scanning,
    Encrypting,
    Decrypting,
    Installing,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct DeviceBackupProgress {
    pub phase: DeviceBackupPhase,
    pub completed_bytes: u64,
    pub total_bytes: u64,
}

#[derive(Clone, Default)]
pub struct DeviceBackupCancellation(Arc<AtomicBool>);

impl DeviceBackupCancellation {
    pub fn reset(&self) {
        self.0.store(false, Ordering::Release);
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn check(&self) -> Result<()> {
        if self.0.load(Ordering::Acquire) {
            bail!("device backup operation canceled");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SecretBundleV1 {
    version: u32,
    identity_secret_hex: String,
    private_channel_capabilities: Option<String>,
    gossip_subscription_state: Option<String>,
    community_node_invite_codes: BTreeMap<String, String>,
}

#[derive(Clone, Debug)]
struct OptionalSecretValue {
    purpose: String,
    key: String,
    value: String,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct OptionalSecretLocator {
    purpose: String,
    key: String,
}

pub struct PreparedDeviceRestore {
    staging_dir: PathBuf,
    public_key: String,
    account_label: Option<String>,
    frontend_state: BTreeMap<String, String>,
    replace_existing: bool,
}

impl PreparedDeviceRestore {
    pub fn staging_db_path(&self) -> PathBuf {
        self.staging_dir.join(DB_FILE_NAME)
    }
}

/// 復元stagingを、runtime・Iroh・background taskを構築せずに検証する。
pub async fn validate_prepared_device_restore(prepared: &PreparedDeviceRestore) -> Result<()> {
    let db_path = prepared.staging_db_path();
    if !db_path.is_file() {
        bail!("restored account database is missing");
    }

    let store = SqliteStore::connect_file(&db_path).await?;
    store.close().await;

    // stagingはprepare時にFileOnlyへ自己完結させる。Autoにすると同じpathへ残った
    // keyring entryが復元fileをshadowし、端末環境によって検証結果が変わる。
    let identity_mode = IdentityStorageMode::FileOnly;
    let keys = load_existing_keys(&db_path, identity_mode)
        .await?
        .ok_or_else(|| anyhow!("restored account identity is missing"))?;
    if keys.public_key_hex() != prepared.public_key {
        bail!("restored account identity does not match the backup manifest");
    }
    let _ = load_community_node_config_from_file(&db_path).await?;
    let _ = load_discovery_config_from_file(&db_path).await?;
    validate_persisted_runtime_state(&db_path, identity_mode).await?;
    Ok(())
}

impl Drop for PreparedDeviceRestore {
    fn drop(&mut self) {
        if self.staging_dir.exists() {
            let _ = fs::remove_dir_all(&self.staging_dir);
        }
    }
}

pub fn preview_device_backup(
    app_data_dir: &Path,
    request: &PreviewDeviceBackupRequest,
) -> Result<DeviceBackupPreview> {
    let source = PathBuf::from(request.path.trim());
    ensure_backup_path_outside_app_data(app_data_dir, &source)?;
    let file = File::open(&source).context("failed to open device backup")?;
    let archive = DeviceBackupReader::open(file, &request.passphrase)?;
    let manifest = archive.manifest();
    let content_bytes = manifest.entries.iter().try_fold(0u64, |total, entry| {
        total
            .checked_add(entry.bytes)
            .ok_or_else(|| anyhow!("device backup total size overflow"))
    })?;
    let existing_account_id = block_on(list_accounts(app_data_dir))?
        .accounts
        .into_iter()
        .find(|account| account.pubkey == manifest.public_key)
        .map(|account| account.id);
    Ok(DeviceBackupPreview {
        public_key: manifest.public_key.clone(),
        account_label: manifest.account_label.clone(),
        created_at: manifest.created_at,
        app_version: manifest.app_version.clone(),
        content_bytes,
        existing_account_id,
        included: manifest.included.clone(),
        requires_reconsent: manifest.requires_reconsent.clone(),
    })
}

pub fn prepare_device_restore<F>(
    app_data_dir: &Path,
    request: &RestoreDeviceBackupRequest,
    cancellation: &DeviceBackupCancellation,
    mut progress: F,
) -> Result<PreparedDeviceRestore>
where
    F: FnMut(DeviceBackupProgress),
{
    recover_interrupted_restore(app_data_dir)?;
    if let Some(phase) = pending_device_restore_phase(app_data_dir)? {
        bail!("device restore transaction is already pending in phase {phase:?}");
    }
    if pending_device_restore_frontend_state(app_data_dir)?.is_some() {
        bail!("restored frontend state must be acknowledged before another restore");
    }
    cancellation.check()?;
    let source = PathBuf::from(request.path.trim());
    ensure_backup_path_outside_app_data(app_data_dir, &source)?;
    let file = File::open(&source).context("failed to open device backup")?;
    let mut archive = DeviceBackupReader::open(file, &request.passphrase)?;
    let manifest = archive.manifest().clone();
    let total_bytes = manifest.entries.iter().try_fold(0u64, |total, entry| {
        total
            .checked_add(entry.bytes)
            .ok_or_else(|| anyhow!("device backup total size overflow"))
    })?;
    let snapshot = block_on(list_accounts(app_data_dir))?;
    let existing = snapshot
        .accounts
        .iter()
        .find(|account| account.pubkey == manifest.public_key);
    if existing.is_some() && !request.replace_existing {
        bail!(
            "device backup account already exists; explicit replacement confirmation is required"
        );
    }

    let staging_dir = unique_account_work_dir(app_data_dir, RESTORE_STAGING_WORK_PREFIX)?;
    fs::create_dir_all(&staging_dir)
        .context("failed to create device restore staging directory")?;
    let extraction = (|| -> Result<(SecretBundleV1, BTreeMap<String, String>)> {
        let mut secret_bundle = None;
        let mut frontend_state = BTreeMap::new();
        let mut completed = 0u64;
        for entry in &manifest.entries {
            cancellation.check()?;
            match entry.name.as_str() {
                SECRETS_ENTRY => {
                    if entry.bytes > FRONTEND_STATE_MAX_BYTES as u64 {
                        bail!("device backup secret bundle exceeds supported size");
                    }
                    let mut bytes = Vec::with_capacity(entry.bytes as usize);
                    archive.read_entry(&mut bytes, |delta| {
                        cancellation.check()?;
                        completed += delta;
                        progress(DeviceBackupProgress {
                            phase: DeviceBackupPhase::Decrypting,
                            completed_bytes: completed,
                            total_bytes,
                        });
                        Ok(())
                    })?;
                    secret_bundle = Some(
                        serde_json::from_slice(&bytes)
                            .context("invalid device backup secret bundle")?,
                    );
                }
                FRONTEND_STATE_ENTRY => {
                    if entry.bytes > FRONTEND_STATE_MAX_BYTES as u64 {
                        bail!("device backup frontend state exceeds supported size");
                    }
                    let mut bytes = Vec::with_capacity(entry.bytes as usize);
                    archive.read_entry(&mut bytes, |delta| {
                        cancellation.check()?;
                        completed += delta;
                        progress(DeviceBackupProgress {
                            phase: DeviceBackupPhase::Decrypting,
                            completed_bytes: completed,
                            total_bytes,
                        });
                        Ok(())
                    })?;
                    frontend_state = serde_json::from_slice(&bytes)
                        .context("invalid device backup frontend state")?;
                    validate_frontend_state(&frontend_state)?;
                }
                name if name.starts_with(FILE_ENTRY_PREFIX) => {
                    let relative = safe_relative_path(&name[FILE_ENTRY_PREFIX.len()..])?;
                    let target = staging_dir.join(relative);
                    if let Some(parent) = target.parent() {
                        fs::create_dir_all(parent)
                            .context("failed to create device restore entry directory")?;
                    }
                    let output = DeviceBackupOutputFile::create_new(&target)
                        .with_context(|| format!("failed to create `{}`", target.display()))?;
                    archive.read_entry(output, |delta| {
                        cancellation.check()?;
                        completed += delta;
                        progress(DeviceBackupProgress {
                            phase: DeviceBackupPhase::Decrypting,
                            completed_bytes: completed,
                            total_bytes,
                        });
                        Ok(())
                    })?;
                }
                _ => bail!("device backup contains an unsupported entry"),
            }
        }
        archive.finish()?;
        let secret_bundle = secret_bundle
            .ok_or_else(|| anyhow!("device backup does not contain its secret bundle"))?;
        Ok((secret_bundle, frontend_state))
    })();

    let (secret_bundle, frontend_state) = match extraction {
        Ok(value) => value,
        Err(error) => {
            let _ = fs::remove_dir_all(&staging_dir);
            return Err(error);
        }
    };
    let validation = (|| -> Result<()> {
        if secret_bundle.version != 1 {
            bail!("unsupported device backup secret bundle version");
        }
        let keys = KukuriKeys::parse(&secret_bundle.identity_secret_hex)
            .context("device backup identity is invalid")?;
        if keys.public_key_hex() != manifest.public_key {
            bail!("device backup identity does not match its manifest");
        }
        let staging_db = staging_dir.join(DB_FILE_NAME);
        if !staging_db.is_file() {
            bail!("device backup does not contain its SQLite database");
        }
        block_on(crate::host::load_desired_subscriptions(&staging_db))
            .context("device backup desired subscription state is invalid")?;
        block_on(persist_keys(
            &staging_db,
            IdentityStorageMode::FileOnly,
            &keys,
        ))?;
        persist_restored_secrets(&staging_db, &secret_bundle)
    })();
    if let Err(error) = validation {
        let _ = fs::remove_dir_all(&staging_dir);
        return Err(error);
    }

    Ok(PreparedDeviceRestore {
        staging_dir,
        public_key: manifest.public_key,
        account_label: manifest.account_label,
        frontend_state: if request.apply_frontend_state {
            frontend_state
        } else {
            BTreeMap::new()
        },
        replace_existing: request.replace_existing,
    })
}

fn active_account_for_db(app_data_dir: &Path, db_path: &Path) -> Result<AccountRecord> {
    let snapshot = block_on(list_accounts(app_data_dir))?;
    let active = snapshot
        .accounts
        .into_iter()
        .find(|account| account.id == snapshot.active_account_id)
        .ok_or_else(|| anyhow!("active account is missing from registry"))?;
    if account_db_path(app_data_dir, &active.id) != db_path {
        bail!("active account database does not match the running runtime");
    }
    Ok(active)
}

fn ensure_backup_path_outside_app_data(app_data_dir: &Path, path: &Path) -> Result<()> {
    let canonical_app_data = app_data_dir
        .canonicalize()
        .context("failed to resolve app data directory")?;
    let canonical_candidate = if path.exists() {
        path.canonicalize()
            .context("failed to resolve device backup path")?
    } else {
        let parent = path
            .parent()
            .filter(|value| !value.as_os_str().is_empty())
            .ok_or_else(|| anyhow!("device backup path must have a parent directory"))?;
        let file_name = path
            .file_name()
            .ok_or_else(|| anyhow!("device backup path must name a file"))?;
        parent
            .canonicalize()
            .context("failed to resolve device backup parent directory")?
            .join(file_name)
    };
    if canonical_candidate.starts_with(&canonical_app_data) {
        bail!("device backup file must be stored outside the kukuri app data directory");
    }
    Ok(())
}

fn collect_secret_bundle(db_path: &Path) -> Result<SecretBundleV1> {
    let keys = block_on(load_existing_keys(db_path, IdentityStorageMode::from_env()))?
        .ok_or_else(|| anyhow!("active account identity is unavailable"))?;
    let private_channel_capabilities = block_on(load_optional_secret(
        db_path,
        IdentityStorageMode::from_env(),
        PRIVATE_CHANNEL_CAPABILITIES_PURPOSE,
        PRIVATE_CHANNEL_CAPABILITIES_KEY,
    ))?;
    let gossip_subscription_state = block_on(load_optional_secret(
        db_path,
        IdentityStorageMode::from_env(),
        GOSSIP_SUBSCRIPTION_STATE_PURPOSE,
        GOSSIP_SUBSCRIPTION_STATE_KEY,
    ))?;
    let mut community_node_invite_codes = BTreeMap::new();
    if let Some(config) = block_on(load_community_node_config_from_file(db_path))? {
        for node in config.nodes {
            if let Some(value) = block_on(load_optional_secret(
                db_path,
                IdentityStorageMode::from_env(),
                COMMUNITY_NODE_INVITE_CODE_PURPOSE,
                &node.base_url,
            ))? {
                community_node_invite_codes.insert(node.base_url, value);
            }
        }
    }
    Ok(SecretBundleV1 {
        version: 1,
        identity_secret_hex: keys.export_secret_hex(),
        private_channel_capabilities,
        gossip_subscription_state,
        community_node_invite_codes,
    })
}

fn persist_restored_secrets(db_path: &Path, bundle: &SecretBundleV1) -> Result<()> {
    if let Some(value) = &bundle.private_channel_capabilities {
        block_on(persist_optional_secret(
            db_path,
            IdentityStorageMode::FileOnly,
            PRIVATE_CHANNEL_CAPABILITIES_PURPOSE,
            PRIVATE_CHANNEL_CAPABILITIES_KEY,
            value,
        ))?;
    }
    if let Some(value) = &bundle.gossip_subscription_state {
        block_on(persist_optional_secret(
            db_path,
            IdentityStorageMode::FileOnly,
            GOSSIP_SUBSCRIPTION_STATE_PURPOSE,
            GOSSIP_SUBSCRIPTION_STATE_KEY,
            value,
        ))?;
    }
    for (base_url, value) in &bundle.community_node_invite_codes {
        block_on(persist_optional_secret(
            db_path,
            IdentityStorageMode::FileOnly,
            COMMUNITY_NODE_INVITE_CODE_PURPOSE,
            base_url,
            value,
        ))?;
    }
    Ok(())
}

fn known_optional_secret_locators(db_path: &Path) -> Result<BTreeSet<OptionalSecretLocator>> {
    let mut locators = BTreeSet::from([
        OptionalSecretLocator {
            purpose: PRIVATE_CHANNEL_CAPABILITIES_PURPOSE.to_string(),
            key: PRIVATE_CHANNEL_CAPABILITIES_KEY.to_string(),
        },
        OptionalSecretLocator {
            purpose: GOSSIP_SUBSCRIPTION_STATE_PURPOSE.to_string(),
            key: GOSSIP_SUBSCRIPTION_STATE_KEY.to_string(),
        },
    ]);
    if let Some(config) = block_on(load_community_node_config_from_file(db_path))? {
        for node in config.nodes {
            for purpose in [
                COMMUNITY_NODE_TOKEN_PURPOSE,
                COMMUNITY_NODE_INVITE_CODE_PURPOSE,
                COMMUNITY_NODE_CONSENT_PURPOSE,
            ] {
                locators.insert(OptionalSecretLocator {
                    purpose: purpose.to_string(),
                    key: node.base_url.clone(),
                });
            }
        }
    }
    Ok(locators)
}

fn load_optional_secrets(
    db_path: &Path,
    mode: IdentityStorageMode,
    storage: &dyn ClientStorage,
    locators: &BTreeSet<OptionalSecretLocator>,
) -> Result<Vec<OptionalSecretValue>> {
    let mut values = Vec::new();
    for locator in locators {
        if let Some(value) = block_on(load_optional_secret_with_storage(
            db_path,
            mode,
            &locator.purpose,
            &locator.key,
            storage,
        ))? {
            values.push(OptionalSecretValue {
                purpose: locator.purpose.clone(),
                key: locator.key.clone(),
                value,
            });
        }
    }
    Ok(values)
}

fn make_account_file_self_contained(
    db_path: &Path,
    mode: IdentityStorageMode,
    storage: &dyn ClientStorage,
    locators: &BTreeSet<OptionalSecretLocator>,
) -> Result<()> {
    if let Some(keys) = block_on(load_existing_keys_with_storage(db_path, mode, storage))? {
        block_on(persist_keys_with_storage(
            db_path,
            IdentityStorageMode::FileOnly,
            &keys,
            storage,
        ))?;
    }
    let secrets = load_optional_secrets(db_path, mode, storage, locators)?;
    for secret in &secrets {
        block_on(persist_optional_secret_with_storage(
            db_path,
            IdentityStorageMode::FileOnly,
            &secret.purpose,
            &secret.key,
            &secret.value,
            storage,
        ))?;
    }
    Ok(())
}

fn scrub_keyring_optional_secrets(
    db_path: &Path,
    locators: &BTreeSet<OptionalSecretLocator>,
    storage: &dyn ClientStorage,
) -> Result<()> {
    for locator in locators {
        block_on(delete_optional_secret_keyring_entry_with_storage(
            db_path,
            &locator.purpose,
            &locator.key,
            storage,
        ))?;
    }
    Ok(())
}

fn safe_relative_path(value: &str) -> Result<PathBuf> {
    if value.is_empty() || value.contains('\\') {
        bail!("device backup contains an invalid entry path");
    }
    let path = Path::new(value);
    let mut safe = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => safe.push(value),
            _ => bail!("device backup contains an invalid entry path"),
        }
    }
    if safe.as_os_str().is_empty() {
        bail!("device backup contains an empty entry path");
    }
    Ok(safe)
}

fn unique_account_work_dir(app_data_dir: &Path, prefix: &str) -> Result<PathBuf> {
    let root = app_data_dir.join(ACCOUNTS_DIR_NAME);
    fs::create_dir_all(&root).context("failed to create accounts directory")?;
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    for attempt in 0..100u32 {
        let path = root.join(format!("{prefix}-{seed}-{}-{attempt}", std::process::id()));
        if !path.exists() {
            return Ok(path);
        }
    }
    bail!("failed to allocate device restore work directory")
}
