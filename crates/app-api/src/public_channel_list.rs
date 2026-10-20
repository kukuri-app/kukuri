//! 公開 channel の一覧(#1840 AC-2、ADR 0068 §6)。表示は手元の行(`PublicChannelStore`)だけを読み、topic の bucket の
//! 索引を有界に読んで、知らない channel と新しい投稿を確かめてから行へ反映する。一覧は channel の投稿を読まず、
//! channel ごとの hint も購読しない。
use std::collections::{BTreeSet, HashMap};

use crate::public_channels::PublicChannelView;
use crate::service::*;
use kukuri_docs_sync::{
    BucketReplica, BucketScope, TimeBucket, TimeIndexCursor, query_time_index_desc,
    query_time_index_window,
};
use kukuri_store::{Page, PublicChannelCursor, PublicChannelOrder, PublicChannelRow};

/// 作成の新しい順の 1 回の照合で、cursor の日(無ければ現在)から古い側へ読む日数(bucket の数)。
const CREATED_BUCKETS: usize = 4;
/// 最後の投稿の新しい順の 1 回の照合で読む日数(現在と前日。続きのページは cursor の日と前日)。
const LAST_POST_BUCKETS: usize = 2;
/// 1 回の照合で、読み先から読んで確かめる metadata と投稿の数の上限。
const VERIFY_LIMIT: usize = 20;
/// 最後の投稿の索引を 1 つの読み先と bucket から読む件数は、ページの件数のこの倍まで(同じ channel の記録が並ぶ)。
const POSTS_PER_ROW: usize = 5;
/// 1 つの読み先と bucket の索引の読み出しの上限(provider の key の一覧の上限 512 に、形の違う key の余裕を足しても
/// 収まる)。
const MAX_INDEX_ENTRIES: usize = 256;

/// 一覧の 1 ページ。
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields = nullable))]
pub struct PublicChannelListView {
    pub items: Vec<PublicChannelView>,
    pub next_cursor: Option<PublicChannelCursor>,
    /// 照合した範囲の索引にあるが、metadata か最後の投稿を読めない・確かめられないスレッドの数。0 なら、画面は何も
    /// 示さない。
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<u32>"))]
    pub unavailable_count: u32,
}

/// 照合で見つけた、確かめる候補(channel ごとに最も新しい記録)。
struct Candidate {
    at_seconds: i64,
    /// 最後の投稿の索引の記録なら、指している投稿と、その記録を書いた docs author。作成の索引なら `None`。
    post: Option<(EnvelopeId, Option<String>)>,
    /// 記録を読んだ読み先と bucket(`readers` の位置)。
    reader: usize,
}

#[derive(Default)]
pub(crate) struct RangeOutcome {
    changed: bool,
    unavailable: usize,
}

impl AppService {
    /// topic の公開 channel を `order` の新しい順に、`cursor` より後ろから `limit` 件まで返す。非表示の作者の
    /// スレッドと削除したスレッド(#1888)は除く(読み飛ばすのは 4 ページまで。ADR 0052 §5)。
    pub async fn list_public_channels(
        &self,
        topic_id: &str,
        order: PublicChannelOrder,
        cursor: Option<PublicChannelCursor>,
        limit: usize,
    ) -> Result<PublicChannelListView> {
        let hidden = self.current_hidden_author_pubkeys().await?;
        let unavailable = self
            .reconcile_public_channels(topic_id, order, cursor.as_ref(), limit, &hidden)
            .await?;
        // 続きは手元の行の続きから読む(タイムラインと同じ。この端末が知らない、照合の範囲より前のスレッドは、
        // 手元の行が尽きると出ない。2026-10-17 のユーザー判断 13)。
        let page = self
            .visible_public_channels(topic_id, order, cursor, limit, &hidden)
            .await?;
        Ok(PublicChannelListView {
            items: self
                .with_row_details(
                    topic_id,
                    page.items.into_iter().map(Into::into).collect(),
                    &hidden,
                )
                .await?,
            next_cursor: page.next_cursor,
            unavailable_count: u32::try_from(unavailable).unwrap_or(u32::MAX),
        })
    }

    async fn visible_public_channels(
        &self,
        topic_id: &str,
        order: PublicChannelOrder,
        mut cursor: Option<PublicChannelCursor>,
        limit: usize,
        hidden: &BTreeSet<String>,
    ) -> Result<Page<PublicChannelRow, PublicChannelCursor>> {
        let mut items = Vec::new();
        if limit == 0 {
            return Ok(Page {
                items,
                next_cursor: cursor,
            });
        }
        for _ in 0..HIDDEN_AUTHOR_SKIP_PAGES {
            let page = self
                .services
                .projection_store
                .list_public_channels(topic_id, order, cursor, limit.max(20))
                .await?;
            for row in page.items {
                if row.deleted || hidden.contains(&row.creator_pubkey) {
                    continue;
                }
                items.push(row);
                if items.len() == limit {
                    let next_cursor = items.last().map(|row| row.cursor(order));
                    return Ok(Page { items, next_cursor });
                }
            }
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        Ok(Page {
            items,
            next_cursor: cursor,
        })
    }

    /// ページの範囲の索引を照合し、確かめられなかった記録の数を返す。範囲ごとに 1 つずつ、間隔(30 秒)を空けて
    /// 照合し、続いている間と間隔の内の取得は前回の数を返す。先頭のページは provider の応答を待たせない(#1624):
    /// 300 ms の猶予を過ぎた照合は背景で続き、反映したら次の取得で並ぶ。
    async fn reconcile_public_channels(
        &self,
        topic_id: &str,
        order: PublicChannelOrder,
        cursor: Option<&PublicChannelCursor>,
        limit: usize,
        hidden: &BTreeSet<String>,
    ) -> Result<usize> {
        let range = format!(
            "public-channels\n{topic_id}\n{order:?}\n{}",
            cursor.map_or_else(String::new, |cursor| format!(
                "{}:{}",
                cursor.at, cursor.channel_id
            ))
        );
        let ledger = &self.services.range_checks;
        let busy_ms = 2 * REMOTE_READ_DEADLINE.as_millis() as i64;
        if limit == 0
            || ledger
                .try_begin_for(&range, Utc::now().timestamp_millis(), busy_ms)
                .await
                .is_none()
        {
            return Ok(ledger.last_result(&range).await.0);
        }
        let app = self.account_handle();
        let (topic_id_owned, cursor_owned, hidden_owned, range_owned) = (
            topic_id.to_owned(),
            cursor.cloned(),
            hidden.clone(),
            range.clone(),
        );
        let task = n0_future::task::spawn(async move {
            let result = app
                .reconcile_public_channel_range(
                    &topic_id_owned,
                    order,
                    cursor_owned.as_ref(),
                    limit,
                    &hidden_owned,
                )
                .await
                .inspect_err(|error| warn!(%error, "public channel list reconcile failed"))
                .unwrap_or_default();
            let checked = match (result.unavailable, result.changed) {
                (0, _) => RangeCheckResult::Complete,
                (_, true) => RangeCheckResult::Progressed,
                (_, false) => RangeCheckResult::Stalled,
            };
            app.services
                .range_checks
                .record_finished(
                    &range_owned,
                    Utc::now().timestamp_millis(),
                    checked,
                    result.unavailable,
                )
                .await;
            if result.changed {
                app.last_sync_ts.set(Utc::now().timestamp_millis()).await;
            }
            result.unavailable
        });
        let waited = match cursor {
            None => n0_future::time::timeout(HEAD_PROVIDER_GRACE, task)
                .await
                .ok()
                .and_then(Result::ok),
            Some(_) => task.await.ok(),
        };
        match waited {
            Some(unavailable) => Ok(unavailable),
            None => Ok(ledger.last_result(&range).await.0),
        }
    }

    /// 範囲の bucket の索引を読み先ごとに読み、channel ごとに最も新しい記録だけを残して、手元の行より新しいものを
    /// 確かめる(1 回 `VERIFY_LIMIT` 件まで)。全体の期限は `REMOTE_READ_DEADLINE`。
    pub(crate) async fn reconcile_public_channel_range(
        &self,
        topic_id: &str,
        order: PublicChannelOrder,
        cursor: Option<&PublicChannelCursor>,
        limit: usize,
        hidden: &BTreeSet<String>,
    ) -> Result<RangeOutcome> {
        let deadline = n0_future::time::Instant::now() + REMOTE_READ_DEADLINE;
        let now = Utc::now().timestamp();
        let bound = cursor.map_or(now + TIME_INDEX_FUTURE_ALLOWANCE_SECS, |cursor| {
            cursor.at.div_euclid(1_000)
        });
        let (days, prefixes): (usize, &[(&str, usize)]) = match order {
            PublicChannelOrder::Created => (CREATED_BUCKETS, &[(PUBLIC_CHANNEL_LIST_PREFIX, 1)]),
            PublicChannelOrder::LastPost => (
                LAST_POST_BUCKETS,
                &[
                    (PUBLIC_CHANNEL_ACTIVITY_PREFIX, POSTS_PER_ROW),
                    (PUBLIC_CHANNEL_LIST_PREFIX, 1),
                ],
            ),
        };
        let mut readers: Vec<(ReplicaId, Arc<dyn DocsSync>)> = Vec::new();
        let mut candidates: HashMap<String, Candidate> = HashMap::new();
        let mut day = Some(TimeBucket::from_unix_seconds(bound.min(now).max(0))?);
        for index in 0..days {
            let Some(bucket) = day else { break };
            day = bucket.previous();
            let replica = BucketReplica::new(
                BucketScope::Topic {
                    topic_id: topic_id.to_owned(),
                },
                bucket,
            )?
            .replica_id();
            let selected = self
                .remote_post_readers(topic_id, None, &replica, None)
                .await
                .inspect_err(|error| warn!(%error, "public channel list reader selection failed"))
                .unwrap_or_default();
            for reader in selected {
                let slot = readers.len();
                readers.push((replica.clone(), reader.clone()));
                for (prefix, per_row) in prefixes {
                    let count = limit.saturating_mul(*per_row).min(MAX_INDEX_ENTRIES);
                    // cursor の日は cursor より古い側を、それより前の日は新しい側から読む(どれも cursor より古い)。
                    let page = match (index, cursor) {
                        (0, Some(_)) => {
                            let before = TimeIndexCursor::not_after(bound);
                            crate::timeout_at(
                                deadline,
                                query_time_index_desc(
                                    &*reader,
                                    &replica,
                                    prefix,
                                    Some(&before),
                                    count,
                                ),
                            )
                            .await
                        }
                        _ => {
                            crate::timeout_at(
                                deadline,
                                query_time_index_window(&*reader, &replica, prefix, bound, count),
                            )
                            .await
                        }
                    };
                    let entries = match page {
                        Ok(Ok(page)) => page.entries,
                        Ok(Err(error)) => {
                            warn!(%error, "public channel index read failed");
                            continue;
                        }
                        Err(_) => break,
                    };
                    for entry in entries {
                        let (channel_id, post) = match *prefix == PUBLIC_CHANNEL_ACTIVITY_PREFIX {
                            true => {
                                let Some((channel_id, post_id)) =
                                    split_public_channel_activity_id(&entry.object_id)
                                else {
                                    continue;
                                };
                                (
                                    channel_id.to_string(),
                                    Some((EnvelopeId::from(post_id), entry.docs_author)),
                                )
                            }
                            false => (entry.object_id, None),
                        };
                        if !ChannelId::new(channel_id.as_str()).is_public_channel()
                            || candidates
                                .get(&channel_id)
                                .is_some_and(|known| known.at_seconds >= entry.created_at)
                        {
                            continue;
                        }
                        candidates.insert(
                            channel_id,
                            Candidate {
                                at_seconds: entry.created_at,
                                post,
                                reader: slot,
                            },
                        );
                    }
                }
            }
        }
        let mut ordered = candidates.into_iter().collect::<Vec<_>>();
        ordered.sort_by(|(left_id, left), (right_id, right)| {
            (right.at_seconds, right_id).cmp(&(left.at_seconds, left_id))
        });
        let mut outcome = RangeOutcome::default();
        let mut verified = 0;
        for (channel_id, candidate) in ordered {
            if verified == VERIFY_LIMIT || n0_future::time::Instant::now() >= deadline {
                break;
            }
            let row = self
                .services
                .projection_store
                .get_public_channel_row(&channel_id)
                .await?;
            let posted_at = candidate.at_seconds.saturating_mul(1_000);
            let post = candidate
                .post
                .filter(|_| row.as_ref().is_none_or(|row| row.last_post_at < posted_at));
            if row.is_some() && post.is_none() {
                continue;
            }
            verified += 1;
            let (replica, reader) = &readers[candidate.reader];
            let checked = crate::timeout_at(
                deadline,
                self.check_public_channel_candidate(
                    topic_id,
                    &ChannelId::new(channel_id.as_str()),
                    row.is_none(),
                    post.map(|post| (post, candidate.at_seconds)),
                    (replica, reader.clone()),
                    hidden,
                ),
            )
            .await;
            reader.finish_remote_object().await;
            match checked {
                Ok(Ok(Some(changed))) => outcome.changed |= changed,
                Ok(Ok(None)) => outcome.unavailable += 1,
                Ok(Err(error)) => {
                    warn!(%error, "public channel list check failed");
                    outcome.unavailable += 1;
                }
                Err(_) => {
                    outcome.unavailable += 1;
                    break;
                }
            }
        }
        Ok(outcome)
    }

    /// 1 つの候補を、記録を読んだ読み先から確かめる。手元に行が無ければ metadata を読んで置き、最後の投稿の記録なら
    /// 指している投稿の署名・channel・時刻を確かめてから、行の最後の投稿の時刻を進める(非表示の作者の投稿は数えない)。
    /// 読めない・確かめられないときは `None`、確かめたときは行を変えたかを返す。
    async fn check_public_channel_candidate(
        &self,
        topic_id: &str,
        channel_id: &ChannelId,
        missing_row: bool,
        post: Option<((EnvelopeId, Option<String>), i64)>,
        (replica, reader): (&ReplicaId, Arc<dyn DocsSync>),
        hidden: &BTreeSet<String>,
    ) -> Result<Option<bool>> {
        if missing_row
            && self
                .read_public_channel_from(
                    reader.clone(),
                    topic_id,
                    channel_id,
                    DocFetchPolicy::LocalThenRemote,
                    None,
                )
                .await?
                .is_none()
        {
            return Ok(None);
        }
        let Some(((post_id, docs_author), at_seconds)) = post else {
            return Ok(Some(missing_row));
        };
        let PostLoad::Verified(post) = load_post_with_hint(
            &*reader,
            replica,
            topic_id,
            &post_id,
            docs_author.as_deref(),
            DocFetchPolicy::LocalThenRemote,
        )
        .await?
        else {
            return Ok(None);
        };
        let header = post.header();
        if header.channel_id.as_ref() != Some(channel_id) || header.created_at != at_seconds {
            return Ok(None);
        }
        if hidden.contains(header.author.as_str()) {
            return Ok(Some(missing_row));
        }
        self.services
            .projection_store
            .note_public_channel_post(channel_id.as_str(), at_seconds.saturating_mul(1_000))
            .await?;
        Ok(Some(true))
    }
}
