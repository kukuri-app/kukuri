//! 購読タスクの「窓の追いつき」(#1239、ADR 0052 §2)。
//!
//! replica は走査しない。時系列の索引の新しい側の固定件数(窓)と、session の固定件数だけを読み、projection に
//! 無いものを key 指定で反映する。読む量は replica の総 entry 数に依存しない。窓より古い範囲は、利用者が
//! 遡ったときのページの範囲の照合(`replica_window.rs`)が反映する。

use super::game_projection_support::hydrate_game_room_from_record;
use super::hydration_support::hydrate_live_session_from_record;
use super::replica_window::{
    RANGE_CHECK_REACTIONS_PER_OBJECT, TIME_INDEX_FUTURE_ALLOWANCE_SECS,
    ensure_index_entries_projected,
};
use super::*;
use kukuri_docs_sync::{DocKeyOrder, DocKeyQuery, query_time_index_window};

/// 窓の大きさ(時系列の索引の新しい側から読む entry 数)。
pub(crate) const REPLICA_WINDOW_ENTRIES: usize = 200;
/// 追いつき 1 回で反映する session の数の上限(live、score game、Dome の room のそれぞれ)。
pub(crate) const SESSION_WINDOW_ENTRIES: usize = 32;
/// 追いつきの最小間隔。event・hint・通知が続いても、これより短い間隔では追いつかない。
pub(crate) const CATCH_UP_MIN_INTERVAL_MS: i64 = 3_000;
/// 何も反映できない追いつきが続くあいだの、間隔の上限。
pub(crate) const CATCH_UP_MAX_INTERVAL_MS: i64 = 300_000;

/// 追いつきの契機をまとめ、間隔を空けて 1 回にする。
///
/// 契機(個別反映が 0 件だった event・hint、取りこぼし、同期の区切り)は短い時間に何度も来る。来るたびに
/// 追いつくと、上限つきの読み出しでも同じ仕事を繰り返す。依頼は覚えておき、期限が来たら 1 回だけ追いつく。
/// 何も反映できない追いつきが続くあいだは、間隔を 2 倍ずつ伸ばす。
#[derive(Debug, Default)]
pub(crate) struct CatchUpSchedule {
    requested: bool,
    /// reaction も読み直す依頼(取りこぼしの後)。
    refresh_reactions: bool,
    next_allowed_at_ms: i64,
    /// 前回の追いつきが終わった時刻。
    last_finished_at_ms: i64,
    empty_runs: u32,
}

/// 期限が来た追いつきの内容。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CatchUpRun {
    pub(crate) refresh_reactions: bool,
}

impl CatchUpSchedule {
    /// 反映するものがあるとは限らない契機(同期の終わり)。空振りで伸びた間隔はそのまま待つ。
    /// 静かな replica では、再 sync のたびに同期の終わりが届く。それだけで追いつきを繰り返さないため。
    pub(crate) fn request(&mut self) {
        self.requested = true;
    }

    /// 反映するものがあると分かっている契機(相手から届いた entry や hint の個別反映が 0 件、本体がそろった)。
    /// 空振りで伸びた間隔を待たず、最小間隔で追いつく。
    pub(crate) fn request_now(&mut self) {
        self.requested = true;
        self.shorten_to_the_minimum_interval();
    }

    /// 取りこぼしの後の依頼。窓の object の取り下げと reaction も読み直す。最小間隔で追いつく。
    pub(crate) fn request_after_lag(&mut self) {
        self.request_now();
        self.refresh_reactions = true;
    }

    /// 失敗した追いつきの依頼を戻す(読み直しの依頼を失わない)。
    pub(crate) fn restore(&mut self, run: CatchUpRun) {
        self.requested = true;
        self.refresh_reactions |= run.refresh_reactions;
    }

    fn shorten_to_the_minimum_interval(&mut self) {
        self.empty_runs = 0;
        self.next_allowed_at_ms = self.next_allowed_at_ms.min(
            self.last_finished_at_ms
                .saturating_add(CATCH_UP_MIN_INTERVAL_MS),
        );
    }

    /// 依頼があり、間隔が過ぎていれば、追いつきを 1 回ぶん取り出す。
    pub(crate) fn take_due(&mut self, now_ms: i64) -> Option<CatchUpRun> {
        if !self.requested || self.next_allowed_at_ms > now_ms {
            return None;
        }
        let run = CatchUpRun {
            refresh_reactions: self.refresh_reactions,
        };
        self.requested = false;
        self.refresh_reactions = false;
        Some(run)
    }

    /// 追いつきの結果から、次に追いつけるまでの間隔を決める。
    pub(crate) fn record_finished(&mut self, now_ms: i64, hydrated: usize) {
        let interval = if hydrated > 0 {
            self.empty_runs = 0;
            CATCH_UP_MIN_INTERVAL_MS
        } else {
            let interval = CATCH_UP_MIN_INTERVAL_MS
                .saturating_mul(1_i64 << self.empty_runs.min(16))
                .min(CATCH_UP_MAX_INTERVAL_MS);
            self.empty_runs = self.empty_runs.saturating_add(1);
            interval
        };
        self.last_finished_at_ms = now_ms;
        self.next_allowed_at_ms = now_ms.saturating_add(interval);
    }

    /// 個別反映で何かが入った。空振りで伸びた間隔を捨て、次の依頼は最小間隔で受ける。
    pub(crate) fn record_progress(&mut self) {
        self.shorten_to_the_minimum_interval();
    }
}

/// 相手から届いた、個別反映が 0 件だった entry が、追いつきの契機になるか。
///
/// 時系列と thread の索引の entry は、個別反映の対象ではないので必ず 0 件になる。投稿 1 件につき 2 件届くので、
/// そのたびに依頼すると、活発な topic では追いつきが最小間隔で走り続ける。索引が指す object が projection に
/// 既にあれば(投稿の本体の event が先に反映した)、追いつくものは無い。
pub(crate) async fn missed_entry_needs_catch_up(
    projection_store: &dyn ProjectionStore,
    key: &str,
) -> bool {
    let indexed_object = ["indexes/timeline/", "indexes/thread/"]
        .iter()
        .find_map(|prefix| key.strip_prefix(prefix))
        .and_then(|rest| rest.rsplit_once('/'))
        .map(|(_, object_id)| object_id)
        .filter(|object_id| !object_id.is_empty());
    match indexed_object {
        // projection を読めなかったときは、依頼する側へ倒す。
        Some(object_id) => projection_store
            .get_object_projection(&EnvelopeId::from(object_id))
            .await
            .map_or(true, |row| row.is_none()),
        None => true,
    }
}

/// 窓の追いつきを 1 回行う。戻り値は、projection へ新しく反映した投稿・取り下げの件数。
///
/// session は毎回反映し直すので(manifest の到着や参加状態で結果が変わる)、件数には数えない。
pub(crate) async fn catch_up_replica_window(
    services: &ServiceHandles,
    topic_id: &str,
    replica: &ReplicaId,
    policy: DocFetchPolicy,
    refresh_all: bool,
) -> Result<usize> {
    let docs_sync = services.docs_sync.as_ref();
    let window = query_time_index_window(
        docs_sync,
        replica,
        "indexes/timeline/",
        Utc::now()
            .timestamp()
            .saturating_add(TIME_INDEX_FUTURE_ALLOWANCE_SECS),
        REPLICA_WINDOW_ENTRIES,
    )
    .await?;
    // 取りこぼしの後と起動時は、projection に既にある object の取り下げも確かめ直す。それ以外の追いつきは、
    // projection に無い object だけを反映する(取り下げは docs の event が key 単位で反映する。追いつきのたびに
    // 窓の全 object の取り下げの key を読み直さない)。
    let mut targets = Vec::new();
    for entry in &window.entries {
        if refresh_all
            || services
                .projection_store
                .get_object_projection(&EnvelopeId::from(entry.object_id.as_str()))
                .await?
                .is_none()
        {
            targets.push(entry.clone());
        }
    }
    let outcome =
        ensure_index_entries_projected(services, topic_id, replica, &targets, policy).await?;
    if refresh_all {
        // 取りこぼした event には reaction も含まれる。窓の object の reaction を、上限つきで読み直す
        // (新しく反映した object の reaction は、上の反映が既に読んでいる)。
        for entry in &window.entries {
            hydrate_reaction_cache_for_target_bounded(
                docs_sync,
                services.projection_store.as_ref(),
                topic_id,
                replica,
                &EnvelopeId::from(entry.object_id.as_str()),
                policy,
                RANGE_CHECK_REACTIONS_PER_OBJECT,
            )
            .await?;
        }
    }
    catch_up_sessions(services, topic_id, replica, policy).await?;
    Ok(outcome.hydrated)
}

/// session の固定件数を、key だけの上限つきの一覧から反映する。
///
/// live session と score game の id は `live-<ms>-…`・`game-<ms>-…` なので、その prefix の key の降順がほぼ
/// 新しい順になる。Dome の room(`dome-<hash>`)や、それ以外の形の id は時刻順に並ばないので、prefix 全体の
/// 昇順と降順の固定件数も読む(id の形は検証の対象ではない)。どれも上限つきで、session の総数に依存しない。
pub(crate) async fn catch_up_sessions(
    services: &ServiceHandles,
    topic_id: &str,
    replica: &ReplicaId,
    policy: DocFetchPolicy,
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
        for record in docs_sync
            .query_replica_exact_bounded(
                replica,
                key.as_str(),
                MAX_ENVELOPE_RECORDS_PER_OBJECT,
                policy,
            )
            .await?
        {
            hydrate_live_session_from_record(
                docs_sync,
                services.blob_service.as_ref(),
                services.projection_store.as_ref(),
                topic_id,
                replica,
                record,
                policy,
            )
            .await?;
        }
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
        for record in docs_sync
            .query_replica_exact_bounded(
                replica,
                key.as_str(),
                MAX_ENVELOPE_RECORDS_PER_OBJECT,
                policy,
            )
            .await?
        {
            hydrate_game_room_from_record(services, topic_id, replica, record).await?;
        }
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

/// 1 つの object の `objects/<id>/` の下で、通知の起点に入れる key の数の上限(`state` と `envelope`)。
const BASELINE_KEYS_PER_OBJECT: usize = 8;

/// 通知の起点(購読を始めた時点で手元の docs に既にある投稿の entry)を、窓の object だけから作る。
///
/// 以前は `objects/` の全 entry を読んでいた(replica の投稿の総数に比例する)。起点は「購読を始める前から
/// 手元にあった entry の event を通知にしない」ためのもので、窓より古い entry の event が再び届いたときは
/// 通知の候補になる(通知の id は決定的なので、既にある通知は重複しない)。
pub(crate) async fn snapshot_window_notification_baseline(
    docs_sync: &dyn DocsSync,
    replica: &ReplicaId,
) -> Result<NotificationDocEventBaseline> {
    let window = query_time_index_window(
        docs_sync,
        replica,
        "indexes/timeline/",
        Utc::now()
            .timestamp()
            .saturating_add(TIME_INDEX_FUTURE_ALLOWANCE_SECS),
        REPLICA_WINDOW_ENTRIES,
    )
    .await?;
    let mut entries = Vec::new();
    for entry in &window.entries {
        let page = docs_sync
            .query_replica_keys(
                replica,
                DocKeyQuery {
                    prefix: stable_key("objects", &format!("{}/", entry.object_id)),
                    order: DocKeyOrder::Ascending,
                    limit: BASELINE_KEYS_PER_OBJECT,
                },
            )
            .await?;
        entries.extend(
            page.entries
                .into_iter()
                .filter(|key| object_id_from_post_key(key.key.as_str()).is_some()),
        );
    }
    Ok(NotificationDocEventBaseline::from_key_entries(&entries))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_are_coalesced_and_spaced() {
        let mut schedule = CatchUpSchedule::default();
        assert_eq!(schedule.take_due(0), None, "nothing was requested");
        schedule.request();
        schedule.request();
        assert_eq!(
            schedule.take_due(0),
            Some(CatchUpRun {
                refresh_reactions: false
            })
        );
        assert_eq!(schedule.take_due(0), None, "one run per request burst");
        schedule.record_finished(0, 1);

        schedule.request_after_lag();
        assert_eq!(
            schedule.take_due(CATCH_UP_MIN_INTERVAL_MS - 1),
            None,
            "the request waits for the interval"
        );
        assert_eq!(
            schedule.take_due(CATCH_UP_MIN_INTERVAL_MS),
            Some(CatchUpRun {
                refresh_reactions: true
            }),
            "and is not lost"
        );
    }

    #[test]
    fn empty_runs_back_off_up_to_the_cap_and_progress_resets_them() {
        let mut schedule = CatchUpSchedule::default();
        let mut now = 0_i64;
        let mut intervals = Vec::new();
        for _ in 0..10 {
            schedule.request();
            let mut next = now;
            while schedule.take_due(next).is_none() {
                next += 1_000;
            }
            schedule.record_finished(next, 0);
            intervals.push(next - now);
            now = next;
        }
        assert!(
            intervals.windows(2).all(|pair| pair[1] >= pair[0]),
            "{intervals:?}"
        );
        assert_eq!(
            *intervals.last().expect("intervals"),
            CATCH_UP_MAX_INTERVAL_MS
        );

        // 同期の終わりだけでは、伸びた間隔を待つ。
        schedule.request();
        assert!(schedule.take_due(now + CATCH_UP_MIN_INTERVAL_MS).is_none());
        // 個別反映で何かが入ったら、伸びた間隔を捨てる。
        schedule.record_progress();
        assert!(
            schedule.take_due(now + CATCH_UP_MIN_INTERVAL_MS).is_some(),
            "after progress, the interval starts over from the minimum"
        );
    }

    #[test]
    fn a_failed_run_keeps_its_refresh_request() {
        let mut schedule = CatchUpSchedule::default();
        schedule.request_after_lag();
        let run = schedule.take_due(0).expect("due");
        schedule.restore(run);
        schedule.record_finished(0, 0);
        assert_eq!(
            schedule.take_due(CATCH_UP_MIN_INTERVAL_MS),
            Some(CatchUpRun {
                refresh_reactions: true
            })
        );
    }

    // 静かな topic では、再 sync のたびに `SyncFinished` が届き、何も反映しない追いつきが続く(間隔は 5 分まで伸びる)。
    // その後に取りこぼし(`Lagged`)が通知されても、既に決まった次の期限は縮まらない。ADR 0052 §2 は
    // 「個別反映か追いつきで何かが入ったら、最小へ戻す」と定めている。
    #[test]
    fn a_lag_after_an_idle_period_is_caught_up_within_the_minimum_interval() {
        let mut schedule = CatchUpSchedule::default();
        let mut now = 0_i64;
        for _ in 0..10 {
            schedule.request();
            while schedule.take_due(now).is_none() {
                now += 1_000;
            }
            schedule.record_finished(now, 0);
        }
        // 新着が個別反映され、その直後に取りこぼしが通知された。
        schedule.record_progress();
        schedule.request_after_lag();
        assert!(
            schedule.take_due(now + CATCH_UP_MIN_INTERVAL_MS).is_some(),
            "a catch-up after a lag must not wait for the interval stretched by empty runs"
        );
    }
}
