//! 投稿の未完了コピーを process ごとに所有し、前 process の分だけを背景で回収する。
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

#[derive(Default)]
pub(crate) struct AttachmentStaging {
    directory: Mutex<Option<Arc<tempfile::TempDir>>>,
}

impl AttachmentStaging {
    pub(crate) fn session(&self, cache: &Path) -> anyhow::Result<Arc<tempfile::TempDir>> {
        let mut directory = self.directory.lock().expect("attachment staging lock");
        if let Some(directory) = &*directory {
            return Ok(directory.clone());
        }
        let root = cache.join("kukuri-post-attachments");
        std::fs::create_dir_all(&root)?;
        let root = root.canonicalize()?;
        let current = Arc::new(tempfile::tempdir_in(&root)?);
        let live = current.path().to_path_buf();
        *directory = Some(current.clone());
        tauri::async_runtime::spawn(async move {
            if let Err(error) = retire_sessions(root, live).await {
                tracing::warn!(%error, "retired attachment copies remain for next startup");
            }
        });
        Ok(current)
    }
}

async fn retire_sessions(root: PathBuf, live: PathBuf) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(&root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let retired = entry.path().canonicalize()?;
        anyhow::ensure!(
            retired.starts_with(&root),
            "staging directory outside cache namespace"
        );
        if retired == live {
            continue;
        }
        loop {
            let path = retired.clone();
            if tauri::async_runtime::spawn_blocking(move || {
                kukuri_iroh_node::remove_dir_step(&path, 32)
            })
            .await??
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cold_start_reclaims_old_copies_and_preserves_the_live_session() {
        let cache = tempfile::tempdir().unwrap();
        let root = cache.path().join("kukuri-post-attachments");
        std::fs::create_dir_all(&root).unwrap();
        let abandoned = tempfile::tempdir_in(&root).unwrap().keep();
        for index in 0..80 {
            std::fs::write(abandoned.join(index.to_string()), b"unfinished copy").unwrap();
        }
        let staging = AttachmentStaging::default();
        let live = staging.session(cache.path()).unwrap();
        let active = tempfile::NamedTempFile::new_in(live.path()).unwrap();
        assert!(Arc::ptr_eq(&live, &staging.session(cache.path()).unwrap()));
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while abandoned.exists() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(active.path().exists());
        assert!(live.path().exists());
    }
}
