use super::game_projection_support::hydrate_game_room_from_record;
use super::hydration_limits::{MissingBodyLedger, ReplicaScanCache, scan_fingerprint};
use super::*;

/// 反映済みの行を、取り下げ済みの投稿として伏せる。`VerifiedPost::withdrawn` から作る行と同じ形にする。
fn scrub_withdrawn_row(mut row: ObjectProjectionRow) -> ObjectProjectionRow {
    row.payload_ref = PayloadRef::InlineText {
        text: String::new(),
    };
    row.content = Some(String::new());
    row.attachments.clear();
    row.repost_of = None;
    row.source_blob_hash = None;
    row.derived_at = Utc::now().timestamp_millis();
    row
}

/// `withdrawals/<object id>/state` の record を 1 件反映した結果(#1239)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PostWithdrawalHydration {
    /// 検証できた取り下げを projection へ反映した。
    Applied,
    /// 対象の envelope がまだ手元に無く、検証できない。対象が届いたときに反映し直す。
    TargetMissing,
    /// 取り下げとして読めない、または署名・著者が対象と合わない。取り下げとして扱わない。
    Invalid,
}

impl PostWithdrawalHydration {
    pub(crate) fn applied(self) -> bool {
        self == Self::Applied
    }
}

/// 取り下げの record を検証して projection へ反映する。
///
/// 読めない record と検証できない record は `Invalid` を返し、エラーにしない。public topic の replica は
/// 誰でも書けるので、読めない record を 1 件置くだけで、投稿の反映や topic 全体の操作を止められないようにする
/// (ADR 0052 §2)。docs と projection の読み書きの失敗はエラーとして返す。
pub(crate) async fn hydrate_post_withdrawal_from_record(
    docs_sync: &dyn DocsSync,
    projection_store: &dyn ProjectionStore,
    replica: &ReplicaId,
    record: DocRecord,
    policy: DocFetchPolicy,
) -> Result<PostWithdrawalHydration> {
    let invalid = |reason: &str, error: &dyn std::fmt::Display| {
        warn!(
            replica = %replica.as_str(),
            key = %record.key,
            reason,
            error = %error,
            "ignored a post withdrawal record that cannot be verified"
        );
        PostWithdrawalHydration::Invalid
    };
    let envelope: KukuriEnvelope = match serde_json::from_slice(&record.value) {
        Ok(envelope) => envelope,
        Err(error) => return Ok(invalid("the record is not an envelope", &error)),
    };
    let content = match envelope.post_withdrawal_content() {
        Ok(Some(content)) => content,
        Ok(None) => {
            return Ok(invalid(
                "the envelope is not a post withdrawal",
                &"kind mismatch",
            ));
        }
        Err(error) => return Ok(invalid("the withdrawal content is malformed", &error)),
    };
    if let Err(error) = envelope.verify() {
        return Ok(invalid("the withdrawal signature is invalid", &error));
    }
    // 同じ key には docs author ごとの record がありうる。先頭の 1 件だけを見ず、上限つきで対象を探す(#1248)。
    let target_records = docs_sync
        .query_replica_exact_bounded(
            replica,
            post_envelope_key(&content.target_object_id).as_str(),
            MAX_ENVELOPE_RECORDS_PER_OBJECT,
            policy,
        )
        .await?;
    let withdrawal = match verify_withdrawal_against_records(
        &envelope,
        &content.target_object_id,
        &target_records,
    ) {
        WithdrawalTargetCheck::Verified(withdrawal) => *withdrawal,
        WithdrawalTargetCheck::Mismatch(error) => {
            return Ok(invalid("the withdrawal does not match the target", &error));
        }
        // 対象として読める envelope がまだ無い。対象が届いたときに反映し直す。
        WithdrawalTargetCheck::TargetMissing => {
            return Ok(PostWithdrawalHydration::TargetMissing);
        }
    };
    projection_store
        .put_post_withdrawal(post_withdrawal_row(withdrawal, replica))
        .await?;

    // 反映済みの行を伏せる。行がまだ無ければ、投稿を反映する時点で伏せた行ができる
    // (`hydrate_object_in_topic` と全件走査は、取り下げを先に確認する)。docs の `state` の値は使わない(#1248)。
    if let Some(row) = projection_store
        .get_object_projection(&content.target_object_id)
        .await?
    {
        projection_store
            .put_object_projection(scrub_withdrawn_row(row))
            .await?;
    }
    Ok(PostWithdrawalHydration::Applied)
}

/// 戻り値は今回反映した取り下げの件数。前回の走査から record が変わっていなければ何もせず 0 を返す(#1225)。
pub(crate) async fn hydrate_post_withdrawals_from_replica(
    docs_sync: &dyn DocsSync,
    projection_store: &dyn ProjectionStore,
    scan_cache: &ReplicaScanCache,
    replica: &ReplicaId,
    policy: DocFetchPolicy,
) -> Result<usize> {
    const PREFIX: &str = "withdrawals/";
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
    let mut hydrated = 0usize;
    let mut complete = true;
    for record in records {
        if !record.key.ends_with("/state") {
            continue;
        }
        match hydrate_post_withdrawal_from_record(
            docs_sync,
            projection_store,
            replica,
            record,
            policy,
        )
        .await?
        {
            PostWithdrawalHydration::Applied => hydrated += 1,
            // 対象の投稿がまだ届いていない。次の走査でやり直す。
            PostWithdrawalHydration::TargetMissing => complete = false,
            // 取り下げとして扱えない record は、走査をやり直しても変わらない。
            PostWithdrawalHydration::Invalid => {}
        }
    }
    if complete {
        scan_cache.record(replica.as_str(), PREFIX, fingerprint);
    } else {
        scan_cache.forget(replica.as_str(), PREFIX);
    }
    Ok(hydrated)
}

/// 戻り値は今回反映した投稿の件数。record が前回の走査と同じで、取り下げにも変化が無ければ 0 を返す(#1225)。
///
/// 取得できない本文 blob は走査のたびに取りに行かず、`MissingBodyLedger` の間隔と回数に従う。
///
/// 行は署名つき envelope(`objects/<id>/envelope`)から作る。検証に通らない object は warn を出して飛ばし、
/// 走査は続ける(#1248)。envelope の record は同じ prefix の読み出しに含まれるので、追加の読み出しは無い。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn hydrate_object_projection_from_replica(
    docs_sync: &dyn DocsSync,
    blob_service: &dyn BlobService,
    projection_store: &dyn ProjectionStore,
    scan_cache: &ReplicaScanCache,
    missing_bodies: &MissingBodyLedger,
    topic_id: &str,
    replica: &ReplicaId,
    policy: DocFetchPolicy,
    withdrawals_changed: bool,
) -> Result<usize> {
    const PREFIX: &str = "objects/";
    let records = query_replica_with_fetch_policy(
        docs_sync,
        replica,
        DocQuery::Prefix(PREFIX.into()),
        policy,
    )
    .await?;
    let fingerprint = scan_fingerprint(&records);
    if !withdrawals_changed && scan_cache.is_unchanged(replica.as_str(), PREFIX, fingerprint) {
        return Ok(0);
    }
    let Some(scope) = ReplicaPostScope::for_replica(replica, topic_id) else {
        // 投稿を置く replica ではない。何も反映しない。
        scan_cache.record(replica.as_str(), PREFIX, fingerprint);
        return Ok(0);
    };
    // object ごとに envelope の record をまとめる(同じ key には docs author ごとの record がありうる)。
    let mut envelope_records: BTreeMap<EnvelopeId, Vec<&DocRecord>> = BTreeMap::new();
    for record in &records {
        if record.key.ends_with("/envelope")
            && let Some(object_id) = object_id_from_post_key(record.key.as_str())
        {
            envelope_records.entry(object_id).or_default().push(record);
        }
    }
    let mut hydrated = 0usize;
    let mut blob_statuses = Vec::new();
    let mut projections = Vec::new();
    for (object_id, candidates) in envelope_records {
        let post = match select_verified_post(candidates, &object_id, replica, &scope) {
            Ok(Some(post)) => post,
            Ok(None) => continue,
            Err(reason) => {
                warn_rejected_post(replica, &object_id, reason);
                continue;
            }
        };
        if projection_store
            .get_post_withdrawal(&object_id)
            .await?
            .is_some()
        {
            projections.push(projection_row_from_post(
                &post.withdrawn(),
                Some(String::new()),
            ));
            hydrated += 1;
            continue;
        }
        let header = post.header();
        let content = match &header.payload_ref {
            PayloadRef::InlineText { text } => Some(text.clone()),
            PayloadRef::BlobText { hash, .. } => {
                let payload =
                    fetch_projection_blob_text_bounded(blob_service, missing_bodies, hash).await;
                blob_statuses.push((
                    hash.clone(),
                    match payload {
                        Some(_) => BlobCacheStatus::Available,
                        None => BlobCacheStatus::Missing,
                    },
                ));
                payload
            }
        };
        for attachment in &header.attachments {
            let status = best_effort_blob_cache_status(blob_service, &attachment.hash).await;
            blob_statuses.push((attachment.hash.clone(), status));
        }
        projections.push(projection_row_from_post(&post, content));
        hydrated += 1;
    }
    projection_store.mark_blob_statuses(blob_statuses).await?;
    projection_store.put_object_projections(projections).await?;
    // 本文が欠けた行は行単位の取り直し(`MissingBodyLedger`)が担うので、走査としては完了とみなす。
    scan_cache.record(replica.as_str(), PREFIX, fingerprint);
    Ok(hydrated)
}

/// 走査中の本文取得。local にあれば読み、無ければ台帳の間隔と回数の内でだけ remote を試す。
pub(crate) async fn fetch_projection_blob_text_bounded(
    blob_service: &dyn BlobService,
    missing_bodies: &MissingBodyLedger,
    hash: &kukuri_core::BlobHash,
) -> Option<String> {
    let local = matches!(
        best_effort_blob_cache_status(blob_service, hash).await,
        BlobCacheStatus::Available | BlobCacheStatus::Pinned
    );
    // 取得を待つ間に走査が abort されても、`attempt` の drop で失敗として記録される。
    let attempt = if local {
        None
    } else {
        Some(missing_bodies.try_begin(hash, Utc::now().timestamp_millis())?)
    };
    let payload = fetch_projection_blob_text(blob_service, hash).await;
    match (payload.is_some(), attempt) {
        (true, Some(attempt)) => attempt.succeed(),
        (true, None) => missing_bodies.forget(hash),
        (false, Some(attempt)) => attempt.fail(),
        (false, None) => {}
    }
    payload
}

/// 検証済みの投稿を 1 件、projection へ反映する。
pub(crate) async fn hydrate_object_projection_from_post(
    blob_service: &dyn BlobService,
    projection_store: &dyn ProjectionStore,
    post: VerifiedPost,
) -> Result<bool> {
    if projection_store
        .get_post_withdrawal(&post.header().object_id)
        .await?
        .is_some()
    {
        projection_store
            .put_object_projection(projection_row_from_post(
                &post.withdrawn(),
                Some(String::new()),
            ))
            .await?;
        return Ok(true);
    }
    let header = post.header();
    let content = match &header.payload_ref {
        PayloadRef::InlineText { text } => Some(text.clone()),
        PayloadRef::BlobText { hash, .. } => {
            let payload = fetch_projection_blob_text(blob_service, hash).await;
            projection_store
                .mark_blob_status(
                    hash,
                    match payload {
                        Some(_) => BlobCacheStatus::Available,
                        None => BlobCacheStatus::Missing,
                    },
                )
                .await?;
            payload
        }
    };
    for attachment in &header.attachments {
        let status = best_effort_blob_cache_status(blob_service, &attachment.hash).await;
        projection_store
            .mark_blob_status(&attachment.hash, status)
            .await?;
    }
    projection_store
        .put_object_projection(projection_row_from_post(&post, content))
        .await?;
    Ok(true)
}

/// object id を 1 つ指定して、その投稿を projection へ反映する(#1239)。replica は走査しない。
///
/// 取り下げを先に反映する。投稿の反映は projection の取り下げ表を見て本文と添付を伏せるため、
/// この順にすると、取り下げより後に反映した投稿でも本文が残らない。取り下げの event が対象の
/// envelope より先に届いて反映できなかった場合も、投稿を反映するときにここで取り直す。
///
/// `policy` は 2 つの key の読み出しに使う。利用者の操作は `LocalOnly`(操作を remote 取得で待たせない)、
/// event・hint・repost 元の解決は `LocalThenRemote`。
///
/// 行は署名つき envelope から作る(#1248)。`topic_id` は、その replica を読む文脈の topic(private channel の
/// replica id は topic を含まない)。検証に通る envelope が無ければ `false` を返す。envelope が後から届いた場合は、
/// その event でもう一度呼ばれる。読む docs の record は、取り下げの key と envelope の key だけ。
pub(crate) async fn hydrate_object_in_topic(
    services: &ServiceHandles,
    topic_id: &str,
    replica: &ReplicaId,
    object_id: &EnvelopeId,
    policy: DocFetchPolicy,
) -> Result<bool> {
    let docs_sync = services.docs_sync.as_ref();
    let projection_store = services.projection_store.as_ref();
    let withdrawal_key = stable_key("withdrawals", &format!("{}/state", object_id.as_str()));
    if let Some(record) =
        query_replica_with_fetch_policy(docs_sync, replica, DocQuery::Exact(withdrawal_key), policy)
            .await?
            .into_iter()
            .next()
    {
        // 反映できなかった取り下げ(検証できない、対象が未着)は、投稿の反映を止めない。
        hydrate_post_withdrawal_from_record(docs_sync, projection_store, replica, record, policy)
            .await?;
    }
    let Some(post) = load_verified_post(docs_sync, replica, topic_id, object_id, policy).await?
    else {
        return Ok(false);
    };
    hydrate_object_projection_from_post(services.blob_service.as_ref(), projection_store, post)
        .await
}

pub(crate) async fn hydrate_reaction_cache_from_replica(
    docs_sync: &dyn DocsSync,
    projection_store: &dyn ProjectionStore,
    scan_cache: &ReplicaScanCache,
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
    let mut hydrated = 0usize;
    for record in records {
        if !record.key.ends_with("/state") {
            continue;
        }
        let reaction: ReactionDocV1 = serde_json::from_slice(record.value.as_slice())?;
        projection_store
            .upsert_reaction_cache(reaction_projection_row_from_doc(&reaction, replica))
            .await?;
        hydrated += 1;
    }
    scan_cache.record(replica.as_str(), PREFIX, fingerprint);
    Ok(hydrated)
}

pub(crate) async fn hydrate_reaction_cache_from_record(
    projection_store: &dyn ProjectionStore,
    replica: &ReplicaId,
    record: DocRecord,
) -> Result<bool> {
    let reaction: ReactionDocV1 = serde_json::from_slice(record.value.as_slice())?;
    projection_store
        .upsert_reaction_cache(reaction_projection_row_from_doc(&reaction, replica))
        .await?;
    Ok(true)
}

pub(crate) async fn hydrate_reaction_cache_from_key(
    docs_sync: &dyn DocsSync,
    projection_store: &dyn ProjectionStore,
    replica: &ReplicaId,
    key: &str,
    policy: DocFetchPolicy,
) -> Result<bool> {
    let Some(record) = query_replica_with_fetch_policy(
        docs_sync,
        replica,
        DocQuery::Exact(key.to_string()),
        policy,
    )
    .await?
    .into_iter()
    .next() else {
        return Ok(false);
    };
    hydrate_reaction_cache_from_record(projection_store, replica, record).await
}

pub(crate) async fn hydrate_reaction_cache_for_target(
    docs_sync: &dyn DocsSync,
    projection_store: &dyn ProjectionStore,
    replica: &ReplicaId,
    target_object_id: &str,
) -> Result<usize> {
    let records = docs_sync
        .query_replica(
            replica,
            DocQuery::Prefix(stable_key("reactions", &format!("{target_object_id}/"))),
        )
        .await?;
    let mut hydrated = 0usize;
    for record in records {
        if !record.key.ends_with("/state") {
            continue;
        }
        hydrated +=
            hydrate_reaction_cache_from_record(projection_store, replica, record).await? as usize;
    }
    Ok(hydrated)
}

pub(crate) async fn hydrate_topic_state(
    services: &ServiceHandles,
    topic_id: &str,
    policy: DocFetchPolicy,
) -> Result<usize> {
    Box::pin(hydrate_subscription_state(
        services,
        topic_id,
        &topic_replica_id(topic_id),
        policy,
    ))
    .await
}

/// replica の全件走査。戻り値は今回反映した件数で、replica に変化が無ければ 0 になる(#1225)。
/// caller はこの値を「進展があったか」の判定に使う。
pub(crate) async fn hydrate_subscription_state(
    services: &ServiceHandles,
    topic_id: &str,
    replica: &ReplicaId,
    policy: DocFetchPolicy,
) -> Result<usize> {
    let docs_sync = services.docs_sync.as_ref();
    let blob_service = services.blob_service.as_ref();
    let projection_store = services.projection_store.as_ref();
    let scan_cache = services.replica_scan_cache.as_ref();
    let withdrawal_count = hydrate_post_withdrawals_from_replica(
        docs_sync,
        projection_store,
        scan_cache,
        replica,
        policy,
    )
    .await?;
    let post_count = Box::pin(hydrate_object_projection_from_replica(
        docs_sync,
        blob_service,
        projection_store,
        scan_cache,
        services.missing_body_ledger.as_ref(),
        topic_id,
        replica,
        policy,
        withdrawal_count > 0,
    ))
    .await?;
    let reaction_count = hydrate_reaction_cache_from_replica(
        docs_sync,
        projection_store,
        scan_cache,
        replica,
        policy,
    )
    .await?;
    let (live_count, live_changed) = hydrate_live_sessions_from_replica(
        docs_sync,
        blob_service,
        projection_store,
        scan_cache,
        topic_id,
        replica,
        policy,
    )
    .await?;
    let (game_count, game_changed) =
        hydrate_game_rooms_from_replica_tracked(services, topic_id, replica, policy).await?;
    // live / game は毎回反映し直すので、record に変化があったときだけ「進展」に数える。
    Ok(withdrawal_count
        + post_count
        + reaction_count
        + if live_changed { live_count } else { 0 }
        + if game_changed { game_count } else { 0 })
}

pub(crate) async fn hydrate_live_sessions_from_replica(
    docs_sync: &dyn DocsSync,
    blob_service: &dyn BlobService,
    projection_store: &dyn ProjectionStore,
    scan_cache: &ReplicaScanCache,
    topic_id: &str,
    replica: &ReplicaId,
    policy: DocFetchPolicy,
) -> Result<(usize, bool)> {
    const PREFIX: &str = "sessions/live/";
    let records = query_replica_with_fetch_policy(
        docs_sync,
        replica,
        DocQuery::Prefix(PREFIX.into()),
        policy,
    )
    .await?;
    // live / game は件数が少なく、manifest の到着や参加状態で反映結果が変わるため、指紋による省略をしない。
    // 指紋は「record に変化があったか」を caller へ返すためだけに使う。
    let changed = scan_cache.observe(replica.as_str(), PREFIX, scan_fingerprint(&records));
    let mut hydrated = 0usize;
    for record in records {
        let state: LiveSessionStateDocV1 = serde_json::from_slice(&record.value)?;
        projection_store
            .mark_blob_status(
                &state.current_manifest.hash,
                blob_status(
                    blob_service
                        .blob_status(&state.current_manifest.hash)
                        .await?,
                ),
            )
            .await?;
        let Some(manifest) =
            fetch_manifest_blob::<LiveSessionManifestBlobV1>(blob_service, &state.current_manifest)
                .await?
        else {
            continue;
        };
        projection_store
            .upsert_live_session_cache(live_projection_row_from_state(
                &state, &manifest, topic_id, replica,
            ))
            .await?;
        hydrated += 1;
    }
    Ok((hydrated, changed))
}

pub(crate) async fn hydrate_live_session_from_record(
    blob_service: &dyn BlobService,
    projection_store: &dyn ProjectionStore,
    topic_id: &str,
    replica: &ReplicaId,
    record: DocRecord,
) -> Result<bool> {
    let state: LiveSessionStateDocV1 = serde_json::from_slice(&record.value)?;
    projection_store
        .mark_blob_status(
            &state.current_manifest.hash,
            blob_status(
                blob_service
                    .blob_status(&state.current_manifest.hash)
                    .await?,
            ),
        )
        .await?;
    let Some(manifest) =
        fetch_manifest_blob::<LiveSessionManifestBlobV1>(blob_service, &state.current_manifest)
            .await?
    else {
        return Ok(false);
    };
    projection_store
        .upsert_live_session_cache(live_projection_row_from_state(
            &state, &manifest, topic_id, replica,
        ))
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
    let Some(record) = docs_sync
        .query_replica(replica, DocQuery::Exact(key.to_string()))
        .await?
        .into_iter()
        .next()
    else {
        return Ok(false);
    };
    hydrate_live_session_from_record(blob_service, projection_store, topic_id, replica, record)
        .await
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

#[cfg(test)]
pub(crate) async fn hydrate_game_rooms_from_replica(
    services: &ServiceHandles,
    topic_id: &str,
    replica: &ReplicaId,
    policy: DocFetchPolicy,
) -> Result<usize> {
    hydrate_game_rooms_from_replica_tracked(services, topic_id, replica, policy)
        .await
        .map(|(hydrated, _)| hydrated)
}

/// 反映した件数と、前回の走査から record に変化があったかを返す。
async fn hydrate_game_rooms_from_replica_tracked(
    services: &ServiceHandles,
    topic_id: &str,
    replica: &ReplicaId,
    policy: DocFetchPolicy,
) -> Result<(usize, bool)> {
    const PREFIX: &str = "sessions/game/";
    let records = query_replica_with_fetch_policy(
        services.docs_sync.as_ref(),
        replica,
        DocQuery::Prefix(PREFIX.into()),
        policy,
    )
    .await?;
    let changed =
        services
            .replica_scan_cache
            .observe(replica.as_str(), PREFIX, scan_fingerprint(&records));
    let mut hydrated = 0usize;
    for record in records {
        hydrated +=
            usize::from(hydrate_game_room_from_record(services, topic_id, replica, record).await?);
    }
    Ok((hydrated, changed))
}

pub(crate) async fn hydrate_game_room_from_key(
    services: &ServiceHandles,
    topic_id: &str,
    replica: &ReplicaId,
    key: &str,
) -> Result<bool> {
    let Some(record) = services
        .docs_sync
        .query_replica(replica, DocQuery::Exact(key.to_string()))
        .await?
        .into_iter()
        .next()
    else {
        return Ok(false);
    };
    hydrate_game_room_from_record(services, topic_id, replica, record).await
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

pub(crate) async fn hydrate_subscription_event(
    services: &ServiceHandles,
    topic_id: &str,
    replica: &ReplicaId,
    key: &str,
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
        return Ok(hydrate_object_in_topic(
            services,
            topic_id,
            replica,
            &object_id,
            DocFetchPolicy::LocalThenRemote,
        )
        .await? as usize);
    }
    if key.starts_with("reactions/") && key.ends_with("/state") {
        return Ok(hydrate_reaction_cache_from_key(
            docs_sync,
            projection_store,
            replica,
            key,
            DocFetchPolicy::LocalThenRemote,
        )
        .await? as usize);
    }
    // #1239: 取り下げの event も key 単位で反映する(以前は全件走査か hint まで反映されなかった)。
    if key.starts_with("withdrawals/") && key.ends_with("/state") {
        let Some(record) = docs_sync
            .query_replica(replica, DocQuery::Exact(key.to_string()))
            .await?
            .into_iter()
            .next()
        else {
            return Ok(0);
        };
        return Ok(hydrate_post_withdrawal_from_record(
            docs_sync,
            projection_store,
            replica,
            record,
            DocFetchPolicy::LocalThenRemote,
        )
        .await?
        .applied() as usize);
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
                    let key = stable_key(
                        "withdrawals",
                        &format!("{}/state", object.object_id.as_str()),
                    );
                    if let Some(record) = docs_sync
                        .query_replica(replica, DocQuery::Exact(key))
                        .await?
                        .into_iter()
                        .next()
                    {
                        hydrated += hydrate_post_withdrawal_from_record(
                            docs_sync,
                            projection_store,
                            replica,
                            record,
                            DocFetchPolicy::LocalThenRemote,
                        )
                        .await?
                        .applied() as usize;
                    }
                    continue;
                }
                if object.object_kind == "reaction" {
                    hydrated += hydrate_reaction_cache_for_target(
                        docs_sync,
                        projection_store,
                        replica,
                        object.object_id.as_str(),
                    )
                    .await?;
                    continue;
                }
                hydrated += hydrate_object_in_topic(
                    services,
                    topic_id,
                    replica,
                    &EnvelopeId::from(object.object_id.as_str()),
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
