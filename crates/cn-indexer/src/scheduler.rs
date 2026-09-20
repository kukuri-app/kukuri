//! Bounded post-ingest scheduling and observable per-revision job state.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

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

#[derive(Clone, Debug)]
pub struct PostFetchJobLease {
    key: PostFetchJobKey,
    source_revision: String,
    lease: u64,
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
    jobs: Mutex<BTreeMap<PostFetchJobKey, PostFetchJobRecord>>,
    next_lease: AtomicU64,
}

impl PostFetchScheduler {
    pub fn new(max_concurrent_posts: usize) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(max_concurrent_posts.max(1))),
            jobs: Mutex::new(BTreeMap::new()),
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

    pub fn reconcile_scope(&self, scope_kind: &str, scope_id: &str, current: &BTreeSet<String>) {
        let now = chrono::Utc::now().timestamp();
        let mut jobs = self.jobs.lock().expect("post scheduler jobs poisoned");
        jobs.retain(|key, job| {
            if key.scope_kind != scope_kind
                || key.scope_id != scope_id
                || current.contains(&key.object_id)
            {
                return true;
            }
            if matches!(
                job.state,
                PostFetchJobState::Queued
                    | PostFetchJobState::Fetching
                    | PostFetchJobState::Processing
                    | PostFetchJobState::RetryWait
            ) {
                job.state = PostFetchJobState::Cancelled;
                job.updated_at = now;
                true
            } else {
                false
            }
        });
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

    #[tokio::test]
    async fn stale_completion_cannot_replace_newer_revision() {
        let scheduler = PostFetchScheduler::new(1);
        let old = scheduler.enqueue(key("post"), "r1".into()).unwrap();
        let _permit = scheduler.start(&old).await.unwrap();
        let new = scheduler.enqueue(key("post"), "r2".into()).unwrap();
        assert!(!scheduler.finish(&old, PostFetchJobState::Completed));
        assert!(scheduler.finish(&new, PostFetchJobState::Completed));
    }
}
