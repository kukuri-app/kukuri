use super::game_projection_support::hydrate_game_room_from_record;
use super::*;

/// `sessions/live/<id>/state` の record を 1 件反映する。
///
/// 行は、owner が署名した manifest と、読んだ replica の topic / channel に照らして確かめた session から作る(#1252)。
/// 検証に通らない record は warn を出して `false` を返し、エラーにしない。
pub(crate) async fn hydrate_live_session_from_record(
    docs_sync: &dyn DocsSync,
    blob_service: &dyn BlobService,
    projection_store: &dyn ProjectionStore,
    topic_id: &str,
    replica: &ReplicaId,
    record: DocRecord,
    policy: DocFetchPolicy,
) -> Result<bool> {
    let Some(verified) =
        verify_live_session_record(docs_sync, blob_service, replica, topic_id, &record, policy)
            .await?
    else {
        return Ok(false);
    };
    hydrate_verified_live_session(projection_store, &verified).await
}

async fn hydrate_verified_live_session(
    projection_store: &dyn ProjectionStore,
    verified: &VerifiedLiveSession,
) -> Result<bool> {
    projection_store
        .mark_blob_status(
            &verified.state().current_manifest.hash,
            BlobCacheStatus::Available,
        )
        .await?;
    projection_store
        .upsert_live_session_cache(live_projection_row(verified))
        .await?;
    Ok(true)
}

pub(crate) async fn hydrate_live_session_from_key(
    docs_sync: &dyn DocsSync,
    blob_service: &dyn BlobService,
    projection_store: &dyn ProjectionStore,
    topic_id: &str,
    replica: &ReplicaId,
    key: &str,
) -> Result<bool> {
    let Some(session_id) = key
        .strip_prefix("sessions/live/")
        .and_then(|rest| rest.strip_suffix("/state"))
    else {
        return Ok(false);
    };
    let Some(verified) = load_verified_live_session(
        docs_sync,
        blob_service,
        replica,
        topic_id,
        session_id,
        DocFetchPolicy::LocalThenRemote,
    )
    .await?
    else {
        return Ok(false);
    };
    hydrate_verified_live_session(projection_store, &verified).await
}

pub(crate) async fn hydrate_live_session_from_key_with_retry(
    docs_sync: &dyn DocsSync,
    blob_service: &dyn BlobService,
    projection_store: &dyn ProjectionStore,
    topic_id: &str,
    replica: &ReplicaId,
    key: &str,
) -> Result<usize> {
    for attempt in 0..session_projection_retry_attempts() {
        if hydrate_live_session_from_key(
            docs_sync,
            blob_service,
            projection_store,
            topic_id,
            replica,
            key,
        )
        .await?
        {
            return Ok(1);
        }
        if attempt + 1 < session_projection_retry_attempts() {
            tokio::time::sleep(session_projection_retry_delay()).await;
        }
    }
    Ok(0)
}

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

pub(crate) async fn hydrate_game_room_from_key_with_retry(
    services: &ServiceHandles,
    topic_id: &str,
    replica: &ReplicaId,
    key: &str,
) -> Result<usize> {
    for attempt in 0..session_projection_retry_attempts() {
        if hydrate_game_room_from_key(services, topic_id, replica, key).await? {
            return Ok(1);
        }
        if attempt + 1 < session_projection_retry_attempts() {
            tokio::time::sleep(session_projection_retry_delay()).await;
        }
    }
    Ok(0)
}

/// key だけを指定する形(test 用)。本番の購読は、event の docs author を渡す `hydrate_subscription_doc_event` を使う。
#[cfg(test)]
pub(crate) async fn hydrate_subscription_event(
    services: &ServiceHandles,
    topic_id: &str,
    replica: &ReplicaId,
    key: &str,
) -> Result<usize> {
    hydrate_doc_event_key(services, topic_id, replica, key, None).await
}

/// docs の event を 1 件、key 単位で反映する。`docs_author` は、その entry を書いた docs author(`DocEvent::docs_author`)。
/// 投稿と取り下げの読み出しで、読む record を選ぶ手がかりに使う(ADR 0053 §3)。
pub(crate) async fn hydrate_subscription_doc_event(
    services: &ServiceHandles,
    topic_id: &str,
    replica: &ReplicaId,
    event: &DocEvent,
) -> Result<usize> {
    let docs_author = event.docs_author.as_deref();
    hydrate_doc_event_key(services, topic_id, replica, event.key.as_str(), docs_author).await
}

async fn hydrate_doc_event_key(
    services: &ServiceHandles,
    topic_id: &str,
    replica: &ReplicaId,
    key: &str,
    docs_author: Option<&str>,
) -> Result<usize> {
    let docs_sync = services.docs_sync.as_ref();
    let blob_service = services.blob_service.as_ref();
    let projection_store = services.projection_store.as_ref();
    // `state` と `envelope` のどちらの event でも反映を試す(#1248)。行は envelope から作るので、`state` が先に
    // 届いた投稿は `envelope` の event で反映される。`envelope` の event は、行がまだ無いときだけ反映する。
    if let Some(object_id) = object_id_from_post_key(key) {
        if key.ends_with("/envelope")
            && projection_store
                .get_object_projection(&object_id)
                .await?
                .is_some()
        {
            return Ok(0);
        }
        return Ok(hydrate_object_in_topic_with_hint(
            services,
            topic_id,
            replica,
            &object_id,
            docs_author,
            DocFetchPolicy::LocalThenRemote,
        )
        .await? as usize);
    }
    // `state` と `envelope` のどちらの event でも反映を試す(#1252)。行は envelope から作る。
    if ReactionKey::from_doc_key(key).is_some() {
        return Ok(hydrate_reaction_cache_from_key(
            docs_sync,
            projection_store,
            topic_id,
            replica,
            key,
            DocFetchPolicy::LocalThenRemote,
        )
        .await? as usize);
    }
    // #1239: 取り下げの event も key 単位で反映する(以前は全件走査か hint まで反映されなかった)。
    if let Some(object_id) = object_id_from_post_withdrawal_key(key) {
        return Ok(hydrate_post_withdrawal_for_object_with_hints(
            docs_sync,
            projection_store,
            replica,
            &object_id,
            WithdrawalReadHints {
                target_docs_author: None,
                writer_docs_author: docs_author,
            },
            DocFetchPolicy::LocalThenRemote,
        )
        .await?
        .is_some_and(PostWithdrawalHydration::applied) as usize);
    }
    if key.starts_with("sessions/live/") && key.ends_with("/state") {
        return hydrate_live_session_from_key_with_retry(
            docs_sync,
            blob_service,
            projection_store,
            topic_id,
            replica,
            key,
        )
        .await;
    }
    if key.starts_with("sessions/game/") && key.ends_with("/state") {
        return hydrate_game_room_from_key_with_retry(services, topic_id, replica, key).await;
    }
    Ok(0)
}

/// replica の内容(投稿・thread・session)を指す hint か。それ以外の hint は、個別反映が 0 件でも
/// 全件走査の契機にしない(#1225)。
pub(crate) fn hint_refers_to_replica_content(hint: &GossipHint) -> bool {
    matches!(
        hint,
        GossipHint::TopicObjectsChanged { .. }
            | GossipHint::ThreadUpdated { .. }
            | GossipHint::SessionChanged { .. }
    )
}

pub(crate) async fn hydrate_subscription_hint(
    services: &ServiceHandles,
    topic_id: &str,
    replica: &ReplicaId,
    hint: &GossipHint,
) -> Result<usize> {
    let docs_sync = services.docs_sync.as_ref();
    let blob_service = services.blob_service.as_ref();
    let projection_store = services.projection_store.as_ref();
    match hint {
        GossipHint::TopicObjectsChanged { objects, .. } => {
            let mut hydrated = 0usize;
            for object in objects {
                if object.object_kind == "post_withdrawal" {
                    hydrated += hydrate_post_withdrawal_for_object_with_hints(
                        docs_sync,
                        projection_store,
                        replica,
                        &EnvelopeId::from(object.object_id.as_str()),
                        WithdrawalReadHints {
                            target_docs_author: None,
                            writer_docs_author: object.docs_author.as_deref(),
                        },
                        DocFetchPolicy::LocalThenRemote,
                    )
                    .await?
                    .is_some_and(PostWithdrawalHydration::applied)
                        as usize;
                    continue;
                }
                if object.object_kind == "reaction" {
                    // #1239: 対象の reaction の総数ぶんを読まない。上限つきで読む(その hint が指す reaction は、
                    // docs の event が key 単位で反映する)。
                    hydrated += hydrate_reaction_cache_for_target_bounded(
                        docs_sync,
                        projection_store,
                        topic_id,
                        replica,
                        &EnvelopeId::from(object.object_id.as_str()),
                        DocFetchPolicy::LocalThenRemote,
                        super::replica_window::RANGE_CHECK_REACTIONS_PER_OBJECT,
                    )
                    .await?;
                    continue;
                }
                hydrated += hydrate_object_in_topic_with_hint(
                    services,
                    topic_id,
                    replica,
                    &EnvelopeId::from(object.object_id.as_str()),
                    object.docs_author.as_deref(),
                    DocFetchPolicy::LocalThenRemote,
                )
                .await? as usize;
            }
            Ok(hydrated)
        }
        GossipHint::ThreadUpdated { object_ids, .. } => {
            let mut hydrated = 0usize;
            for object_id in object_ids {
                hydrated += hydrate_object_in_topic(
                    services,
                    topic_id,
                    replica,
                    object_id,
                    DocFetchPolicy::LocalThenRemote,
                )
                .await? as usize;
            }
            Ok(hydrated)
        }
        GossipHint::SessionChanged {
            session_id,
            object_kind,
            ..
        } => match object_kind.as_str() {
            "live-session" => {
                hydrate_live_session_from_key_with_retry(
                    docs_sync,
                    blob_service,
                    projection_store,
                    topic_id,
                    replica,
                    stable_key("sessions/live", &format!("{session_id}/state")).as_str(),
                )
                .await
            }
            "game-session" => {
                hydrate_game_room_from_key_with_retry(
                    services,
                    topic_id,
                    replica,
                    stable_key("sessions/game", &format!("{session_id}/state")).as_str(),
                )
                .await
            }
            _ => Ok(0),
        },
        GossipHint::ProfileUpdated { .. }
        | GossipHint::Presence { .. }
        | GossipHint::Typing { .. }
        | GossipHint::LivePresence { .. }
        | GossipHint::MetaverseRoomEvent { .. }
        | GossipHint::DomeHostHeartbeat { .. }
        | GossipHint::DirectMessageFrame { .. }
        | GossipHint::DirectMessageAck { .. } => Ok(0),
    }
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
    }
}

pub(crate) fn profile_timeline_page(
    posts: Vec<ProfileTimelineItem>,
    cursor: Option<TimelineCursor>,
    limit: usize,
) -> Page<ProfileTimelineItem> {
    if limit == 0 {
        return Page {
            items: Vec::new(),
            next_cursor: cursor,
        };
    }

    let mut items = Vec::new();
    let mut next_cursor = None;
    for post in posts {
        let include = cursor.as_ref().is_none_or(|current| {
            post.created_at() < current.created_at
                || (post.created_at() == current.created_at
                    && post.object_id() < &current.object_id)
        });
        if !include {
            continue;
        }
        if items.len() >= limit {
            next_cursor = Some(TimelineCursor {
                created_at: post.created_at(),
                object_id: post.object_id().clone(),
            });
            break;
        }
        items.push(post);
    }

    Page { items, next_cursor }
}
