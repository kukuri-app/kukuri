//! Each maintenance lane owns at most one future. Dropping the scheduler drops
//! these futures too: no detached tasks may outlive an account/runtime shutdown.
use std::collections::HashSet;

use futures_util::{FutureExt, StreamExt, future::BoxFuture, stream::FuturesUnordered};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) enum MaintenanceJob {
    Session(String),
    Observations(String),
    Connectivity,
}

#[derive(Default)]
pub(super) struct MaintenanceTasks<'a> {
    active: HashSet<MaintenanceJob>,
    futures: FuturesUnordered<BoxFuture<'a, MaintenanceJob>>,
}

impl<'a> MaintenanceTasks<'a> {
    pub(super) fn insert(
        &mut self,
        job: MaintenanceJob,
        work: impl Future<Output = ()> + Send + 'a,
    ) {
        if self.active.insert(job.clone()) {
            self.futures.push(
                async move {
                    work.await;
                    job
                }
                .boxed(),
            );
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.active.is_empty()
    }

    pub(super) async fn next(&mut self) -> Option<MaintenanceJob> {
        let job = self.futures.next().await?;
        self.active.remove(&job);
        Some(job)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    #[tokio::test]
    async fn blocked_lanes_do_not_stop_other_nodes_or_duplicate_work() {
        let mut tasks = MaintenanceTasks::default();
        let blocked = MaintenanceJob::Session("blocked".into());
        let ready = MaintenanceJob::Session("ready".into());
        let calls = Arc::new(AtomicUsize::new(0));
        let guard = Arc::new(tokio::sync::Mutex::new(()));
        tasks.insert(blocked.clone(), {
            let guard = guard.clone();
            async move {
                let _held = guard.lock().await;
                std::future::pending::<()>().await;
            }
        });
        tasks.insert(
            MaintenanceJob::Observations("blocked".into()),
            std::future::pending(),
        );
        for _ in 0..3 {
            let calls = calls.clone();
            tasks.insert(ready.clone(), async move {
                calls.fetch_add(1, Ordering::SeqCst);
            });
        }
        // One poll is sufficient; there are no timers, network requests or sleeps.
        assert_eq!(tasks.next().now_or_never(), Some(Some(ready.clone())));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(guard.try_lock().is_err(), "blocked lane was polled");
        tasks.insert(ready.clone(), async {});
        assert_eq!(tasks.next().now_or_never(), Some(Some(ready)));
        assert!(tasks.next().now_or_never().is_none());
        drop(tasks);
        assert!(
            guard.try_lock().is_ok(),
            "shutdown must drop in-flight work synchronously"
        );
    }
}
