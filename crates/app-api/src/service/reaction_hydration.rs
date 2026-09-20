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

pub(crate) async fn hydrate_reaction_cache_for_target(
    docs_sync: &dyn DocsSync,
    projection_store: &dyn ProjectionStore,
    topic_id: &str,
    replica: &ReplicaId,
    target_object_id: &str,
) -> Result<usize> {
    let records = docs_sync
        .query_replica(
            replica,
            DocQuery::Prefix(stable_key("reactions", &format!("{target_object_id}/"))),
        )
        .await?;
    hydrate_reactions_from_records(projection_store, topic_id, replica, &records).await
}
