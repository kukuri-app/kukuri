//! 通信状態の差分の push(#1221 R2-D)。
//!
//! transport(neighbor・受信・購読)、app-api(同期の時刻・docs の活動・gossip の停止)、CN の session が
//! 変わった所に印を付け、ここは印の付いた部分だけを作り直して差分を送る。変化の無い間は何もしない。
//! 旧来の 3 秒ごとの全体の作り直しと等値比較(WP-B13 の Decision)は失効した(ADR 0055 §3.2)。
use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Weak};
use std::time::Duration;

use kukuri_transport::StatusKey;
use tracing::warn;

use super::*;

/// 同じ種類の頻繁な変化(受信の時刻・`last_sync_ts`)は、この間隔に 1 回までにまとめる。
pub(crate) const SYNC_STATUS_COALESCE: Duration = Duration::from_secs(1);

/// 最後に送った件数と CN の node。印が付いても変わっていない部分は送らない。
#[derive(Default)]
struct Emitted {
    summary: Option<SyncStatus>,
    nodes: HashMap<String, CommunityNodeNodeStatus>,
}

impl DesktopRuntime {
    async fn emit_sync_status_delta(&self, keys: BTreeSet<StatusKey>, emitted: &mut Emitted) {
        #[cfg(test)]
        self.sync_status_delta_reads
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let (sync_status, removed_topics) =
            match self.app_service.sync_status_delta(Some(&keys)).await {
                Ok((status, removed)) => {
                    let summary = SyncStatus {
                        topic_diagnostics: Vec::new(),
                        ..status.clone()
                    };
                    let unchanged = status.topic_diagnostics.is_empty()
                        && removed.is_empty()
                        && emitted.summary.as_ref() == Some(&summary);
                    emitted.summary = Some(summary);
                    ((!unchanged).then(|| Box::new(status)), removed)
                }
                Err(error) => {
                    warn!(error = %format!("{error:#}"), "failed to build the sync status delta");
                    (None, Vec::new())
                }
            };
        let config = self.community_node_config.lock().await.clone();
        let dirty = if keys.contains(&StatusKey::All) {
            config
                .nodes
                .iter()
                .map(|node| node.base_url.clone())
                .chain(emitted.nodes.keys().cloned())
                .collect::<BTreeSet<_>>()
        } else {
            keys.into_iter()
                .filter_map(|key| match key {
                    StatusKey::CommunityNode(base_url) => Some(base_url),
                    _ => None,
                })
                .collect()
        };
        let (mut community_node_statuses, mut removed_community_nodes) = (Vec::new(), Vec::new());
        for base_url in dirty {
            let Some(node) = config.nodes.iter().find(|node| node.base_url == base_url) else {
                emitted.nodes.remove(&base_url);
                removed_community_nodes.push(base_url);
                continue;
            };
            match self.community_node_status(node.clone(), None, None).await {
                Ok(status) if emitted.nodes.get(&base_url) != Some(&status) => {
                    emitted.nodes.insert(base_url, status.clone());
                    community_node_statuses.push(status);
                }
                Ok(_) => {}
                Err(error) => {
                    warn!(%base_url, error = %format!("{error:#}"), "failed to build the community-node status delta");
                }
            }
        }
        if sync_status.is_none()
            && community_node_statuses.is_empty()
            && removed_community_nodes.is_empty()
        {
            return;
        }
        self.emit_event(RuntimeEvent::SyncStatusChanged {
            sync_status,
            removed_topics,
            community_node_statuses,
            removed_community_nodes,
        });
    }

    pub async fn start_sync_status_observer(self: &Arc<Self>) {
        let mut task = self.sync_status_observer_task.lock().await;
        if task.as_ref().is_some_and(|handle| !handle.is_finished()) {
            return;
        }
        let changes = self.iroh_stack.status_changes.clone();
        let weak: Weak<Self> = Arc::downgrade(self);
        *task = Some(tokio::spawn(async move {
            let mut emitted = Emitted::default();
            loop {
                let keys = changes.changed().await;
                let Some(runtime) = weak.upgrade() else {
                    return;
                };
                runtime.emit_sync_status_delta(keys, &mut emitted).await;
                drop(runtime);
                tokio::time::sleep(SYNC_STATUS_COALESCE).await;
            }
        }));
    }
}
