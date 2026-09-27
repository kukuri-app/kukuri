use std::fmt::Debug;
use std::future::Future;
use std::sync::OnceLock;
use std::time::Duration;

use async_trait::async_trait;
use thiserror::Error;
use tokio::sync::{Mutex, MutexGuard};
use tokio::time::{sleep, timeout};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TestResource {
    ProcessEnvironment,
    IdentityStorage,
    IrohNetwork,
    DhtTestnet,
    CommunityNodeServer,
}

fn resource_mutex(resource: TestResource) -> &'static Mutex<()> {
    static PROCESS_ENVIRONMENT: OnceLock<Mutex<()>> = OnceLock::new();
    static IDENTITY_STORAGE: OnceLock<Mutex<()>> = OnceLock::new();
    static IROH_NETWORK: OnceLock<Mutex<()>> = OnceLock::new();
    static DHT_TESTNET: OnceLock<Mutex<()>> = OnceLock::new();
    static COMMUNITY_NODE_SERVER: OnceLock<Mutex<()>> = OnceLock::new();

    match resource {
        TestResource::ProcessEnvironment => PROCESS_ENVIRONMENT.get_or_init(Mutex::default),
        TestResource::IdentityStorage => IDENTITY_STORAGE.get_or_init(Mutex::default),
        TestResource::IrohNetwork => IROH_NETWORK.get_or_init(Mutex::default),
        TestResource::DhtTestnet => DHT_TESTNET.get_or_init(Mutex::default),
        TestResource::CommunityNodeServer => COMMUNITY_NODE_SERVER.get_or_init(Mutex::default),
    }
}

pub async fn lock_test_resource(resource: TestResource) -> MutexGuard<'static, ()> {
    resource_mutex(resource).lock().await
}

pub fn try_lock_test_resource(resource: TestResource) -> Option<MutexGuard<'static, ()>> {
    resource_mutex(resource).try_lock().ok()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PollState<T> {
    Pending,
    Ready(T),
}

#[derive(Debug, Error)]
pub enum PollError<E> {
    #[error("poll timed out")]
    Timeout,
    #[error("poll operation failed: {0}")]
    Operation(E),
}

pub async fn poll_until<F, Fut, T, E>(
    deadline: Duration,
    interval: Duration,
    stable_ready_polls: usize,
    mut operation: F,
) -> Result<T, PollError<E>>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<PollState<T>, E>>,
{
    let required = stable_ready_polls.max(1);
    timeout(deadline, async {
        let mut ready_polls = 0usize;
        loop {
            match operation().await.map_err(PollError::Operation)? {
                PollState::Ready(value) => {
                    ready_polls += 1;
                    if ready_polls >= required {
                        return Ok(value);
                    }
                }
                PollState::Pending => ready_polls = 0,
            }
            sleep(interval).await;
        }
    })
    .await
    .map_err(|_| PollError::Timeout)?
}

pub fn constrained_timeout(local: Duration, constrained: Duration) -> Duration {
    if cfg!(target_os = "windows") || std::env::var_os("GITHUB_ACTIONS").is_some() {
        constrained
    } else {
        local
    }
}

fn flag_value_enabled(value: Option<&str>) -> bool {
    value.is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes"
        )
    })
}

pub fn env_flag_enabled(name: &str) -> bool {
    flag_value_enabled(std::env::var(name).ok().as_deref())
}

pub fn gated_env_url(gate_name: &str, url_name: &str, default_url: &str) -> Option<String> {
    if !env_flag_enabled(gate_name) {
        return None;
    }
    Some(
        std::env::var(url_name)
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| default_url.to_string()),
    )
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TopicSyncSnapshot {
    pub topic: String,
    pub peer_count: usize,
    pub configured_peer_count: usize,
    pub missing_peer_count: usize,
    pub delivery_state: String,
    pub status_detail: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SyncSnapshot {
    pub connected: bool,
    pub peer_count: usize,
    pub status_detail: String,
    pub last_error: Option<String>,
    pub discovery_connected_peer_count: usize,
    pub docs_assist_peer_count: usize,
    pub topics: Vec<TopicSyncSnapshot>,
}

#[async_trait]
pub trait SyncStatusSource {
    type Error: Debug + Send;

    async fn sync_snapshot(&self) -> Result<SyncSnapshot, Self::Error>;
}

pub fn format_sync_snapshot(snapshot: &SyncSnapshot, topic: &str) -> String {
    let topic_status = snapshot
        .topics
        .iter()
        .find(|entry| entry.topic == topic)
        .map(|entry| {
            format!(
                "topic_peers={}, configured_peer_count={}, missing_peer_count={}, delivery_state={}, status_detail={}",
                entry.peer_count,
                entry.configured_peer_count,
                entry.missing_peer_count,
                entry.delivery_state,
                entry.status_detail
            )
        })
        .unwrap_or_else(|| "topic_status=missing".to_string());
    format!(
        "connected={}, peer_count={}, status_detail={}, last_error={:?}, discovery_connected_peer_count={}, docs_assist_peer_count={}, {}",
        snapshot.connected,
        snapshot.peer_count,
        snapshot.status_detail,
        snapshot.last_error,
        snapshot.discovery_connected_peer_count,
        snapshot.docs_assist_peer_count,
        topic_status
    )
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;

    use super::*;

    #[tokio::test]
    async fn poll_requires_consecutive_ready_results() {
        let states = [true, true, false, true, true, true];
        let mut index = 0usize;
        let value = poll_until(Duration::from_secs(1), Duration::ZERO, 3, || {
            let ready = states[index];
            index += 1;
            async move {
                Ok::<_, Infallible>(if ready {
                    PollState::Ready(index)
                } else {
                    PollState::Pending
                })
            }
        })
        .await
        .expect("poll result");
        assert_eq!(value, 6);
    }

    #[tokio::test]
    async fn poll_reports_timeout() {
        let result = poll_until(
            Duration::from_millis(5),
            Duration::from_millis(1),
            1,
            || async { Ok::<_, Infallible>(PollState::<()>::Pending) },
        )
        .await;
        assert!(matches!(result, Err(PollError::Timeout)));
    }

    #[test]
    fn snapshot_format_keeps_diagnostics() {
        let snapshot = SyncSnapshot {
            connected: true,
            peer_count: 1,
            status_detail: "ready".into(),
            topics: vec![TopicSyncSnapshot {
                topic: "kukuri:test".into(),
                peer_count: 1,
                delivery_state: "Live".into(),
                status_detail: "joined".into(),
                ..TopicSyncSnapshot::default()
            }],
            ..SyncSnapshot::default()
        };
        assert!(format_sync_snapshot(&snapshot, "kukuri:test").contains("delivery_state=Live"));
    }

    #[test]
    fn env_gate_truth_values_are_normalized() {
        for value in ["1", "true", "TRUE", " yes ", "YES"] {
            assert!(flag_value_enabled(Some(value)), "{value}");
        }
        for value in ["", "0", "false", "on"] {
            assert!(!flag_value_enabled(Some(value)), "{value}");
        }
        assert!(!flag_value_enabled(None));
    }

    #[tokio::test]
    async fn resource_locks_exclude_only_the_same_resource() {
        let environment = lock_test_resource(TestResource::ProcessEnvironment).await;
        assert!(try_lock_test_resource(TestResource::ProcessEnvironment).is_none());
        let iroh = try_lock_test_resource(TestResource::IrohNetwork)
            .expect("independent resource should remain available");
        drop(iroh);
        drop(environment);
        assert!(try_lock_test_resource(TestResource::ProcessEnvironment).is_some());
    }
}
