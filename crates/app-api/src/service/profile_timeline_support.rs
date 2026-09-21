//! プロフィールのタイムライン(author replica の投稿と repost)を、時系列の索引から上限つきで読む(#1239)。
//!
//! 投稿と repost を書くとき、`indexes/profile/<timeline_sort_key>/<object id>` の索引も書く。取得は、その索引を
//! cursor から新しい順に読み、ページの行の record だけを key 指定で読む。replica の投稿の総数を読まない。
//!
//! 索引の無い replica(索引を書く前の版が書いた投稿)との互換: 自分の replica は、購読タスクが背景で索引を補い、
//! 補い終えたら `indexes/profile-complete` を書く。その key が無い replica では、索引の読み出しに、`profile/posts/`・
//! `profile/reposts/` の key の上限つきの一覧(それぞれ `PROFILE_LEGACY_KEYS` 件)から読んだ行を合わせる(best effort)。

use super::*;
use crate::service::author_state_support::{
    CHECKPOINT_AFTER, CHECKPOINT_DONE, CHECKPOINT_RESTART, HexBucketedKeys,
};
use crate::service::projection_support::HIDDEN_AUTHOR_SKIP_PAGES;
use kukuri_docs_sync::{
    DocKeyOrder, DocKeyQuery, TimeIndexCursor, query_time_index_desc,
    query_time_index_desc_by_author,
};

const PROFILE_INDEX_PREFIX: &str = "indexes/profile/";
/// 自分の replica の索引を補い終えた印。
const PROFILE_INDEX_COMPLETE_KEY: &str = "indexes/profile-complete";
/// 索引の無い replica で、投稿・repost の key をそれぞれ何件まで一覧するか。
const PROFILE_LEGACY_KEYS: usize = 128;
/// 索引を補うときに、1 回の key の一覧で読む件数。超えたら object id の次の桁で分けて読む。
const PROFILE_BACKFILL_BATCH: usize = 256;
/// 索引を補う 1 回の、key の一覧の query の数の上限。
const PROFILE_BACKFILL_MAX_QUERIES: usize = 4_096;

fn profile_index_key(created_at: i64, object_id: &EnvelopeId) -> String {
    stable_key(
        "indexes/profile",
        &format!(
            "{}/{}",
            timeline_sort_key(created_at, object_id),
            object_id.as_str()
        ),
    )
}

/// プロフィールの索引の entry を 1 件書く。
pub(crate) async fn persist_profile_index_entry(
    docs_sync: &dyn DocsSync,
    replica: &ReplicaId,
    created_at: i64,
    object_id: &EnvelopeId,
    kind: &str,
) -> Result<()> {
    docs_sync
        .apply_doc_op(
            replica,
            DocOp::SetJson {
                key: profile_index_key(created_at, object_id),
                value: serde_json::json!({
                    "object_id": object_id,
                    "created_at": created_at,
                    "kind": kind,
                }),
            },
        )
        .await
}

fn item_position(item: &ProfileTimelineItem) -> (i64, String) {
    (item.created_at(), item.object_id().as_str().to_string())
}

/// `profile/posts/<object id>` の record から、検証に通った投稿を返す。
async fn profile_post_from_record(
    docs_sync: &dyn DocsSync,
    replica: &ReplicaId,
    author_pubkey: &str,
    record: &DocRecord,
) -> Result<Option<ProfilePost>> {
    let expected_profile_topic_id = author_profile_topic_id(author_pubkey);
    let doc = match serde_json::from_slice::<AuthorProfilePostDocV1>(record.value.as_slice()) {
        Ok(doc)
            if doc.author_pubkey.as_str() == author_pubkey
                && doc.profile_topic_id == expected_profile_topic_id =>
        {
            doc
        }
        Ok(_) => {
            warn!(
                author_pubkey = %author_pubkey,
                key = %record.key,
                "ignoring profile post doc with mismatched author or topic"
            );
            return Ok(None);
        }
        Err(error) => {
            warn!(
                author_pubkey = %author_pubkey,
                key = %record.key,
                error = %error,
                "failed to decode profile post doc"
            );
            return Ok(None);
        }
    };
    let Some(envelope) = fetch_author_envelope_by_id(
        docs_sync,
        replica,
        &doc.envelope_id,
        DocFetchPolicy::LocalOnly,
    )
    .await?
    else {
        return Ok(None);
    };
    match parse_profile_post(&envelope) {
        Ok(Some(post))
            if post.author_pubkey == doc.author_pubkey
                && post.profile_topic_id == doc.profile_topic_id
                && post.published_topic_id == doc.published_topic_id
                && post.object_id == doc.object_id
                && post.created_at == doc.created_at
                && post.object_kind == doc.object_kind
                && post.content == doc.content
                && post.attachments == doc.attachments
                && post.reply_to_object_id == doc.reply_to_object_id
                && post.root_id == doc.root_id =>
        {
            Ok(Some(post))
        }
        Ok(_) => Ok(None),
        Err(error) => {
            warn!(
                author_pubkey = %author_pubkey,
                key = %record.key,
                envelope_id = %doc.envelope_id.as_str(),
                error = %error,
                "ignoring invalid profile post envelope"
            );
            Ok(None)
        }
    }
}

/// `profile/reposts/<object id>` の record から、検証に通った repost を返す。
async fn profile_repost_from_record(
    docs_sync: &dyn DocsSync,
    replica: &ReplicaId,
    author_pubkey: &str,
    record: &DocRecord,
) -> Result<Option<ProfileRepost>> {
    let expected_profile_topic_id = author_profile_topic_id(author_pubkey);
    let doc = match serde_json::from_slice::<AuthorProfileRepostDocV1>(record.value.as_slice()) {
        Ok(doc)
            if doc.author_pubkey.as_str() == author_pubkey
                && doc.profile_topic_id == expected_profile_topic_id =>
        {
            doc
        }
        Ok(_) => {
            warn!(
                author_pubkey = %author_pubkey,
                key = %record.key,
                "ignoring profile repost doc with mismatched author or topic"
            );
            return Ok(None);
        }
        Err(error) => {
            warn!(
                author_pubkey = %author_pubkey,
                key = %record.key,
                error = %error,
                "failed to decode profile repost doc"
            );
            return Ok(None);
        }
    };
    let Some(envelope) = fetch_author_envelope_by_id(
        docs_sync,
        replica,
        &doc.envelope_id,
        DocFetchPolicy::LocalOnly,
    )
    .await?
    else {
        return Ok(None);
    };
    match parse_profile_repost(&envelope) {
        Ok(Some(repost))
            if repost.author_pubkey == doc.author_pubkey
                && repost.profile_topic_id == doc.profile_topic_id
                && repost.published_topic_id == doc.published_topic_id
                && repost.object_id == doc.object_id
                && repost.created_at == doc.created_at
                && repost.commentary == doc.commentary
                && repost.repost_of == doc.repost_of =>
        {
            Ok(Some(repost))
        }
        Ok(_) => Ok(None),
        Err(error) => {
            warn!(
                author_pubkey = %author_pubkey,
                key = %record.key,
                envelope_id = %doc.envelope_id.as_str(),
                error = %error,
                "ignoring invalid profile repost envelope"
            );
            Ok(None)
        }
    }
}

/// 手元の record から、プロフィールの行を 1 件読む(key 指定。投稿、無ければ repost)。
///
/// 同じ key に docs author ごとの record がありうる(誰でも書ける replica)。先頭の 1 件だけを見ず、上限つきで調べ、
/// 検証に通った最初の record を使う。
async fn load_profile_item(
    docs_sync: &dyn DocsSync,
    replica: &ReplicaId,
    author_pubkey: &str,
    object_id: &str,
    docs_author: Option<&str>,
) -> Result<Option<ProfileTimelineItem>> {
    // 著者の docs author が分かれば、docs author と key の組で 1 件読む(ADR 0053 §6。他の名義の record は何件あっても
    // 読まない)。無い・検証に通らないときは、下の上限つきの読み出しに落とす(旧 record)。
    if let Some(docs_author) = docs_author {
        for (prefix, is_post) in [("profile/posts", true), ("profile/reposts", false)] {
            let Some(record) = docs_sync
                .query_replica_by_author(
                    replica,
                    docs_author,
                    stable_key(prefix, object_id).as_str(),
                    DocFetchPolicy::LocalOnly,
                )
                .await?
            else {
                continue;
            };
            if is_post {
                if let Some(post) =
                    profile_post_from_record(docs_sync, replica, author_pubkey, &record).await?
                {
                    return Ok(Some(ProfileTimelineItem::Post(post)));
                }
            } else if let Some(repost) =
                profile_repost_from_record(docs_sync, replica, author_pubkey, &record).await?
            {
                return Ok(Some(ProfileTimelineItem::Repost(repost)));
            }
        }
    }
    for record in docs_sync
        .query_replica_exact_bounded(
            replica,
            stable_key("profile/posts", object_id).as_str(),
            MAX_ENVELOPE_RECORDS_PER_OBJECT,
            DocFetchPolicy::LocalOnly,
        )
        .await?
    {
        if let Some(post) =
            profile_post_from_record(docs_sync, replica, author_pubkey, &record).await?
        {
            return Ok(Some(ProfileTimelineItem::Post(post)));
        }
    }
    for record in docs_sync
        .query_replica_exact_bounded(
            replica,
            stable_key("profile/reposts", object_id).as_str(),
            MAX_ENVELOPE_RECORDS_PER_OBJECT,
            DocFetchPolicy::LocalOnly,
        )
        .await?
    {
        if let Some(repost) =
            profile_repost_from_record(docs_sync, replica, author_pubkey, &record).await?
        {
            return Ok(Some(ProfileTimelineItem::Repost(repost)));
        }
    }
    Ok(None)
}

/// `key` が、著者の docs author の名義で置かれているか(ADR 0053 §6)。誰でも書ける replica なので、他の名義の key は
/// 数えない。docs author が分からないときは `false`(印は無いものとして扱い、旧 record を合わせる)。
async fn authored_key_exists(
    docs_sync: &dyn DocsSync,
    replica: &ReplicaId,
    docs_author: Option<&str>,
    key: &str,
) -> Result<bool> {
    let Some(docs_author) = docs_author else {
        return Ok(false);
    };
    Ok(docs_sync
        .query_replica_keys_by_author(
            replica,
            docs_author,
            DocKeyQuery {
                prefix: key.to_string(),
                order: DocKeyOrder::Ascending,
                limit: 1,
            },
        )
        .await?
        .entries
        .iter()
        .any(|entry| entry.key == key))
}

async fn has_key(docs_sync: &dyn DocsSync, replica: &ReplicaId, key: &str) -> Result<bool> {
    let page = docs_sync
        .query_replica_keys(
            replica,
            DocKeyQuery {
                prefix: key.to_string(),
                order: DocKeyOrder::Ascending,
                limit: 1,
            },
        )
        .await?;
    Ok(page.entries.iter().any(|entry| entry.key == key))
}

/// 索引の無い replica の行を、key の上限つきの一覧から読む(新しい順)。
async fn load_legacy_profile_items(
    docs_sync: &dyn DocsSync,
    replica: &ReplicaId,
    author_pubkey: &str,
    docs_author: Option<&str>,
) -> Result<Vec<ProfileTimelineItem>> {
    let mut items = Vec::new();
    let mut seen = BTreeSet::new();
    for prefix in ["profile/posts/", "profile/reposts/"] {
        let page = docs_sync
            .query_replica_keys(
                replica,
                DocKeyQuery {
                    prefix: prefix.to_string(),
                    order: DocKeyOrder::Ascending,
                    limit: PROFILE_LEGACY_KEYS,
                },
            )
            .await?;
        for entry in page.entries {
            let Some(object_id) = entry.key.strip_prefix(prefix) else {
                continue;
            };
            if !seen.insert(object_id.to_string()) {
                continue;
            }
            if let Some(item) =
                load_profile_item(docs_sync, replica, author_pubkey, object_id, docs_author).await?
            {
                items.push(item);
            }
        }
    }
    items.sort_by_key(|item| std::cmp::Reverse(item_position(item)));
    Ok(items)
}

/// プロフィールのタイムラインの 1 ページ(#1239)。replica は走査しない。
///
/// 索引を cursor から新しい順に `limit` 件ずつ読み、ページの行だけを key 指定で読む。非表示の著者の行は除き、
/// 読むページ数は `HIDDEN_AUTHOR_SKIP_PAGES` まで。`next_cursor` は、`limit` 件そろえば最後に返した行の位置、
/// 読むページ数の上限に達したら読み進めた位置、尽きたら `None`。
pub(crate) async fn profile_timeline_page_from_docs(
    docs_sync: &dyn DocsSync,
    author_pubkey: &str,
    docs_author: Option<&str>,
    cursor: Option<TimelineCursor>,
    limit: usize,
    hidden_author_pubkeys: &BTreeSet<String>,
) -> Result<Page<ProfileTimelineItem>> {
    if limit == 0 {
        return Ok(Page {
            items: Vec::new(),
            next_cursor: cursor,
        });
    }
    let replica = author_replica_id(author_pubkey);
    // 補い終えた印は、著者の docs author の名義のものだけを見る(他人が置いた印で、旧 record を隠させない)。
    let legacy =
        if authored_key_exists(docs_sync, &replica, docs_author, PROFILE_INDEX_COMPLETE_KEY).await?
        {
            Vec::new()
        } else {
            load_legacy_profile_items(docs_sync, &replica, author_pubkey, docs_author).await?
        };
    let mut position = cursor.map(|cursor| (cursor.created_at, cursor.object_id.0));
    let mut items: Vec<ProfileTimelineItem> = Vec::new();
    for _ in 0..HIDDEN_AUTHOR_SKIP_PAGES {
        let index_cursor = position
            .as_ref()
            .map(|(created_at, object_id)| TimeIndexCursor {
                created_at: *created_at,
                object_id: object_id.clone(),
            });
        // 著者の docs author が分かれば、その名義の索引の key だけをたどる(他の名義の key でページを埋めさせない)。
        let page = match docs_author {
            Some(docs_author) => {
                query_time_index_desc_by_author(
                    docs_sync,
                    &replica,
                    PROFILE_INDEX_PREFIX,
                    docs_author,
                    index_cursor.as_ref(),
                    limit,
                )
                .await?
            }
            None => {
                query_time_index_desc(
                    docs_sync,
                    &replica,
                    PROFILE_INDEX_PREFIX,
                    index_cursor.as_ref(),
                    limit,
                )
                .await?
            }
        };
        // 索引の、まだ読んでいない側の境界。境界より古い行は、次の回で読む。
        let boundary = match (&page.resume, page.entries.len() >= limit) {
            (Some(resume), _) => Some((resume.created_at, resume.object_id.clone())),
            (None, true) => page
                .entries
                .last()
                .map(|entry| (entry.created_at, entry.object_id.clone())),
            (None, false) => None,
        };
        let mut candidates = Vec::new();
        let mut seen = BTreeSet::new();
        for entry in &page.entries {
            // 読んだ印は、行の検証と位置の照合に通った後で付ける。先に付けると、同じ object id の偽の索引の entry が
            // 本物の entry と旧 record を読み飛ばさせる。
            if seen.contains(&entry.object_id) {
                continue;
            }
            if let Some(item) = load_profile_item(
                docs_sync,
                &replica,
                author_pubkey,
                &entry.object_id,
                docs_author,
            )
            .await?
                && item_position(&item) == (entry.created_at, entry.object_id.clone())
            {
                seen.insert(entry.object_id.clone());
                candidates.push(item);
            }
        }
        for item in &legacy {
            let item_pos = item_position(item);
            let after_cursor = position.as_ref().is_none_or(|current| item_pos < *current);
            let before_boundary = boundary.as_ref().is_none_or(|edge| item_pos > *edge);
            if after_cursor && before_boundary && seen.insert(item_pos.1.clone()) {
                candidates.push(item.clone());
            }
        }
        candidates.sort_by_key(|item| std::cmp::Reverse(item_position(item)));
        let mut remaining = candidates.into_iter();
        for item in remaining.by_ref() {
            position = Some(item_position(&item));
            if !profile_timeline_item_is_hidden(&item, hidden_author_pubkeys) {
                items.push(item);
            }
            if items.len() >= limit {
                break;
            }
        }
        if items.len() >= limit {
            let more = boundary.is_some()
                || remaining.next().is_some()
                || legacy.iter().any(|item| {
                    position
                        .as_ref()
                        .is_some_and(|current| item_position(item) < *current)
                });
            let next_cursor = more.then(|| {
                let (created_at, object_id) = position.clone().expect("a returned row");
                TimelineCursor {
                    created_at,
                    object_id: EnvelopeId::from(object_id.as_str()),
                }
            });
            return Ok(Page { items, next_cursor });
        }
        match boundary {
            Some(edge) => position = Some(edge),
            None => {
                return Ok(Page {
                    items,
                    next_cursor: None,
                });
            }
        }
    }
    // 非表示の著者の行が続き、読むページ数の上限に達した。読み進めた位置を返す。
    let next_cursor = position.map(|(created_at, object_id)| TimelineCursor {
        created_at,
        object_id: EnvelopeId::from(object_id.as_str()),
    });
    Ok(Page { items, next_cursor })
}

/// 自分の replica の、索引の無い投稿・repost に索引を補う(#1239)。背景で 1 回だけ行う。
///
/// 補い終えたら `indexes/profile-complete` を書き、以後は何もしない。key の一覧は `PROFILE_BACKFILL_BATCH` 件ずつ、
/// 超えたら object id の次の桁で分けて読む。読み終えた桶の位置を store に残し、途中で止まっても続きから読む
/// (索引が既にある行は書かない)。
pub(crate) async fn backfill_own_profile_index(
    docs_sync: &dyn DocsSync,
    projection_store: &dyn ProjectionStore,
    author_pubkey: &str,
) -> Result<usize> {
    backfill_own_profile_index_with(
        docs_sync,
        projection_store,
        author_pubkey,
        PROFILE_BACKFILL_BATCH,
        PROFILE_BACKFILL_MAX_QUERIES,
    )
    .await
}

pub(crate) async fn backfill_own_profile_index_with(
    docs_sync: &dyn DocsSync,
    projection_store: &dyn ProjectionStore,
    author_pubkey: &str,
    batch: usize,
    max_queries: usize,
) -> Result<usize> {
    let replica = author_replica_id(author_pubkey);
    // 補うかどうかは、この端末の位置(`sync_checkpoints`)で決める。replica の印は、ほかの端末や同期の前の状態を表さない。
    // 自分の replica なので、自分の docs author で組を先に読む。
    let docs_author = docs_sync.local_docs_author().await?;
    let mut written = 0usize;
    for (prefix, kind) in [("profile/posts/", "post"), ("profile/reposts/", "repost")] {
        // 読み終えた桶の位置を残し、止まっても続きから読む。
        let checkpoint_key = format!("profile-index-backfill/{author_pubkey}/{prefix}");
        let checkpoint = projection_store
            .get_sync_checkpoint(&checkpoint_key)
            .await?;
        if checkpoint.as_deref() == Some(CHECKPOINT_DONE) {
            continue;
        }
        let done_through = checkpoint
            .as_deref()
            .and_then(|value| value.strip_prefix(CHECKPOINT_AFTER))
            .map(str::to_string);
        let mut keys = HexBucketedKeys::new(prefix, batch, max_queries, done_through);
        while let Some(entries) = keys.next_batch(docs_sync, &replica).await? {
            let mut seen = BTreeSet::new();
            for entry in entries {
                let Some(object_id) = entry.key.strip_prefix(prefix) else {
                    continue;
                };
                if !seen.insert(object_id.to_string()) {
                    continue;
                }
                let Some(item) = load_profile_item(
                    docs_sync,
                    &replica,
                    author_pubkey,
                    object_id,
                    docs_author.as_deref(),
                )
                .await?
                else {
                    continue;
                };
                let key = profile_index_key(item.created_at(), item.object_id());
                if index_entry_exists(docs_sync, &replica, docs_author.as_deref(), key.as_str())
                    .await?
                {
                    continue;
                }
                persist_profile_index_entry(
                    docs_sync,
                    &replica,
                    item.created_at(),
                    item.object_id(),
                    kind,
                )
                .await?;
                written += 1;
            }
            if let Some(bucket) = keys.last_bucket() {
                projection_store
                    .put_sync_checkpoint(&checkpoint_key, &format!("{CHECKPOINT_AFTER}{bucket}"))
                    .await?;
            }
            tokio::task::yield_now().await;
        }
        if keys.stopped_early() {
            // query 数の上限で止まった。位置を残したので、次の購読で続きから読む。
            return Ok(written);
        }
        projection_store
            .put_sync_checkpoint(&checkpoint_key, CHECKPOINT_DONE)
            .await?;
    }
    if !index_entry_exists(
        docs_sync,
        &replica,
        docs_author.as_deref(),
        PROFILE_INDEX_COMPLETE_KEY,
    )
    .await?
    {
        docs_sync
            .apply_doc_op(
                &replica,
                DocOp::SetJson {
                    key: PROFILE_INDEX_COMPLETE_KEY.to_string(),
                    value: serde_json::json!({ "version": 1 }),
                },
            )
            .await?;
    }
    Ok(written)
}

/// 自分の書き込みの key が既にあるか。自分の docs author が分かれば、その名義の key だけを見る(他人が同じ key を先に
/// 置いても、自分の名義の entry を書く)。分からなければ、名義を問わずに見る。
async fn index_entry_exists(
    docs_sync: &dyn DocsSync,
    replica: &ReplicaId,
    docs_author: Option<&str>,
    key: &str,
) -> Result<bool> {
    match docs_author {
        Some(_) => authored_key_exists(docs_sync, replica, docs_author, key).await,
        None => has_key(docs_sync, replica, key).await,
    }
}

/// 自分の索引の補完を最初からやり直す(#1239)。自分の replica の event を取りこぼしたとき(`Lagged`)に使う。
pub(crate) async fn restart_own_profile_index_backfill(
    projection_store: &dyn ProjectionStore,
    author_pubkey: &str,
) -> Result<()> {
    for prefix in ["profile/posts/", "profile/reposts/"] {
        projection_store
            .put_sync_checkpoint(
                &format!("profile-index-backfill/{author_pubkey}/{prefix}"),
                CHECKPOINT_RESTART,
            )
            .await?;
    }
    Ok(())
}

/// 自分の replica に届いた `profile/posts/<id>`・`profile/reposts/<id>` の key に、索引が無ければ足す(#1239)。
///
/// 索引を書く前の版の端末が書いた投稿が、補完を読み終えた後に同期で届いても、索引から読めるようにする。
/// 索引の entry は追記だけで、既存の状態を巻き戻さない。戻り値は索引を足したか。
pub(crate) async fn index_own_profile_key(
    docs_sync: &dyn DocsSync,
    author_pubkey: &str,
    key: &str,
) -> Result<OwnProfileIndex> {
    let Some(object_id) = key
        .strip_prefix("profile/posts/")
        .or_else(|| key.strip_prefix("profile/reposts/"))
    else {
        return Ok(OwnProfileIndex::NotProfileKey);
    };
    let replica = author_replica_id(author_pubkey);
    let docs_author = docs_sync.local_docs_author().await?;
    let Some(item) = load_profile_item(
        docs_sync,
        &replica,
        author_pubkey,
        object_id,
        docs_author.as_deref(),
    )
    .await?
    else {
        // 本体(doc か envelope)がまだ手元に無いか、検証に通らない。
        return Ok(OwnProfileIndex::NotReadable);
    };
    let index_key = profile_index_key(item.created_at(), item.object_id());
    if index_entry_exists(
        docs_sync,
        &replica,
        docs_author.as_deref(),
        index_key.as_str(),
    )
    .await?
    {
        return Ok(OwnProfileIndex::Present);
    }
    persist_profile_index_entry(
        docs_sync,
        &replica,
        item.created_at(),
        item.object_id(),
        item_kind(&item),
    )
    .await?;
    Ok(OwnProfileIndex::Indexed)
}

/// `index_own_profile_key` の結果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OwnProfileIndex {
    /// 索引を足した。
    Indexed,
    /// 自分の名義の索引が既にある。
    Present,
    /// 行を読めない(本体がまだ手元に無い、または検証に通らない)。
    NotReadable,
    /// 投稿・repost の key ではない。
    NotProfileKey,
}

/// 索引の値に入れる行の種類(読んだ行から決める。key の prefix と食い違うことがある)。
fn item_kind(item: &ProfileTimelineItem) -> &'static str {
    match item {
        ProfileTimelineItem::Post(_) => "post",
        ProfileTimelineItem::Repost(_) => "repost",
    }
}
