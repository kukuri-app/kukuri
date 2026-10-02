//! アカウント毎データディレクトリ(`<app_data>/accounts/<id>/kukuri.db`)と、その
//! 列挙・アクティブ選択を持つ registry(`<app_data>/accounts.json`)。
//!
//! registry は秘密情報を含まないメタデータのみを持つ。秘密鍵は従来どおり
//! `identity` モジュール(keyring / `<db>.identity-key`)が各アカウントの db path
//! 配下で管理する。旧 flat レイアウト(`<app_data>/kukuri.db`)は初回起動時に
//! アカウントディレクトリへ一括移行し、以後 flat レイアウトはサポートしない。
//!
//! 移行は「鍵を新レイアウトへ複製 → registry 書き込み(commit point)→ ファイル
//! 移動と旧実体の削除」の順で行い、どの時点でクラッシュしても鍵が最低ひとつの
//! 場所から読めるようにする。registry 書き込み後の残骸は次回起動時に再開する。

use std::fs;
pub(crate) mod create;
#[cfg(not(target_family = "wasm"))]
pub(crate) mod display;
pub(crate) mod lifecycle;
use std::path::{Path, PathBuf};
use web_time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use kukuri_core::KukuriKeys;
use serde::{Deserialize, Serialize};

use crate::community_node::{
    COMMUNITY_NODE_INVITE_CODE_PURPOSE, COMMUNITY_NODE_TOKEN_PURPOSE,
    load_community_node_config_from_file,
};
use crate::identity::{
    IdentityStorageMode, delete_identity, delete_optional_secret, load_existing_keys,
    load_optional_secret, load_or_create_keys, persist_keys, persist_optional_secret,
};
use crate::paths::DB_FILE_NAME;
use crate::runtime::{
    GOSSIP_SUBSCRIPTION_STATE_KEY, GOSSIP_SUBSCRIPTION_STATE_PURPOSE,
    PRIVATE_CHANNEL_CAPABILITIES_KEY, PRIVATE_CHANNEL_CAPABILITIES_PURPOSE,
};

pub(crate) const ACCOUNTS_DIR_NAME: &str = "accounts";
pub(crate) const ACCOUNTS_REGISTRY_FILE_NAME: &str = "accounts.json";
const ACCOUNTS_REGISTRY_VERSION: u32 = 1;
const ACCOUNT_ID_HEX_CHARS: usize = 16;

// 移行時に flat レイアウトからアカウントディレクトリへ移す db 兄弟ファイル。
// harness の cleanup_runtime_artifacts と同じ per-account state の一覧に基づく。
// `app-consent.json` は端末レベルの状態のため意図的に含めない。
const FLAT_SIBLING_EXTENSIONS: [&str; 6] = [
    "db-shm",
    "db-wal",
    "discovery.json",
    "community-node.json",
    "content-display.json",
    "subscriptions.json",
];
const FLAT_SIBLING_DIR_EXTENSIONS: [&str; 1] = ["iroh-data"];
// optional secret の file fallback(`kukuri.<purpose>-<hash>`)の prefix 一覧。
const OPTIONAL_SECRET_FILE_PREFIXES: [&str; 4] = [
    "private-channel-capabilities-",
    "gossip-subscription-state-",
    "community-node-token-",
    "community-node-invite-code-",
];

/// アカウントの公開メタデータ。秘密情報は含まない。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct AccountRecord {
    pub id: String,
    pub pubkey: String,
    #[serde(default)]
    pub label: Option<String>,
    pub created_at: i64,
    pub last_used_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct AccountsRegistryFile {
    version: u32,
    active_account_id: String,
    accounts: Vec<AccountRecord>,
    #[serde(default)]
    history: Vec<String>,
    #[serde(default)]
    profile_setup: Vec<String>,
    #[serde(default)]
    logout_sequence: u64,
    #[serde(default)]
    pending_logout: Option<lifecycle::PreparedLogout>,
    #[serde(default)]
    pending_creation: Option<create::PreparedAccountCreation>,
    #[serde(default)]
    completed_creations: std::collections::BTreeMap<String, create::PreparedAccountCreation>,
}

/// アカウント一覧とアクティブ選択のスナップショット。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct AccountsSnapshot {
    pub active_account_id: String,
    pub accounts: Vec<AccountRecord>,
}

/// 暗号化されたアカウント鍵エクスポートの結果。`export` は暗号化 envelope で
/// あり、平文秘密鍵は含まない。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct AccountKeyExport {
    pub export: String,
    pub public_key: String,
}

/// #1211: 移行元が表示する QR・リンク。招待の秘密を含むので Debug で redact する。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct AccountTransferLink {
    pub link: String,
    pub expires_at_ms: i64,
}

impl std::fmt::Debug for AccountTransferLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccountTransferLink")
            .field("link", &"<redacted>")
            .field("expires_at_ms", &self.expires_at_ms)
            .finish()
    }
}

/// インポート前にパスフレーズなしで確認できるメタデータ。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct AccountKeyImportPreview {
    pub version: u32,
    pub kdf: String,
    pub public_key: String,
    pub already_registered: bool,
}

/// インポート前の fingerprint 確認用メタデータを返す。復号は行わない。
pub async fn preview_account_key_import(
    app_data_dir: &Path,
    export: &str,
) -> Result<AccountKeyImportPreview> {
    let preview = kukuri_core::preview_account_key_export(export)?;
    let public_key = preview.public_key.as_str().to_string();
    let already_registered = load_registry(app_data_dir)
        .await?
        .map(|registry| {
            registry
                .accounts
                .iter()
                .any(|record| record.pubkey == public_key)
        })
        .unwrap_or(false);
    Ok(AccountKeyImportPreview {
        version: preview.version,
        kdf: preview.kdf,
        public_key,
        already_registered,
    })
}

/// エクスポート envelope を復号・検証し、新しいアカウントとして追加する。
/// アクティブアカウントは切り替えない(切替は `switch_account` 側の責務)。
pub async fn import_account_key_from_env(
    app_data_dir: &Path,
    export: &str,
    passphrase: &str,
    label: Option<String>,
) -> Result<AccountRecord> {
    let keys = kukuri_core::decrypt_account_key_export(export, passphrase)?;
    add_account_from_env(app_data_dir, &keys, label, false).await
}

pub(crate) fn account_id_for_pubkey(pubkey_hex: &str) -> Result<String> {
    let trimmed = pubkey_hex.trim();
    if trimmed.len() < ACCOUNT_ID_HEX_CHARS || !trimmed.is_ascii() {
        bail!("invalid account pubkey `{trimmed}`");
    }
    Ok(trimmed[..ACCOUNT_ID_HEX_CHARS].to_ascii_lowercase())
}

fn registry_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join(ACCOUNTS_REGISTRY_FILE_NAME)
}

pub fn account_db_path(app_data_dir: &Path, account_id: &str) -> PathBuf {
    app_data_dir
        .join(ACCOUNTS_DIR_NAME)
        .join(account_id)
        .join(DB_FILE_NAME)
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

async fn load_registry(app_data_dir: &Path) -> Result<Option<AccountsRegistryFile>> {
    let path = registry_path(app_data_dir);
    let Some(raw) = crate::storage::read_file(&path)
        .await
        .with_context(|| format!("failed to read accounts registry `{}`", path.display()))?
    else {
        return Ok(None);
    };
    let registry: AccountsRegistryFile = serde_json::from_slice(&raw)
        .with_context(|| format!("failed to parse accounts registry `{}`", path.display()))?;
    if registry.version != ACCOUNTS_REGISTRY_VERSION {
        bail!(
            "unsupported accounts registry version `{}` in `{}`",
            registry.version,
            path.display()
        );
    }
    Ok(Some(registry))
}

async fn save_registry(app_data_dir: &Path, registry: &AccountsRegistryFile) -> Result<()> {
    let path = registry_path(app_data_dir);
    let bytes =
        serde_json::to_vec_pretty(registry).context("failed to encode accounts registry")?;
    crate::storage::write_file(&path, &bytes)
        .await
        .with_context(|| format!("failed to persist accounts registry `{}`", path.display()))
}

/// `ensure_accounts_initialized` の環境変数版(`KUKURI_DISABLE_KEYRING` を反映)。
pub async fn ensure_accounts_initialized_from_env(app_data_dir: &Path) -> Result<PathBuf> {
    ensure_accounts_initialized(app_data_dir, IdentityStorageMode::from_env()).await
}

/// `add_account` の環境変数版。
pub async fn add_account_from_env(
    app_data_dir: &Path,
    keys: &KukuriKeys,
    label: Option<String>,
    set_active: bool,
) -> Result<AccountRecord> {
    add_account(
        app_data_dir,
        IdentityStorageMode::from_env(),
        keys,
        label,
        set_active,
    )
    .await
}

/// accounts レイアウトを初期化し、アクティブアカウントの db path を返す。
///
/// - registry なし + flat レイアウトあり → 一括移行
/// - registry なし + 何もなし → 初回アカウント生成
/// - registry あり → 未完了の移行残骸があれば再開してから active を返す
pub(crate) async fn ensure_accounts_initialized(
    app_data_dir: &Path,
    mode: IdentityStorageMode,
) -> Result<PathBuf> {
    match load_registry(app_data_dir).await? {
        Some(registry) => {
            let registry = resume_flat_migration_if_needed(app_data_dir, mode, registry).await?;
            let active = registry
                .accounts
                .iter()
                .find(|record| record.id == registry.active_account_id)
                .ok_or_else(|| {
                    anyhow!(
                        "active account `{}` is missing from the accounts registry",
                        registry.active_account_id
                    )
                })?;
            let db = account_db_path(app_data_dir, active.id.as_str());
            verify_persisted_identity(&db, mode, &active.pubkey).await?;
            Ok(db)
        }
        None => {
            let flat_db = app_data_dir.join(DB_FILE_NAME);
            let has_flat_identity = load_existing_keys(&flat_db, mode).await?.is_some();
            if has_flat_identity || flat_db.exists() {
                migrate_flat_layout(app_data_dir, mode).await
            } else {
                create_first_account(app_data_dir, mode).await
            }
        }
    }
}

pub async fn list_accounts(app_data_dir: &Path) -> Result<AccountsSnapshot> {
    let registry = load_registry(app_data_dir)
        .await?
        .ok_or_else(|| anyhow!("accounts registry is not initialized"))?;
    Ok(AccountsSnapshot {
        active_account_id: registry.active_account_id,
        accounts: registry.accounts,
    })
}

/// 検証済みの鍵を新しいアカウントとして追加する(インポート用)。
///
/// 同一 pubkey が登録済みならエラーで拒否し、既存アカウントには一切触れない。
/// アカウントディレクトリと identity を作り切ってから registry へ追記するため、
/// 途中失敗時は registry が変わらず、残骸ディレクトリは再実行で上書きされる。
pub(crate) async fn add_account(
    app_data_dir: &Path,
    mode: IdentityStorageMode,
    keys: &KukuriKeys,
    label: Option<String>,
    set_active: bool,
) -> Result<AccountRecord> {
    let mut registry = load_registry(app_data_dir)
        .await?
        .ok_or_else(|| anyhow!("accounts registry is not initialized"))?;
    let pubkey = keys.public_key_hex();
    let id = account_id_for_pubkey(pubkey.as_str())?;
    if registry
        .accounts
        .iter()
        .any(|record| record.pubkey == pubkey || record.id == id)
    {
        bail!("account with public key `{pubkey}` already exists");
    }

    let db_path = account_db_path(app_data_dir, id.as_str());
    create_account_dir(&db_path)?;
    if let Some(existing) = load_existing_keys(&db_path, mode).await? {
        if existing.public_key_hex() != pubkey {
            bail!("retained account identity does not match imported key");
        }
    } else {
        // An existing DB without a readable identity is not a new account.
        if db_path.exists() {
            bail!("retained account identity is missing");
        }
        persist_keys(&db_path, mode, keys).await?;
    }
    verify_persisted_identity(&db_path, mode, pubkey.as_str()).await?;

    let now = now_millis();
    let record = AccountRecord {
        id: id.clone(),
        pubkey,
        label: label.filter(|value| !value.trim().is_empty()),
        created_at: now,
        last_used_at: now,
    };
    registry.accounts.push(record.clone());
    if set_active {
        registry.active_account_id = id;
    }
    save_registry(app_data_dir, &registry).await?;
    Ok(record)
}

/// アクティブアカウントを切り替え、切替後の record を返す。
pub async fn set_active_account(app_data_dir: &Path, account_id: &str) -> Result<AccountRecord> {
    let mut registry = load_registry(app_data_dir)
        .await?
        .ok_or_else(|| anyhow!("accounts registry is not initialized"))?;
    let previous = registry.active_account_id.clone();
    let record = registry
        .accounts
        .iter_mut()
        .find(|record| record.id == account_id)
        .ok_or_else(|| anyhow!("unknown account `{account_id}`"))?;
    record.last_used_at = now_millis();
    let snapshot = record.clone();
    registry.active_account_id = snapshot.id.clone();
    if previous != account_id {
        registry
            .history
            .retain(|id| id != &previous && id != account_id);
        registry.history.insert(0, previous);
    }
    save_registry(app_data_dir, &registry).await?;
    Ok(snapshot)
}

/// 検証とstagingを完了した復元先をregistryへ登録し、active accountにする。
/// directoryとidentityの設置はbackup moduleが先に完了させる。
pub(crate) async fn register_restored_account(
    app_data_dir: &Path,
    pubkey: &str,
    label: Option<String>,
) -> Result<AccountRecord> {
    let mut registry = load_registry(app_data_dir)
        .await?
        .ok_or_else(|| anyhow!("accounts registry is not initialized"))?;
    let id = account_id_for_pubkey(pubkey)?;
    let now = now_millis();
    let record = if let Some(existing) = registry
        .accounts
        .iter_mut()
        .find(|record| record.pubkey == pubkey)
    {
        if let Some(label) = label.filter(|value| !value.trim().is_empty()) {
            existing.label = Some(label);
        }
        existing.last_used_at = now;
        existing.clone()
    } else {
        let record = AccountRecord {
            id: id.clone(),
            pubkey: pubkey.to_string(),
            label: label.filter(|value| !value.trim().is_empty()),
            created_at: now,
            last_used_at: now,
        };
        registry.accounts.push(record.clone());
        record
    };
    registry.active_account_id = id;
    save_registry(app_data_dir, &registry).await?;
    Ok(record)
}

fn create_account_dir(db_path: &Path) -> Result<()> {
    let dir = db_path
        .parent()
        .ok_or_else(|| anyhow!("invalid account db path `{}`", db_path.display()))?;
    fs::create_dir_all(dir)
        .with_context(|| format!("failed to create account dir `{}`", dir.display()))
}

pub(crate) async fn verify_persisted_identity(
    db_path: &Path,
    mode: IdentityStorageMode,
    expected_pubkey: &str,
) -> Result<()> {
    let restored = load_existing_keys(db_path, mode)
        .await?
        .ok_or_else(|| anyhow!("persisted account identity is not readable back"))?;
    if restored.public_key_hex() != expected_pubkey {
        bail!("persisted account identity does not match the expected public key");
    }
    Ok(())
}

async fn migrate_flat_layout(app_data_dir: &Path, mode: IdentityStorageMode) -> Result<PathBuf> {
    let flat_db = app_data_dir.join(DB_FILE_NAME);
    // 鍵ファイルのない flat db(孤児 db)は従来どおり新規生成で救済する。
    let keys = load_or_create_keys(&flat_db, mode).await?;
    let pubkey = keys.public_key_hex();
    let id = account_id_for_pubkey(pubkey.as_str())?;
    let new_db = account_db_path(app_data_dir, id.as_str());
    create_account_dir(&new_db)?;
    persist_keys(&new_db, mode, &keys).await?;
    verify_persisted_identity(&new_db, mode, pubkey.as_str()).await?;

    let now = now_millis();
    let registry = AccountsRegistryFile {
        version: ACCOUNTS_REGISTRY_VERSION,
        history: Vec::new(),
        profile_setup: Vec::new(),
        logout_sequence: 0,
        pending_logout: None,
        pending_creation: None,
        completed_creations: Default::default(),
        active_account_id: id.clone(),
        accounts: vec![AccountRecord {
            id,
            pubkey,
            label: None,
            created_at: now,
            last_used_at: now,
        }],
    };
    // commit point。ここまでは flat レイアウトに一切触れていない。
    save_registry(app_data_dir, &registry).await?;

    finish_flat_migration(app_data_dir, mode, &new_db).await?;
    Ok(new_db)
}

async fn create_first_account(app_data_dir: &Path, mode: IdentityStorageMode) -> Result<PathBuf> {
    let keys = KukuriKeys::generate();
    let pubkey = keys.public_key_hex();
    let id = account_id_for_pubkey(pubkey.as_str())?;
    let new_db = account_db_path(app_data_dir, id.as_str());
    create_account_dir(&new_db)?;
    persist_keys(&new_db, mode, &keys).await?;
    verify_persisted_identity(&new_db, mode, pubkey.as_str()).await?;

    let now = now_millis();
    let registry = AccountsRegistryFile {
        version: ACCOUNTS_REGISTRY_VERSION,
        history: Vec::new(),
        profile_setup: vec![id.clone()],
        logout_sequence: 0,
        pending_logout: None,
        pending_creation: None,
        completed_creations: Default::default(),
        active_account_id: id.clone(),
        accounts: vec![AccountRecord {
            id,
            pubkey,
            label: None,
            created_at: now,
            last_used_at: now,
        }],
    };
    save_registry(app_data_dir, &registry).await?;
    Ok(new_db)
}

/// registry 書き込み後にクラッシュした移行を再開する。
///
/// flat identity が残っている場合は、その pubkey に対応するアカウントへ移行を
/// 続行する(registry 未登録なら非アクティブの新規アカウントとして登録する)。
/// flat identity がなく db ファイルだけが残っている場合は手動配置とみなして
/// 触らない。
async fn resume_flat_migration_if_needed(
    app_data_dir: &Path,
    mode: IdentityStorageMode,
    mut registry: AccountsRegistryFile,
) -> Result<AccountsRegistryFile> {
    let flat_db = app_data_dir.join(DB_FILE_NAME);
    let Some(keys) = load_existing_keys(&flat_db, mode).await? else {
        return Ok(registry);
    };
    let pubkey = keys.public_key_hex();
    let id = account_id_for_pubkey(pubkey.as_str())?;
    let new_db = account_db_path(app_data_dir, id.as_str());
    create_account_dir(&new_db)?;
    persist_keys(&new_db, mode, &keys).await?;
    verify_persisted_identity(&new_db, mode, pubkey.as_str()).await?;
    if !registry.accounts.iter().any(|record| record.id == id) {
        let now = now_millis();
        registry.accounts.push(AccountRecord {
            id,
            pubkey,
            label: None,
            created_at: now,
            last_used_at: now,
        });
        save_registry(app_data_dir, &registry).await?;
    }
    finish_flat_migration(app_data_dir, mode, &new_db).await?;
    Ok(registry)
}

/// flat レイアウトの実体をアカウントディレクトリへ移し、旧実体を削除する。
/// すべての操作は「元があれば移す / 消す」の冪等な形で書かれており、途中で
/// クラッシュしても再実行で完了する。
async fn finish_flat_migration(
    app_data_dir: &Path,
    mode: IdentityStorageMode,
    new_db: &Path,
) -> Result<()> {
    let flat_db = app_data_dir.join(DB_FILE_NAME);

    move_flat_entry(&flat_db, new_db)?;
    for extension in FLAT_SIBLING_EXTENSIONS {
        move_flat_entry(
            &flat_db.with_extension(extension),
            &new_db.with_extension(extension),
        )?;
    }
    for extension in FLAT_SIBLING_DIR_EXTENSIONS {
        move_flat_entry(
            &flat_db.with_extension(extension),
            &new_db.with_extension(extension),
        )?;
    }
    move_optional_secret_files(app_data_dir, new_db)?;

    // keyring に旧 db path 名義で保存されている optional secret を新名義へ移す。
    // keyring が読めない環境では file fallback が上の移動で済んでいるため、
    // 読み出し失敗は best effort として無視する。
    let mut pairs: Vec<(String, String)> = vec![
        (
            PRIVATE_CHANNEL_CAPABILITIES_PURPOSE.to_string(),
            PRIVATE_CHANNEL_CAPABILITIES_KEY.to_string(),
        ),
        (
            GOSSIP_SUBSCRIPTION_STATE_PURPOSE.to_string(),
            GOSSIP_SUBSCRIPTION_STATE_KEY.to_string(),
        ),
    ];
    if let Ok(Some(config)) = load_community_node_config_from_file(new_db).await {
        for node in config.nodes {
            pairs.push((
                COMMUNITY_NODE_TOKEN_PURPOSE.to_string(),
                node.base_url.clone(),
            ));
            pairs.push((
                COMMUNITY_NODE_INVITE_CODE_PURPOSE.to_string(),
                node.base_url,
            ));
        }
    }
    for (purpose, key) in pairs {
        if let Ok(Some(value)) =
            load_optional_secret(&flat_db, mode, purpose.as_str(), key.as_str()).await
            && load_optional_secret(new_db, mode, purpose.as_str(), key.as_str())
                .await
                .ok()
                .flatten()
                .is_none()
        {
            persist_optional_secret(new_db, mode, purpose.as_str(), key.as_str(), value.as_str())
                .await?;
        }
        let _ = delete_optional_secret(&flat_db, mode, purpose.as_str(), key.as_str()).await;
    }

    // 最後に旧 identity 実体を削除する。ここまでのどこで落ちても、鍵は
    // 新レイアウト側(persist_keys 済み)か旧レイアウト側の少なくとも一方に残る。
    delete_identity(&flat_db, mode).await?;
    Ok(())
}

fn move_flat_entry(source: &Path, destination: &Path) -> Result<()> {
    if !source.exists() {
        return Ok(());
    }
    if destination.exists() {
        // 再開時に移動済みの実体が両側に見える状況は rename では発生しないが、
        // 手動コピー等の残骸で起こり得る。移行先を正としてソースを残す。
        return Ok(());
    }
    fs::rename(source, destination).with_context(|| {
        format!(
            "failed to move `{}` to `{}`",
            source.display(),
            destination.display()
        )
    })
}

fn move_optional_secret_files(app_data_dir: &Path, new_db: &Path) -> Result<()> {
    let account_dir = new_db
        .parent()
        .ok_or_else(|| anyhow!("invalid account db path `{}`", new_db.display()))?;
    let db_stem = format!(
        "{}.",
        Path::new(DB_FILE_NAME)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("kukuri")
    );
    let entries = fs::read_dir(app_data_dir)
        .with_context(|| format!("failed to read app data dir `{}`", app_data_dir.display()))?;
    for entry in entries {
        let entry = entry.context("failed to read app data dir entry")?;
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        let Some(suffix) = name.strip_prefix(db_stem.as_str()) else {
            continue;
        };
        if OPTIONAL_SECRET_FILE_PREFIXES
            .iter()
            .any(|prefix| suffix.starts_with(prefix))
        {
            move_flat_entry(&entry.path(), &account_dir.join(name))?;
        }
    }
    Ok(())
}
