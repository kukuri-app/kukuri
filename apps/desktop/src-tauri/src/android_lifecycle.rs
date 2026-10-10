use std::future::Future;

use tokio::sync::{Mutex, watch};

use crate::state::DesktopStartupStatus;

#[derive(Clone, Copy, Default)]
struct LifecycleState {
    paused: Option<bool>,
    online: Option<bool>,
}

impl LifecycleState {
    fn effective_paused(self) -> Option<bool> {
        (self.paused.is_some() || self.online.is_some())
            .then_some(self.paused == Some(true) || self.online == Some(false))
    }
}

#[cfg(target_os = "android")]
pub(crate) struct AndroidLifecycle {
    changes: watch::Sender<LifecycleState>,
    worker: std::sync::Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
}

#[cfg(target_os = "android")]
impl Default for AndroidLifecycle {
    fn default() -> Self {
        Self {
            changes: watch::channel(LifecycleState::default()).0,
            worker: Default::default(),
        }
    }
}

#[cfg(target_os = "android")]
impl AndroidLifecycle {
    pub(crate) fn set_paused(&self, paused: bool) {
        self.changes.send_if_modified(|current| {
            if current.paused == Some(paused) {
                return false;
            }
            current.paused = Some(paused);
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
pub(crate) fn network_plugin() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    use tauri::{
        Manager,
        ipc::{Channel, InvokeResponseBody},
    };

    tauri::plugin::Builder::new("android-network")
        .setup(|app, api| {
            let changes = app.state::<AndroidLifecycle>().changes.clone();
            let handler = Channel::<serde_json::Value>::new(move |message| {
                if let InvokeResponseBody::Json(message) = message {
                    let online = serde_json::from_str::<bool>(&message)?;
                    tracing::info!(online, "android network changed");
                    // 同じ online 値でも route が変われば、現在の需要へ再通知する。
                    changes.send_modify(|state| state.online = Some(online));
                }
                Ok(())
            });
            let plugin = api.register_android_plugin("app.kukuri.android", "NetworkPlugin")?;
            plugin.run_mobile_plugin::<()>("watch", serde_json::json!({ "handler": handler }))?;
            Ok(())
        })
        .build()
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
    mut changes: watch::Receiver<LifecycleState>,
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
            let paused = changes.borrow_and_update().effective_paused();
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
        let (events, changes) = watch::channel(LifecycleState::default());
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
        events.send_modify(|state| state.paused = Some(true));
        tokio::task::yield_now().await;
        assert!(calls.try_recv().is_err());
        events.send_modify(|state| {
            state.paused = Some(false);
            state.online = Some(true);
        });
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
        let (events, changes) = watch::channel(LifecycleState::default());
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
            events.send_modify(|state| {
                state.paused = Some(index % 2 == 0);
                state.online = Some(index % 2 != 0);
            });
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

    #[tokio::test]
    async fn online_and_route_changes_preserve_the_activity_pause() {
        let (events, changes) = watch::channel(LifecycleState::default());
        let (_status, startup) = watch::channel(DesktopStartupStatus::Ready);
        let (sent, mut calls) = mpsc::channel(4);
        let task = tokio::spawn(async move {
            drive(changes, startup, &Mutex::new(()), |paused| {
                let sent = sent.clone();
                async move { sent.send(paused).await.unwrap() }
            })
            .await;
        });
        for (paused, online, expected) in [
            (true, false, true),
            (true, true, true),
            (false, true, false),
            (false, false, true),
            (false, true, false),
            // online のままの Wi-Fi → mobile / route の変化も復帰入口を通す。
            (false, true, false),
        ] {
            events.send_replace(LifecycleState {
                paused: Some(paused),
                online: Some(online),
            });
            assert_eq!(
                tokio::time::timeout(Duration::from_secs(1), calls.recv())
                    .await
                    .unwrap(),
                Some(expected)
            );
        }
        drop(events);
        task.await.unwrap();
    }
}
