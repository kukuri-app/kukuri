//! Each maintenance lane owns at most one future. Dropping the scheduler drops
//! these futures too: no detached tasks may outlive an account/runtime shutdown.
use std::collections::HashSet;

use futures_util::{
    StreamExt,
    future::{Join, Ready, join, ready},
    stream::FuturesUnordered,
};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) enum MaintenanceJob {
    Session(String),
    Observations(String),
}

/// 1 つの呼出元の作業の future の型で持つ（box に包まない）。Send は作業の future から決まるので、
/// wasm の Send でない HTTP の future もそのまま入る。
pub(super) struct MaintenanceTasks<F: Future<Output = ()>> {
    active: HashSet<MaintenanceJob>,
    futures: FuturesUnordered<Join<F, Ready<MaintenanceJob>>>,
}

impl<F: Future<Output = ()>> Default for MaintenanceTasks<F> {
    fn default() -> Self {
        Self {
            active: HashSet::new(),
            futures: FuturesUnordered::new(),
        }
    }
}

impl<F: Future<Output = ()>> MaintenanceTasks<F> {
    pub(super) fn insert(&mut self, job: MaintenanceJob, work: F) {
        if self.active.insert(job.clone()) {
            self.futures.push(join(work, ready(job)));
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.active.is_empty()
    }

    pub(super) async fn next(&mut self) -> Option<MaintenanceJob> {
        let ((), job) = self.futures.next().await?;
        self.active.remove(&job);
        Some(job)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::FutureExt;
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
            .boxed()
        });
        tasks.insert(
            MaintenanceJob::Observations("blocked".into()),
            std::future::pending().boxed(),
        );
        for _ in 0..3 {
            let calls = calls.clone();
            tasks.insert(
                ready.clone(),
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                }
                .boxed(),
            );
        }
        // One poll is sufficient; there are no timers, network requests or sleeps.
        assert_eq!(tasks.next().now_or_never(), Some(Some(ready.clone())));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(guard.try_lock().is_err(), "blocked lane was polled");
        tasks.insert(ready.clone(), async {}.boxed());
        assert_eq!(tasks.next().now_or_never(), Some(Some(ready)));
        assert!(tasks.next().now_or_never().is_none());
        drop(tasks);
        assert!(
            guard.try_lock().is_ok(),
            "shutdown must drop in-flight work synchronously"
        );
    }
}
