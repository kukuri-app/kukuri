//! 旧 iroh store(新旧の namespace・remote 内容が同居した保存領域)の退役(#1221 R5-I)。
//!
//! node は新しい root の store で動く。endpoint secret は旧 root から原子的に写し、endpoint ID を保つ。

use std::path::Path;

use anyhow::{Context, Result};

use crate::node::ENDPOINT_SECRET_FILE_NAME;

/// `root` に endpoint secret が無く `legacy_root` にあれば、同じ内容を `root` へ原子的に写す(一時 file へ書いてから
/// 名前を変える)。旧 root の file は残す。何度呼んでも同じ結果になる。
pub fn adopt_endpoint_secret(legacy_root: &Path, root: &Path) -> Result<()> {
    let target = root.join(ENDPOINT_SECRET_FILE_NAME);
    let source = legacy_root.join(ENDPOINT_SECRET_FILE_NAME);
    if target.exists() || !source.is_file() {
        return Ok(());
    }
    std::fs::create_dir_all(root)
        .with_context(|| format!("failed to create iroh store root {}", root.display()))?;
    let staging = root.join(format!("{ENDPOINT_SECRET_FILE_NAME}.tmp"));
    std::fs::copy(&source, &staging).with_context(|| {
        format!(
            "failed to copy endpoint secret from {} to {}",
            source.display(),
            staging.display()
        )
    })?;
    std::fs::OpenOptions::new()
        .write(true)
        .open(&staging)?
        .sync_all()?;
    std::fs::rename(&staging, &target).with_context(|| {
        format!(
            "failed to move endpoint secret into place at {}",
            target.display()
        )
    })
}
