use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use kukuri_core::KukuriKeys;

use crate::storage::{ClientStorage, FILE_SERVICE, KeyringUnavailable, path_key, platform_storage};

const KEYRING_SERVICE: &str = "org.kukuri.desktop";
const BACKEND_FILE: &str = "file";
const BACKEND_KEYRING: &str = "keyring";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IdentityStorageMode {
    Auto,
    FileOnly,
}

impl IdentityStorageMode {
    pub(crate) fn from_env() -> Self {
        match std::env::var("KUKURI_DISABLE_KEYRING") {
            Ok(value) if matches!(value.trim(), "1" | "true" | "TRUE" | "yes" | "YES") => {
                Self::FileOnly
            }
            _ => Self::Auto,
        }
    }
}

pub(crate) async fn load_or_create_keys(
    db_path: &Path,
    mode: IdentityStorageMode,
) -> Result<KukuriKeys> {
    load_or_create_keys_with_storage(db_path, mode, platform_storage()).await
}

/// 既存の identity を「生成せずに」読み込む(accounts 移行の検出・再開用)。
/// backend marker があるのに実体へ到達できない場合は fail-loud で Err を返す。
pub(crate) async fn load_existing_keys(
    db_path: &Path,
    mode: IdentityStorageMode,
) -> Result<Option<KukuriKeys>> {
    load_existing_keys_with_storage(db_path, mode, platform_storage()).await
}

/// 既知の鍵を db_path 配下の identity storage へ保存する(accounts 移行 / import 用)。
/// `load_or_create_keys` の新規生成分岐と同じ backend 選択(Auto: keyring 優先、
/// 失敗時 file)で永続化し、backend marker まで書き切る。
pub(crate) async fn persist_keys(
    db_path: &Path,
    mode: IdentityStorageMode,
    keys: &KukuriKeys,
) -> Result<()> {
    persist_keys_with_storage(db_path, mode, keys, platform_storage()).await
}

/// db_path 配下の identity 実体(keyring entry / key file / legacy nsec / marker)を
/// すべて削除する(accounts 移行完了後の旧 flat レイアウト掃除用)。
pub(crate) async fn delete_identity(db_path: &Path, mode: IdentityStorageMode) -> Result<()> {
    let storage = platform_storage();
    if mode == IdentityStorageMode::Auto {
        for account in keyring_account_candidates(db_path) {
            storage.delete(KEYRING_SERVICE, account.as_str()).await?;
        }
    }
    delete_file(storage, &key_file_path(db_path)).await?;
    delete_file(storage, &legacy_key_file_path(db_path)).await?;
    delete_file(storage, &backend_marker_path(db_path)).await
}

pub(crate) async fn load_optional_secret(
    db_path: &Path,
    mode: IdentityStorageMode,
    purpose: &str,
    key: &str,
) -> Result<Option<String>> {
    load_optional_secret_with_storage(db_path, mode, purpose, key, platform_storage()).await
}

pub(crate) async fn persist_optional_secret(
    db_path: &Path,
    mode: IdentityStorageMode,
    purpose: &str,
    key: &str,
    secret: &str,
) -> Result<()> {
    persist_optional_secret_with_storage(db_path, mode, purpose, key, secret, platform_storage())
        .await
}

pub(crate) async fn delete_optional_secret(
    db_path: &Path,
    mode: IdentityStorageMode,
    purpose: &str,
    key: &str,
) -> Result<()> {
    let storage = platform_storage();
    if mode == IdentityStorageMode::Auto {
        delete_optional_secret_keyring_entry_with_storage(db_path, purpose, key, storage).await?;
    }
    delete_file(storage, &optional_secret_file_path(db_path, purpose, key)).await
}

pub(crate) async fn load_or_create_keys_with_storage(
    db_path: &Path,
    mode: IdentityStorageMode,
    storage: &dyn ClientStorage,
) -> Result<KukuriKeys> {
    if let Some(backend) = load_backend_marker(storage, db_path).await? {
        return load_keys_with_backend(db_path, backend.as_str(), mode, storage).await;
    }

    if mode == IdentityStorageMode::Auto
        && let Ok(Some(secret)) = load_secret_from_keyring(db_path, storage).await
    {
        write_backend_marker(storage, db_path, BACKEND_KEYRING).await?;
        return parse_keys(secret.as_str());
    }

    if let Some(secret) = load_secret_from_file(storage, db_path).await? {
        write_backend_marker(storage, db_path, BACKEND_FILE).await?;
        return parse_keys(secret.as_str());
    }

    let keys = KukuriKeys::generate();
    let encoded = keys.export_secret_hex();

    if mode == IdentityStorageMode::Auto
        && persist_secret_to_keyring(db_path, encoded.as_str(), storage)
            .await
            .is_ok()
    {
        write_backend_marker(storage, db_path, BACKEND_KEYRING).await?;
    } else {
        persist_secret_to_file(storage, db_path, encoded.as_str()).await?;
        write_backend_marker(storage, db_path, BACKEND_FILE).await?;
    }

    Ok(keys)
}

pub(crate) async fn load_existing_keys_with_storage(
    db_path: &Path,
    mode: IdentityStorageMode,
    storage: &dyn ClientStorage,
) -> Result<Option<KukuriKeys>> {
    if let Some(backend) = load_backend_marker(storage, db_path).await? {
        return load_keys_with_backend(db_path, backend.as_str(), mode, storage)
            .await
            .map(Some);
    }
    if mode == IdentityStorageMode::Auto
        && let Ok(Some(secret)) = load_secret_from_keyring(db_path, storage).await
    {
        return parse_keys(secret.as_str()).map(Some);
    }
    if let Some(secret) = load_secret_from_file(storage, db_path).await? {
        return parse_keys(secret.as_str()).map(Some);
    }
    Ok(None)
}

pub(crate) async fn persist_keys_with_storage(
    db_path: &Path,
    mode: IdentityStorageMode,
    keys: &KukuriKeys,
    storage: &dyn ClientStorage,
) -> Result<()> {
    let encoded = keys.export_secret_hex();
    if mode == IdentityStorageMode::Auto
        && persist_secret_to_keyring(db_path, encoded.as_str(), storage)
            .await
            .is_ok()
    {
        write_backend_marker(storage, db_path, BACKEND_KEYRING).await?;
        // 旧 file 実体が残ると marker=keyring と実体が食い違うため掃除する。
        let _ = delete_file(storage, &key_file_path(db_path)).await;
        let _ = delete_file(storage, &legacy_key_file_path(db_path)).await;
        return Ok(());
    }
    persist_secret_to_file(storage, db_path, encoded.as_str()).await?;
    write_backend_marker(storage, db_path, BACKEND_FILE).await?;
    if mode == IdentityStorageMode::Auto {
        for account in keyring_account_candidates(db_path) {
            let _ = storage.delete(KEYRING_SERVICE, account.as_str()).await;
        }
    }
    Ok(())
}

async fn load_keys_with_backend(
    db_path: &Path,
    backend: &str,
    mode: IdentityStorageMode,
    storage: &dyn ClientStorage,
) -> Result<KukuriKeys> {
    match backend {
        BACKEND_KEYRING => {
            if mode == IdentityStorageMode::FileOnly {
                return Err(anyhow!(
                    "persisted identity is stored in keyring, but keyring is disabled"
                ));
            }
            let secret = load_secret_from_keyring(db_path, storage)
                .await?
                .ok_or_else(|| anyhow!("persisted keyring identity is unavailable"))?;
            parse_keys(secret.as_str())
        }
        BACKEND_FILE => {
            let secret = load_secret_from_file(storage, db_path)
                .await?
                .ok_or_else(|| anyhow!("persisted identity file is unavailable"))?;
            parse_keys(secret.as_str())
        }
        other => Err(anyhow!("unknown identity backend `{other}`")),
    }
}

fn parse_keys(secret: &str) -> Result<KukuriKeys> {
    KukuriKeys::parse(secret).context("failed to parse persisted secret key")
}

pub(crate) async fn load_optional_secret_with_storage(
    db_path: &Path,
    mode: IdentityStorageMode,
    purpose: &str,
    key: &str,
    storage: &dyn ClientStorage,
) -> Result<Option<String>> {
    if mode == IdentityStorageMode::Auto {
        for account in optional_secret_account_candidates(db_path, purpose, key) {
            match keyring_get(storage, account.as_str()).await {
                Ok(Some(secret)) => return Ok(Some(secret)),
                Ok(None) => {}
                // Headless Linux environments can have no default keyring provider at all.
                // Such environments could only have persisted this optional value through
                // the file fallback, so continue there without weakening other keyring errors.
                Err(error) if is_missing_default_keyring(&error) => break,
                Err(error) => {
                    return Err(error).context("failed to read optional secret from keyring");
                }
            }
        }
    }

    read_text(storage, &optional_secret_file_path(db_path, purpose, key)).await
}

fn is_missing_default_keyring(error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|cause| cause.downcast_ref::<KeyringUnavailable>().is_some())
}

pub(crate) async fn persist_optional_secret_with_storage(
    db_path: &Path,
    mode: IdentityStorageMode,
    purpose: &str,
    key: &str,
    secret: &str,
    storage: &dyn ClientStorage,
) -> Result<()> {
    let account = optional_secret_account(db_path, purpose, key);
    if mode == IdentityStorageMode::Auto {
        if storage
            .set(KEYRING_SERVICE, account.as_str(), secret.as_bytes())
            .await
            .is_ok()
        {
            let _ = delete_file(storage, &optional_secret_file_path(db_path, purpose, key)).await;
            return Ok(());
        }
        // set 失敗時に旧 entry を残すと、load が keyring を優先するため file へ書いた
        // 新しい値が恒久的にシャドウされる(例: Windows Credential Manager の blob 上限
        // 超過で set が失敗し始めるケース)。best effort で削除してから file へ倒す。
        for account in optional_secret_account_candidates(db_path, purpose, key) {
            let _ = storage.delete(KEYRING_SERVICE, account.as_str()).await;
        }
    }

    write_secret_file(
        storage,
        &optional_secret_file_path(db_path, purpose, key),
        secret,
    )
    .await
}

pub(crate) async fn delete_optional_secret_keyring_entry_with_storage(
    db_path: &Path,
    purpose: &str,
    key: &str,
    storage: &dyn ClientStorage,
) -> Result<()> {
    for account in optional_secret_account_candidates(db_path, purpose, key) {
        match storage.delete(KEYRING_SERVICE, account.as_str()).await {
            Ok(()) => {}
            Err(error) if is_missing_default_keyring(&error) => return Ok(()),
            Err(error) => {
                return Err(error).context("failed to delete optional secret from keyring");
            }
        }
    }
    Ok(())
}

async fn keyring_get(storage: &dyn ClientStorage, account: &str) -> Result<Option<String>> {
    storage
        .get(KEYRING_SERVICE, account)
        .await?
        .map(|secret| String::from_utf8(secret).context("a keyring secret is not UTF-8"))
        .transpose()
}

async fn load_secret_from_keyring(
    db_path: &Path,
    storage: &dyn ClientStorage,
) -> Result<Option<String>> {
    for account in keyring_account_candidates(db_path) {
        if let Some(secret) = keyring_get(storage, account.as_str())
            .await
            .context("failed to read secret from keyring")?
        {
            return Ok(Some(secret));
        }
    }
    Ok(None)
}

async fn persist_secret_to_keyring(
    db_path: &Path,
    secret: &str,
    storage: &dyn ClientStorage,
) -> Result<()> {
    storage
        .set(
            KEYRING_SERVICE,
            keyring_account(db_path).as_str(),
            secret.as_bytes(),
        )
        .await
        .context("failed to persist secret into keyring")
}

async fn load_secret_from_file(
    storage: &dyn ClientStorage,
    db_path: &Path,
) -> Result<Option<String>> {
    if let Some(secret) = read_text(storage, &key_file_path(db_path)).await? {
        return Ok(Some(secret));
    }
    read_text(storage, &legacy_key_file_path(db_path)).await
}

/// file の中身を前後の空白を除いた文字列で読む。無ければ `None`。
pub(crate) async fn read_text(storage: &dyn ClientStorage, path: &Path) -> Result<Option<String>> {
    let Some(bytes) = storage.get(FILE_SERVICE, &path_key(path)?).await? else {
        return Ok(None);
    };
    let text = String::from_utf8(bytes)
        .with_context(|| format!("failed to read `{}` as UTF-8", path.display()))?;
    Ok(Some(text.trim().to_string()))
}

async fn persist_secret_to_file(
    storage: &dyn ClientStorage,
    db_path: &Path,
    secret: &str,
) -> Result<()> {
    write_secret_file(storage, &key_file_path(db_path), secret).await?;
    delete_file(storage, &legacy_key_file_path(db_path)).await
}

async fn write_secret_file(storage: &dyn ClientStorage, path: &Path, secret: &str) -> Result<()> {
    storage
        .set(FILE_SERVICE, &path_key(path)?, secret.as_bytes())
        .await
        .with_context(|| format!("failed to persist identity file `{}`", path.display()))
}

async fn load_backend_marker(
    storage: &dyn ClientStorage,
    db_path: &Path,
) -> Result<Option<String>> {
    read_text(storage, &backend_marker_path(db_path))
        .await
        .context("failed to read identity backend marker")
}

async fn write_backend_marker(
    storage: &dyn ClientStorage,
    db_path: &Path,
    backend: &str,
) -> Result<()> {
    let path = backend_marker_path(db_path);
    storage
        .set(FILE_SERVICE, &path_key(&path)?, backend.as_bytes())
        .await
        .with_context(|| {
            format!(
                "failed to persist identity backend marker `{}`",
                path.display()
            )
        })
}

async fn delete_file(storage: &dyn ClientStorage, path: &Path) -> Result<()> {
    storage
        .delete(FILE_SERVICE, &path_key(path)?)
        .await
        .with_context(|| format!("failed to delete secret file `{}`", path.display()))
}

fn keyring_account(db_path: &Path) -> String {
    format!("db:{}", resolve_db_path(db_path).display())
}

/// 読込・削除で試す keyring account。先頭が書込先の正規 account で、以降は旧版が
/// DB ファイル不在時に書いた未正規化パスの account(存在しうる場合のみ)。
fn keyring_account_candidates(db_path: &Path) -> Vec<String> {
    account_candidates(
        keyring_account(db_path),
        format!("db:{}", db_path.display()),
    )
}

/// DB ファイルの有無で keyring account が変わらないよう、ファイルが無ければ親ディレクトリを
/// 正規化して結合する。旧実装は `canonicalize(db_path)` 失敗時に生のパスへ倒していたため、
/// Windows のクリーンインストールでは作成時 `C:\...` と再読込時 `\\?\C:\...` で account が
/// 食い違い、keyring の identity に到達できなくなっていた。
fn resolve_db_path(db_path: &Path) -> PathBuf {
    if let Ok(resolved) = std::fs::canonicalize(db_path) {
        return resolved;
    }
    if let (Some(parent), Some(file_name)) = (db_path.parent(), db_path.file_name())
        && !parent.as_os_str().is_empty()
        && let Ok(parent) = std::fs::canonicalize(parent)
    {
        return parent.join(file_name);
    }
    db_path.to_path_buf()
}

fn account_candidates(primary: String, legacy: String) -> Vec<String> {
    if primary == legacy {
        vec![primary]
    } else {
        vec![primary, legacy]
    }
}

fn key_file_path(db_path: &Path) -> PathBuf {
    db_path.with_extension("identity-key")
}

// 互換パス(REFACTORING.md「互換パスと sunset 条件」参照)。
// 旧 `.nsec`(bech32 表記)は読み込んでも新形式 `.identity-key` へ再保存されず残り続ける。
// 鍵が見つからない場合 `load_or_create_keys` は黙って新しい鍵を生成するため、この読込パス
// だけを消すと旧ファイルの利用者が気づかないまま別人の鍵になる。
// 撤去条件(WP-C8 で確定): `.nsec` を検知したら起動を止めて案内を出す処理(fail-loud)と
// セットで撤去すること。単独では削除しない。
fn legacy_key_file_path(db_path: &Path) -> PathBuf {
    db_path.with_extension("nsec")
}

fn backend_marker_path(db_path: &Path) -> PathBuf {
    db_path.with_extension("identity-store")
}

fn optional_secret_account(db_path: &Path, purpose: &str, key: &str) -> String {
    optional_secret_account_for(resolve_db_path(db_path).as_path(), purpose, key)
}

fn optional_secret_account_candidates(db_path: &Path, purpose: &str, key: &str) -> Vec<String> {
    account_candidates(
        optional_secret_account(db_path, purpose, key),
        optional_secret_account_for(db_path, purpose, key),
    )
}

fn optional_secret_account_for(path: &Path, purpose: &str, key: &str) -> String {
    format!(
        "db:{}:{}:{}",
        path.display(),
        purpose,
        optional_secret_suffix(key)
    )
}

fn optional_secret_file_path(db_path: &Path, purpose: &str, key: &str) -> PathBuf {
    db_path.with_extension(format!("{purpose}-{}", optional_secret_suffix(key)))
}

fn optional_secret_suffix(key: &str) -> String {
    blake3::hash(key.as_bytes()).to_hex().to_string()
}

#[cfg(test)]
mod tests;
