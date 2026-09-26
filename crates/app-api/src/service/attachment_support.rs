use super::*;

pub(crate) async fn register_private_channel_replica_secrets(
    docs_sync: &dyn DocsSync,
    state: &JoinedPrivateChannelState,
) -> Result<()> {
    for epoch in private_channel_epoch_capabilities(state) {
        let replica =
            private_channel_replica_for_epoch(state.channel_id.as_str(), epoch.epoch_id.as_str());
        docs_sync
            .register_private_replica_secret(&replica, epoch.namespace_secret_hex.as_str())
            .await?;
    }
    Ok(())
}

pub(crate) async fn blob_view_status_for_payload(
    blob_service: &dyn BlobService,
    payload_ref: &PayloadRef,
) -> Result<BlobViewStatus> {
    match payload_ref {
        PayloadRef::InlineText { .. } => Ok(BlobViewStatus::Available),
        PayloadRef::BlobText { hash, .. } => {
            Ok(best_effort_blob_view_status(blob_service, hash).await)
        }
    }
}

pub(crate) async fn attachment_views_from_refs(
    blob_service: &dyn BlobService,
    refs: &[kukuri_core::AssetRef],
) -> Result<Vec<AttachmentView>> {
    let mut attachments = Vec::with_capacity(refs.len());
    for attachment in refs {
        attachments.push(AttachmentView {
            hash: attachment.hash.as_str().to_string(),
            mime: attachment.mime.clone(),
            bytes: attachment.bytes,
            role: attachment_role_name(&attachment.role).to_string(),
            status: best_effort_blob_view_status(blob_service, &attachment.hash).await,
            provenance: None,
        });
    }
    Ok(attachments)
}

pub(crate) async fn direct_message_attachment_views(
    blob_service: &dyn BlobService,
    manifest: Option<&DirectMessageAttachmentManifestV1>,
) -> Result<Vec<AttachmentView>> {
    let Some(manifest) = manifest else {
        return Ok(Vec::new());
    };
    let mut attachments = Vec::new();
    attachments.push(AttachmentView {
        hash: manifest.original.hash.as_str().to_string(),
        mime: manifest.original.mime.clone(),
        bytes: manifest.original.bytes,
        role: match manifest.kind {
            DirectMessageAttachmentKind::Image => "image_original".into(),
            DirectMessageAttachmentKind::Video => "video_manifest".into(),
        },
        status: best_effort_blob_view_status(blob_service, &manifest.original.hash).await,
        provenance: None,
    });
    if let Some(poster) = manifest.poster.as_ref() {
        attachments.push(AttachmentView {
            hash: poster.hash.as_str().to_string(),
            mime: poster.mime.clone(),
            bytes: poster.bytes,
            role: "video_poster".into(),
            status: best_effort_blob_view_status(blob_service, &poster.hash).await,
            provenance: None,
        });
    }
    Ok(attachments)
}

pub(crate) fn direct_message_preview(row: &DirectMessageMessageRow) -> String {
    if let Some(text) = row
        .text
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return text.chars().take(80).collect();
    }
    match row
        .attachment_manifest
        .as_ref()
        .map(|manifest| &manifest.kind)
    {
        Some(DirectMessageAttachmentKind::Image) => "[Image]".into(),
        Some(DirectMessageAttachmentKind::Video) => "[Video]".into(),
        None => String::new(),
    }
}

pub(crate) struct DirectMessageMaterializationGuard<'a> {
    pub(crate) projection_store: &'a dyn ProjectionStore,
    pub(crate) local_author_pubkey: &'a str,
    pub(crate) peer_pubkey: &'a str,
}

impl DirectMessageMaterializationGuard<'_> {
    async fn is_mutual(&self) -> Result<bool> {
        Ok(self
            .projection_store
            .get_author_relationship(self.local_author_pubkey, self.peer_pubkey)
            .await?
            .as_ref()
            .is_some_and(|relationship| relationship.mutual))
    }
}

pub(crate) async fn materialize_direct_message_manifest(
    blob_service: &dyn BlobService,
    guard: &DirectMessageMaterializationGuard<'_>,
    keys: &KukuriKeys,
    sender_pubkey: &Pubkey,
    message_id: &str,
    manifest: Option<&DirectMessageAttachmentManifestV1>,
) -> Result<Option<DirectMessageAttachmentManifestV1>> {
    let Some(manifest) = manifest else {
        return Ok(None);
    };
    let original = materialize_direct_message_blob_ref(
        blob_service,
        guard,
        keys,
        sender_pubkey,
        message_id,
        &manifest.original,
    )
    .await?;
    let Some(original) = original else {
        return Ok(None);
    };
    let poster = match manifest.poster.as_ref() {
        Some(poster) => {
            let poster = materialize_direct_message_blob_ref(
                blob_service,
                guard,
                keys,
                sender_pubkey,
                message_id,
                poster,
            )
            .await?;
            let Some(poster) = poster else {
                return Ok(None);
            };
            Some(poster)
        }
        None => None,
    };
    Ok(Some(DirectMessageAttachmentManifestV1 {
        attachment_id: manifest.attachment_id.clone(),
        kind: manifest.kind.clone(),
        original,
        poster,
    }))
}

pub(crate) async fn materialize_direct_message_blob_ref(
    blob_service: &dyn BlobService,
    guard: &DirectMessageMaterializationGuard<'_>,
    keys: &KukuriKeys,
    sender_pubkey: &Pubkey,
    message_id: &str,
    encrypted_ref: &DirectMessageEncryptedBlobRefV1,
) -> Result<Option<DirectMessageEncryptedBlobRefV1>> {
    if !guard.is_mutual().await? {
        return Ok(None);
    }
    let Some(bytes) = blob_service.fetch_blob(&encrypted_ref.hash).await? else {
        anyhow::bail!("direct message attachment blob is missing");
    };
    if !guard.is_mutual().await? {
        return Ok(None);
    }
    let encrypted: DirectMessageEncryptedAttachmentV1 = serde_json::from_slice(bytes.as_slice())
        .context("failed to decode direct message attachment blob")?;
    let decrypted = decrypt_direct_message_attachment(keys, sender_pubkey, message_id, &encrypted)?;
    if !guard.is_mutual().await? {
        return Ok(None);
    }
    let local = blob_service
        .put_blob(decrypted, encrypted_ref.mime.as_str())
        .await?;
    Ok(Some(DirectMessageEncryptedBlobRefV1 {
        blob_id: encrypted_ref.blob_id.clone(),
        hash: local.hash,
        mime: encrypted_ref.mime.clone(),
        bytes: encrypted_ref.bytes,
        nonce_hex: String::new(),
    }))
}

pub(crate) fn blob_view_status(status: BlobStatus) -> BlobViewStatus {
    match status {
        BlobStatus::Missing => BlobViewStatus::Missing,
        BlobStatus::Available => BlobViewStatus::Available,
        BlobStatus::Pinned => BlobViewStatus::Pinned,
    }
}

pub(crate) fn blob_status(status: BlobStatus) -> BlobCacheStatus {
    match status {
        BlobStatus::Missing => BlobCacheStatus::Missing,
        BlobStatus::Available => BlobCacheStatus::Available,
        BlobStatus::Pinned => BlobCacheStatus::Pinned,
    }
}

pub(crate) fn attachment_role_name(role: &AssetRole) -> &'static str {
    match role {
        AssetRole::ImageOriginal => "image_original",
        AssetRole::ImagePreview => "image_preview",
        AssetRole::VideoPoster => "video_poster",
        AssetRole::VideoManifest => "video_manifest",
        AssetRole::ProfileAvatar => "profile_avatar",
        AssetRole::Attachment => "attachment",
    }
}

pub(crate) fn sanitize_game_participants(participants: Vec<String>) -> Result<Vec<String>> {
    let mut seen = BTreeSet::new();
    let normalized = participants
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .filter(|value| seen.insert(value.clone()))
        .collect::<Vec<_>>();
    if normalized.len() < 2 {
        anyhow::bail!("game room requires at least two unique participants");
    }
    Ok(normalized)
}

pub(crate) fn validate_game_room_transition(
    current: &GameRoomStatus,
    next: &GameRoomStatus,
) -> Result<()> {
    match (current, next) {
        (GameRoomStatus::Ended, GameRoomStatus::Ended) => {
            anyhow::bail!("ended game room cannot be updated")
        }
        (GameRoomStatus::Ended, _) => anyhow::bail!("ended game room cannot be updated"),
        (GameRoomStatus::Waiting, GameRoomStatus::Waiting)
        | (GameRoomStatus::Waiting, GameRoomStatus::Running)
        | (GameRoomStatus::Waiting, GameRoomStatus::Ended)
        | (GameRoomStatus::Running, GameRoomStatus::Running)
        | (GameRoomStatus::Running, GameRoomStatus::Paused)
        | (GameRoomStatus::Running, GameRoomStatus::Ended)
        | (GameRoomStatus::Paused, GameRoomStatus::Paused)
        | (GameRoomStatus::Paused, GameRoomStatus::Running)
        | (GameRoomStatus::Paused, GameRoomStatus::Ended) => Ok(()),
        (GameRoomStatus::Waiting, GameRoomStatus::Paused) => {
            anyhow::bail!("game room cannot pause before it starts")
        }
        (GameRoomStatus::Running, GameRoomStatus::Waiting)
        | (GameRoomStatus::Paused, GameRoomStatus::Waiting) => {
            anyhow::bail!("game room cannot move back to waiting")
        }
    }
}

pub(crate) fn validate_game_room_scores(
    manifest: &GameRoomManifestBlobV1,
    scores: &[GameScoreView],
) -> Result<()> {
    if manifest.scores.len() != scores.len() {
        anyhow::bail!("score update must include all participants");
    }
    let expected = manifest
        .scores
        .iter()
        .map(|score| score.participant_id.clone())
        .collect::<BTreeSet<_>>();
    let provided = scores
        .iter()
        .map(|score| score.participant_id.clone())
        .collect::<BTreeSet<_>>();
    if expected != provided {
        anyhow::bail!("score update participants do not match the room roster");
    }
    let expected_labels = manifest
        .scores
        .iter()
        .map(|score| (score.participant_id.as_str(), score.label.as_str()))
        .collect::<BTreeMap<_, _>>();
    for score in scores {
        if expected_labels.get(score.participant_id.as_str()) != Some(&score.label.as_str()) {
            anyhow::bail!("score update labels do not match the room roster");
        }
    }
    Ok(())
}

pub(crate) fn channel_storage_id(channel_id: Option<&ChannelId>) -> String {
    channel_id
        .map(|value| value.as_str().to_string())
        .unwrap_or_else(|| PUBLIC_CHANNEL_ID.to_string())
}

pub(crate) fn channel_hint_topic_for(topic_id: &str, channel_id: Option<&ChannelId>) -> TopicId {
    channel_id
        .map(|value| private_channel_hint_topic(value.as_str()))
        .unwrap_or_else(|| TopicId::new(topic_id))
}

pub(crate) fn channel_id_from_storage(channel_id: &str) -> Option<ChannelId> {
    (channel_id != PUBLIC_CHANNEL_ID).then(|| ChannelId::new(channel_id.to_string()))
}

pub(crate) fn channel_id_for_view(channel_id: &str) -> Option<String> {
    channel_id_from_storage(channel_id).map(|value| value.as_str().to_string())
}

pub(crate) fn joined_private_channel_key(topic_id: &str, channel_id: &str) -> String {
    format!("{topic_id}::{channel_id}")
}

pub(crate) fn live_presence_task_key(topic_id: &str, channel_id: &str, session_id: &str) -> String {
    format!("{topic_id}::{channel_id}::{session_id}")
}

pub(crate) fn short_id_suffix(author_pubkey: &str) -> &str {
    author_pubkey.get(..8).unwrap_or(author_pubkey)
}

pub(crate) fn normalize_topic_name(topic: String) -> Option<String> {
    let normalized = topic
        .strip_prefix(kukuri_core::wire::HINT_TOPIC_PREFIX)
        .map_or(topic.clone(), ToOwned::to_owned);
    if normalized.starts_with(kukuri_core::wire::PRIVATE_CHANNEL_TOPIC_PREFIX)
        || normalized.starts_with(kukuri_core::wire::DM_TOPIC_PREFIX)
    {
        None
    } else {
        Some(normalized)
    }
}

pub(crate) fn normalize_topics(topics: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut normalized = Vec::new();
    for topic in topics {
        let Some(topic) = normalize_topic_name(topic) else {
            continue;
        };
        if seen.insert(topic.clone()) {
            normalized.push(topic);
        }
    }
    normalized
}

pub(crate) fn normalize_topic_diagnostics(
    diagnostics: Vec<TopicPeerSnapshot>,
) -> Vec<TopicPeerSnapshot> {
    let mut merged = BTreeMap::<String, TopicPeerSnapshot>::new();
    for diagnostic in diagnostics {
        let Some(topic) = normalize_topic_name(diagnostic.topic) else {
            continue;
        };
        let entry = merged
            .entry(topic.clone())
            .or_insert_with(|| TopicPeerSnapshot {
                topic: topic.clone(),
                joined: false,
                peer_count: 0,
                connected_peers: Vec::new(),
                configured_peer_ids: Vec::new(),
                missing_peer_ids: Vec::new(),
                active_path: diagnostic.active_path.clone(),
                rendezvous_peer_ids: Vec::new(),
                fallback_peer_ids: Vec::new(),
                last_received_at: None,
                status_detail: diagnostic.status_detail.clone(),
                last_error: diagnostic.last_error.clone(),
            });
        entry.joined |= diagnostic.joined;
        entry.peer_count = entry.peer_count.max(diagnostic.peer_count);
        for peer in diagnostic.connected_peers {
            if !entry.connected_peers.contains(&peer) {
                entry.connected_peers.push(peer);
            }
        }
        for peer in diagnostic.configured_peer_ids {
            if !entry.configured_peer_ids.contains(&peer) {
                entry.configured_peer_ids.push(peer);
            }
        }
        for peer in diagnostic.missing_peer_ids {
            if !entry.missing_peer_ids.contains(&peer) {
                entry.missing_peer_ids.push(peer);
            }
        }
        for peer in diagnostic.rendezvous_peer_ids {
            if !entry.rendezvous_peer_ids.contains(&peer) {
                entry.rendezvous_peer_ids.push(peer);
            }
        }
        for peer in diagnostic.fallback_peer_ids {
            if !entry.fallback_peer_ids.contains(&peer) {
                entry.fallback_peer_ids.push(peer);
            }
        }
        if connection_path_rank(&diagnostic.active_path) > connection_path_rank(&entry.active_path)
        {
            entry.active_path = diagnostic.active_path;
        }
        entry.last_received_at = match (entry.last_received_at, diagnostic.last_received_at) {
            (Some(left), Some(right)) => Some(left.max(right)),
            (None, value) | (value, None) => value,
        };
        if entry.status_detail.starts_with("No peers configured")
            || entry.status_detail.starts_with("Waiting")
        {
            entry.status_detail = diagnostic.status_detail;
        }
        if entry.last_error.is_none() {
            entry.last_error = diagnostic.last_error;
        }
    }
    merged.into_values().collect()
}

fn connection_path_rank(path: &ConnectionPath) -> u8 {
    match path {
        ConnectionPath::DirectP2p => 0,
        ConnectionPath::RelaySupportedP2p => 1,
        ConnectionPath::RelayFallback => 2,
    }
}

pub(crate) fn merge_optional_timestamp(left: Option<i64>, right: Option<i64>) -> Option<i64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (None, value) | (value, None) => value,
    }
}

pub(crate) fn combine_delivery_states(left: DeliveryState, right: DeliveryState) -> DeliveryState {
    use DeliveryState::*;

    match (left, right) {
        (Live, _) | (_, Live) => Live,
        (DurableReady, _) | (_, DurableReady) => DurableReady,
        (DurableRecovering, _) | (_, DurableRecovering) => DurableRecovering,
        _ => Offline,
    }
}

pub(crate) fn delivery_state_for_topic(
    gossip_peer_count: usize,
    docs_assist_peer_count: usize,
    last_docs_activity_at: Option<i64>,
) -> DeliveryState {
    if gossip_peer_count > 0 {
        DeliveryState::Live
    } else if last_docs_activity_at.is_some() {
        DeliveryState::DurableReady
    } else if docs_assist_peer_count > 0 {
        DeliveryState::DurableRecovering
    } else {
        DeliveryState::Offline
    }
}

pub(crate) fn effective_sync_status_detail(
    base: &str,
    delivery_state: DeliveryState,
    docs_assist_peer_count: usize,
    subscribed_topic_count: usize,
) -> String {
    match delivery_state {
        DeliveryState::Live | DeliveryState::Offline => base.to_string(),
        DeliveryState::DurableRecovering => {
            if subscribed_topic_count > 0 {
                format!(
                    "docs-assisted recovery is in progress via {docs_assist_peer_count} peer(s); live topic delivery is unavailable"
                )
            } else {
                format!(
                    "docs-assisted recovery is in progress via {docs_assist_peer_count} peer(s)"
                )
            }
        }
        DeliveryState::DurableReady => {
            format!(
                "docs-assisted durable sync is available via {docs_assist_peer_count} peer(s); live topic delivery is unavailable"
            )
        }
    }
}

pub(crate) fn effective_topic_status_detail(
    base: &str,
    delivery_state: DeliveryState,
    docs_assist_peer_count: usize,
) -> String {
    match delivery_state {
        DeliveryState::Live | DeliveryState::Offline => base.to_string(),
        DeliveryState::DurableRecovering => format!(
            "docs-assisted recovery is in progress via {docs_assist_peer_count} peer(s); live topic delivery is unavailable"
        ),
        DeliveryState::DurableReady => format!(
            "docs-assisted durable sync is available via {docs_assist_peer_count} peer(s); live topic delivery is unavailable"
        ),
    }
}

impl Drop for AppService {
    fn drop(&mut self) {
        if let Ok(mut leases) = self.subscription_registry.scope_leases.try_lock() {
            drop(leases.clear());
        }
        if let Ok(mut heartbeats) = self.subscription_registry.dome_heartbeats.try_lock() {
            heartbeats.clear();
        }
        if let Ok(mut tasks) = self.subscription_registry.live_presence_tasks.try_lock() {
            for (_, handle) in tasks.drain() {
                handle.abort();
            }
        }
    }
}
