//! 時系列の索引と projection を照合し、欠けている object を key 指定で反映する(#1239、ADR 0052 §2)。
//!
//! replica は走査しない。1 回の照合が読む索引の entry 数には上限があり、replica の総 entry 数に依存しない。
//! 読み出しは `LocalOnly` で、表示を remote 取得で待たせない。

use super::*;
use kukuri_docs_sync::{
    DocKeyOrder, DocKeyQuery, TimeIndexCursor, TimeIndexEntry, query_time_index_desc,
    query_time_index_window,
};

/// 1 回の照合で、replica 1 つから読む時系列の索引の entry 数の上限。呼び出し側の `limit` もこの値で抑える。
/// 反映できない entry が続く範囲は、この件数まで読み進めて止まり、続きは次の照合が読む。
pub(crate) const RANGE_CHECK_ENTRY_LIMIT: usize = 200;
/// 1 回の照合で読む thread の索引の entry 数の上限。これを超える thread は、古い側のこの件数だけを照合する
/// (新しい側は docs の event と窓の追いつきで反映される)。
pub(crate) const THREAD_CHECK_ENTRY_LIMIT: usize = 512;
/// `created_at` は投稿者の申告値。現在時刻よりこの秒数を超えて未来の entry は、新しい側の読み出しで読み飛ばす。
pub(crate) const TIME_INDEX_FUTURE_ALLOWANCE_SECS: i64 = 600;
/// 欠けが無かった範囲を、次に照合するまでの間隔。
pub(crate) const RANGE_CHECK_INTERVAL_MS: i64 = 30_000;
/// まだ反映できない entry が残った範囲と、読み進めている途中の範囲を、次に照合するまでの間隔。
pub(crate) const RANGE_CHECK_RETRY_INTERVAL_MS: i64 = 5_000;
/// 反映できない entry が残ったまま進展の無い範囲の、照合の間隔の上限。進展が無い間は、間隔を 2 倍ずつ伸ばす。
pub(crate) const RANGE_CHECK_STALLED_MAX_INTERVAL_MS: i64 = 300_000;
/// 照合の台帳の上限。超えたら期限の切れた項目を捨て、それでも超えるなら記録せずに照合する。
pub(crate) const RANGE_CHECK_LEDGER_LIMIT: usize = 4_096;

#[derive(Clone, Debug, Default)]
struct RangeCheckState {
    next_check_at_ms: i64,
    /// 反映できない entry が続いて、上限まで読んでも 1 ページぶんに届かなかった範囲の、続きの起点。
    resume_before: Option<TimeIndexCursor>,
    /// 反映できない entry が残ったまま、何も反映できなかった照合が続いた回数。
    stalled_checks: u32,
}

/// 確かめ終えた範囲の状態。次の照合までの間隔を決める。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RangeCheckResult {
    /// 欠けが無かった。
    Complete,
    /// まだ反映できない entry が残ったが、今回なにかを反映できた。
    Progressed,
    /// まだ反映できない entry が残り、今回は何も反映できなかった。
    Stalled,
}

/// 同じ範囲の照合を、表示の更新のたびに繰り返さないための台帳。
///
/// 照合は上限つきだが、先頭ページは数秒ごとに取得されうる。範囲(replica・索引・起点・件数)ごとに
/// 次の照合の時刻を持ち、それまでは projection だけで応える。
#[derive(Default)]
pub(crate) struct RangeCheckLedger {
    entries: Mutex<HashMap<String, RangeCheckState>>,
}

impl RangeCheckLedger {
    /// この範囲を今照合してよいか。照合してよいときは、再試行の間隔ぶん次の照合を先へ進め、
    /// 前回の照合が読み進めた位置(あれば)を返す。
    pub(crate) async fn try_begin(
        &self,
        range_key: &str,
        now_ms: i64,
    ) -> Option<Option<TimeIndexCursor>> {
        let mut entries = self.entries.lock().await;
        if let Some(state) = entries.get_mut(range_key) {
            if state.next_check_at_ms > now_ms {
                return None;
            }
            state.next_check_at_ms = now_ms.saturating_add(RANGE_CHECK_RETRY_INTERVAL_MS);
            return Some(state.resume_before.clone());
        }
        if entries.len() >= RANGE_CHECK_LEDGER_LIMIT {
            entries.retain(|_, state| state.next_check_at_ms > now_ms);
            if entries.len() >= RANGE_CHECK_LEDGER_LIMIT {
                // 記録できない範囲は、間隔を空けずに照合する(照合そのものは上限つき)。
                return Some(None);
            }
        }
        entries.insert(
            range_key.to_string(),
            RangeCheckState {
                next_check_at_ms: now_ms.saturating_add(RANGE_CHECK_RETRY_INTERVAL_MS),
                resume_before: None,
                stalled_checks: 0,
            },
        );
        Some(None)
    }

    /// 1 ページぶんを確かめ終えた範囲の、次の照合までの間隔を決める。
    ///
    /// 欠けが無ければ 30 秒。反映できない entry が残っていて、今回なにかを反映できたなら 5 秒。
    /// 何も反映できない照合が続く間は、5 秒から 2 倍ずつ伸ばす(上限 5 分)。同じ仕事を繰り返さないため。
    pub(crate) async fn record_finished(
        &self,
        range_key: &str,
        now_ms: i64,
        result: RangeCheckResult,
    ) {
        if let Some(state) = self.entries.lock().await.get_mut(range_key) {
            state.resume_before = None;
            let interval = match result {
                RangeCheckResult::Complete => {
                    state.stalled_checks = 0;
                    RANGE_CHECK_INTERVAL_MS
                }
                RangeCheckResult::Progressed => {
                    state.stalled_checks = 0;
                    RANGE_CHECK_RETRY_INTERVAL_MS
                }
                RangeCheckResult::Stalled => {
                    let interval = RANGE_CHECK_RETRY_INTERVAL_MS
                        .saturating_mul(1_i64 << state.stalled_checks.min(16))
                        .min(RANGE_CHECK_STALLED_MAX_INTERVAL_MS);
                    state.stalled_checks = state.stalled_checks.saturating_add(1);
                    interval
                }
            };
            state.next_check_at_ms = now_ms.saturating_add(interval);
        }
    }

    /// 上限まで読んでも 1 ページぶんに届かなかった範囲。次の照合は、読み進めた位置から続ける。
    pub(crate) async fn record_resume(&self, range_key: &str, resume_before: TimeIndexCursor) {
        if let Some(state) = self.entries.lock().await.get_mut(range_key) {
            state.resume_before = Some(resume_before);
        }
    }

    /// 間隔を空けずに次も照合する範囲を、台帳から外す。
    pub(crate) async fn forget(&self, range_key: &str) {
        self.entries.lock().await.remove(range_key);
    }

    #[cfg(test)]
    pub(crate) async fn len(&self) -> usize {
        self.entries.lock().await.len()
    }

    /// test 用: 台帳にある範囲のうち、次の照合が最も遅い時刻。
    #[cfg(test)]
    pub(crate) async fn latest_next_check_at_ms_for_test(&self) -> Option<i64> {
        self.entries
            .lock()
            .await
            .values()
            .map(|state| state.next_check_at_ms)
            .max()
    }

    /// test 用: 間隔が過ぎた状態にする(読み進めた位置は保つ)。
    #[cfg(test)]
    pub(crate) async fn expire_all_for_test(&self) {
        for state in self.entries.lock().await.values_mut() {
            state.next_check_at_ms = 0;
        }
    }
}

/// 照合の結果。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RangeCheckOutcome {
    /// 今回 projection へ反映した件数(投稿と取り下げ)。
    pub(crate) hydrated: usize,
    /// projection に既にあった object の件数。
    pub(crate) present: usize,
    /// 索引にあるが、まだ反映できない件数(entry の本体や、取り下げの対象の envelope が手元に無い)。
    /// 後の照合で拾えるので、この範囲は短い間隔で照合し直す。
    pub(crate) unresolved: usize,
    /// 索引にあるが、投稿として読めない件数。照合し直しても変わらないので、反映の対象から外す。
    pub(crate) invalid: usize,
}

impl RangeCheckOutcome {
    fn result(&self) -> RangeCheckResult {
        if self.unresolved == 0 {
            RangeCheckResult::Complete
        } else if self.hydrated > 0 {
            RangeCheckResult::Progressed
        } else {
            RangeCheckResult::Stalled
        }
    }

    fn merge(&mut self, other: Self) {
        self.hydrated += other.hydrated;
        self.present += other.present;
        self.unresolved += other.unresolved;
        self.invalid += other.invalid;
    }
}

/// 索引の entry のうち、projection に無い object を key 指定で反映する。
///
/// projection に既にある object は、取り下げが未反映のときだけ `withdrawals/<object id>/state` を key 指定で
/// 確認する(取り下げの event を取りこぼした行の本文を出し続けない)。
///
/// 1 件の entry が読めなくても、残りの entry の反映を続ける。索引と `objects/` は、その replica に書ける誰もが
/// 置けるので、読めない record を 1 件置くだけでページの取得を失敗させられないようにする(ADR 0052 §2)。
/// docs と projection の読み書きの失敗は、エラーとして返す。
pub(crate) async fn ensure_index_entries_projected(
    services: &ServiceHandles,
    topic_id: &str,
    replica: &ReplicaId,
    entries: &[TimeIndexEntry],
    policy: DocFetchPolicy,
) -> Result<RangeCheckOutcome> {
    let docs_sync = services.docs_sync.as_ref();
    let projection_store = services.projection_store.as_ref();
    let mut outcome = RangeCheckOutcome::default();
    for entry in entries {
        let object_id = EnvelopeId::from(entry.object_id.as_str());
        if projection_store
            .get_object_projection(&object_id)
            .await?
            .is_none()
        {
            // 1 回に多数の object を反映するので、本文は手元にあるものだけを読む(表示を remote 取得で待たせない)。
            match hydrate_object_in_topic_with(
                services,
                topic_id,
                replica,
                &object_id,
                policy,
                BodyFetch::LocalOnly,
            )
            .await?
            {
                ObjectHydration::Hydrated => outcome.hydrated += 1,
                ObjectHydration::Missing => outcome.unresolved += 1,
                ObjectHydration::Invalid => outcome.invalid += 1,
            }
            continue;
        }
        outcome.present += 1;
        if projection_store
            .get_post_withdrawal(&object_id)
            .await?
            .is_some()
        {
            continue;
        }
        // 同じ key には docs author ごとの record がありうる。先頭の 1 件だけを見ると、検証に通らない record を
        // 先に置くだけで、著者の正しい取り下げを無効にできてしまうので、上限つきで複数を調べる helper を通す(#1250)。
        match hydrate_post_withdrawal_for_object(
            docs_sync,
            projection_store,
            replica,
            &object_id,
            policy,
        )
        .await?
        {
            Some(PostWithdrawalHydration::Applied) => outcome.hydrated += 1,
            Some(PostWithdrawalHydration::TargetMissing) => outcome.unresolved += 1,
            Some(PostWithdrawalHydration::Invalid) | None => {}
        }
    }
    Ok(outcome)
}

/// 取得側へ返す照合の結果。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RangeReconcile {
    /// 今回 projection へ反映した件数。
    pub(crate) hydrated: usize,
    /// 照合した索引の範囲のうち、projection に在る object の件数(今回反映したものと、既にあったもの)。
    /// 台帳の間隔の内で照合しなかった replica のぶんは含まない。
    pub(crate) projected: usize,
}

impl RangeReconcile {
    /// 取得側が最初に読んだページ(`page_rows` 行)を読み直すべきか。
    ///
    /// 読み直すのは、照合した範囲に、最初のページの行数より多くの object が projection に在ると分かったときだけ。
    /// 今回反映したときのほか、最初にページを読んでから照合するまでのあいだに、購読タスクが同じ範囲を
    /// 反映していたときも当てはまる(照合は「既にある」と数えるだけになる)。欠けの無い定常状態では読み直さない。
    /// ページの読み出しを、必要の無いときに重ねないため。
    pub(crate) fn page_is_stale(&self, page_rows: usize) -> bool {
        self.hydrated > 0 || self.projected > page_rows
    }
}

fn time_index_cursor(cursor: &TimelineCursor) -> TimeIndexCursor {
    TimeIndexCursor {
        created_at: cursor.created_at,
        object_id: cursor.object_id.as_str().to_string(),
    }
}

impl AppService {
    /// タイムラインの 1 ページぶんの範囲(`before` より古い側、`before` が無ければ新しい側)を、scope の各
    /// replica の時系列の索引と照合して、欠けている object を反映する。戻り値は反映した件数。
    ///
    /// private channel の scope は、参加状態の確認(`scope_replicas`)を通った epoch の replica だけを読む。
    #[cfg(test)]
    pub(crate) async fn reconcile_timeline_range(
        &self,
        topic_id: &str,
        scope: &TimelineScope,
        before: Option<&TimelineCursor>,
        limit: usize,
    ) -> Result<usize> {
        Ok(self
            .reconcile_timeline_range_checked(topic_id, scope, before, limit)
            .await?
            .hydrated)
    }

    /// `reconcile_timeline_range` の本体。取得側は、照合したかどうかでページの読み直しを決める。
    pub(crate) async fn reconcile_timeline_range_checked(
        &self,
        topic_id: &str,
        scope: &TimelineScope,
        before: Option<&TimelineCursor>,
        limit: usize,
    ) -> Result<RangeReconcile> {
        let limit = limit.min(RANGE_CHECK_ENTRY_LIMIT);
        let mut reconcile = RangeReconcile::default();
        if limit == 0 {
            return Ok(reconcile);
        }
        let before = before.map(time_index_cursor);
        for replica in self.scope_replicas(topic_id, scope).await? {
            if let Some(outcome) = self
                .reconcile_replica_timeline_range(topic_id, &replica, before.as_ref(), limit)
                .await?
            {
                reconcile.hydrated += outcome.hydrated;
                reconcile.projected += outcome.hydrated + outcome.present;
            }
        }
        Ok(reconcile)
    }

    /// replica 1 つの時系列の索引を、`before` より古い側(無ければ新しい側)へ読み、projection に在る object が
    /// `limit` 件に届くまで反映する。反映できない entry(本体が未着、投稿として読めない)は読み飛ばして先へ進む。
    /// そうしないと、反映できない entry が `limit` 件続いただけで、その先の投稿へ遡れなくなる。
    ///
    /// 1 回に読む索引の entry 数は `RANGE_CHECK_ENTRY_LIMIT` まで。届かなかったときは、読み進めた位置を
    /// 台帳に残し、次の照合がそこから続ける。台帳の間隔の内で照合しなかったときは `None`。
    async fn reconcile_replica_timeline_range(
        &self,
        topic_id: &str,
        replica: &ReplicaId,
        before: Option<&TimeIndexCursor>,
        limit: usize,
    ) -> Result<Option<RangeCheckOutcome>> {
        const INDEX_PREFIX: &str = "indexes/timeline/";
        let range_key = format!(
            "{}\n{INDEX_PREFIX}\n{}\n{limit}",
            replica.as_str(),
            before
                .map(|cursor| format!("{:020}-{}", cursor.created_at, cursor.object_id))
                .unwrap_or_default(),
        );
        let now = Utc::now();
        let Some(resume_before) = self
            .services
            .range_checks
            .try_begin(range_key.as_str(), now.timestamp_millis())
            .await
        else {
            return Ok(None);
        };
        let docs_sync = self.services.docs_sync.as_ref();
        let mut position = resume_before.or_else(|| before.cloned());
        let mut total = RangeCheckOutcome::default();
        let mut entries_read = 0usize;
        let mut index_exhausted = false;
        // 1 回目は、ページに要る件数だけを読む(欠けが無ければこれで終わる)。反映できない entry があって
        // 届かなかったときは、読む件数を 4 倍ずつ増やす。同じ位置からの読み直しの回数を抑えるため。
        let mut batch = limit;
        loop {
            let resolved = total.hydrated + total.present;
            if resolved >= limit {
                break;
            }
            let wanted = batch
                .max(limit - resolved)
                .min(RANGE_CHECK_ENTRY_LIMIT - entries_read);
            if wanted == 0 {
                break;
            }
            batch = batch.saturating_mul(4);
            let entries = match position.as_ref() {
                Some(cursor) => {
                    query_time_index_desc(docs_sync, replica, INDEX_PREFIX, Some(cursor), wanted)
                        .await?
                }
                None => {
                    query_time_index_window(
                        docs_sync,
                        replica,
                        INDEX_PREFIX,
                        now.timestamp()
                            .saturating_add(TIME_INDEX_FUTURE_ALLOWANCE_SECS),
                        wanted,
                    )
                    .await?
                }
            };
            let Some(last) = entries.last() else {
                index_exhausted = true;
                break;
            };
            position = Some(TimeIndexCursor {
                created_at: last.created_at,
                object_id: last.object_id.clone(),
            });
            entries_read += entries.len();
            if entries.len() < wanted {
                index_exhausted = true;
            }
            total.merge(
                ensure_index_entries_projected(
                    &self.services,
                    topic_id,
                    replica,
                    &entries,
                    DocFetchPolicy::LocalOnly,
                )
                .await?,
            );
            if index_exhausted {
                break;
            }
        }

        let ledger = self.services.range_checks.as_ref();
        if entries_read == 0 && before.is_none() {
            // 索引がまだ空の replica(参加した直後など)は、間隔を空けずに次の取得でも読む。
            // 読むのは key だけの読み出し 1 回で、投稿が同期された直後のページをすぐに組み立てられる。
            ledger.forget(range_key.as_str()).await;
            return Ok(Some(total));
        }
        let page_resolved = total.hydrated + total.present >= limit;
        match position {
            Some(resume) if !page_resolved && !index_exhausted => {
                ledger.record_resume(range_key.as_str(), resume).await;
            }
            _ => {
                ledger
                    .record_finished(range_key.as_str(), now.timestamp_millis(), total.result())
                    .await;
            }
        }
        Ok(Some(total))
    }

    /// thread の索引(`indexes/thread/<root>/`)と projection を照合して、欠けている object を反映する。
    ///
    /// root が projection にあれば、その channel の replica だけを読む。無ければ、参加中の scope の replica を読む。
    #[cfg(test)]
    pub(crate) async fn reconcile_thread(
        &self,
        topic_id: &str,
        thread_root: &EnvelopeId,
    ) -> Result<usize> {
        Ok(self
            .reconcile_thread_checked(topic_id, thread_root)
            .await?
            .hydrated)
    }

    /// `reconcile_thread` の本体。取得側は、照合したかどうかでページの読み直しを決める。
    pub(crate) async fn reconcile_thread_checked(
        &self,
        topic_id: &str,
        thread_root: &EnvelopeId,
    ) -> Result<RangeReconcile> {
        let root_channel = self
            .services
            .projection_store
            .get_object_projection(thread_root)
            .await?
            .map(|row| row.channel_id);
        let scope = match root_channel.as_deref() {
            Some(PUBLIC_CHANNEL_ID) => TimelineScope::Public,
            Some(channel_id) => TimelineScope::Channel {
                channel_id: ChannelId::new(channel_id),
            },
            None => TimelineScope::AllJoined,
        };
        let index_prefix = stable_key("indexes/thread", &format!("{}/", thread_root.as_str()));
        // 参加していない private channel の thread は照合しない(replica を読まない)。表示は従来どおり
        // projection の行だけで組み立てる。
        let Ok(replicas) = self.scope_replicas(topic_id, &scope).await else {
            return Ok(RangeReconcile::default());
        };
        let mut reconcile = RangeReconcile::default();
        for replica in replicas {
            let range_key = format!("{}\n{index_prefix}", replica.as_str());
            let now_ms = Utc::now().timestamp_millis();
            if self
                .services
                .range_checks
                .try_begin(range_key.as_str(), now_ms)
                .await
                .is_none()
            {
                continue;
            }
            let keys = self
                .services
                .docs_sync
                .query_replica_keys(
                    &replica,
                    DocKeyQuery {
                        prefix: index_prefix.clone(),
                        order: DocKeyOrder::Ascending,
                        limit: THREAD_CHECK_ENTRY_LIMIT,
                    },
                )
                .await?;
            let entries = keys
                .entries
                .into_iter()
                .filter_map(|entry| thread_index_entry(index_prefix.as_str(), entry.key))
                .collect::<Vec<_>>();
            let outcome = ensure_index_entries_projected(
                &self.services,
                topic_id,
                &replica,
                &entries,
                DocFetchPolicy::LocalOnly,
            )
            .await?;
            self.services
                .range_checks
                .record_finished(range_key.as_str(), now_ms, outcome.result())
                .await;
            reconcile.hydrated += outcome.hydrated;
            reconcile.projected += outcome.hydrated + outcome.present;
        }
        Ok(reconcile)
    }
}

/// `indexes/thread/<root>/<sort key>/<object id>` から object id を取り出す。形の違う key は捨てる。
fn thread_index_entry(index_prefix: &str, key: String) -> Option<TimeIndexEntry> {
    let rest = key.strip_prefix(index_prefix)?;
    let (sort_key, object_id) = rest.rsplit_once('/')?;
    let (time, sort_object_id) = sort_key.split_once('-')?;
    if sort_object_id != object_id || object_id.is_empty() {
        return None;
    }
    let created_at = time.parse::<i64>().ok()?;
    Some(TimeIndexEntry {
        created_at,
        object_id: object_id.to_string(),
        key,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn range_checks_are_spaced_per_range_and_bounded() {
        let ledger = RangeCheckLedger::default();
        assert!(ledger.try_begin("range-a", 0).await.is_some());
        assert!(
            ledger
                .try_begin("range-a", RANGE_CHECK_RETRY_INTERVAL_MS - 1)
                .await
                .is_none(),
            "an unfinished range waits for the retry interval"
        );
        assert!(
            ledger.try_begin("range-b", 0).await.is_some(),
            "ranges are independent"
        );
        assert!(
            ledger
                .try_begin("range-a", RANGE_CHECK_RETRY_INTERVAL_MS)
                .await
                .is_some()
        );
        ledger
            .record_finished(
                "range-a",
                RANGE_CHECK_RETRY_INTERVAL_MS,
                RangeCheckResult::Complete,
            )
            .await;
        assert!(
            ledger
                .try_begin(
                    "range-a",
                    RANGE_CHECK_RETRY_INTERVAL_MS + RANGE_CHECK_INTERVAL_MS - 1
                )
                .await
                .is_none(),
            "a complete range waits for the full interval"
        );

        let ledger = RangeCheckLedger::default();
        for index in 0..RANGE_CHECK_LEDGER_LIMIT {
            assert!(
                ledger
                    .try_begin(&format!("range-{index}"), 0)
                    .await
                    .is_some()
            );
        }
        assert!(
            ledger.try_begin("one-too-many", 0).await.is_some(),
            "a full ledger still allows the bounded check"
        );
        assert_eq!(
            ledger.len().await,
            RANGE_CHECK_LEDGER_LIMIT,
            "but it does not grow"
        );
    }

    #[tokio::test]
    async fn a_stuck_range_resumes_from_where_the_last_check_stopped() {
        let ledger = RangeCheckLedger::default();
        assert_eq!(ledger.try_begin("range", 0).await, Some(None));
        let resume = TimeIndexCursor {
            created_at: 1_758_000_000,
            object_id: "a".repeat(64),
        };
        ledger.record_resume("range", resume.clone()).await;
        assert_eq!(
            ledger
                .try_begin("range", RANGE_CHECK_RETRY_INTERVAL_MS)
                .await,
            Some(Some(resume)),
            "the next check continues from the recorded position"
        );
        ledger
            .record_finished(
                "range",
                RANGE_CHECK_RETRY_INTERVAL_MS,
                RangeCheckResult::Progressed,
            )
            .await;
        assert_eq!(
            ledger
                .try_begin("range", RANGE_CHECK_RETRY_INTERVAL_MS * 2)
                .await,
            Some(None),
            "a finished range starts over from its original position"
        );
    }

    #[tokio::test]
    async fn a_stalled_range_is_checked_less_and_less_often() {
        let ledger = RangeCheckLedger::default();
        let mut now = 0_i64;
        let mut intervals = Vec::new();
        assert!(ledger.try_begin("range", now).await.is_some());
        for _ in 0..8 {
            ledger
                .record_finished("range", now, RangeCheckResult::Stalled)
                .await;
            // 次に照合できる最初の時刻を探す(見つかった時刻で、次の照合が始まる)。
            let mut next = now;
            while ledger.try_begin("range", next).await.is_none() {
                next += 1_000;
            }
            intervals.push(next - now);
            now = next;
        }
        assert_eq!(intervals[0], RANGE_CHECK_RETRY_INTERVAL_MS);
        assert!(
            intervals.windows(2).all(|pair| pair[1] >= pair[0]),
            "{intervals:?}"
        );
        assert_eq!(
            *intervals.last().expect("intervals"),
            RANGE_CHECK_STALLED_MAX_INTERVAL_MS,
            "{intervals:?}"
        );

        // 反映できたら、間隔は元へ戻る。
        ledger
            .record_finished("range", now, RangeCheckResult::Progressed)
            .await;
        assert!(
            ledger
                .try_begin("range", now + RANGE_CHECK_RETRY_INTERVAL_MS)
                .await
                .is_some()
        );
    }

    // 取得側は、照合で「最初のページより多くの object が projection に在る」と分かったときだけページを読み直す。
    // 欠けの無い定常状態では読み直さない(ページの読み出しを、必要の無いときに重ねない)。
    #[test]
    fn the_page_is_read_again_only_when_the_projection_holds_more_than_the_page_showed() {
        let steady = RangeReconcile {
            hydrated: 0,
            projected: 20,
        };
        assert!(!steady.page_is_stale(20), "nothing changed");
        assert!(
            !RangeReconcile::default().page_is_stale(0),
            "no check ran, or the index is empty"
        );
        let hydrated_now = RangeReconcile {
            hydrated: 3,
            projected: 20,
        };
        assert!(hydrated_now.page_is_stale(17));
        // 最初にページを読んでから照合するまでのあいだに、購読タスクが同じ範囲を反映していた。
        let hydrated_by_someone_else = RangeReconcile {
            hydrated: 0,
            projected: 20,
        };
        assert!(hydrated_by_someone_else.page_is_stale(0));
    }

    #[test]
    fn thread_index_keys_are_parsed_and_malformed_keys_are_dropped() {
        let prefix = "indexes/thread/root/";
        let id = "a".repeat(64);
        let entry = thread_index_entry(prefix, format!("{prefix}{:020}-{id}/{id}", 1_758_000_000))
            .expect("valid key");
        assert_eq!(entry.created_at, 1_758_000_000);
        assert_eq!(entry.object_id, id);
        assert!(thread_index_entry(prefix, format!("{prefix}junk")).is_none());
        assert!(
            thread_index_entry(prefix, format!("{prefix}{:020}-{id}/other", 1)).is_none(),
            "the sort key and the object id must agree"
        );
    }
}
