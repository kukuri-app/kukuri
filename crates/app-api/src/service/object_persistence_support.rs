use super::*;

pub(crate) async fn persist_post_object(
    docs_sync: &dyn DocsSync,
    replica: &ReplicaId,
    object: CanonicalPostHeader,
    envelope: KukuriEnvelope,
) -> Result<()> {
    let sort_key = timeline_sort_key(object.created_at, &object.object_id);
    let object_json = serde_json::to_value(&object)?;
    docs_sync.open_replica(replica).await?;
    docs_sync
        .apply_doc_op(
            replica,
            DocOp::SetJson {
                key: stable_key("objects", &format!("{}/state", object.object_id.as_str())),
                value: object_json,
            },
        )
        .await?;
    docs_sync
        .apply_doc_op(
            replica,
            DocOp::SetJson {
                key: stable_key(
                    "objects",
                    &format!("{}/envelope", object.object_id.as_str()),
                ),
                value: serde_json::to_value(envelope)?,
            },
        )
        .await?;
    docs_sync
        .apply_doc_op(
            replica,
            DocOp::SetJson {
                key: stable_key(
                    "indexes/timeline",
                    &format!("{sort_key}/{}", object.object_id.as_str()),
                ),
                value: serde_json::json!({
                    "object_id": object.object_id,
                    "created_at": object.created_at,
                    "object_kind": object.object_kind,
                }),
            },
        )
        .await?;
    let root_id = object
        .root
        .clone()
        .unwrap_or_else(|| object.object_id.clone());
    docs_sync
        .apply_doc_op(
            replica,
            DocOp::SetJson {
                key: stable_key(
                    "indexes/thread",
                    &format!(
                        "{}/{sort_key}/{}",
                        root_id.as_str(),
                        object.object_id.as_str()
                    ),
                ),
                value: serde_json::json!({
                    "object_id": object.object_id,
                    "root_id": root_id,
                    "reply_to": object.reply_to,
                }),
            },
        )
        .await?;
    Ok(())
}

pub(crate) async fn persist_post_withdrawal(
    docs_sync: &dyn DocsSync,
    replica: &ReplicaId,
    target_object_id: &EnvelopeId,
    envelope: &KukuriEnvelope,
) -> Result<()> {
    docs_sync.open_replica(replica).await?;
    docs_sync
        .apply_doc_op(
            replica,
            DocOp::SetJson {
                key: stable_key(
                    "withdrawals",
                    &format!("{}/state", target_object_id.as_str()),
                ),
                value: serde_json::to_value(envelope)?,
            },
        )
        .await
}

pub(crate) fn post_withdrawal_row(
    withdrawal: PostWithdrawalV1,
    replica: &ReplicaId,
) -> PostWithdrawalRow {
    PostWithdrawalRow {
        target_object_id: withdrawal.target_object_id,
        target_author_pubkey: withdrawal.target_author.as_str().to_string(),
        source_replica_id: replica.clone(),
        withdrawal_envelope_id: withdrawal.withdrawal_envelope_id,
        withdrawn_at: withdrawal.withdrawn_at,
        generation: withdrawal.generation,
        replacement_object_id: withdrawal.replacement_object_id,
        reason_visibility: withdrawal.reason_visibility,
        reason: withdrawal.reason,
    }
}

pub(crate) async fn persist_media_manifest(
    replica: &ReplicaId,
    envelope: &KukuriEnvelope,
    manifest: &KukuriMediaManifestV1,
    docs_sync: &dyn DocsSync,
) -> Result<()> {
    docs_sync.open_replica(replica).await?;
    docs_sync
        .apply_doc_op(
            replica,
            DocOp::SetJson {
                key: stable_key(
                    "manifests/media",
                    &format!("{}/state", manifest.manifest_id),
                ),
                value: serde_json::to_value(manifest)?,
            },
        )
        .await?;
    docs_sync
        .apply_doc_op(
            replica,
            DocOp::SetJson {
                key: stable_key(
                    "manifests/media",
                    &format!("{}/envelope", manifest.manifest_id),
                ),
                value: serde_json::to_value(envelope)?,
            },
        )
        .await?;
    Ok(())
}

/// live session・game room の署名つき envelope を、state と同じ replica の `envelopes/<envelope id>` へ置く(#1252)。
pub(crate) async fn persist_session_envelope(
    docs_sync: &dyn DocsSync,
    replica: &ReplicaId,
    envelope: &KukuriEnvelope,
) -> Result<()> {
    docs_sync.open_replica(replica).await?;
    docs_sync
        .apply_doc_op(
            replica,
            DocOp::SetJson {
                key: stable_key("envelopes", envelope.id.as_str()),
                value: serde_json::to_value(envelope)?,
            },
        )
        .await
}

pub(crate) async fn persist_live_session_state(
    docs_sync: &dyn DocsSync,
    replica: &ReplicaId,
    state: &LiveSessionStateDocV1,
) -> Result<()> {
    docs_sync.open_replica(replica).await?;
    docs_sync
        .apply_doc_op(
            replica,
            DocOp::SetJson {
                key: stable_key("sessions/live", &format!("{}/state", state.session_id)),
                value: serde_json::to_value(state)?,
            },
        )
        .await?;
    Ok(())
}

pub(crate) async fn persist_game_room_state(
    docs_sync: &dyn DocsSync,
    replica: &ReplicaId,
    state: &GameRoomStateDocV1,
) -> Result<()> {
    docs_sync.open_replica(replica).await?;
    docs_sync
        .apply_doc_op(
            replica,
            DocOp::SetJson {
                key: stable_key("sessions/game", &format!("{}/state", state.room_id)),
                value: serde_json::to_value(state)?,
            },
        )
        .await?;
    Ok(())
}

pub(crate) async fn persist_private_channel_metadata(
    docs_sync: &dyn DocsSync,
    replica: &ReplicaId,
    metadata: &PrivateChannelMetadataDocV1,
) -> Result<()> {
    docs_sync.open_replica(replica).await?;
    docs_sync
        .apply_doc_op(
            replica,
            DocOp::SetJson {
                key: stable_key("channels", "metadata"),
                value: serde_json::to_value(metadata)?,
            },
        )
        .await?;
    docs_sync
        .apply_doc_op(
            replica,
            DocOp::SetJson {
                key: stable_key("channels", "topic"),
                value: serde_json::json!({ "topic_id": metadata.topic_id }),
            },
        )
        .await
}

pub(crate) async fn persist_private_channel_policy(
    docs_sync: &dyn DocsSync,
    keys: &KukuriKeys,
    policy: &PrivateChannelPolicyDocV1,
    replica: &ReplicaId,
) -> Result<()> {
    let envelope = build_private_channel_policy_envelope(keys, policy)?;
    docs_sync.open_replica(replica).await?;
    docs_sync
        .apply_doc_op(
            replica,
            DocOp::SetJson {
                key: stable_key("channels", "policy/envelope"),
                value: serde_json::to_value(envelope)?,
            },
        )
        .await
}

pub(crate) async fn persist_private_channel_participant(
    docs_sync: &dyn DocsSync,
    keys: &KukuriKeys,
    participant: &PrivateChannelParticipantDocV1,
    replica: &ReplicaId,
) -> Result<()> {
    let envelope = build_private_channel_participant_envelope(keys, participant)?;
    docs_sync.open_replica(replica).await?;
    docs_sync
        .apply_doc_op(
            replica,
            DocOp::SetJson {
                key: stable_key(
                    "channels/participants",
                    &format!("{}/envelope", participant.participant_pubkey.as_str()),
                ),
                value: serde_json::to_value(envelope)?,
            },
        )
        .await
}

pub(crate) async fn persist_private_channel_epoch_handoff_grant(
    docs_sync: &dyn DocsSync,
    keys: &KukuriKeys,
    grant: &PrivateChannelEpochHandoffGrantDocV1,
    replica: &ReplicaId,
) -> Result<()> {
    let envelope = build_private_channel_epoch_handoff_grant_envelope(keys, grant)?;
    docs_sync.open_replica(replica).await?;
    docs_sync
        .apply_doc_op(
            replica,
            DocOp::SetJson {
                key: stable_key(
                    "channels/rotation-grants",
                    &format!("{}/envelope", grant.recipient_pubkey.as_str()),
                ),
                value: serde_json::to_value(envelope)?,
            },
        )
        .await
}

pub(crate) async fn fetch_private_channel_metadata_from_replica(
    docs_sync: &dyn DocsSync,
    replica: &ReplicaId,
    policy: DocFetchPolicy,
) -> Result<Option<PrivateChannelMetadataDocV1>> {
    let Some(record) = query_replica_with_fetch_policy(
        docs_sync,
        replica,
        DocQuery::Exact(stable_key("channels", "metadata")),
        policy,
    )
    .await?
    .into_iter()
    .next() else {
        return Ok(None);
    };
    let mut metadata: PrivateChannelMetadataDocV1 = serde_json::from_slice(&record.value)?;
    if metadata.owner_pubkey.as_str().trim().is_empty() {
        metadata.owner_pubkey = metadata.creator_pubkey.clone();
    }
    Ok(Some(metadata))
}

pub(crate) async fn fetch_private_channel_policy_from_replica(
    docs_sync: &dyn DocsSync,
    replica: &ReplicaId,
    policy: DocFetchPolicy,
) -> Result<Option<PrivateChannelPolicyDocV1>> {
    let Some(record) = query_replica_with_fetch_policy(
        docs_sync,
        replica,
        DocQuery::Exact(stable_key("channels", "policy/envelope")),
        policy,
    )
    .await?
    .into_iter()
    .next() else {
        return Ok(None);
    };
    let envelope: KukuriEnvelope = serde_json::from_slice(&record.value)?;
    envelope.verify()?;
    parse_private_channel_policy(&envelope)
}

pub(crate) async fn fetch_private_channel_participants_from_replica(
    docs_sync: &dyn DocsSync,
    replica: &ReplicaId,
    policy: DocFetchPolicy,
) -> Result<Vec<PrivateChannelParticipantDocV1>> {
    let records = query_replica_with_fetch_policy(
        docs_sync,
        replica,
        DocQuery::Prefix(stable_key("channels/participants", "")),
        policy,
    )
    .await?;
    let mut items = Vec::new();
    for record in records {
        if !record.key.ends_with("/envelope") {
            continue;
        }
        let envelope: KukuriEnvelope = serde_json::from_slice(&record.value)?;
        envelope.verify()?;
        if let Some(participant) = parse_private_channel_participant(&envelope)? {
            items.push(participant);
        }
    }
    items.sort_by(|left, right| left.participant_pubkey.cmp(&right.participant_pubkey));
    Ok(items)
}

pub(crate) async fn fetch_private_channel_epoch_handoff_grant_from_replica(
    docs_sync: &dyn DocsSync,
    replica: &ReplicaId,
    recipient_pubkey: &str,
    policy: DocFetchPolicy,
) -> Result<Option<PrivateChannelEpochHandoffGrantDocV1>> {
    let Some(record) = query_replica_with_fetch_policy(
        docs_sync,
        replica,
        DocQuery::Exact(stable_key(
            "channels/rotation-grants",
            &format!("{recipient_pubkey}/envelope"),
        )),
        policy,
    )
    .await?
    .into_iter()
    .next() else {
        return Ok(None);
    };
    let envelope: KukuriEnvelope = serde_json::from_slice(&record.value)?;
    envelope.verify()?;
    parse_private_channel_epoch_handoff_grant(&envelope)
}

/// 参加 record を 1 件、key 指定で読む(#1221 R5-C)。署名者と record の参加者が一致するものだけを返す。
pub(crate) async fn fetch_private_channel_participant_from_replica(
    docs_sync: &dyn DocsSync,
    replica: &ReplicaId,
    participant_pubkey: &str,
    policy: DocFetchPolicy,
) -> Result<Option<PrivateChannelParticipantDocV1>> {
    let records = docs_sync
        .query_replica_exact_bounded(
            replica,
            stable_key(
                "channels/participants",
                &format!("{participant_pubkey}/envelope"),
            )
            .as_str(),
            MAX_ENVELOPE_RECORDS_PER_OBJECT,
            policy,
        )
        .await?;
    for record in records {
        let Ok(envelope) = serde_json::from_slice::<KukuriEnvelope>(&record.value) else {
            continue;
        };
        if envelope.verify().is_err() {
            continue;
        }
        if let Ok(Some(participant)) = parse_private_channel_participant(&envelope)
            && participant.participant_pubkey.as_str() == participant_pubkey
        {
            return Ok(Some(participant));
        }
    }
    Ok(None)
}

/// epoch の参加に要る制御 record(#1221 R5-C)。
pub(crate) struct PrivateEpochSnapshot {
    pub(crate) metadata: PrivateChannelMetadataDocV1,
    pub(crate) policy: PrivateChannelPolicyDocV1,
    /// 自分の参加 record(同じ epoch のもの)。
    pub(crate) local_participant: Option<PrivateChannelParticipantDocV1>,
}

/// 1 つの source から、metadata・policy・owner と自分の参加 record を key 指定で読む。
/// owner が同じ epoch の有効な参加者として見えなければ `None`(参加者の全件は読まない)。
pub(crate) async fn read_private_epoch_snapshot(
    docs_sync: &dyn DocsSync,
    replica: &ReplicaId,
    local_pubkey: &str,
    policy: DocFetchPolicy,
) -> Result<Option<PrivateEpochSnapshot>> {
    let Some(metadata) =
        fetch_private_channel_metadata_from_replica(docs_sync, replica, policy).await?
    else {
        return Ok(None);
    };
    let Some(channel_policy) =
        fetch_private_channel_policy_from_replica(docs_sync, replica, policy).await?
    else {
        return Ok(None);
    };
    let in_epoch = |participant: &PrivateChannelParticipantDocV1| {
        participant.epoch_id == channel_policy.epoch_id
            && participant.channel_id == channel_policy.channel_id
    };
    let owner = fetch_private_channel_participant_from_replica(
        docs_sync,
        replica,
        channel_policy.owner_pubkey.as_str(),
        policy,
    )
    .await?;
    if !owner
        .as_ref()
        .is_some_and(|owner| in_epoch(owner) && owner.is_owner && owner.left_at.is_none())
    {
        return Ok(None);
    }
    let local_participant =
        fetch_private_channel_participant_from_replica(docs_sync, replica, local_pubkey, policy)
            .await?
            .filter(in_epoch);
    Ok(Some(PrivateEpochSnapshot {
        metadata,
        policy: channel_policy,
        local_participant,
    }))
}

pub(crate) async fn store_manifest_blob<T: Serialize>(
    blob_service: &dyn BlobService,
    manifest: &T,
    mime: &str,
) -> Result<StoredBlob> {
    let payload = serde_json::to_vec(manifest)?;
    blob_service.put_blob(payload, mime).await
}

pub(crate) async fn fetch_manifest_blob<T: DeserializeOwned>(
    blob_service: &dyn BlobService,
    blob_ref: &ManifestBlobRef,
) -> Result<Option<T>> {
    let Some(bytes) = (match tokio::time::timeout(
        projection_blob_fetch_timeout(),
        blob_service.fetch_blob(&blob_ref.hash),
    )
    .await
    {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(_)) | Err(_) => None,
    }) else {
        return Ok(None);
    };
    Ok(Some(serde_json::from_slice(&bytes)?))
}

pub(crate) fn projection_blob_fetch_timeout() -> tokio::time::Duration {
    if cfg!(target_os = "windows") || std::env::var_os("GITHUB_ACTIONS").is_some() {
        tokio::time::Duration::from_secs(5)
    } else {
        tokio::time::Duration::from_secs(2)
    }
}

pub(crate) fn projection_blob_status_timeout() -> tokio::time::Duration {
    if cfg!(target_os = "windows") || std::env::var_os("GITHUB_ACTIONS").is_some() {
        tokio::time::Duration::from_secs(1)
    } else {
        tokio::time::Duration::from_millis(250)
    }
}

pub(crate) async fn fetch_projection_blob_text(
    blob_service: &dyn BlobService,
    hash: &kukuri_core::BlobHash,
) -> Option<String> {
    match tokio::time::timeout(
        projection_blob_fetch_timeout(),
        blob_service.fetch_blob(hash),
    )
    .await
    {
        Ok(Ok(Some(bytes))) => Some(String::from_utf8_lossy(&bytes).to_string()),
        Ok(Ok(None)) | Ok(Err(_)) | Err(_) => None,
    }
}

/// 表示用 projection に記録する blob の状態。ローカルの有無だけを見て、remote から取得しない。
///
/// #1152: 投稿の添付は受信・hydration の時点では成人向け advisory が未判明のため、ここで取得すると
/// 表示設定 OFF の取得ゲート（ADR 0046 §4 / §6.2、`blob_media_payload`）を迂回して bytes を
/// 永続化してしまう。remote 取得はゲートを持つ表示要求（`blob_media_payload`）と本文取得
/// （`fetch_projection_blob_text`）に限る。
pub(crate) async fn best_effort_blob_cache_status(
    blob_service: &dyn BlobService,
    hash: &kukuri_core::BlobHash,
) -> BlobCacheStatus {
    match tokio::time::timeout(
        projection_blob_status_timeout(),
        blob_service.local_blob_status(hash),
    )
    .await
    {
        Ok(Ok(status)) => blob_status(status),
        Ok(Err(_)) | Err(_) => BlobCacheStatus::Missing,
    }
}

/// view に載せる blob の状態。`best_effort_blob_cache_status` と同じく remote から取得しない（#1152）。
pub(crate) async fn best_effort_blob_view_status(
    blob_service: &dyn BlobService,
    hash: &kukuri_core::BlobHash,
) -> BlobViewStatus {
    match tokio::time::timeout(
        projection_blob_status_timeout(),
        blob_service.local_blob_status(hash),
    )
    .await
    {
        Ok(Ok(status)) => blob_view_status(status),
        Ok(Err(_)) | Err(_) => BlobViewStatus::Missing,
    }
}

/// 行は検証済みの live session からしか作れない(#1252)。
pub(crate) fn live_projection_row(verified: &VerifiedLiveSession) -> LiveSessionProjectionRow {
    let state = verified.state();
    let manifest = verified.manifest();
    LiveSessionProjectionRow {
        session_id: state.session_id.clone(),
        revision: manifest.revision,
        topic_id: verified.topic_id().to_string(),
        channel_id: channel_storage_id(state.channel_id.as_ref()),
        host_pubkey: state.owner_pubkey.as_str().to_string(),
        title: manifest.title.clone(),
        description: manifest.description.clone(),
        status: state.status.clone(),
        started_at: manifest.started_at,
        ended_at: manifest.ended_at,
        updated_at: state.updated_at,
        source_replica_id: verified.replica().clone(),
        source_key: stable_key("sessions/live", &format!("{}/state", state.session_id)),
        manifest_blob_hash: state.current_manifest.hash.clone(),
        derived_at: Utc::now().timestamp_millis(),
        projection_version: kukuri_store::VERIFIED_SESSION_PROJECTION_VERSION,
        viewer_count: 0,
    }
}

/// 行は検証済みの game room からしか作れない(#1252)。
pub(crate) fn game_projection_row(verified: &VerifiedGameRoom) -> GameRoomProjectionRow {
    let state = verified.state();
    let manifest = verified.manifest();
    GameRoomProjectionRow {
        room_id: state.room_id.clone(),
        score_revision: manifest.score_revision,
        topic_id: verified.topic_id().to_string(),
        channel_id: channel_storage_id(state.channel_id.as_ref()),
        host_pubkey: state.owner_pubkey.as_str().to_string(),
        title: manifest.title.clone(),
        description: manifest.description.clone(),
        status: state.status.clone(),
        phase_label: manifest.phase_label.clone(),
        scores: manifest.scores.clone(),
        room_kind: manifest.room_kind.clone(),
        metaverse: manifest.metaverse.clone(),
        updated_at: state.updated_at,
        source_replica_id: verified.replica().clone(),
        source_key: stable_key("sessions/game", &format!("{}/state", state.room_id)),
        manifest_blob_hash: state.current_manifest.hash.clone(),
        derived_at: Utc::now().timestamp_millis(),
        projection_version: kukuri_store::VERIFIED_SESSION_PROJECTION_VERSION,
    }
}

/// 投稿の行は、検証済みの投稿(`VerifiedPost`)からしか作れない(#1248)。docs の `state` の値から行を作る経路を
/// 型で塞ぐ。
pub(crate) fn projection_row_from_post(
    post: &VerifiedPost,
    content: Option<String>,
) -> ObjectProjectionRow {
    let header = post.header();
    let source_replica_id = post.replica();
    let source_blob_hash = match &header.payload_ref {
        PayloadRef::BlobText { hash, .. } => Some(hash.clone()),
        PayloadRef::InlineText { .. } => None,
    };
    ObjectProjectionRow {
        object_id: header.object_id.clone(),
        topic_id: header.topic_id.as_str().to_string(),
        channel_id: channel_storage_id(header.channel_id.as_ref()),
        author_pubkey: header.author.as_str().to_string(),
        created_at: header.created_at,
        object_kind: header.object_kind.clone(),
        root_object_id: header.root.clone(),
        reply_to_object_id: header.reply_to.clone(),
        payload_ref: header.payload_ref.clone(),
        content,
        attachments: header.attachments.clone(),
        repost_of: header.repost_of.clone(),
        content_labels: header.content_labels.clone(),
        source_replica_id: source_replica_id.clone(),
        source_key: stable_key("objects", &format!("{}/state", header.object_id.as_str())),
        source_envelope_id: header.envelope_id.clone(),
        source_blob_hash,
        source_docs_author: post.docs_author().map(str::to_string),
        derived_at: Utc::now().timestamp_millis(),
        projection_version: kukuri_store::VERIFIED_OBJECT_PROJECTION_VERSION,
    }
}

/// 行は検証済みの reaction からしか作れない(#1252)。
pub(crate) fn reaction_projection_row(verified: &VerifiedReaction) -> ReactionProjectionRow {
    let reaction = verified.doc();
    let source_replica_id = verified.replica();
    ReactionProjectionRow {
        source_replica_id: source_replica_id.clone(),
        target_object_id: reaction.target_object_id.clone(),
        reaction_id: reaction.reaction_id.clone(),
        author_pubkey: reaction.author_pubkey.as_str().to_string(),
        created_at: reaction.created_at,
        updated_at: reaction.updated_at,
        reaction_key_kind: reaction.reaction_key_kind.clone(),
        normalized_reaction_key: reaction.normalized_reaction_key.clone(),
        emoji: reaction.emoji.clone(),
        custom_asset_id: reaction.custom_asset_id.clone(),
        custom_asset_snapshot: reaction.custom_asset_snapshot.clone(),
        status: reaction.status.clone(),
        source_key: stable_key(
            "reactions",
            &format!(
                "{}/{}/state",
                reaction.target_object_id.as_str(),
                reaction.reaction_id.as_str()
            ),
        ),
        source_envelope_id: reaction.envelope_id.clone(),
        derived_at: Utc::now().timestamp_millis(),
        projection_version: kukuri_store::VERIFIED_REACTION_PROJECTION_VERSION,
    }
}

pub(crate) fn custom_reaction_asset_view_from_snapshot(
    snapshot: &CustomReactionAssetSnapshotV1,
) -> CustomReactionAssetView {
    CustomReactionAssetView {
        asset_id: snapshot.asset_id.clone(),
        owner_pubkey: snapshot.owner_pubkey.as_str().to_string(),
        blob_hash: snapshot.blob_hash.as_str().to_string(),
        search_key: search_key_or_asset_id(
            snapshot.search_key.as_str(),
            snapshot.asset_id.as_str(),
        ),
        mime: snapshot.mime.clone(),
        bytes: snapshot.bytes,
        width: snapshot.width,
        height: snapshot.height,
    }
}

pub(crate) fn custom_reaction_asset_view_from_doc(
    asset: &CustomReactionAssetDocV1,
) -> CustomReactionAssetView {
    CustomReactionAssetView {
        asset_id: asset.asset_id.clone(),
        owner_pubkey: asset.author_pubkey.as_str().to_string(),
        blob_hash: asset.blob_hash.as_str().to_string(),
        search_key: search_key_or_asset_id(asset.search_key.as_str(), asset.asset_id.as_str()),
        mime: asset.mime.clone(),
        bytes: asset.bytes,
        width: asset.width,
        height: asset.height,
    }
}

pub(crate) fn bookmarked_custom_reaction_view_from_row(
    row: BookmarkedCustomReactionRow,
) -> BookmarkedCustomReactionView {
    let asset_id = row.asset_id;
    BookmarkedCustomReactionView {
        asset_id: asset_id.clone(),
        owner_pubkey: row.owner_pubkey,
        blob_hash: row.blob_hash.as_str().to_string(),
        search_key: search_key_or_asset_id(row.search_key.as_str(), asset_id.as_str()),
        mime: row.mime,
        bytes: row.bytes,
        width: row.width,
        height: row.height,
    }
}

pub(crate) fn recent_reaction_view_from_projection(
    row: &ReactionProjectionRow,
) -> RecentReactionView {
    RecentReactionView {
        reaction_key_kind: reaction_key_kind_label(&row.reaction_key_kind).to_string(),
        normalized_reaction_key: row.normalized_reaction_key.clone(),
        emoji: row.emoji.clone(),
        custom_asset: row
            .custom_asset_snapshot
            .as_ref()
            .map(custom_reaction_asset_view_from_snapshot),
        updated_at: row.updated_at,
    }
}

pub(crate) fn reaction_key_kind_label(kind: &ReactionKeyKind) -> &'static str {
    match kind {
        ReactionKeyKind::Emoji => "emoji",
        ReactionKeyKind::CustomAsset => "custom_asset",
    }
}

pub(crate) fn reaction_key_view_from_projection(row: &ReactionProjectionRow) -> ReactionKeyView {
    ReactionKeyView {
        reaction_key_kind: reaction_key_kind_label(&row.reaction_key_kind).to_string(),
        normalized_reaction_key: row.normalized_reaction_key.clone(),
        emoji: row.emoji.clone(),
        custom_asset: row
            .custom_asset_snapshot
            .as_ref()
            .map(custom_reaction_asset_view_from_snapshot),
    }
}

pub(crate) fn reaction_cache_key(
    source_replica_id: &ReplicaId,
    target_object_id: &EnvelopeId,
) -> String {
    format!(
        "{}:{}",
        source_replica_id.as_str(),
        target_object_id.as_str()
    )
}

pub(crate) fn reaction_state_view_from_rows(
    source_replica_id: &ReplicaId,
    target_object_id: &EnvelopeId,
    rows: Vec<ReactionProjectionRow>,
    current_author: &str,
) -> ReactionStateView {
    let mut summary = BTreeMap::<String, ReactionSummaryView>::new();
    let mut my_reactions = Vec::new();
    for row in rows {
        let key_view = reaction_key_view_from_projection(&row);
        if row.status == ObjectStatus::Active {
            summary
                .entry(row.normalized_reaction_key.clone())
                .and_modify(|value| value.count += 1)
                .or_insert_with(|| ReactionSummaryView {
                    reaction_key_kind: key_view.reaction_key_kind.clone(),
                    normalized_reaction_key: key_view.normalized_reaction_key.clone(),
                    emoji: key_view.emoji.clone(),
                    custom_asset: key_view.custom_asset.clone(),
                    count: 1,
                });
            if row.author_pubkey == current_author {
                my_reactions.push(key_view);
            }
        }
    }
    ReactionStateView {
        target_object_id: target_object_id.as_str().to_string(),
        source_replica_id: source_replica_id.as_str().to_string(),
        reaction_summary: summary.into_values().collect(),
        my_reactions,
    }
}

pub(crate) fn search_key_or_asset_id(search_key: &str, asset_id: &str) -> String {
    let normalized = search_key.trim();
    if normalized.is_empty() {
        return asset_id.to_string();
    }
    normalized.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn epoch_handoff_grant_uses_legacy_storage_key() {
        let owner =
            KukuriKeys::parse("0000000000000000000000000000000000000000000000000000000000000001")
                .expect("owner key");
        let recipient =
            KukuriKeys::parse("0000000000000000000000000000000000000000000000000000000000000002")
                .expect("recipient key");
        let grant = encrypt_private_channel_epoch_handoff_grant(
            &owner,
            &PrivateChannelEpochHandoffGrantPayloadV1 {
                channel_id: ChannelId::new("channel-fixture"),
                topic_id: TopicId::new("kukuri:topic:fixture"),
                owner_pubkey: owner.public_key(),
                recipient_pubkey: recipient.public_key(),
                old_epoch_id: "epoch-7".into(),
                new_epoch_id: "epoch-8".into(),
                new_namespace_secret_hex:
                    "0000000000000000000000000000000000000000000000000000000000000003".into(),
            },
        )
        .expect("grant");
        let docs_sync = MemoryDocsSync::default();
        let replica = ReplicaId::new("private:fixture:epoch-7");

        persist_private_channel_epoch_handoff_grant(&docs_sync, &owner, &grant, &replica)
            .await
            .expect("persist grant");

        let expected_key = format!(
            "channels/rotation-grants/{}/envelope",
            recipient.public_key_hex()
        );
        let records = docs_sync
            .query_replica(&replica, DocQuery::Exact(expected_key.clone()))
            .await
            .expect("query grant");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].key, expected_key);
        assert_eq!(
            serde_json::from_slice::<KukuriEnvelope>(&records[0].value)
                .expect("stored envelope")
                .kind,
            "channel-rotation-grant"
        );
    }
}
