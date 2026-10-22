//! 指定した公開 topic の内容を、画面の列なしに取り込んで保持する(#1909、ADR 0055 §1.1)。CLI daemon が使う。
//!
//! - 保持する topic ごとに topic の lease(holder `retain:<topic>`)を取る。lease は gossip への参加と、既存の lease の
//!   task による topic の hint の受信に使う。公開スレッドの lease は取らない。
//! - account ごとに 1 つの task が、1 回に 1 つの topic を選び、現在と前日の bucket の時刻順の索引
//!   (`indexes/timeline/`・`channels/list/`・`channels/activity/`)を、続きの位置から古い順に上限つきで読む。新しい
//!   対象は既存の反映の関数(署名・scope・bucket の日の検証つき)で取り込む。公開スレッドの投稿は
//!   `channels/activity/` から見つけるので、公開スレッドの数に比例する購読も task も作らない。
//! - 取り込んだ投稿の本文・添付・リンクプレビュー・リアクション(custom の画像)も取って保持する。現在と前日の bucket の
//!   取り下げ・リアクション・session の key は、反映していない版だけを読む。Dome の session は、owner の制御領域の
//!   Instance・hosting・Preset と資材も保持する。
//! - 読めなかった記録と、取れなかった blob を持つ対象は保存済みと数えず、取り直しの予定(SQLite)に入れて先へ進む。
//! - 前日より前の日は、保持の期間の内を新しい日から古い日へ 1 日ずつ、索引と session の key を最後まで 1 回読む。読む日は、
//!   読む前に topic の bucket の replica を保護の登録に置き、取り込んだ record と blob に日の保護参照
//!   (`retain:<topic>:<bucket>`)を付ける。期間を過ぎた日・容量を超えた分の古い日・保持をやめた topic の日の保護参照は、
//!   1 回 128 件まで外す(ADR 0055)。

use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::atomic::Ordering;
use std::time::Duration;

use crate::service::*;
use kukuri_docs_sync::{
    BucketReplica, BucketScope, DocKeyEntry, DocKeyOrder, DocKeyQuery, TimeBucket, TimeIndexCursor,
    TimeIndexEntry, query_time_index_asc,
};
use kukuri_store::{
    MAX_RETENTION_DAYS, MIN_RETENTION_CAPACITY_BYTES, TopicRetentionCursor, TopicRetentionRetry,
};

mod content;

/// 1 回に取り込む対象の上限(取り直し・索引・key の読みを合わせて)。読み先ごとの読みと、対象 1 件の記録の取り込みには、
/// それぞれ既存の期限(`REMOTE_READ_DEADLINE`)を持たせる(1 回全体で期限を分け合うと、届かない読み先が期限を使い切り、
/// 試していない対象を失敗に数える)。
pub(crate) const RETENTION_STEP_ENTRIES: usize = 32;
/// 続きが残っているときの、次の回までの間。
const RETENTION_BUSY_INTERVAL: Duration = Duration::from_secs(1);
/// 追いついたときの、次の回までの間。
const RETENTION_IDLE_INTERVAL: Duration = Duration::from_secs(30);
/// 1 回に外す保護参照と、登録より前に置かれた record に付ける保護参照の、それぞれの上限。
const RETENTION_RECLAIM_STEP: usize = 128;
/// topic のタイムラインの索引。
const TOPIC_TIMELINE_INDEX: &str = "indexes/timeline/";
/// 読む索引(topic の投稿、公開スレッドの作成、公開スレッドの投稿)。
const RETENTION_INDEXES: [&str; 3] = [
    TOPIC_TIMELINE_INDEX,
    PUBLIC_CHANNEL_LIST_PREFIX,
    PUBLIC_CHANNEL_ACTIVITY_PREFIX,
];
/// 現在と前日の bucket で、反映していない版を読む key(prefix、1 回の上限、順)。取り下げとリアクションは 512 件、
/// session は種類ごとに新しい側から 64 件(判断 6)。前日より前の日は session(後ろの 2 つ)だけを読む。
const RETENTION_SCANS: [(&str, usize, DocKeyOrder); 4] = [
    ("withdrawals/", 512, DocKeyOrder::Ascending),
    ("reactions/", 512, DocKeyOrder::Ascending),
    ("sessions/live/", 64, DocKeyOrder::Descending),
    ("sessions/game/", 64, DocKeyOrder::Descending),
];
/// 投稿ごとに、投稿の日と翌日の bucket から読むリアクションの上限。
const RETENTION_POST_REACTIONS: usize = 32;
/// blob 1 件の取得の期限。最短 60 秒で、大きい blob は 1 MiB/s で間に合う長さにする。
const RETENTION_BLOB_DEADLINE: Duration = Duration::from_secs(60);
const RETENTION_BLOB_BYTES_PER_SEC: u64 = 1 << 20;
/// 取り直しの予定の対象の種類(投稿、公開スレッドの metadata、リアクションの key)。session は `read_session` の種類の
/// 名前(`live-session`・`game-session`)を使う。
const RETAIN_POST: &str = "post";
const RETAIN_CHANNEL: &str = "channel";
const RETAIN_REACTION: &str = "reaction";

/// 公開 topic の保持の保存先。
#[derive(Clone)]
pub(crate) struct TopicRetentionHandles {
    pub(crate) store: Arc<dyn TopicRetentionStore>,
    /// 添付などの大きな blob を cache へ置く前に受け取る dir(cache と同じ volume に置き、置いた後に消す)。
    #[cfg_attr(target_family = "wasm", allow(dead_code))]
    pub(crate) staging: std::path::PathBuf,
}

impl ServiceHandles {
    /// 公開 topic の保持の設定と続きの位置を置く store と、大きな blob を受け取る dir。native の runtime が入れる。
    pub fn with_topic_retention(
        mut self,
        store: Arc<dyn TopicRetentionStore>,
        staging: std::path::PathBuf,
    ) -> Self {
        self.topic_retention = Some(TopicRetentionHandles { store, staging });
        self
    }
}

/// 保持の設定と進み具合。
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TopicRetentionView {
    /// 保持の期間(日)と容量(bytes)。
    pub retention_days: u32,
    pub capacity_bytes: u64,
    /// 保護参照を付けた record と blob の大きさの合計。
    pub used_bytes: u64,
    pub topics: Vec<RetainedTopicView>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RetainedTopicView {
    pub topic: String,
    /// 保持を始めた時刻(ミリ秒)。
    pub retained_at: i64,
    /// 現在と前日の bucket の、索引ごとに読み終えた最後の entry。
    pub progress: Vec<RetentionProgressView>,
    /// 取り直しを待つ対象の数。
    pub pending_retries: u64,
    /// 取り直しを諦めた対象の数。
    pub given_up: u64,
    /// この topic の日の保護参照を付けた record と blob の大きさの合計。
    pub used_bytes: u64,
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

/// 現在と前日の bucket で反映した key の版(key と content hash の組の hash)。topic・bucket・prefix ごとに、最後に読んだ
/// 一覧にある分だけを持つ。
#[derive(Default)]
pub(crate) struct RetentionState(HashMap<(String, u64, &'static str), HashSet<u64>>);

impl RetentionState {
    /// `reads(topic, bucket)` が偽の位置(保持をやめた topic と、前日より前の bucket)の記録を消す。
    fn keep_only(&mut self, reads: impl Fn(&str, u64) -> bool) {
        self.0.retain(|(topic, bucket, _), _| reads(topic, *bucket));
    }
}

/// key の版の印。
fn key_version(entry: &DocKeyEntry) -> u64 {
    let mut hasher = DefaultHasher::new();
    (&entry.key, &entry.content_hash).hash(&mut hasher);
    hasher.finish()
}

/// 現在と前日の bucket で見つけた key 1 件の反映の結果。
enum Scanned {
    /// 保持していない投稿へのもの(投稿を保持した後の回に反映する)。
    Pending,
    /// 反映した(反映する新しい版が無かったときを含む)。
    Done,
    /// 参照する blob を取れなかった。取り直しの予定に入れる。
    Retry(&'static str, String),
}

/// 読み先ごとの索引の 1 ページを、時刻の順に合わせたもの。
struct MergedIndexPage {
    /// entry と、それを返した読み先の位置。
    entries: Vec<(TimeIndexEntry, usize)>,
    /// 読み残しがある(次の回を早める)。
    more: bool,
    /// どれかの読み先が答えた(前日より前の日を読み終えたかの判定に使う)。
    answered: bool,
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
) -> MergedIndexPage {
    let mut merged: BTreeMap<(i64, String), (TimeIndexEntry, usize)> = BTreeMap::new();
    let mut bound: Option<(i64, String)> = None;
    let (mut more, mut answered) = (false, false);
    for (slot, reader) in readers.iter().enumerate() {
        let Ok(Ok(page)) = n0_future::time::timeout(
            REMOTE_READ_DEADLINE,
            query_time_index_asc(reader.as_ref(), replica, prefix, after, limit),
        )
        .await
        else {
            continue;
        };
        answered = true;
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
    MergedIndexPage {
        entries,
        more,
        answered,
    }
}

/// 保持の期間の内の最も古い bucket。期間は今日を含めて `retention_days` 日。
fn retention_cutoff(current: TimeBucket, retention_days: u32) -> u64 {
    current
        .index()
        .saturating_sub(u64::from(retention_days.max(1)) - 1)
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

/// topic の bucket の replica。
fn topic_bucket(topic: &str, bucket: TimeBucket) -> Result<ReplicaId> {
    Ok(BucketReplica::new(
        BucketScope::Topic {
            topic_id: topic.to_owned(),
        },
        bucket,
    )?
    .replica_id())
}

/// blob 1 件の取得の期限。
fn blob_deadline(bytes: u64) -> Duration {
    RETENTION_BLOB_DEADLINE.max(Duration::from_secs(bytes / RETENTION_BLOB_BYTES_PER_SEC))
}

/// 索引の entry が指す対象(種類と ID)。公開スレッドでない channel の entry は対象にしない。
fn index_target(prefix: &str, entry: &TimeIndexEntry) -> Option<(&'static str, String)> {
    match prefix {
        PUBLIC_CHANNEL_LIST_PREFIX => ChannelId::new(entry.object_id.as_str())
            .is_public_channel()
            .then(|| (RETAIN_CHANNEL, entry.object_id.clone())),
        PUBLIC_CHANNEL_ACTIVITY_PREFIX => {
            match split_public_channel_activity_id(&entry.object_id) {
                Some((channel, post)) if ChannelId::new(channel).is_public_channel() => {
                    Some((RETAIN_POST, post.to_owned()))
                }
                _ => None,
            }
        }
        _ => Some((RETAIN_POST, entry.object_id.clone())),
    }
}

/// session の key(`sessions/<種類>/<ID>/state`・`locator`)の ID。
fn session_key_id<'a>(prefix: &str, key: &'a str) -> Option<&'a str> {
    let rest = key.strip_prefix(prefix)?;
    let id = rest
        .strip_suffix("/state")
        .or_else(|| rest.strip_suffix("/locator"))?;
    (!id.is_empty() && !id.contains('/')).then_some(id)
}

impl AppService {
    fn topic_retention_handles(&self) -> Result<&TopicRetentionHandles> {
        self.services
            .topic_retention
            .as_ref()
            .context("topic retention is not available on this client")
    }

    /// 公開 topic の保持を設定・解除する。設定は topic の lease を取って取り込みの task を起こし、解除は lease を外して
    /// その topic の続きの位置と取り直しの予定を消す。lease の上限(64)に達していれば `ScopeLimitReached` で拒否し、
    /// 何も保存しない。
    pub async fn set_topic_retention(&self, topic: &str, retained: bool) -> Result<()> {
        let store = &self.topic_retention_handles()?.store;
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

    /// 保持の期間(日。1〜365)と容量(bytes。1 GiB 以上)を変える。
    pub async fn set_topic_retention_limits(
        &self,
        retention_days: u32,
        capacity_bytes: u64,
    ) -> Result<()> {
        anyhow::ensure!(
            (1..=MAX_RETENTION_DAYS).contains(&retention_days)
                && capacity_bytes >= MIN_RETENTION_CAPACITY_BYTES,
            "retention limits are out of range"
        );
        self.topic_retention_handles()?
            .store
            .set_topic_retention_limits(retention_days, capacity_bytes)
            .await?;
        self.subscription_registry.topic_retention_wake.notify_one();
        Ok(())
    }

    /// 保持の期間と容量と保護の合計、保持する topic と、topic ごとの現在と前日の bucket の続きの位置、取り直しを待つ数と
    /// 諦めた数、保護の合計。
    pub async fn topic_retention(&self) -> Result<TopicRetentionView> {
        let store = &self.topic_retention_handles()?.store;
        let limits = store.topic_retention_limits().await?;
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
            let (pending_retries, given_up) =
                store.topic_retention_retry_counts(&row.topic_id).await?;
            let used_bytes = store.topic_retention_used(&row.topic_id).await?;
            topics.push(RetainedTopicView {
                topic: row.topic_id,
                retained_at: row.retained_at,
                progress,
                pending_retries,
                given_up,
                used_bytes,
            });
        }
        Ok(TopicRetentionView {
            retention_days: limits.retention_days,
            capacity_bytes: limits.capacity_bytes,
            used_bytes: limits.used_bytes,
            topics,
        })
    }

    /// 起動時に、保存した topic の lease を取り直し、取り込みの task を起こす。lease の上限で取れない topic は warn に
    /// して起動を続ける(desired の復元と同じ)。保持する topic が無ければ、保持をやめた日の保護参照が残っているときだけ、
    /// 外し終えるまで task を起こす。
    pub async fn start_topic_retention(&self) -> Result<()> {
        let Some(handles) = &self.services.topic_retention else {
            return Ok(());
        };
        let topics = handles.store.retained_topics(MAX_ACTIVE_SCOPES).await?;
        if topics.is_empty() && self.reclaim_topic_retention().await? == 0 {
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

    /// 期間を過ぎた日・容量を超えた分の古い日・保持をやめた topic の日の保護参照を外し、登録より前に置かれた record に
    /// 保護参照を付ける(それぞれ 1 回 128 件まで)。戻り値は、進めた件数。
    async fn reclaim_topic_retention(&self) -> Result<usize> {
        let store = &self.topic_retention_handles()?.store;
        let current = TimeBucket::from_unix_seconds(Utc::now().timestamp())?;
        let cutoff = retention_cutoff(
            current,
            store.topic_retention_limits().await?.retention_days,
        );
        Ok(store
            .reclaim_topic_retention_step(cutoff, RETENTION_RECLAIM_STEP)
            .await?
            + store
                .protect_topic_retention_scopes_step(RETENTION_RECLAIM_STEP)
                .await?)
    }

    /// 取り込みの task の本体。保持する topic を順に 1 つずつ進め、保持する topic も外す保護参照も無くなったら終わる。
    async fn run_topic_retention(&self) {
        let registry = &self.subscription_registry;
        let mut next = 0usize;
        let mut busy = BTreeSet::new();
        let mut state = RetentionState::default();
        loop {
            if registry.topic_retention_closed.load(Ordering::Acquire) {
                return;
            }
            let mut reclaiming = false;
            match self
                .topic_retention_round(&mut next, &mut busy, &mut state)
                .await
            {
                Ok(None) => {
                    // 保持する topic が無ければ終わる。設定と同じ lock の下で読み直し、同時に足された topic を取りこぼさない
                    // (設定は行を置いてから lock を取り、動いている task を見たら起こさない)。
                    let mut owner = registry.topic_retention_task.lock().await;
                    let empty = match self.topic_retention_handles() {
                        Ok(handles) => handles
                            .store
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
                Ok(Some(reclaimed)) => reclaiming = reclaimed,
                Err(error) => warn!(%error, "topic retention step deferred"),
            }
            let wait = if busy.is_empty() && !reclaiming {
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

    /// 保護参照を外して付ける段を 1 回進め、保持する topic を順に 1 つ選んで 1 回進める。`busy` は続きが残っている topic。
    /// 保持する topic が無く、保護参照の段も進まなければ `None`。それ以外は、保護参照の段を進めたか。
    async fn topic_retention_round(
        &self,
        next: &mut usize,
        busy: &mut BTreeSet<String>,
        state: &mut RetentionState,
    ) -> Result<Option<bool>> {
        let reclaimed = self.reclaim_topic_retention().await? > 0;
        let topics = self
            .topic_retention_handles()?
            .store
            .retained_topics(MAX_ACTIVE_SCOPES)
            .await?;
        if topics.is_empty() {
            return Ok(reclaimed.then_some(true));
        }
        busy.retain(|topic| topics.iter().any(|row| row.topic_id == *topic));
        state.keep_only(|topic, _| topics.iter().any(|row| row.topic_id == topic));
        let topic = topics[*next % topics.len()].topic_id.clone();
        *next = next.wrapping_add(1);
        // gossip を止めた topic は取り込まない(lease の task も止まっている)。
        if self.is_topic_gossip_disabled(&topic).await {
            busy.remove(&topic);
            return Ok(Some(reclaimed));
        }
        if self.step_topic_retention(&topic, state).await? {
            busy.insert(topic);
        } else {
            busy.remove(&topic);
        }
        Ok(Some(reclaimed))
    }

    /// topic を 1 回進める。時刻の来た取り直しを先に行い、現在と前日の bucket と、前日より前でまだ読み終えていない最も
    /// 新しい日の索引ごとに、続きの位置から新しい対象を古い順に取り込んで位置を進め、key の反映していない版を反映する
    /// (前日より前の日は session だけ)。前日より前の日は、索引と session を最後まで読んだら読み終えた日にする。戻り値は、
    /// 読み残しがあるか。
    pub(crate) async fn step_topic_retention(
        &self,
        topic: &str,
        state: &mut RetentionState,
    ) -> Result<bool> {
        let store = self.topic_retention_handles()?.store.clone();
        let now = Utc::now();
        let current = TimeBucket::from_unix_seconds(now.timestamp())?;
        let previous = current.previous().unwrap_or(current);
        let cutoff = retention_cutoff(
            current,
            store.topic_retention_limits().await?.retention_days,
        );
        // 停止していた間の日を含めて期間の内の日の行を作り、期間を過ぎた日の続きの位置を消す。
        store
            .open_topic_retention_days(topic, cutoff, current.index())
            .await?;
        store.prune_topic_retention_cursors(topic, cutoff).await?;
        let backfill = match previous.index().checked_sub(1) {
            Some(before) => {
                store
                    .next_topic_retention_backfill(topic, cutoff, before)
                    .await?
            }
            None => None,
        };
        state.keep_only(|retained, bucket| {
            retained != topic || bucket >= previous.index() || Some(bucket) == backfill
        });
        let mut left = RETENTION_STEP_ENTRIES;
        for retry in store
            .due_topic_retention_retries(topic, now.timestamp_millis(), left)
            .await?
        {
            left -= 1;
            let complete = self
                .retain_retry(topic, &retry)
                .await
                .unwrap_or_else(|error| {
                    warn!(topic, %error, "retention will retry an object later");
                    false
                });
            if *self.services.content_closed.borrow() {
                return Ok(true);
            }
            if complete {
                store
                    .finish_topic_retention_retry(topic, &retry.object_kind, &retry.object_id)
                    .await?;
            } else {
                store
                    .schedule_topic_retention_retry(
                        topic,
                        &retry.object_kind,
                        &retry.object_id,
                        retry.bucket,
                        Utc::now().timestamp_millis(),
                    )
                    .await?;
            }
        }
        let cursors = store.topic_retention_cursors(topic, cutoff).await?;
        let not_after = now
            .timestamp()
            .saturating_add(TIME_INDEX_FUTURE_ALLOWANCE_SECS);
        let mut more = false;
        let mut days = vec![previous, current];
        days.dedup();
        days.extend(backfill.map(TimeBucket::from_index).transpose()?);
        for bucket in days {
            let live = bucket >= previous;
            let replica = topic_bucket(topic, bucket)?;
            // 容量を超えて保護を外した日は読まない。読む日は、置かれる record に保護参照を付ける登録をする。
            if !store
                .protect_topic_retention_day(topic, bucket.index(), replica.as_str())
                .await?
            {
                continue;
            }
            let readers = self
                .remote_post_readers(topic, None, &replica, None)
                .await
                .inspect_err(|error| warn!(%error, "retention reader selection failed"))
                .unwrap_or_default();
            let Some(first) = readers.first().cloned() else {
                continue;
            };
            let mut read_to_end = true;
            for prefix in RETENTION_INDEXES {
                if left == 0 {
                    return Ok(true);
                }
                let after = cursors
                    .iter()
                    .find(|cursor| cursor.index_prefix == prefix && cursor.bucket == bucket.index())
                    .map(|cursor| TimeIndexCursor {
                        created_at: cursor.created_at,
                        object_id: cursor.object_id.clone(),
                    });
                let page =
                    merged_index_page(&readers, &replica, prefix, after.as_ref(), left, not_after)
                        .await;
                more |= page.more;
                read_to_end &= page.answered && !page.more;
                for (entry, slot) in page.entries {
                    if let Some((kind, id)) = index_target(prefix, &entry) {
                        let reader = readers[slot].clone();
                        let read = n0_future::time::timeout(
                            REMOTE_READ_DEADLINE,
                            self.retain_record(
                                topic,
                                &replica,
                                kind,
                                &id,
                                entry.docs_author.as_deref(),
                                reader.clone(),
                            ),
                        )
                        .await;
                        reader.finish_remote_object().await;
                        // shutdown の途中の対象は反映されないまま返る。続きの位置を進めず、次の起動で読み直す。
                        if *self.services.content_closed.borrow() {
                            return Ok(true);
                        }
                        let complete = match read {
                            Ok(Ok(true)) => {
                                self.retain_content(topic, bucket.index(), kind, &id).await
                            }
                            Ok(Err(error)) => {
                                warn!(topic, %error, "retention will retry a record later");
                                false
                            }
                            Ok(Ok(false)) | Err(_) => false,
                        };
                        if !complete {
                            store
                                .schedule_topic_retention_retry(
                                    topic,
                                    kind,
                                    &id,
                                    bucket.index(),
                                    Utc::now().timestamp_millis(),
                                )
                                .await?;
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
            let scans = if live {
                &RETENTION_SCANS[..]
            } else {
                &RETENTION_SCANS[2..]
            };
            let (scanned_more, scanned) = self
                .scan_retention_keys(topic, bucket, &replica, &first, state, &mut left, scans)
                .await?;
            more |= scanned_more;
            if !live && read_to_end && scanned && !scanned_more {
                store
                    .finish_topic_retention_day(topic, bucket.index())
                    .await?;
                // 次の回で、次に古い日を読む。
                more = true;
            }
        }
        Ok(more)
    }

    /// bucket の `scans` の key を 1 つの読み先から読み、反映していない版を反映する。取り下げとリアクションは、保持して
    /// いる投稿へのものだけを反映する。戻り値は、上限で読み残したかと、すべて読めたか。
    #[allow(clippy::too_many_arguments)]
    async fn scan_retention_keys(
        &self,
        topic: &str,
        bucket: TimeBucket,
        replica: &ReplicaId,
        reader: &Arc<dyn DocsSync>,
        state: &mut RetentionState,
        left: &mut usize,
        scans: &[(&'static str, usize, DocKeyOrder)],
    ) -> Result<(bool, bool)> {
        let store = self.topic_retention_handles()?.store.clone();
        let (mut more, mut complete) = (false, true);
        for &(prefix, limit, order) in scans {
            let page = n0_future::time::timeout(
                REMOTE_READ_DEADLINE,
                reader.query_replica_keys(
                    replica,
                    DocKeyQuery {
                        prefix: prefix.to_owned(),
                        order,
                        limit,
                    },
                ),
            )
            .await;
            reader.finish_remote_object().await;
            let Ok(Ok(page)) = page else {
                complete = false;
                continue;
            };
            let slot = (topic.to_owned(), bucket.index(), prefix);
            let seen = state.0.remove(&slot).unwrap_or_default();
            let mut kept = HashSet::new();
            let mut sessions = HashSet::new();
            for entry in &page.entries {
                let version = key_version(entry);
                if seen.contains(&version) {
                    kept.insert(version);
                    continue;
                }
                if *left == 0 {
                    more = true;
                    continue;
                }
                let scanned = self
                    .retain_scanned(
                        topic,
                        bucket.index(),
                        replica,
                        prefix,
                        entry,
                        reader,
                        &mut sessions,
                    )
                    .await;
                if *self.services.content_closed.borrow() {
                    return Ok((true, false));
                }
                match scanned {
                    Ok(Scanned::Pending) => continue,
                    Ok(Scanned::Done) => {}
                    Ok(Scanned::Retry(kind, id)) => {
                        store
                            .schedule_topic_retention_retry(
                                topic,
                                kind,
                                &id,
                                bucket.index(),
                                Utc::now().timestamp_millis(),
                            )
                            .await?;
                    }
                    // 次の回に同じ版を読み直す。
                    Err(error) => {
                        warn!(topic, %error, "retention will read a key again");
                        complete = false;
                        *left -= 1;
                        continue;
                    }
                }
                kept.insert(version);
                *left -= 1;
            }
            state.0.insert(slot, kept);
        }
        Ok((more, complete))
    }
}
