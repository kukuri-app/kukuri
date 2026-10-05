use super::game_projection_support::hydrate_game_room_from_record;
use super::*;

/// `sessions/live/<id>/state` の record を 1 件反映する。
///
/// 行は、owner が署名した manifest と、読んだ replica の topic / channel に照らして確かめた session から作る(#1252)。
/// 検証に通らない record は warn を出して `false` を返し、エラーにしない。
pub(crate) async fn hydrate_live_session_from_record(
    services: &ServiceHandles,
    topic_id: &str,
    replica: &ReplicaId,
    record: DocRecord,
    policy: DocFetchPolicy,
) -> Result<bool> {
    let result = super::session_integrity::inspect_live_session_record(
        services.docs_sync.as_ref(),
        services.blob_service.as_ref(),
        replica,
        topic_id,
        &record,
        policy,
    )
    .await?;
    let Some(verified) = result.verified() else {
        return Ok(false);
    };
    let _guard = services
        .live_session_projections
        .lock(&verified.state().session_id)
        .await;
    let current = services
        .docs_sync
        .query_replica_exact_bounded(
            replica,
            &record.key,
            MAX_ENVELOPE_RECORDS_PER_OBJECT,
            DocFetchPolicy::LocalOnly,
        )
        .await?;
    if !current
        .iter()
        .any(|candidate| candidate.value == record.value)
    {
        return Ok(false);
    }
    hydrate_verified_live_session(services.projection_store.as_ref(), &verified).await
}

async fn hydrate_verified_live_session(
    projection_store: &dyn ProjectionStore,
    verified: &VerifiedLiveSession,
) -> Result<bool> {
    projection_store
        .upsert_live_session_cache(live_projection_row(verified))
        .await?;
    Ok(true)
}

#[cfg(test)]
pub(crate) async fn hydrate_game_room_from_key(
    services: &ServiceHandles,
    topic_id: &str,
    replica: &ReplicaId,
    key: &str,
) -> Result<bool> {
    // 同じ key には docs author ごとの record がありうる。先頭の 1 件だけを見ない(#1252)。
    let records = services
        .docs_sync
        .query_replica_exact_bounded(
            replica,
            key,
            MAX_ENVELOPE_RECORDS_PER_OBJECT,
            DocFetchPolicy::LocalThenRemote,
        )
        .await?;
    let mut hydrated = false;
    for record in records {
        hydrated |= hydrate_game_room_from_record(services, topic_id, replica, record).await?;
    }
    Ok(hydrated)
}

pub(crate) fn hint_targets_topic(hint: &GossipHint, topic: &str) -> bool {
    match hint {
        GossipHint::TopicObjectsChanged { topic_id, .. }
        | GossipHint::Presence { topic_id, .. }
        | GossipHint::Typing { topic_id, .. }
        | GossipHint::SessionChanged { topic_id, .. }
        | GossipHint::LivePresence { topic_id, .. }
        | GossipHint::MetaverseRoomEvent { topic_id, .. }
        | GossipHint::DomeHostHeartbeat { topic_id, .. }
        | GossipHint::DirectMessageFrame { topic_id, .. }
        | GossipHint::DirectMessageAck { topic_id, .. } => topic_id.as_str() == topic,
        GossipHint::ThreadUpdated { .. } | GossipHint::ProfileUpdated { .. } => true,
        GossipHint::AccountSyncChanged { .. } => false,
    }
}

/// 一度だけ局所反映する。manifest取得は表示要求が所有する。
pub(crate) async fn hydrate_session_key(
    services: &ServiceHandles,
    topic: &str,
    replica: &ReplicaId,
    key: &str,
) -> Result<usize> {
    hydrate_session_key_for_fetch(services, topic, replica, key, None, None).await
}

pub(crate) async fn hydrate_session_key_for_fetch(
    services: &ServiceHandles,
    topic: &str,
    replica: &ReplicaId,
    key: &str,
    worker: Option<u64>,
    expected_hash: Option<&str>,
) -> Result<usize> {
    if !is_session_state_key(key) {
        return Ok(0);
    }
    let records = services
        .docs_sync
        .query_replica_exact_bounded(
            replica,
            key,
            MAX_ENVELOPE_RECORDS_PER_OBJECT,
            DocFetchPolicy::LocalOnly,
        )
        .await?;
    if records.is_empty()
        || expected_hash.is_some_and(|hash| !records.iter().any(|r| r.content_hash == hash))
    {
        services
            .session_projections
            .defer_entry(topic, replica, key, expected_hash)
            .await;
    }
    let mut applied = 0;
    let mut missing = Vec::new();
    let mut newest_live: Option<(i64, DocRecord)> = None;
    use super::session_integrity::{
        SessionRead, inspect_game_room_record, inspect_live_session_record,
    };
    for record in records {
        if key.starts_with("sessions/live/") {
            match inspect_live_session_record(
                services.docs_sync.as_ref(),
                services.blob_service.as_ref(),
                replica,
                topic,
                &record,
                DocFetchPolicy::LocalOnly,
            )
            .await?
            {
                SessionRead::MissingManifest(hash) => {
                    if !missing.contains(&hash) {
                        missing.push(hash);
                    }
                }
                SessionRead::Ready(verified)
                    if newest_live
                        .as_ref()
                        .is_none_or(|(revision, _)| *revision < verified.revision()) =>
                {
                    newest_live = Some((verified.revision(), record));
                }
                _ => {}
            }
        } else {
            match inspect_game_room_record(
                services.docs_sync.as_ref(),
                services.blob_service.as_ref(),
                replica,
                topic,
                &record,
                DocFetchPolicy::LocalOnly,
            )
            .await?
            {
                SessionRead::MissingManifest(hash) => {
                    if !missing.contains(&hash) {
                        missing.push(hash);
                    }
                }
                SessionRead::Ready(_) => {
                    applied += hydrate_game_room_from_record(services, topic, replica, record)
                        .await? as usize;
                }
                _ => {}
            }
        }
    }
    if let Some((_, record)) = newest_live {
        applied += hydrate_live_session_from_record(
            services,
            topic,
            replica,
            record,
            DocFetchPolicy::LocalOnly,
        )
        .await? as usize;
    }
    services
        .session_projections
        .observe_key(topic, replica, key, missing, worker)
        .await;
    Ok(applied)
}

pub(crate) fn is_session_state_key(key: &str) -> bool {
    ["sessions/live/", "sessions/game/"].iter().any(|prefix| {
        key.strip_prefix(prefix)
            .and_then(|rest| rest.strip_suffix("/state"))
            .is_some_and(|id| !id.is_empty() && !id.contains('/'))
    })
}
