use super::*;

/// remote の投稿の event から通知の候補を作る。
///
/// actor・本文・channel は、署名つき envelope と replica の scope を確かめた投稿からだけ作る(#1248)。
/// `topic_id` は、その replica を購読している topic。`state` と `envelope` のどちらの event でも試す
/// (`state` が先に届いた投稿は、`envelope` の event で通知になる)。通知の id は envelope id から決まるので、
/// 2 回試しても重複しない。
pub(crate) async fn notification_candidate_from_object_event(
    projection_store: &dyn ProjectionStore,
    docs_sync: &dyn DocsSync,
    blob_service: &dyn BlobService,
    local_author_pubkey: &str,
    topic_id: &str,
    event: &DocEvent,
) -> Result<Option<NotificationCandidate>> {
    if event.source_peer.is_none() {
        return Ok(None);
    }
    let Some(object_id) = object_id_from_post_key(event.key.as_str()) else {
        return Ok(None);
    };
    let Some(post) = load_verified_post(
        docs_sync,
        &event.replica_id,
        topic_id,
        &object_id,
        DocFetchPolicy::LocalThenRemote,
    )
    .await?
    else {
        return Ok(None);
    };
    let header = post.header();
    if header.author.as_str() == local_author_pubkey {
        return Ok(None);
    }
    let content = notification_text_from_payload_ref(blob_service, &header.payload_ref).await;
    let repost_commentary = if header.object_kind == "repost" {
        normalize_repost_commentary(content.clone())
    } else {
        None
    };
    let reply_preview = if header.object_kind == "repost" {
        repost_commentary.clone().or(content.clone())
    } else {
        content.clone()
    };
    if let Some(reply_to_object_id) = header.reply_to.as_ref()
        && projection_store
            .get_object_projection(reply_to_object_id)
            .await?
            .as_ref()
            .is_some_and(|row| row.author_pubkey == local_author_pubkey)
    {
        return Ok(Some(NotificationCandidate {
            kind: NotificationKind::Reply,
            actor_pubkey: header.author.as_str().to_string(),
            source_envelope_id: Some(header.envelope_id.clone()),
            source_replica_id: Some(event.replica_id.clone()),
            topic_id: Some(header.topic_id.as_str().to_string()),
            channel_id: header
                .channel_id
                .as_ref()
                .map(|value| value.as_str().to_string()),
            object_id: Some(header.object_id.clone()),
            dm_id: None,
            message_id: None,
            preview_text: notification_preview_text(reply_preview),
            content_labels: Some(header.content_labels.clone()),
            created_at: header.created_at,
            received_at: Utc::now().timestamp_millis(),
        }));
    }
    if header.channel_id.is_none()
        && let Some(repost_of) = header.repost_of.as_ref()
        && repost_of.source_author_pubkey.as_str() == local_author_pubkey
    {
        let (kind, preview_source) = if repost_commentary.is_some() {
            (NotificationKind::QuoteRepost, repost_commentary)
        } else {
            (
                NotificationKind::Repost,
                normalize_optional_text(Some(repost_of.content.clone())),
            )
        };
        return Ok(Some(NotificationCandidate {
            kind,
            actor_pubkey: header.author.as_str().to_string(),
            source_envelope_id: Some(header.envelope_id.clone()),
            source_replica_id: Some(event.replica_id.clone()),
            topic_id: Some(header.topic_id.as_str().to_string()),
            channel_id: None,
            object_id: Some(header.object_id.clone()),
            dm_id: None,
            message_id: None,
            preview_text: notification_preview_text(preview_source),
            content_labels: Some(header.content_labels.clone()),
            created_at: header.created_at,
            received_at: Utc::now().timestamp_millis(),
        }));
    }
    let mention_source = if header.object_kind == "repost" {
        repost_commentary
    } else {
        normalize_optional_text(content)
    };
    if mention_source
        .as_deref()
        .is_some_and(|text| text_contains_pubkey_mention(text, local_author_pubkey))
    {
        return Ok(Some(NotificationCandidate {
            kind: NotificationKind::Mention,
            actor_pubkey: header.author.as_str().to_string(),
            source_envelope_id: Some(header.envelope_id.clone()),
            source_replica_id: Some(event.replica_id.clone()),
            topic_id: Some(header.topic_id.as_str().to_string()),
            channel_id: header
                .channel_id
                .as_ref()
                .map(|value| value.as_str().to_string()),
            object_id: Some(header.object_id.clone()),
            dm_id: None,
            message_id: None,
            preview_text: notification_preview_text(mention_source),
            content_labels: Some(header.content_labels.clone()),
            created_at: header.created_at,
            received_at: Utc::now().timestamp_millis(),
        }));
    }
    Ok(None)
}

pub(crate) async fn notification_candidate_from_follow_event(
    _store: &dyn Store,
    docs_sync: &dyn DocsSync,
    local_author_pubkey: &str,
    author_pubkey: &str,
    event: &DocEvent,
) -> Result<Option<NotificationCandidate>> {
    if event.source_peer.is_none() || !event.key.starts_with("graph/follows/") {
        return Ok(None);
    }
    let Some(record) = docs_sync
        .query_replica(&event.replica_id, DocQuery::Exact(event.key.clone()))
        .await?
        .into_iter()
        .next()
    else {
        return Ok(None);
    };
    let doc: FollowEdgeDocV1 = serde_json::from_slice(&record.value)?;
    if doc.subject_pubkey.as_str() != author_pubkey {
        return Ok(None);
    }
    let Some(envelope) = fetch_author_envelope_by_id(
        docs_sync,
        &event.replica_id,
        &doc.envelope_id,
        DocFetchPolicy::LocalThenRemote,
    )
    .await?
    else {
        return Ok(None);
    };
    let Some(edge) = parse_follow_edge(&envelope)? else {
        return Ok(None);
    };
    if edge.subject_pubkey.as_str() == local_author_pubkey
        || edge.target_pubkey.as_str() != local_author_pubkey
        || edge.status != FollowEdgeStatus::Active
    {
        return Ok(None);
    }
    Ok(Some(NotificationCandidate {
        kind: NotificationKind::Followed,
        actor_pubkey: edge.subject_pubkey.as_str().to_string(),
        source_envelope_id: Some(edge.envelope_id.clone()),
        source_replica_id: Some(event.replica_id.clone()),
        topic_id: None,
        channel_id: None,
        object_id: None,
        dm_id: None,
        message_id: None,
        preview_text: None,
        content_labels: None,
        created_at: edge.updated_at,
        received_at: Utc::now().timestamp_millis(),
    }))
}

pub(crate) async fn notification_text_from_payload_ref(
    blob_service: &dyn BlobService,
    payload_ref: &PayloadRef,
) -> Option<String> {
    match payload_ref {
        PayloadRef::InlineText { text } => Some(text.clone()),
        PayloadRef::BlobText { hash, .. } => fetch_projection_blob_text(blob_service, hash).await,
    }
}

pub(crate) fn notification_preview_text(value: Option<String>) -> Option<String> {
    normalize_optional_text(value)
        .map(|text| text.chars().take(NOTIFICATION_PREVIEW_LIMIT).collect())
}

pub(crate) fn notification_kind_key(kind: &NotificationKind) -> &'static str {
    match kind {
        NotificationKind::Mention => "mention",
        NotificationKind::Reply => "reply",
        NotificationKind::Repost => "repost",
        NotificationKind::QuoteRepost => "quote_repost",
        NotificationKind::DirectMessage => "direct_message",
        NotificationKind::Followed => "followed",
    }
}

pub(crate) fn notification_doc_event_fingerprint_parts(key: &str, content_hash: &str) -> String {
    format!("{key}|{content_hash}")
}

pub(crate) fn notification_doc_event_fingerprint(event: &DocEvent) -> String {
    notification_doc_event_fingerprint_parts(&event.key, &event.content_hash)
}

pub(crate) fn document_notification_id(
    recipient_pubkey: &str,
    kind: &NotificationKind,
    source_envelope_id: &EnvelopeId,
) -> String {
    format!(
        "notification:{recipient_pubkey}:{}:{}",
        notification_kind_key(kind),
        source_envelope_id.as_str()
    )
}

pub(crate) fn direct_message_notification_id(
    recipient_pubkey: &str,
    kind: &NotificationKind,
    dm_id: &str,
    message_id: &str,
) -> String {
    format!(
        "notification:{recipient_pubkey}:{}:{dm_id}:{message_id}",
        notification_kind_key(kind)
    )
}

pub(crate) fn text_contains_pubkey_mention(text: &str, pubkey: &str) -> bool {
    let bytes = text.as_bytes();
    let pubkey_bytes = pubkey.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'@' {
            let start = index + 1;
            let end = start + 64;
            if end <= bytes.len() {
                let candidate = &bytes[start..end];
                let next_is_hex = bytes
                    .get(end)
                    .is_some_and(|value| char::from(*value).is_ascii_hexdigit());
                if !next_is_hex
                    && candidate.len() == 64
                    && candidate
                        .iter()
                        .all(|value| char::from(*value).is_ascii_hexdigit())
                    && candidate.len() == pubkey_bytes.len()
                    && candidate
                        .iter()
                        .zip(pubkey_bytes.iter())
                        .all(|(left, right)| {
                            char::from(*left).eq_ignore_ascii_case(&char::from(*right))
                        })
                {
                    return true;
                }
            }
        }
        index += 1;
    }
    false
}

pub(crate) fn normalize_author_pubkey(pubkey: &str) -> Result<String> {
    let trimmed = pubkey.trim();
    if trimmed.len() != 64 || !trimmed.chars().all(|value| value.is_ascii_hexdigit()) {
        return Err(anyhow::anyhow!("invalid author pubkey"));
    }
    Ok(trimmed.to_string())
}

pub(crate) fn author_social_view_from_parts(
    author_pubkey: &str,
    profile: Option<&Profile>,
    relationship: Option<&AuthorRelationshipProjectionRow>,
    muted: bool,
    blocking: bool,
    blocked_by: bool,
) -> AuthorSocialView {
    AuthorSocialView {
        author_pubkey: author_pubkey.to_string(),
        name: profile.and_then(|profile| profile.name.clone()),
        display_name: profile.and_then(|profile| profile.display_name.clone()),
        about: profile.and_then(|profile| profile.about.clone()),
        picture_asset: profile_asset_view_from_ref(
            profile.and_then(|profile| profile.picture_asset.as_ref()),
        ),
        updated_at: profile.map(|profile| profile.updated_at),
        following: relationship.is_some_and(|relationship| relationship.following),
        followed_by: relationship.is_some_and(|relationship| relationship.followed_by),
        mutual: relationship.is_some_and(|relationship| relationship.mutual),
        friend_of_friend: relationship.is_some_and(|relationship| relationship.friend_of_friend),
        friend_of_friend_via_pubkeys: relationship
            .map(|relationship| relationship.friend_of_friend_via_pubkeys.clone())
            .unwrap_or_default(),
        provenance: None,
        muted,
        blocking,
        blocked_by,
    }
}

pub(crate) fn author_social_view_sort_key(
    left: &AuthorSocialView,
    right: &AuthorSocialView,
) -> std::cmp::Ordering {
    fn key(value: Option<&str>) -> (u8, String) {
        let normalized = value
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.to_ascii_lowercase());
        match normalized {
            Some(value) => (0, value),
            None => (1, String::new()),
        }
    }

    key(left.display_name.as_deref())
        .cmp(&key(right.display_name.as_deref()))
        .then_with(|| key(left.name.as_deref()).cmp(&key(right.name.as_deref())))
        .then_with(|| left.author_pubkey.cmp(&right.author_pubkey))
}
