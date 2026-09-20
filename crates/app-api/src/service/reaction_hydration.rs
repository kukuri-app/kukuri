//! reaction を docs から projection へ反映する(#1252)。検証は `reaction_integrity.rs`。

use super::hydration_limits::{ReplicaScanCache, scan_fingerprint};
use super::*;

/// 戻り値は今回反映した reaction の件数。
///
/// 行は署名つき envelope(`reactions/<target>/<reaction id>/envelope`)から作る(#1252)。検証に通らない reaction は
/// warn を出して飛ばし、走査は続ける。envelope の record は同じ prefix の読み出しに含まれるので、追加の読み出しは無い。
pub(crate) async fn hydrate_reaction_cache_from_replica(
    docs_sync: &dyn DocsSync,
    projection_store: &dyn ProjectionStore,
    scan_cache: &ReplicaScanCache,
    topic_id: &str,
    replica: &ReplicaId,
    policy: DocFetchPolicy,
) -> Result<usize> {
    const PREFIX: &str = "reactions/";
    let records = query_replica_with_fetch_policy(
        docs_sync,
        replica,
        DocQuery::Prefix(PREFIX.into()),
        policy,
    )
    .await?;
    let fingerprint = scan_fingerprint(&records);
    if scan_cache.is_unchanged(replica.as_str(), PREFIX, fingerprint) {
        return Ok(0);
    }
    let hydrated =
        hydrate_reactions_from_records(projection_store, topic_id, replica, &records).await?;
    scan_cache.record(replica.as_str(), PREFIX, fingerprint);
    Ok(hydrated)
}

/// 読み出し済みの record のうち、envelope の record から reaction を検証して反映する。docs は読まない。
async fn hydrate_reactions_from_records(
    projection_store: &dyn ProjectionStore,
    topic_id: &str,
    replica: &ReplicaId,
    records: &[DocRecord],
) -> Result<usize> {
    let Some(scope) = ReplicaPostScope::for_replica(replica, topic_id) else {
        // reaction を置く replica ではない。何も反映しない。
        return Ok(0);
    };
    // reaction ごとに envelope の record をまとめる(同じ key には docs author ごとの record がありうる)。
    let mut envelope_records: BTreeMap<ReactionKey, Vec<&DocRecord>> = BTreeMap::new();
    for record in records {
        if record.key.ends_with("/envelope")
            && let Some(key) = ReactionKey::from_doc_key(record.key.as_str())
        {
            envelope_records.entry(key).or_default().push(record);
        }
    }
    let mut hydrated = 0usize;
    for (key, candidates) in envelope_records {
        match select_verified_reaction(candidates, &key, replica, &scope) {
            Ok(Some(reaction)) => {
                hydrated += hydrate_reaction_cache_from_reaction(projection_store, &reaction)
                    .await? as usize;
            }
            Ok(None) => {}
            Err(reason) => warn_rejected_reaction(replica, &key, reason),
        }
    }
    Ok(hydrated)
}

/// 検証済みの reaction を 1 件、projection へ反映する。
///
/// 反映済みの行より古い envelope では行を戻さない。public topic の replica は誰でも書けるので、古い署名つき envelope を
/// 置き直すだけで、取り消した reaction を復活させられないようにする。
pub(crate) async fn hydrate_reaction_cache_from_reaction(
    projection_store: &dyn ProjectionStore,
    reaction: &VerifiedReaction,
) -> Result<bool> {
    let doc = reaction.doc();
    if let Some(existing) = projection_store
        .get_reaction_cache(reaction.replica(), &doc.target_object_id, &doc.reaction_id)
        .await?
        && existing.updated_at > doc.updated_at
    {
        return Ok(false);
    }
    projection_store
        .upsert_reaction_cache(reaction_projection_row(reaction))
        .await?;
    Ok(true)
}

/// reaction の key(`state` と `envelope` のどちらでもよい)を 1 つ指定して反映する。replica は走査しない。
///
/// `topic_id` は、その replica を読む文脈の topic。読む docs の record は、その reaction の envelope の key だけ。
pub(crate) async fn hydrate_reaction_cache_from_key(
    docs_sync: &dyn DocsSync,
    projection_store: &dyn ProjectionStore,
    topic_id: &str,
    replica: &ReplicaId,
    key: &str,
    policy: DocFetchPolicy,
) -> Result<bool> {
    let Some(key) = ReactionKey::from_doc_key(key) else {
        return Ok(false);
    };
    let Some(reaction) = load_verified_reaction(docs_sync, replica, topic_id, &key, policy).await?
    else {
        return Ok(false);
    };
    hydrate_reaction_cache_from_reaction(projection_store, &reaction).await
}

/// reaction id の先頭の 1 文字ごとの一覧で読む key の数(reaction 4 件ぶん)。
const REACTION_KEYS_PER_LEAD: usize = 8;

/// 対象の投稿 1 件の reaction を、上限つきで反映する(#1239)。replica は走査しない。
///
/// 読むのは、`reactions/<target>/` の key だけの上限つきの一覧(`max_reactions` 件ぶん)と、見つかった reaction ごとの
/// envelope の key(#1252 の検証)。読む量は対象の reaction の総数に依存しない。上限を超える reaction は、ここでは
/// 反映しない(docs の event と、購読タスクの反映が拾う)。検証に通らない reaction は飛ばす。
pub(crate) async fn hydrate_reaction_cache_for_target_bounded(
    docs_sync: &dyn DocsSync,
    projection_store: &dyn ProjectionStore,
    topic_id: &str,
    replica: &ReplicaId,
    target_object_id: &EnvelopeId,
    policy: DocFetchPolicy,
    max_reactions: usize,
) -> Result<usize> {
    // reaction 1 件につき、`state` と `envelope` の 2 つの key がある。
    let prefix = stable_key("reactions", &format!("{}/", target_object_id.as_str()));
    let list = |prefix: String, limit: usize| async move {
        docs_sync
            .query_replica_keys(
                replica,
                kukuri_docs_sync::DocKeyQuery {
                    prefix,
                    order: kukuri_docs_sync::DocKeyOrder::Ascending,
                    limit,
                },
            )
            .await
    };
    let reaction_keys = |page: kukuri_docs_sync::DocKeyPage| {
        page.entries
            .iter()
            .filter_map(|entry| ReactionKey::from_doc_key(entry.key.as_str()))
            .filter(|key| key.target_object_id == *target_object_id)
            .collect::<BTreeSet<_>>()
    };
    let head = list(prefix.clone(), max_reactions.saturating_mul(2)).await?;
    let truncated = head.reached_limit;
    let mut keys = reaction_keys(head).into_iter().collect::<Vec<_>>();
    if truncated {
        // 一覧が上限で打ち切られた。先頭に並ぶ key だけを見ていると、正しい reaction より先に並ぶ key を置くだけで、
        // その投稿の reaction を隠せてしまう。reaction id(16 進)の先頭の 1 文字ごとに少しずつ読み、混ぜて選ぶ。
        // 読む量は定数(16 回の上限つきの一覧)で、reaction の総数に依存しない。それでも覆えない分は best effort。
        let mut buckets = Vec::new();
        for lead in "0123456789abcdef".chars() {
            let page = list(format!("{prefix}{lead}"), REACTION_KEYS_PER_LEAD).await?;
            buckets.push(reaction_keys(page).into_iter().collect::<Vec<_>>());
        }
        let mut mixed = Vec::new();
        let mut index = 0usize;
        while mixed.len() < max_reactions && buckets.iter().any(|bucket| index < bucket.len()) {
            for bucket in &buckets {
                if let Some(key) = bucket.get(index)
                    && !mixed.contains(key)
                {
                    mixed.push(key.clone());
                }
            }
            index += 1;
        }
        // 打ち切られた一覧の先頭のぶんは、残りの枠へ入れる。
        for key in keys {
            if !mixed.contains(&key) {
                mixed.push(key);
            }
        }
        keys = mixed;
    }
    let mut hydrated = 0usize;
    for key in keys.into_iter().take(max_reactions) {
        hydrated += hydrate_reaction_cache_from_key(
            docs_sync,
            projection_store,
            topic_id,
            replica,
            key.envelope_key().as_str(),
            policy,
        )
        .await? as usize;
    }
    Ok(hydrated)
}
