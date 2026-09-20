//! 時系列の索引と projection を照合し、欠けている object を key 指定で反映する(#1239、ADR 0052 §2)。
//!
//! replica は走査しない。1 回の照合が読む索引の entry 数は呼び出し側の `limit`(上限つき)で決まり、
//! replica の総 entry 数に依存しない。読み出しは `LocalOnly` で、表示を remote 取得で待たせない。

use super::*;
use kukuri_docs_sync::{
    DocKeyOrder, DocKeyQuery, TimeIndexCursor, TimeIndexEntry, query_time_index_desc,
    query_time_index_window,
};

/// 1 回の照合で読む時系列の索引の entry 数の上限。呼び出し側の `limit` をこの値で抑える。
pub(crate) const RANGE_CHECK_ENTRY_LIMIT: usize = 200;
/// 1 回の照合で読む thread の索引の entry 数の上限。これを超える thread は、古い側のこの件数だけを照合する
/// (新しい側は docs の event と窓の追いつきで反映される)。
pub(crate) const THREAD_CHECK_ENTRY_LIMIT: usize = 512;
/// `created_at` は投稿者の申告値。現在時刻よりこの秒数を超えて未来の entry は、新しい側の読み出しで読み飛ばす。
pub(crate) const TIME_INDEX_FUTURE_ALLOWANCE_SECS: i64 = 600;
/// 欠けが無かった範囲を、次に照合するまでの間隔。
pub(crate) const RANGE_CHECK_INTERVAL_MS: i64 = 30_000;
/// 反映できない entry(本体が手元に無い)が残った範囲を、次に照合するまでの間隔。
pub(crate) const RANGE_CHECK_RETRY_INTERVAL_MS: i64 = 5_000;
/// 照合の台帳の上限。超えたら期限の切れた項目を捨て、それでも超えるなら記録せずに照合する。
pub(crate) const RANGE_CHECK_LEDGER_LIMIT: usize = 4_096;

/// 同じ範囲の照合を、表示の更新のたびに繰り返さないための台帳。
///
/// 照合は上限つきだが、先頭ページは数秒ごとに取得されうる。範囲(replica・索引・起点・件数)ごとに
/// 次の照合の時刻を持ち、それまでは projection だけで応える。
#[derive(Default)]
pub(crate) struct RangeCheckLedger {
    next_check_at_ms: Mutex<HashMap<String, i64>>,
}

impl RangeCheckLedger {
    /// この範囲を今照合してよいか。`true` を返したときは、再試行の間隔ぶん次の照合を先へ進める。
    pub(crate) async fn try_begin(&self, range_key: &str, now_ms: i64) -> bool {
        let mut entries = self.next_check_at_ms.lock().await;
        if entries
            .get(range_key)
            .is_some_and(|next_check_at| *next_check_at > now_ms)
        {
            return false;
        }
        if !entries.contains_key(range_key) && entries.len() >= RANGE_CHECK_LEDGER_LIMIT {
            entries.retain(|_, next_check_at| *next_check_at > now_ms);
            if entries.len() >= RANGE_CHECK_LEDGER_LIMIT {
                // 記録できない範囲は、間隔を空けずに照合する(照合そのものは上限つき)。
                return true;
            }
        }
        entries.insert(
            range_key.to_string(),
            now_ms.saturating_add(RANGE_CHECK_RETRY_INTERVAL_MS),
        );
        true
    }

    /// 欠けの無かった範囲の、次の照合を先へ延ばす。
    pub(crate) async fn record_complete(&self, range_key: &str, now_ms: i64) {
        if let Some(next_check_at) = self.next_check_at_ms.lock().await.get_mut(range_key) {
            *next_check_at = now_ms.saturating_add(RANGE_CHECK_INTERVAL_MS);
        }
    }

    #[cfg(test)]
    pub(crate) async fn len(&self) -> usize {
        self.next_check_at_ms.lock().await.len()
    }
}

/// 照合の結果。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RangeCheckOutcome {
    /// 今回 projection へ反映した件数(投稿と取り下げ)。
    pub(crate) hydrated: usize,
    /// 索引にあるが反映できなかった件数(entry の本体が手元に無い、など)。
    pub(crate) unresolved: usize,
}

impl RangeCheckOutcome {
    fn merge(&mut self, other: Self) {
        self.hydrated += other.hydrated;
        self.unresolved += other.unresolved;
    }
}

/// 索引の entry のうち、projection に無い object を key 指定で反映する。
///
/// projection に既にある object は、取り下げが未反映のときだけ `withdrawals/<object id>/state` を key 指定で
/// 確認する(取り下げの event を取りこぼした行の本文を出し続けない)。
pub(crate) async fn ensure_index_entries_projected(
    services: &ServiceHandles,
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
            if hydrate_object_by_id(services, replica, &object_id, policy).await? {
                outcome.hydrated += 1;
            } else {
                outcome.unresolved += 1;
            }
            continue;
        }
        if projection_store
            .get_post_withdrawal(&object_id)
            .await?
            .is_some()
        {
            continue;
        }
        let withdrawal_key = stable_key("withdrawals", &format!("{}/state", object_id.as_str()));
        if let Some(record) = query_replica_with_fetch_policy(
            docs_sync,
            replica,
            DocQuery::Exact(withdrawal_key),
            policy,
        )
        .await?
        .into_iter()
        .next()
            && hydrate_post_withdrawal_from_record(
                docs_sync,
                projection_store,
                replica,
                record,
                policy,
            )
            .await?
            .applied()
        {
            outcome.hydrated += 1;
        }
    }
    Ok(outcome)
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
    pub(crate) async fn reconcile_timeline_range(
        &self,
        topic_id: &str,
        scope: &TimelineScope,
        before: Option<&TimelineCursor>,
        limit: usize,
    ) -> Result<usize> {
        let limit = limit.min(RANGE_CHECK_ENTRY_LIMIT);
        if limit == 0 {
            return Ok(0);
        }
        const INDEX_PREFIX: &str = "indexes/timeline/";
        let before = before.map(time_index_cursor);
        let mut total = RangeCheckOutcome::default();
        for replica in self.scope_replicas(topic_id, scope).await? {
            let range_key = format!(
                "{}\n{INDEX_PREFIX}\n{}\n{limit}",
                replica.as_str(),
                before
                    .as_ref()
                    .map(|cursor| format!("{:020}-{}", cursor.created_at, cursor.object_id))
                    .unwrap_or_default(),
            );
            let now = Utc::now();
            if !self
                .services
                .range_checks
                .try_begin(range_key.as_str(), now.timestamp_millis())
                .await
            {
                continue;
            }
            let docs_sync = self.services.docs_sync.as_ref();
            let entries = match before.as_ref() {
                Some(cursor) => {
                    query_time_index_desc(docs_sync, &replica, INDEX_PREFIX, Some(cursor), limit)
                        .await?
                }
                None => {
                    query_time_index_window(
                        docs_sync,
                        &replica,
                        INDEX_PREFIX,
                        now.timestamp()
                            .saturating_add(TIME_INDEX_FUTURE_ALLOWANCE_SECS),
                        limit,
                    )
                    .await?
                }
            };
            let outcome = ensure_index_entries_projected(
                &self.services,
                &replica,
                &entries,
                DocFetchPolicy::LocalOnly,
            )
            .await?;
            if outcome.unresolved == 0 {
                self.services
                    .range_checks
                    .record_complete(range_key.as_str(), now.timestamp_millis())
                    .await;
            }
            total.merge(outcome);
        }
        Ok(total.hydrated)
    }

    /// thread の索引(`indexes/thread/<root>/`)と projection を照合して、欠けている object を反映する。
    ///
    /// root が projection にあれば、その channel の replica だけを読む。無ければ、参加中の scope の replica を読む。
    pub(crate) async fn reconcile_thread(
        &self,
        topic_id: &str,
        thread_root: &EnvelopeId,
    ) -> Result<usize> {
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
            return Ok(0);
        };
        let mut total = RangeCheckOutcome::default();
        for replica in replicas {
            let range_key = format!("{}\n{index_prefix}", replica.as_str());
            let now_ms = Utc::now().timestamp_millis();
            if !self
                .services
                .range_checks
                .try_begin(range_key.as_str(), now_ms)
                .await
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
                .into_iter()
                .filter_map(|entry| thread_index_entry(index_prefix.as_str(), entry.key))
                .collect::<Vec<_>>();
            let outcome = ensure_index_entries_projected(
                &self.services,
                &replica,
                &entries,
                DocFetchPolicy::LocalOnly,
            )
            .await?;
            if outcome.unresolved == 0 {
                self.services
                    .range_checks
                    .record_complete(range_key.as_str(), now_ms)
                    .await;
            }
            total.merge(outcome);
        }
        Ok(total.hydrated)
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
        assert!(ledger.try_begin("range-a", 0).await);
        assert!(
            !ledger
                .try_begin("range-a", RANGE_CHECK_RETRY_INTERVAL_MS - 1)
                .await,
            "an incomplete range waits for the retry interval"
        );
        assert!(
            ledger.try_begin("range-b", 0).await,
            "ranges are independent"
        );
        assert!(
            ledger
                .try_begin("range-a", RANGE_CHECK_RETRY_INTERVAL_MS)
                .await
        );
        ledger
            .record_complete("range-a", RANGE_CHECK_RETRY_INTERVAL_MS)
            .await;
        assert!(
            !ledger
                .try_begin(
                    "range-a",
                    RANGE_CHECK_RETRY_INTERVAL_MS + RANGE_CHECK_INTERVAL_MS - 1
                )
                .await,
            "a complete range waits for the full interval"
        );

        let ledger = RangeCheckLedger::default();
        for index in 0..RANGE_CHECK_LEDGER_LIMIT {
            assert!(ledger.try_begin(&format!("range-{index}"), 0).await);
        }
        assert!(
            ledger.try_begin("one-too-many", 0).await,
            "a full ledger still allows the bounded check"
        );
        assert_eq!(
            ledger.len().await,
            RANGE_CHECK_LEDGER_LIMIT,
            "but it does not grow"
        );
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
