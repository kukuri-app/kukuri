//! 購読タスクの「窓の追いつき」(#1239、ADR 0052 §2)。
//!
//! replica は走査しない。時系列の索引の新しい側の固定件数(窓)と、session の固定件数だけを読み、projection に
//! 無いものを key 指定で反映する。読む量は replica の総 entry 数に依存しない。窓より古い範囲は、利用者が
//! 遡ったときのページの範囲の照合(`replica_window.rs`)が反映する。

use super::*;
use kukuri_docs_sync::{DocKeyOrder, DocKeyQuery};

/// 窓の大きさ(時系列の索引の新しい側から読む entry 数)。
pub(crate) const REPLICA_WINDOW_ENTRIES: usize = 200;
/// 追いつき 1 回で反映する session の数の上限(live、score game、Dome の room のそれぞれ)。
pub(crate) const SESSION_WINDOW_ENTRIES: usize = 32;

/// session の固定件数を、key だけの上限つきの一覧から反映する。
///
/// live session と score game の id は `live-<ms>-…`・`game-<ms>-…` なので、その prefix の key の降順がほぼ
/// 新しい順になる。Dome の room(`dome-<hash>`)や、それ以外の形の id は時刻順に並ばないので、prefix 全体の
/// 昇順と降順の固定件数も読む(id の形は検証の対象ではない)。どれも上限つきで、session の総数に依存しない。
pub(crate) async fn catch_up_sessions(
    services: &ServiceHandles,
    topic_id: &str,
    replica: &ReplicaId,
    _policy: DocFetchPolicy,
) -> Result<()> {
    let docs_sync = services.docs_sync.as_ref();
    let mut live_keys = BTreeSet::new();
    for (prefix, order) in [
        ("sessions/live/live-", DocKeyOrder::Descending),
        ("sessions/live/", DocKeyOrder::Ascending),
        ("sessions/live/", DocKeyOrder::Descending),
    ] {
        live_keys.extend(session_state_keys(docs_sync, replica, prefix, order).await?);
    }
    for key in live_keys {
        super::hydration_support::hydrate_session_key(services, topic_id, replica, &key).await?;
    }
    let mut game_keys = BTreeSet::new();
    for (prefix, order) in [
        ("sessions/game/game-", DocKeyOrder::Descending),
        ("sessions/game/", DocKeyOrder::Ascending),
        ("sessions/game/", DocKeyOrder::Descending),
    ] {
        game_keys.extend(session_state_keys(docs_sync, replica, prefix, order).await?);
    }
    for key in game_keys {
        super::hydration_support::hydrate_session_key(services, topic_id, replica, &key).await?;
    }
    Ok(())
}

/// `prefix` の下の `…/state` の key を、最大 `SESSION_WINDOW_ENTRIES` 件返す。
async fn session_state_keys(
    docs_sync: &dyn DocsSync,
    replica: &ReplicaId,
    prefix: &str,
    order: DocKeyOrder,
) -> Result<Vec<String>> {
    // session 1 件につき、`state` のほかの key がありうるので、余裕を持って読む。
    let page = docs_sync
        .query_replica_keys(
            replica,
            DocKeyQuery {
                prefix: prefix.to_string(),
                order,
                limit: SESSION_WINDOW_ENTRIES.saturating_mul(4),
            },
        )
        .await?;
    let mut keys = page
        .entries
        .into_iter()
        .map(|entry| entry.key)
        .filter(|key| key.ends_with("/state"))
        .collect::<Vec<_>>();
    keys.dedup();
    keys.truncate(SESSION_WINDOW_ENTRIES);
    Ok(keys)
}

impl AppService {
    /// live session・game room の一覧のために、scope の各 replica の session の固定件数を反映する(#1239)。
    ///
    /// replica は走査しない。参加状態の確認(`scope_replicas`)を通った replica だけを、手元の docs から読む。
    /// 同じ replica は間隔を空ける(一覧は数秒ごとに取得されうる。新しい session は docs の event が反映する)。
    pub(crate) async fn catch_up_scope_sessions(
        &self,
        topic_id: &str,
        scope: &TimelineScope,
    ) -> Result<()> {
        for replica in self.scope_replicas(topic_id, scope).await? {
            let key = format!(
                "{}
sessions",
                replica.as_str()
            );
            let now_ms = Utc::now().timestamp_millis();
            if self
                .services
                .range_checks
                .try_begin(key.as_str(), now_ms)
                .await
                .is_none()
            {
                continue;
            }
            catch_up_sessions(
                &self.services,
                topic_id,
                &replica,
                DocFetchPolicy::LocalOnly,
            )
            .await?;
        }
        Ok(())
    }
}
