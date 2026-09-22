//! docs停止の所有権。callerのcancelで同期停止やcapability失効が半端にならないようにする。

use super::*;
use tokio::sync::oneshot;

#[cfg(test)]
pub(super) enum TestHook {
    FailLeave,
    LoseCloseAck,
    Pause {
        entered: Arc<tokio::sync::Notify>,
        resume: Arc<tokio::sync::Notify>,
    },
}

impl IrohDocsSync {
    pub(super) async fn close_replica_owned(
        &self,
        replica_id: &ReplicaId,
        revoke: bool,
    ) -> Result<()> {
        // registry変更より前のcancelは無変更。変更を始めた後はownerのtaskが完了まで持つ。
        let mut tasks = self.close_tasks.lock().await;
        while tasks.try_join_next().is_some() {}
        if tasks.len() >= 32 {
            tasks.join_next().await;
        }
        #[cfg(test)]
        let hook = self.close_hook.lock().await.take();
        let mut replicas = self.replicas.clone().lock_owned().await;
        let secrets = self.private_replica_secrets.clone();
        let id = replica_id.as_str().to_string();
        let (send, receive) = oneshot::channel();
        tasks.spawn(async move {
            #[cfg(test)]
            if let Some(TestHook::Pause { entered, resume }) = &hook {
                entered.notify_one();
                resume.notified().await;
            }
            // revokeも同じtask内。secretを先に消してからlock待ちするとcancel時にsyncが残る。
            if revoke {
                secrets.lock().await.remove(&id);
            }
            let result = if let Some(mut handle) = replicas.remove(&id) {
                handle.sync_requested = false;
                handle.closing = true;
                if let Some(task) = handle.live_task.take() {
                    task.abort();
                    let _ = task.await;
                }
                let leave = async {
                    #[cfg(test)]
                    if matches!(&hook, Some(TestHook::FailLeave)) {
                        anyhow::bail!("injected leave failure");
                    }
                    handle.doc.leave().await
                }
                .await;
                match leave {
                    Ok(()) => {
                        // closeはRPC前に共有closedフラグを立てる。失敗してもmapへ戻さない。
                        let result = handle.doc.close().await;
                        #[cfg(test)]
                        let result = if matches!(&hook, Some(TestHook::LoseCloseAck)) {
                            Err(anyhow::anyhow!("injected close acknowledgement loss"))
                        } else {
                            result
                        };
                        result
                    }
                    Err(error) => {
                        // leave未完了はquery/restartから隔離し、明示closeだけで再試行する。
                        replicas.insert(id, handle);
                        Err(error)
                    }
                }
            } else {
                Ok(())
            };
            let _ = send.send(result);
        });
        drop(tasks);
        receive.await.context("replica close task stopped")?
    }
}

#[cfg(test)]
#[path = "iroh_sync_lifecycle_tests.rs"]
mod tests;
