//! 通信状態の変わった部分の印(#1221 R2-D)。
//!
//! transport(neighbor・受信・購読)、app-api(同期の時刻・docs の活動・gossip の停止)、desktop-runtime
//! (CN の session)が、変わった所に印を付ける。読み手(desktop-runtime の observer)は印の付いた部分だけを
//! 作り直して差分を送る。読み手が一度も待っていない間は印を持たない(CLI など、差分を送らない所で溜めない)。
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use tokio::sync::Notify;

/// 溜める印の上限。超えたら個別の印をやめ、全体を作り直す印(`All`)にする。
const MAX_PENDING_KEYS: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum StatusKey {
    /// 印が上限を超えた。稼働中の topic と CN の node を全て作り直す。
    All,
    /// 件数・同期の時刻など、topic に属さない値。
    Summary,
    /// transport の topic 名(`hint/...` を含む)。
    Topic(String),
    /// CN の node(base URL)。
    CommunityNode(String),
}

#[derive(Default)]
struct Pending {
    watched: bool,
    keys: BTreeSet<StatusKey>,
}

#[derive(Clone, Default)]
pub struct StatusChanges(Arc<(Mutex<Pending>, Notify)>);

impl StatusChanges {
    pub fn mark(&self, key: StatusKey) {
        let mut pending = self.0.0.lock().unwrap_or_else(|error| error.into_inner());
        if !pending.watched {
            return;
        }
        if pending.keys.len() >= MAX_PENDING_KEYS {
            pending.keys.insert(StatusKey::All);
        } else {
            pending.keys.insert(key);
        }
        drop(pending);
        self.0.1.notify_one();
    }

    /// 次の印まで待ち、溜まった印を取り出す。最初の呼出しから印を受け付ける。
    pub async fn changed(&self) -> BTreeSet<StatusKey> {
        loop {
            let notified = self.0.1.notified();
            {
                let mut pending = self.0.0.lock().unwrap_or_else(|error| error.into_inner());
                pending.watched = true;
                if !pending.keys.is_empty() {
                    return std::mem::take(&mut pending.keys);
                }
            }
            notified.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn marks_are_kept_only_while_watched_and_collapse_past_the_limit() {
        let changes = StatusChanges::default();
        changes.mark(StatusKey::Summary);
        let waiter = n0_future::task::spawn({
            let changes = changes.clone();
            async move { changes.changed().await }
        });
        tokio::task::yield_now().await;
        while !changes.0.0.lock().unwrap().watched {
            tokio::task::yield_now().await;
        }
        changes.mark(StatusKey::Topic("a".into()));
        changes.mark(StatusKey::Topic("a".into()));
        assert_eq!(
            waiter.await.unwrap(),
            BTreeSet::from([StatusKey::Topic("a".into())]),
            "a mark before anyone watched is not kept, a repeated mark is one key"
        );
        for index in 0..MAX_PENDING_KEYS + 10 {
            changes.mark(StatusKey::Topic(index.to_string()));
        }
        let keys = changes.changed().await;
        assert_eq!(keys.len(), MAX_PENDING_KEYS + 1);
        assert!(keys.contains(&StatusKey::All));
    }
}
