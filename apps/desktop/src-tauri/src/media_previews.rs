use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::{Result, ensure};
use tokio::sync::watch;

const MAX_PREVIEW_LEASES: usize = 256;

struct PreviewLease {
    path: PathBuf,
    cancel: watch::Sender<bool>,
}

pub(crate) struct MediaPreviewFiles {
    root: Option<PathBuf>,
    leases: Mutex<HashMap<String, PreviewLease>>,
    cancelled: Mutex<VecDeque<String>>,
}

impl MediaPreviewFiles {
    pub(crate) fn new(root: Option<PathBuf>) -> Self {
        if let Some(root) = &root {
            let _ = std::fs::remove_dir_all(root);
        }
        Self {
            root,
            leases: Mutex::new(HashMap::new()),
            cancelled: Mutex::new(VecDeque::new()),
        }
    }

    pub(crate) fn begin(
        &self,
        request_id: &str,
        mime: &str,
    ) -> Result<(PathBuf, watch::Receiver<bool>)> {
        ensure!(mime.starts_with("image/") || mime.starts_with("video/"), "unsupported media display MIME");
        ensure!(
            valid_request_id(request_id),
            "invalid media display request id"
        );
        let root = self.root.as_ref().ok_or_else(|| anyhow::anyhow!("app cache path unavailable"))?;
        std::fs::create_dir_all(root)?;
        let mut leases = self.leases.lock().expect("media previews poisoned");
        let mut cancelled = self.cancelled.lock().expect("media previews poisoned");
        if let Some(index) = cancelled.iter().position(|id| id == request_id) {
            cancelled.remove(index);
            anyhow::bail!("media display request was cancelled");
        }
        ensure!(leases.len() < MAX_PREVIEW_LEASES, "media display lease limit reached");
        ensure!(!leases.contains_key(request_id), "duplicate media display request id");
        let path = root.join(format!("{}.{}", uuid::Uuid::new_v4(), extension(mime)));
        let (cancel, cancelled) = watch::channel(false);
        leases.insert(request_id.to_string(), PreviewLease { path: path.clone(), cancel });
        Ok((path, cancelled))
    }

    pub(crate) fn contains(&self, request_id: &str) -> bool {
        self.leases.lock().expect("media previews poisoned").contains_key(request_id)
    }

    pub(crate) fn release(&self, request_id: &str) {
        if !valid_request_id(request_id) {
            return;
        }
        let mut leases = self.leases.lock().expect("media previews poisoned");
        if let Some(lease) = leases.remove(request_id) {
            let _ = lease.cancel.send(true);
            remove_display_file(lease.path);
        } else {
            let mut cancelled = self.cancelled.lock().expect("media previews poisoned");
            if cancelled.len() == MAX_PREVIEW_LEASES {
                cancelled.pop_front();
            }
            cancelled.push_back(request_id.to_string());
        }
    }

    pub(crate) fn clear(&self) {
        let leases = std::mem::take(&mut *self.leases.lock().expect("media previews poisoned"));
        for (_, lease) in leases {
            let _ = lease.cancel.send(true);
            remove_display_file(lease.path);
        }
    }
}

fn remove_display_file(path: PathBuf) {
    match std::fs::remove_file(&path) {
        Ok(()) => return,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(_) => {}
    }
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        runtime.spawn(async move {
            for delay_ms in [50, 200, 1000, 3000] {
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                if tokio::fs::remove_file(&path).await.is_ok() || !path.exists() {
                    break;
                }
            }
        });
    }
}

impl Drop for MediaPreviewFiles {
    fn drop(&mut self) {
        self.clear();
    }
}

fn extension(mime: &str) -> &'static str {
    match mime {
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/avif" => "avif",
        "video/mp4" => "mp4",
        "video/webm" => "webm",
        "video/quicktime" => "mov",
        _ => "bin",
    }
}

fn valid_request_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_and_account_clear_remove_only_owned_preview_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("kukuri-display");
        let previews = MediaPreviewFiles::new(Some(root.clone()));
        let (first, _) = previews.begin("first", "video/mp4").unwrap();
        let (second, _) = previews.begin("second", "image/png").unwrap();
        std::fs::write(&first, b"first").unwrap();
        std::fs::write(&second, b"second").unwrap();
        let outside = dir.path().join("outside");
        std::fs::write(&outside, b"keep").unwrap();
        previews.release("first");
        assert!(!first.exists() && second.exists());
        previews.clear();
        assert!(!second.exists() && outside.exists());
        previews.release("early-cancel");
        assert!(previews.begin("early-cancel", "image/png").is_err());
    }

    // #1419 AC-3: 他の表示要求が取得待ち(remote は最大 30 秒)で残っていても、次の要求を断らない。
    // 手元 cache にある画像は、この要求の中ですぐ返る。
    #[test]
    fn pending_requests_do_not_refuse_the_next_display_request() {
        let dir = tempfile::tempdir().unwrap();
        let previews = MediaPreviewFiles::new(Some(dir.path().join("kukuri-display")));
        let pending = (0..16)
            .map(|index| previews.begin(&format!("pending-{index}"), "image/png"))
            .collect::<Result<Vec<_>>>()
            .unwrap();
        assert!(previews.begin("local", "image/png").is_ok());
        drop(pending);
    }
}
