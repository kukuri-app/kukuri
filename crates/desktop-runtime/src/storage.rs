//! 鍵・設定・状態の保存（ADR 0059 §1）。`service` と `account` で引く非同期の get・set・delete。
//! `service` が `FILE_SERVICE` のときは `account` が保存先の path（Web では仮想の path）、それ以外は OS の keyring の
//! service。native は keyring と file、Web は IndexedDB が実装する（W4 AC-2）。上の層（identity の backend の選択、
//! 設定の形式）は platform に依らない。

use std::path::Path;

use anyhow::{Result, anyhow};
use async_trait::async_trait;

/// `account` を path として扱う service。
pub(crate) const FILE_SERVICE: &str = "file";

#[async_trait]
pub trait ClientStorage: Send + Sync {
    async fn get(&self, service: &str, account: &str) -> Result<Option<Vec<u8>>>;
    async fn set(&self, service: &str, account: &str, value: &[u8]) -> Result<()>;
    async fn delete(&self, service: &str, account: &str) -> Result<()>;
}

/// file の service の `account`（path の文字列）。
pub(crate) fn path_key(path: &Path) -> Result<String> {
    path.to_str()
        .map(str::to_string)
        .ok_or_else(|| anyhow!("path `{}` is not UTF-8", path.display()))
}

/// 既定の保存先の file を読む。無ければ `None`。
pub(crate) async fn read_file(path: &Path) -> Result<Option<Vec<u8>>> {
    platform_storage().get(FILE_SERVICE, &path_key(path)?).await
}

/// 既定の保存先の file を置き換える（native は権限 0600 で原子的に書き、fsync する）。
pub(crate) async fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    platform_storage()
        .set(FILE_SERVICE, &path_key(path)?, bytes)
        .await
}

/// 既定の保存先の file を消す。無ければ何もしない。
pub(crate) async fn delete_file(path: &Path) -> Result<()> {
    platform_storage()
        .delete(FILE_SERVICE, &path_key(path)?)
        .await
}

/// 既定の provider が無い keyring（headless の Linux 等）。その環境では file への fallback だけが使える。
#[derive(Debug)]
pub struct KeyringUnavailable;

impl std::fmt::Display for KeyringUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("no default keyring is available")
    }
}

impl std::error::Error for KeyringUnavailable {}

/// 既定の保存先。native は keyring と file。Web は起動時に `install_platform_storage` で入れる。
pub(crate) fn platform_storage() -> &'static dyn ClientStorage {
    #[cfg(not(target_family = "wasm"))]
    return &native::NativeStorage;
    #[cfg(target_family = "wasm")]
    return *PLATFORM_STORAGE
        .get()
        .expect("the platform storage is installed before the runtime starts");
}

#[cfg(target_family = "wasm")]
static PLATFORM_STORAGE: std::sync::OnceLock<&'static dyn ClientStorage> =
    std::sync::OnceLock::new();

/// Web の保存先を入れる（runtime を起動する前に 1 回）。
#[cfg(target_family = "wasm")]
pub fn install_platform_storage(storage: &'static dyn ClientStorage) {
    let _ = PLATFORM_STORAGE.set(storage);
}

#[cfg(not(target_family = "wasm"))]
#[cfg(test)]
pub(crate) use native::NativeStorage;
#[cfg(not(target_family = "wasm"))]
pub(crate) use native::write_private_file_atomically;

#[cfg(not(target_family = "wasm"))]
mod native {
    use std::fs::OpenOptions;
    use std::io::Write;
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;
    use std::path::Path;

    use anyhow::{Context, Result, anyhow};
    use async_trait::async_trait;
    use keyring::{Entry, Error as KeyringError};

    use super::{ClientStorage, FILE_SERVICE, KeyringUnavailable};

    pub(crate) struct NativeStorage;

    // 既定の provider が無いことは `Entry::new` の失敗（`NoDefaultStore`）として返る。
    fn entry(service: &str, account: &str) -> Result<Entry> {
        Entry::new(service, account)
            .map_err(entry_error)
            .context("failed to initialize keyring entry")
    }

    fn entry_error(error: KeyringError) -> anyhow::Error {
        match error {
            KeyringError::NoDefaultStore => anyhow!(KeyringUnavailable),
            error => anyhow!(error),
        }
    }

    #[test]
    fn missing_default_keyring_is_reported_as_unavailable() {
        let error = entry_error(KeyringError::NoDefaultStore).context("wrapped");
        assert!(error.chain().any(|cause| cause.is::<KeyringUnavailable>()));
    }

    #[async_trait]
    impl ClientStorage for NativeStorage {
        async fn get(&self, service: &str, account: &str) -> Result<Option<Vec<u8>>> {
            if service == FILE_SERVICE {
                return match std::fs::read(account) {
                    Ok(bytes) => Ok(Some(bytes)),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                    Err(error) => Err(error).with_context(|| format!("failed to read `{account}`")),
                };
            }
            match entry(service, account)?.get_password() {
                Ok(secret) => Ok(Some(secret.into_bytes())),
                Err(KeyringError::NoEntry) => Ok(None),
                Err(error) => Err(anyhow!(error)).context("failed to read secret from keyring"),
            }
        }

        async fn set(&self, service: &str, account: &str, value: &[u8]) -> Result<()> {
            if service == FILE_SERVICE {
                let path = Path::new(account);
                if let Some(parent) = path
                    .parent()
                    .filter(|parent| !parent.as_os_str().is_empty())
                {
                    std::fs::create_dir_all(parent)
                        .with_context(|| format!("failed to create `{}`", parent.display()))?;
                }
                return write_private_file_atomically(path, value);
            }
            let secret = std::str::from_utf8(value).context("a keyring secret is not UTF-8")?;
            entry(service, account)?
                .set_password(secret)
                .context("failed to persist secret into keyring")
        }

        async fn delete(&self, service: &str, account: &str) -> Result<()> {
            if service == FILE_SERVICE {
                return match std::fs::remove_file(account) {
                    Ok(()) => Ok(()),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    Err(error) => {
                        Err(error).with_context(|| format!("failed to delete `{account}`"))
                    }
                };
            }
            match entry(service, account)?.delete_credential() {
                Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
                Err(error) => Err(anyhow!(error)).context("failed to delete secret from keyring"),
            }
        }
    }

    // 途中クラッシュで既存内容が破損しないよう、同一ディレクトリの temp ファイルへ
    // write → fsync → rename で置換する(issue #574)。失敗は fail-loud で伝播させる。
    pub(crate) fn write_private_file_atomically(path: &Path, bytes: &[u8]) -> Result<()> {
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| anyhow!("invalid private file path `{}`", path.display()))?;
        let temp_path = path.with_file_name(format!("{file_name}.tmp"));
        let mut options = OpenOptions::new();
        options.create(true).write(true).truncate(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options
            .open(&temp_path)
            .with_context(|| format!("failed to create temp file `{}`", temp_path.display()))?;
        file.write_all(bytes)
            .with_context(|| format!("failed to write temp file `{}`", temp_path.display()))?;
        file.sync_all()
            .with_context(|| format!("failed to sync temp file `{}`", temp_path.display()))?;
        drop(file);
        std::fs::rename(&temp_path, path).with_context(|| {
            format!(
                "failed to rename temp file `{}` to `{}`",
                temp_path.display(),
                path.display()
            )
        })?;
        // rename 自体の durability を確保するため、unix では親ディレクトリも fsync する。
        // Windows は std にディレクトリ fsync の手段がないため rename の atomic 置換のみ。
        #[cfg(unix)]
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::File::open(parent)
                .and_then(|dir| dir.sync_all())
                .with_context(|| format!("failed to sync directory `{}`", parent.display()))?;
        }
        Ok(())
    }
}
