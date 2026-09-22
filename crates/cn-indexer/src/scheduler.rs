//! Bounded post-ingest scheduling and observable per-revision job state.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

use serde::{Deserialize, Serialize};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct PostFetchJobKey {
    pub scope_kind: String,
    pub scope_id: String,
    pub object_id: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PostFetchJobState {
    Queued,
    Fetching,
    Processing,
    RetryWait,
    Completed,
    Suppressed,
    Cancelled,
}

#[derive(Clone, Debug)]
struct PostFetchJobRecord {
    source_revision: String,
    lease: u64,
    state: PostFetchJobState,
    queued_at: i64,
    updated_at: i64,
    attempts: u32,
}

#[derive(Debug)]
pub struct PostFetchJobLease {
    key: PostFetchJobKey,
    source_revision: String,
    lease: u64,
    jobs: Weak<Mutex<BTreeMap<PostFetchJobKey, PostFetchJobRecord>>>,
}

// leaseは実行futureが所有する。await中のcancelでも、同世代の実行中記録を回収可能にする。
impl Drop for PostFetchJobLease {
    fn drop(&mut self) {
        let Some(jobs) = self.jobs.upgrade() else {
            return;
        };
        let mut jobs = jobs.lock().expect("post scheduler jobs poisoned");
        if let Some(current) = jobs.get_mut(&self.key)
            && current.lease == self.lease
            && matches!(
                current.state,
                PostFetchJobState::Queued
                    | PostFetchJobState::Fetching
                    | PostFetchJobState::Processing
            )
        {
            current.state = PostFetchJobState::Cancelled;
            current.updated_at = chrono::Utc::now().timestamp();
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct PostFetchSchedulerSnapshot {
    pub queued: u64,
    pub fetching: u64,
    pub processing: u64,
    pub retry_wait: u64,
    pub completed: u64,
    pub suppressed: u64,
    pub cancelled: u64,
    pub oldest_pending_at: Option<i64>,
}

#[derive(Debug)]
pub struct PostFetchScheduler {
    permits: Arc<Semaphore>,
    jobs: Arc<Mutex<BTreeMap<PostFetchJobKey, PostFetchJobRecord>>>,
    next_lease: AtomicU64,
}

/// 実行中と直近の診断記録を含む固定窓。再試行の正本はdocsであり、この台帳ではない。
const MAX_RECORDED_JOBS: usize = 1024;

impl PostFetchScheduler {
    pub fn new(max_concurrent_posts: usize) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(max_concurrent_posts.max(1))),
            jobs: Arc::new(Mutex::new(BTreeMap::new())),
            next_lease: AtomicU64::new(0),
        }
    }

    pub fn enqueue(
        &self,
        key: PostFetchJobKey,
        source_revision: String,
    ) -> Option<PostFetchJobLease> {
        let mut jobs = self.jobs.lock().expect("post scheduler jobs poisoned");
        if jobs.get(&key).is_some_and(|existing| {
            existing.source_revision == source_revision
                && matches!(
                    existing.state,
                    PostFetchJobState::Queued
                        | PostFetchJobState::Fetching
                        | PostFetchJobState::Processing
                )
        }) {
            return None;
        }
        if !jobs.contains_key(&key) && jobs.len() >= MAX_RECORDED_JOBS {
            let oldest = jobs
                .iter()
                .filter(|(_, job)| {
                    !matches!(
                        job.state,
                        PostFetchJobState::Queued
                            | PostFetchJobState::Fetching
                            | PostFetchJobState::Processing
                    )
                })
                .min_by_key(|(_, job)| job.lease)
                .map(|(key, _)| key.clone());
            // 全枠が実行中ならそのleaseを壊さず延期する。次のevent/passで再要求できる。
            jobs.remove(&oldest?);
        }
        let lease = self.next_lease.fetch_add(1, Ordering::Relaxed) + 1;
        let now = chrono::Utc::now().timestamp();
        jobs.insert(
            key.clone(),
            PostFetchJobRecord {
                source_revision: source_revision.clone(),
                lease,
                state: PostFetchJobState::Queued,
                queued_at: now,
                updated_at: now,
                attempts: 0,
            },
        );
        Some(PostFetchJobLease {
            key,
            source_revision,
            lease,
            jobs: Arc::downgrade(&self.jobs),
        })
    }

    pub async fn start(&self, lease: &PostFetchJobLease) -> Option<OwnedSemaphorePermit> {
        let permit = Arc::clone(&self.permits).acquire_owned().await.ok()?;
        let mut jobs = self.jobs.lock().expect("post scheduler jobs poisoned");
        let current = jobs.get_mut(&lease.key)?;
        if current.lease != lease.lease || current.source_revision != lease.source_revision {
            return None;
        }
        current.state = PostFetchJobState::Fetching;
        current.attempts = current.attempts.saturating_add(1);
        current.updated_at = chrono::Utc::now().timestamp();
        Some(permit)
    }

    pub fn mark_processing(&self, lease: &PostFetchJobLease) -> bool {
        self.transition_current(lease, PostFetchJobState::Processing)
    }

    pub fn is_current(&self, lease: &PostFetchJobLease) -> bool {
        self.jobs
            .lock()
            .expect("post scheduler jobs poisoned")
            .get(&lease.key)
            .is_some_and(|current| {
                current.lease == lease.lease
                    && current.source_revision == lease.source_revision
                    && !matches!(current.state, PostFetchJobState::Cancelled)
            })
    }

    pub fn finish(&self, lease: &PostFetchJobLease, state: PostFetchJobState) -> bool {
        debug_assert!(matches!(
            state,
            PostFetchJobState::RetryWait
                | PostFetchJobState::Completed
                | PostFetchJobState::Suppressed
                | PostFetchJobState::Cancelled
        ));
        self.transition_current(lease, state)
    }

    fn transition_current(&self, lease: &PostFetchJobLease, state: PostFetchJobState) -> bool {
        let mut jobs = self.jobs.lock().expect("post scheduler jobs poisoned");
        let Some(current) = jobs.get_mut(&lease.key) else {
            return false;
        };
        if current.lease != lease.lease || current.source_revision != lease.source_revision {
            return false;
        }
        current.state = state;
        current.updated_at = chrono::Utc::now().timestamp();
        true
    }

    pub fn snapshot(&self) -> PostFetchSchedulerSnapshot {
        let jobs = self.jobs.lock().expect("post scheduler jobs poisoned");
        let mut snapshot = PostFetchSchedulerSnapshot::default();
        for job in jobs.values() {
            match job.state {
                PostFetchJobState::Queued => snapshot.queued += 1,
                PostFetchJobState::Fetching => snapshot.fetching += 1,
                PostFetchJobState::Processing => snapshot.processing += 1,
                PostFetchJobState::RetryWait => snapshot.retry_wait += 1,
                PostFetchJobState::Completed => snapshot.completed += 1,
                PostFetchJobState::Suppressed => snapshot.suppressed += 1,
                PostFetchJobState::Cancelled => snapshot.cancelled += 1,
            }
            if matches!(
                job.state,
                PostFetchJobState::Queued
                    | PostFetchJobState::Fetching
                    | PostFetchJobState::Processing
                    | PostFetchJobState::RetryWait
            ) {
                snapshot.oldest_pending_at = Some(
                    snapshot
                        .oldest_pending_at
                        .map_or(job.queued_at, |oldest| oldest.min(job.queued_at)),
                );
            }
        }
        snapshot
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(id: &str) -> PostFetchJobKey {
        PostFetchJobKey {
            scope_kind: "public_topic".into(),
            scope_id: "general".into(),
            object_id: id.into(),
        }
    }

    #[tokio::test]
    async fn bounded_scheduler_allows_healthy_job_while_slow_job_is_waiting() {
        let scheduler = Arc::new(PostFetchScheduler::new(2));
        let slow = scheduler.enqueue(key("slow"), "r1".into()).unwrap();
        let healthy = scheduler.enqueue(key("healthy"), "r1".into()).unwrap();
        let next = scheduler.enqueue(key("next"), "r1".into()).unwrap();
        let slow_permit = scheduler.start(&slow).await.unwrap();
        let healthy_permit = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            scheduler.start(&healthy),
        )
        .await
        .expect("healthy job obtains the independent slot")
        .unwrap();
        drop(healthy_permit);
        let next_permit = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            scheduler.start(&next),
        )
        .await
        .expect("released slot is replenished without waiting for the slow job")
        .unwrap();
        drop((slow_permit, next_permit));
    }

    #[test]
    fn completed_job_history_does_not_grow_with_rotated_buckets() {
        let scheduler = PostFetchScheduler::new(2);
        for index in 0..2048 {
            let lease = scheduler
                .enqueue(key(&format!("post-{index}")), "signed-id".into())
                .unwrap();
            assert!(scheduler.finish(&lease, PostFetchJobState::Completed));
        }
        assert!(scheduler.snapshot().completed <= 1024);
    }

    #[tokio::test]
    async fn stale_completion_cannot_replace_newer_revision() {
        let scheduler = PostFetchScheduler::new(1);
        let old = scheduler.enqueue(key("post"), "r1".into()).unwrap();
        let _permit = scheduler.start(&old).await.unwrap();
        let new = scheduler.enqueue(key("post"), "r2".into()).unwrap();
        assert!(!scheduler.finish(&old, PostFetchJobState::Completed));
        drop(old);
        assert!(scheduler.is_current(&new));
        assert!(scheduler.finish(&new, PostFetchJobState::Completed));
    }

    #[tokio::test]
    async fn capacity_defers_new_jobs_without_cancelling_active_leases() {
        let scheduler = PostFetchScheduler::new(1);
        let running = scheduler.enqueue(key("running"), "r1".into()).unwrap();
        let _permit = scheduler.start(&running).await.unwrap();
        let mut queued = Vec::new();
        for index in 1..MAX_RECORDED_JOBS {
            queued.push(
                scheduler
                    .enqueue(key(&format!("queued-{index}")), "r1".into())
                    .unwrap(),
            );
        }
        assert!(scheduler.enqueue(key("later"), "r1".into()).is_none());
        assert!(scheduler.is_current(&running));
        assert!(queued.iter().all(|lease| scheduler.is_current(lease)));
        assert!(scheduler.finish(&queued[0], PostFetchJobState::Completed));
        assert!(scheduler.enqueue(key("later"), "r1".into()).is_some());
        assert!(scheduler.is_current(&running));
    }

    #[tokio::test]
    async fn cancelled_waiting_and_fetching_jobs_release_their_records() {
        for waiting in [false, true] {
            let scheduler = Arc::new(PostFetchScheduler::new(1));
            let blocker = scheduler.enqueue(key("blocker"), "r1".into()).unwrap();
            let held = if waiting {
                scheduler.start(&blocker).await
            } else {
                None
            };
            let (ready, observed) = tokio::sync::oneshot::channel();
            let task_scheduler = scheduler.clone();
            let task = tokio::spawn(async move {
                let lease = task_scheduler
                    .enqueue(key("cancelled"), "r1".into())
                    .unwrap();
                if waiting {
                    let _ = ready.send(());
                } else {
                    let _permit = task_scheduler.start(&lease).await.unwrap();
                    let _ = ready.send(());
                    std::future::pending::<()>().await;
                }
                let _permit = task_scheduler.start(&lease).await;
                std::future::pending::<()>().await;
            });
            observed.await.unwrap();
            task.abort();
            let _ = task.await;
            assert_eq!(scheduler.snapshot().cancelled, 1, "waiting={waiting}");
            drop(held);
            let next = scheduler
                .enqueue(key("another-scope-post"), "r1".into())
                .unwrap();
            assert!(scheduler.start(&next).await.is_some());
        }
    }
}
