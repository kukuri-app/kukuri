use super::*;

pub(crate) fn notification_candidate_from_verified_post(
    local_author_pubkey: &str,
    post: &VerifiedPost,
    content: Option<String>,
    reply_to_local: bool,
) -> Option<NotificationCandidate> {
    let header = post.header();
    if header.author.as_str() == local_author_pubkey {
        return None;
    }
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
    if header.reply_to.is_some() && reply_to_local {
        return Some(NotificationCandidate {
            kind: NotificationKind::Reply,
            actor_pubkey: header.author.as_str().to_string(),
            source_envelope_id: Some(header.envelope_id.clone()),
            source_replica_id: Some(post.replica().clone()),
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
        });
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
        return Some(NotificationCandidate {
            kind,
            actor_pubkey: header.author.as_str().to_string(),
            source_envelope_id: Some(header.envelope_id.clone()),
            source_replica_id: Some(post.replica().clone()),
            topic_id: Some(header.topic_id.as_str().to_string()),
            channel_id: None,
            object_id: Some(header.object_id.clone()),
            dm_id: None,
            message_id: None,
            preview_text: notification_preview_text(preview_source),
            content_labels: Some(header.content_labels.clone()),
            created_at: header.created_at,
            received_at: Utc::now().timestamp_millis(),
        });
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
        return Some(NotificationCandidate {
            kind: NotificationKind::Mention,
            actor_pubkey: header.author.as_str().to_string(),
            source_envelope_id: Some(header.envelope_id.clone()),
            source_replica_id: Some(post.replica().clone()),
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
        });
    }
    None
}

pub(crate) fn notification_candidate_from_verified_follow(
    local_author_pubkey: &str,
    replica: &ReplicaId,
    edge: &FollowEdge,
) -> Option<NotificationCandidate> {
    if edge.subject_pubkey.as_str() == local_author_pubkey
        || edge.target_pubkey.as_str() != local_author_pubkey
        || edge.status != FollowEdgeStatus::Active
    {
        return None;
    }
    Some(NotificationCandidate {
        kind: NotificationKind::Followed,
        actor_pubkey: edge.subject_pubkey.as_str().to_string(),
        source_envelope_id: Some(edge.envelope_id.clone()),
        source_replica_id: Some(replica.clone()),
        topic_id: None,
        channel_id: None,
        object_id: None,
        dm_id: None,
        message_id: None,
        preview_text: None,
        content_labels: None,
        created_at: edge.updated_at,
        received_at: Utc::now().timestamp_millis(),
    })
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
    pubkey_mentions(text).any(|candidate| candidate.eq_ignore_ascii_case(pubkey))
}

pub(crate) fn pubkey_mentions(text: &str) -> impl Iterator<Item = &str> {
    text.match_indices('@').filter_map(|(at, _)| {
        let rest = text.get(at + 1..)?;
        let candidate = rest.get(..64)?;
        (candidate.bytes().all(|byte| byte.is_ascii_hexdigit())
            && !rest.as_bytes().get(64).is_some_and(u8::is_ascii_hexdigit))
        .then_some(candidate)
    })
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
