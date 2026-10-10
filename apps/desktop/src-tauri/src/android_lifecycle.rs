use std::future::Future;

use tokio::sync::{Mutex, watch};

use crate::state::DesktopStartupStatus;

#[cfg(target_os = "android")]
pub(crate) struct AndroidLifecycle {
    changes: watch::Sender<Option<bool>>,
    worker: std::sync::Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
}

#[cfg(target_os = "android")]
impl Default for AndroidLifecycle {
    fn default() -> Self {
        Self {
            changes: watch::channel(None).0,
            worker: Default::default(),
        }
    }
}

#[cfg(target_os = "android")]
impl AndroidLifecycle {
    pub(crate) fn set_paused(&self, paused: bool) {
        self.changes.send_if_modified(|current| {
            if *current == Some(paused) {
                return false;
            }
            *current = Some(paused);
            true
        });
    }

    pub(crate) fn stop(&self) {
        if let Some(worker) = self.worker.lock().expect("lifecycle worker lock").take() {
            worker.abort();
        }
    }
}

#[cfg(target_os = "android")]
pub(crate) fn start(app: tauri::AppHandle) {
    use crate::{
        restore_lifecycle::DesktopOperationState,
        state::{DesktopStartupState, DesktopState},
    };
    use tauri::Manager;

    let changes = app.state::<AndroidLifecycle>().changes.subscribe();
    let startup = app.state::<DesktopStartupState>().subscribe();
    let worker = tauri::async_runtime::spawn({
        let app = app.clone();
        async move {
            let operations = app.state::<DesktopOperationState>();
            drive(changes, startup, &operations.switch_guard, |paused| {
                let app = app.clone();
                async move {
                    if let Some(host) = app.try_state::<DesktopState>().map(|state| state.host()) {
                        if paused {
                            host.suspend().await;
                        } else {
                            host.resume().await;
                        }
                        tracing::info!(paused, "android lifecycle applied");
                    }
                }
            })
            .await;
        }
    });
    *app.state::<AndroidLifecycle>()
        .worker
        .lock()
        .expect("lifecycle worker lock") = Some(worker);
}

async fn drive<F, Fut>(
    mut changes: watch::Receiver<Option<bool>>,
    mut startup: watch::Receiver<DesktopStartupStatus>,
    operations: &Mutex<()>,
    mut apply: F,
) where
    F: FnMut(bool) -> Fut,
    Fut: Future<Output = ()>,
{
    loop {
        {
            // 初期化・復元・切替を待ってから、最新の状態と現在 host を使う。callback ごとの task は作らない。
            let _guard = operations.lock().await;
            let paused = *changes.borrow_and_update();
            let ready = matches!(*startup.borrow_and_update(), DesktopStartupStatus::Ready);
            if let Some(paused) = paused
                && ready
            {
                apply(paused).await;
            }
        }
        let changed = tokio::select! {
            changed = changes.changed() => changed,
            changed = startup.changed() => changed,
        };
        if changed.is_err() {
            break;
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::disallowed_methods,
    reason = "Tauri の native 専用 test。共用 crate のブラウザ向け時刻・task 制約の対象外"
)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn consent_gate_preserves_only_the_latest_lifecycle_state() {
        let (events, changes) = watch::channel(None);
        let (status, startup) =
            watch::channel(crate::state::consent_required_status(&Default::default()));
        let (sent, mut calls) = mpsc::channel(4);
        let task = tokio::spawn(async move {
            drive(changes, startup, &Mutex::new(()), |paused| {
                let sent = sent.clone();
                async move { sent.send(paused).await.unwrap() }
            })
            .await;
        });
        events.send_replace(Some(true));
        tokio::task::yield_now().await;
        assert!(calls.try_recv().is_err());
        events.send_replace(Some(false));
        status.send_replace(DesktopStartupStatus::Ready);
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), calls.recv())
                .await
                .unwrap(),
            Some(false)
        );
        drop(events);
        task.await.unwrap();
    }

    #[tokio::test]
    async fn callbacks_wait_for_operations_and_coalesce_without_a_task_per_event() {
        let (events, changes) = watch::channel(None);
        let (_status, startup) = watch::channel(DesktopStartupStatus::Ready);
        let operations = std::sync::Arc::new(Mutex::new(()));
        let guard = operations.lock().await;
        let (sent, mut calls) = mpsc::channel(4);
        let task = tokio::spawn({
            let operations = operations.clone();
            async move {
                drive(changes, startup, &operations, |paused| {
                    let sent = sent.clone();
                    async move { sent.send(paused).await.unwrap() }
                })
                .await;
            }
        });
        tokio::task::yield_now().await;
        for index in 0..1000 {
            events.send_replace(Some(index % 2 == 0));
        }
        assert!(calls.try_recv().is_err());
        drop(guard);
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), calls.recv())
                .await
                .unwrap(),
            Some(false)
        );
        tokio::task::yield_now().await;
        assert!(calls.try_recv().is_err());
        drop(events);
        task.await.unwrap();
    }
}
