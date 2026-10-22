//! 指定した公開 topic の内容を、画面の列なしに取り込んで保持する(#1909、ADR 0055 §1.1)。CLI daemon が使う。
//!
//! - 保持する topic ごとに topic の lease(holder `retain:<topic>`)を取る。lease は gossip への参加と、既存の lease の
//!   task による topic の hint の受信に使う。公開スレッドの lease は取らない。
//! - account ごとに 1 つの task が、1 回に 1 つの topic を選び、現在と前日の bucket の時刻順の索引
//!   (`indexes/timeline/`・`channels/list/`・`channels/activity/`)を、続きの位置から古い順に上限つきで読む。新しい
//!   対象は既存の反映の関数(署名・scope・bucket の日の検証つき)で取り込む。公開スレッドの投稿は
//!   `channels/activity/` から見つけるので、公開スレッドの数に比例する購読も task も作らない。

use std::sync::atomic::Ordering;
use std::time::Duration;

use crate::service::*;
use kukuri_docs_sync::{
    BucketReplica, BucketScope, TimeBucket, TimeIndexCursor, TimeIndexEntry, query_time_index_asc,
};
use kukuri_store::TopicRetentionCursor;

/// 1 回に取り込む対象の上限。
pub(crate) const RETENTION_STEP_ENTRIES: usize = 32;
/// 1 回の全体の期限。間に合わなかった対象は、続きの位置を進めずに次の回へ残す。
const RETENTION_STEP_DEADLINE: Duration = Duration::from_secs(60);
/// 続きが残っているときの、次の回までの間。
const RETENTION_BUSY_INTERVAL: Duration = Duration::from_secs(1);
/// 追いついたときの、次の回までの間。
const RETENTION_IDLE_INTERVAL: Duration = Duration::from_secs(30);
/// 同じ対象の取り込みがこの回数続けて失敗したら、その対象を飛ばして先へ進む。
const RETENTION_ENTRY_ATTEMPTS: u32 = 3;
/// topic のタイムラインの索引。
const TOPIC_TIMELINE_INDEX: &str = "indexes/timeline/";
/// 読む索引(topic の投稿、公開スレッドの作成、公開スレッドの投稿)。
const RETENTION_INDEXES: [&str; 3] = [
    TOPIC_TIMELINE_INDEX,
    PUBLIC_CHANNEL_LIST_PREFIX,
    PUBLIC_CHANNEL_ACTIVITY_PREFIX,
];

/// 保持の設定と進み具合。
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TopicRetentionView {
    pub topics: Vec<RetainedTopicView>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RetainedTopicView {
    pub topic: String,
    /// 保持を始めた時刻(ミリ秒)。
    pub retained_at: i64,
    /// 現在と前日の bucket の、索引ごとに読み終えた最後の entry。
    pub progress: Vec<RetentionProgressView>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RetentionProgressView {
    pub index: String,
    pub bucket: u64,
    /// entry の時刻(秒)。
    pub created_at: i64,
    pub object_id: String,
}

impl From<TopicRetentionCursor> for RetentionProgressView {
    fn from(cursor: TopicRetentionCursor) -> Self {
        Self {
            index: cursor.index_prefix,
            bucket: cursor.bucket,
            created_at: cursor.created_at,
            object_id: cursor.object_id,
        }
    }
}

/// 取り込みの位置((topic, 索引, bucket))。
type RetentionSlot = (String, &'static str, u64);

/// 続けて失敗している対象(位置ごとに、対象の（時刻, ID）と回数)。保持する topic の数 × 索引 × 2 bucket まで。
#[derive(Default)]
pub(crate) struct RetentionFailures(HashMap<RetentionSlot, ((i64, String), u32)>);

impl RetentionFailures {
    /// 失敗を数え、飛ばしてよい(回数に達した)かを返す。
    fn note(
        &mut self,
        topic: &str,
        prefix: &'static str,
        bucket: u64,
        entry: &TimeIndexEntry,
    ) -> bool {
        let at = (entry.created_at, entry.object_id.clone());
        let slot = self
            .0
            .entry((topic.to_owned(), prefix, bucket))
            .or_insert_with(|| (at.clone(), 0));
        if slot.0 != at {
            *slot = (at, 0);
        }
        slot.1 += 1;
        slot.1 >= RETENTION_ENTRY_ATTEMPTS
    }

    fn clear(&mut self, topic: &str, prefix: &'static str, bucket: u64) {
        self.0.remove(&(topic.to_owned(), prefix, bucket));
    }
}

/// 読み先ごとの索引の 1 ページを、時刻の順に合わせたもの。
struct MergedIndexPage {
    /// entry と、それを返した読み先の位置。
    entries: Vec<(TimeIndexEntry, usize)>,
    /// 読み残しがある(次の回を早める)。
    more: bool,
}

/// 読み先ごとに、続きの位置より新しい側を古い順に `limit` 件まで読み、時刻の順に合わせる。ページが埋まった読み先は、
/// そのページの最後より後ろを読んでいないので、それより後ろの entry は返さない(次の回に同じ位置から読み直す)。
/// `not_after`(秒)より後ろの時刻の entry は返さず、続きの位置もそこを越えない(未来の時刻の key で位置を先へ
/// 飛ばさない)。
async fn merged_index_page(
    readers: &[Arc<dyn DocsSync>],
    replica: &ReplicaId,
    prefix: &str,
    after: Option<&TimeIndexCursor>,
    limit: usize,
    not_after: i64,
    deadline: n0_future::time::Instant,
) -> MergedIndexPage {
    let mut merged: BTreeMap<(i64, String), (TimeIndexEntry, usize)> = BTreeMap::new();
    let mut bound: Option<(i64, String)> = None;
    let mut more = false;
    for (slot, reader) in readers.iter().enumerate() {
        let Ok(Ok(page)) = crate::timeout_at(
            deadline,
            query_time_index_asc(reader.as_ref(), replica, prefix, after, limit),
        )
        .await
        else {
            continue;
        };
        if page.entries.len() >= limit
            && let Some(last) = page.entries.last()
        {
            let last = (last.created_at, last.object_id.clone());
            if bound.as_ref().is_none_or(|bound| last < *bound) {
                bound = Some(last);
            }
        }
        more |= page.resume.is_some();
        for entry in page.entries {
            merged
                .entry((entry.created_at, entry.object_id.clone()))
                .or_insert((entry, slot));
        }
    }
    let mut entries = Vec::new();
    for (key, value) in merged {
        if key.0 > not_after
            || bound.as_ref().is_some_and(|bound| key > *bound)
            || entries.len() == limit
        {
            more = true;
            break;
        }
        entries.push(value);
    }
    MergedIndexPage { entries, more }
}

/// 保持できる topic の名前。公開の topic だけ(private channel・DM・account 同期・公開スレッドの hint の topic と、
/// `hint/` 付きの名前は受け付けない)。
fn validate_retention_topic(topic: &str) -> Result<()> {
    anyhow::ensure!(
        !topic.is_empty()
            && topic.len() <= 512
            && !topic.chars().any(char::is_whitespace)
            && !topic.starts_with(kukuri_core::wire::HINT_TOPIC_PREFIX)
            && !kukuri_core::wire::is_non_public_topic(topic),
        "topic is not a public topic"
    );
    Ok(())
}

impl AppService {
    fn topic_retention_store(&self) -> Result<Arc<dyn TopicRetentionStore>> {
        self.services
            .topic_retention
            .clone()
            .context("topic retention is not available on this client")
    }

    /// 公開 topic の保持を設定・解除する。設定は topic の lease を取って取り込みの task を起こし、解除は lease を外して
    /// その topic の続きの位置を消す。lease の上限(64)に達していれば `ScopeLimitReached` で拒否し、何も保存しない。
    pub async fn set_topic_retention(&self, topic: &str, retained: bool) -> Result<()> {
        let store = self.topic_retention_store()?;
        validate_retention_topic(topic)?;
        let holder = retain_holder(topic);
        if !retained {
            store.release_topic(topic).await?;
            self.release_scope_holder(&holder).await;
            // 保持する topic が無くなったら、task は次の回で終わる。
            self.subscription_registry.topic_retention_wake.notify_one();
            return Ok(());
        }
        self.set_scope_holder(&holder, [ScopeKey::Topic(topic.to_owned())])
            .await?;
        if let Err(error) = store
            .retain_topic(topic, Utc::now().timestamp_millis())
            .await
        {
            self.release_scope_holder(&holder).await;
            return Err(error);
        }
        self.start_topic_retention_task().await?;
        self.subscription_registry.topic_retention_wake.notify_one();
        Ok(())
    }

    /// 保持する topic と、topic ごとの現在と前日の bucket の続きの位置。
    pub async fn topic_retention(&self) -> Result<TopicRetentionView> {
        let store = self.topic_retention_store()?;
        let current = TimeBucket::from_unix_seconds(Utc::now().timestamp())?;
        let from = current.previous().unwrap_or(current).index();
        let mut topics = Vec::new();
        for row in store.retained_topics(MAX_ACTIVE_SCOPES).await? {
            let progress = store
                .topic_retention_cursors(&row.topic_id, from)
                .await?
                .into_iter()
                .map(Into::into)
                .collect();
            topics.push(RetainedTopicView {
                topic: row.topic_id,
                retained_at: row.retained_at,
                progress,
            });
        }
        Ok(TopicRetentionView { topics })
    }

    /// 起動時に、保存した topic の lease を取り直し、取り込みの task を起こす。lease の上限で取れない topic は warn に
    /// して起動を続ける(desired の復元と同じ)。保持する topic が無ければ何もしない。
    pub async fn start_topic_retention(&self) -> Result<()> {
        let Some(store) = self.services.topic_retention.clone() else {
            return Ok(());
        };
        let topics = store.retained_topics(MAX_ACTIVE_SCOPES).await?;
        if topics.is_empty() {
            return Ok(());
        }
        for row in topics {
            if let Err(error) = self
                .set_scope_holder(
                    &retain_holder(&row.topic_id),
                    [ScopeKey::Topic(row.topic_id.clone())],
                )
                .await
            {
                warn!(topic = %row.topic_id, %error, "retained topic is not subscribed");
            }
        }
        self.start_topic_retention_task().await
    }

    async fn start_topic_retention_task(&self) -> Result<()> {
        let registry = &self.subscription_registry;
        anyhow::ensure!(
            !registry.topic_retention_closed.load(Ordering::Acquire),
            "topic retention is closed"
        );
        let mut owner = registry.topic_retention_task.lock().await;
        if owner.as_ref().is_some_and(|task| !task.is_finished()) {
            return Ok(());
        }
        let app = self.account_handle();
        *owner = Some(AbortOnDropTask::new(n0_future::task::spawn(async move {
            app.run_topic_retention().await;
        })));
        Ok(())
    }

    pub(crate) async fn shutdown_topic_retention(&self) {
        let registry = &self.subscription_registry;
        registry
            .topic_retention_closed
            .store(true, Ordering::Release);
        let task = registry.topic_retention_task.lock().await.take();
        if let Some(task) = task {
            task.abort();
            let _ = n0_future::time::timeout(Duration::from_secs(2), task.wait()).await;
        }
    }

    /// 取り込みの task の本体。保持する topic を順に 1 つずつ進め、保持する topic が無くなったら終わる。
    async fn run_topic_retention(&self) {
        let registry = &self.subscription_registry;
        let mut next = 0usize;
        let mut busy = BTreeSet::new();
        let mut failures = RetentionFailures::default();
        loop {
            if registry.topic_retention_closed.load(Ordering::Acquire) {
                return;
            }
            match self
                .topic_retention_round(&mut next, &mut busy, &mut failures)
                .await
            {
                Ok(false) => {
                    // 保持する topic が無ければ終わる。設定と同じ lock の下で読み直し、同時に足された topic を取りこぼさない
                    // (設定は行を置いてから lock を取り、動いている task を見たら起こさない)。
                    let mut owner = registry.topic_retention_task.lock().await;
                    let empty = match self.topic_retention_store() {
                        Ok(store) => store
                            .retained_topics(1)
                            .await
                            .is_ok_and(|topics| topics.is_empty()),
                        Err(_) => true,
                    };
                    if empty {
                        // 自分の handle を外して終わる。外すと abort の印が付くが、この後に await は無いのでそのまま終わる。
                        owner.take();
                        return;
                    }
                }
                Ok(true) => {}
                Err(error) => warn!(%error, "topic retention step deferred"),
            }
            let wait = if busy.is_empty() {
                RETENTION_IDLE_INTERVAL
            } else {
                RETENTION_BUSY_INTERVAL
            };
            tokio::select! {
                _ = n0_future::time::sleep(wait) => {}
                _ = registry.topic_retention_wake.notified() => {}
            }
        }
    }

    /// 保持する topic を順に 1 つ選んで 1 回進める。`busy` は続きが残っている topic。保持する topic が無ければ `false`。
    async fn topic_retention_round(
        &self,
        next: &mut usize,
        busy: &mut BTreeSet<String>,
        failures: &mut RetentionFailures,
    ) -> Result<bool> {
        let store = self.topic_retention_store()?;
        let topics = store.retained_topics(MAX_ACTIVE_SCOPES).await?;
        if topics.is_empty() {
            return Ok(false);
        }
        busy.retain(|topic| topics.iter().any(|row| row.topic_id == *topic));
        let topic = topics[*next % topics.len()].topic_id.clone();
        *next = next.wrapping_add(1);
        // gossip を止めた topic は取り込まない(lease の task も止まっている)。
        if self.is_topic_gossip_disabled(&topic).await {
            busy.remove(&topic);
            return Ok(true);
        }
        if self.step_topic_retention(&topic, failures).await? {
            busy.insert(topic);
        } else {
            busy.remove(&topic);
        }
        Ok(true)
    }

    /// topic を 1 回進める。現在と前日の bucket の索引ごとに、続きの位置から新しい対象を古い順に取り込み、位置を進める。
    /// 戻り値は、読み残しがあるか。
    pub(crate) async fn step_topic_retention(
        &self,
        topic: &str,
        failures: &mut RetentionFailures,
    ) -> Result<bool> {
        let store = self.topic_retention_store()?;
        let deadline = n0_future::time::Instant::now() + RETENTION_STEP_DEADLINE;
        let now = Utc::now().timestamp();
        let current = TimeBucket::from_unix_seconds(now)?;
        let previous = current.previous().unwrap_or(current);
        store
            .prune_topic_retention_cursors(topic, previous.index())
            .await?;
        let cursors = store
            .topic_retention_cursors(topic, previous.index())
            .await?;
        let not_after = now.saturating_add(TIME_INDEX_FUTURE_ALLOWANCE_SECS);
        let mut left = RETENTION_STEP_ENTRIES;
        let mut more = false;
        let buckets: &[TimeBucket] = if previous == current {
            &[current]
        } else {
            &[previous, current]
        };
        for bucket in buckets {
            let replica = BucketReplica::new(
                BucketScope::Topic {
                    topic_id: topic.to_owned(),
                },
                *bucket,
            )?
            .replica_id();
            let readers = self
                .remote_post_readers(topic, None, &replica, None)
                .await
                .inspect_err(|error| warn!(%error, "retention reader selection failed"))
                .unwrap_or_default();
            if readers.is_empty() {
                continue;
            }
            for prefix in RETENTION_INDEXES {
                if left == 0 || n0_future::time::Instant::now() >= deadline {
                    return Ok(true);
                }
                let after = cursors
                    .iter()
                    .find(|cursor| cursor.index_prefix == prefix && cursor.bucket == bucket.index())
                    .map(|cursor| TimeIndexCursor {
                        created_at: cursor.created_at,
                        object_id: cursor.object_id.clone(),
                    });
                let page = merged_index_page(
                    &readers,
                    &replica,
                    prefix,
                    after.as_ref(),
                    left,
                    not_after,
                    deadline,
                )
                .await;
                more |= page.more;
                for (entry, slot) in page.entries {
                    let reader = readers[slot].clone();
                    let retained = crate::timeout_at(
                        deadline,
                        self.retain_index_entry(topic, &replica, prefix, &entry, reader.clone()),
                    )
                    .await;
                    reader.finish_remote_object().await;
                    match retained {
                        Ok(Ok(())) => failures.clear(topic, prefix, bucket.index()),
                        failed => {
                            let error = match failed {
                                Ok(Err(error)) => error,
                                _ => anyhow::anyhow!("retention step deadline elapsed"),
                            };
                            // 次の回に同じ位置から読み直す。続けて失敗した対象だけを飛ばす。
                            if !failures.note(topic, prefix, bucket.index(), &entry) {
                                warn!(topic, %error, "retention will retry an entry");
                                return Ok(true);
                            }
                            warn!(topic, %error, "retention skipped an entry after repeated failures");
                            failures.clear(topic, prefix, bucket.index());
                        }
                    }
                    store
                        .put_topic_retention_cursor(
                            topic,
                            &TopicRetentionCursor {
                                index_prefix: prefix.to_owned(),
                                bucket: bucket.index(),
                                created_at: entry.created_at,
                                object_id: entry.object_id.clone(),
                            },
                        )
                        .await?;
                    left -= 1;
                }
            }
        }
        Ok(more)
    }

    /// 索引の entry 1 件が指す対象を、その entry を返した読み先から取り込む。手元に既にある投稿は読まない。
    async fn retain_index_entry(
        &self,
        topic: &str,
        replica: &ReplicaId,
        prefix: &str,
        entry: &TimeIndexEntry,
        reader: Arc<dyn DocsSync>,
    ) -> Result<()> {
        if prefix == PUBLIC_CHANNEL_LIST_PREFIX {
            let channel = ChannelId::new(entry.object_id.as_str());
            if !channel.is_public_channel() {
                return Ok(());
            }
            let known = self
                .services
                .projection_store
                .get_public_channel_row(channel.as_str())
                .await?
                .filter(|row| row.topic_id == topic)
                .map(|row| row.revision);
            self.read_public_channel_from(
                reader,
                topic,
                &channel,
                DocFetchPolicy::LocalThenRemote,
                known,
            )
            .await?;
            return Ok(());
        }
        let post_id = if prefix == PUBLIC_CHANNEL_ACTIVITY_PREFIX {
            match split_public_channel_activity_id(&entry.object_id) {
                Some((channel, post)) if ChannelId::new(channel).is_public_channel() => post,
                _ => return Ok(()),
            }
        } else {
            entry.object_id.as_str()
        };
        let object_id = EnvelopeId::from(post_id);
        if self
            .services
            .projection_store
            .get_object_projection(&object_id)
            .await?
            .is_some()
        {
            return Ok(());
        }
        let mut services = self.services.clone();
        services.docs_sync = reader;
        hydrate_object_in_topic_with(
            &services,
            topic,
            replica,
            &object_id,
            entry.docs_author.as_deref(),
            DocFetchPolicy::LocalThenRemote,
            BodyFetch::Bounded,
        )
        .await?;
        Ok(())
    }
}
