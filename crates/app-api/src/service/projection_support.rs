use super::*;

/// 非表示の著者の行を読み飛ばすために、1 回の取得で読む projection のページ数の上限(ADR 0052 §5)。
/// 非表示の著者の投稿が続く範囲でも、1 回の取得が読む行数を定数で抑える。
pub(crate) const HIDDEN_AUTHOR_SKIP_PAGES: usize = 4;

enum VisibleRows {
    /// `limit` 件に届いた、または行が尽きた。値は、返すページの `next_cursor`。
    Done(Option<TimelineCursor>),
    /// まだ `limit` 件に届かず、続きがある。値は、次に読むページの cursor。
    Continue(TimelineCursor),
}

/// 読んだページから、非表示の著者の行を除いて `items` へ足す。
///
/// `limit` 件に届いたときの `next_cursor` は、ページの末尾ではなく、最後に返した行の位置にする。ページの末尾に
/// すると、同じページの残りの行(まだ返していない行)を次の取得が飛ばしてしまう。
fn take_visible_rows(
    page: Page<ObjectProjectionRow>,
    items: &mut Vec<ObjectProjectionRow>,
    limit: usize,
    hidden_author_pubkeys: &BTreeSet<String>,
) -> VisibleRows {
    let page_next = page.next_cursor;
    let rows = page.items.len();
    for (index, row) in page.items.into_iter().enumerate() {
        if object_projection_row_is_hidden(&row, hidden_author_pubkeys) {
            continue;
        }
        let position = TimelineCursor {
            created_at: row.created_at,
            object_id: row.object_id.clone(),
        };
        items.push(row);
        if items.len() >= limit {
            let exhausted = index + 1 == rows && page_next.is_none();
            return VisibleRows::Done((!exhausted).then_some(position));
        }
    }
    match page_next {
        Some(next_cursor) => VisibleRows::Continue(next_cursor),
        None => VisibleRows::Done(None),
    }
}

pub(crate) async fn filtered_timeline_page(
    projection_store: &dyn ProjectionStore,
    topic_id: &str,
    cursor: Option<TimelineCursor>,
    limit: usize,
    channel_id: &str,
    hidden_author_pubkeys: &BTreeSet<String>,
) -> Result<Page<ObjectProjectionRow>> {
    if limit == 0 {
        return Ok(Page {
            items: Vec::new(),
            next_cursor: cursor,
        });
    }
    let mut current_cursor = cursor;
    let mut items = Vec::new();
    let page_size = limit.max(20);
    for _ in 0..HIDDEN_AUTHOR_SKIP_PAGES {
        let page = ObjectProjectionStore::list_topic_timeline_in_channel(
            projection_store,
            topic_id,
            channel_id,
            current_cursor.clone(),
            page_size,
        )
        .await?;
        match take_visible_rows(page, &mut items, limit, hidden_author_pubkeys) {
            VisibleRows::Done(next_cursor) => return Ok(Page { items, next_cursor }),
            VisibleRows::Continue(next_cursor) => current_cursor = Some(next_cursor),
        }
    }
    // 非表示の著者の行が続いて、上限まで読んでも `limit` 件に届かなかった。集まった分と、読み進めた位置を返す
    // (呼び出し側は、その位置から続きを取得できる)。
    Ok(Page {
        items,
        next_cursor: current_cursor,
    })
}

pub(crate) async fn filtered_thread_page(
    projection_store: &dyn ProjectionStore,
    topic_id: &str,
    thread_root_object_id: &EnvelopeId,
    cursor: Option<TimelineCursor>,
    limit: usize,
    allowed_channel: Option<&str>,
    hidden_author_pubkeys: &BTreeSet<String>,
) -> Result<Page<ObjectProjectionRow>> {
    if limit == 0 {
        return Ok(Page {
            items: Vec::new(),
            next_cursor: cursor,
        });
    }
    let mut current_cursor = cursor;
    let mut items = Vec::new();
    let page_size = limit.max(20);
    for _ in 0..HIDDEN_AUTHOR_SKIP_PAGES {
        let page = ObjectProjectionStore::list_thread_filtered(
            projection_store,
            topic_id,
            thread_root_object_id,
            allowed_channel,
            current_cursor.clone(),
            page_size,
        )
        .await?;
        match take_visible_rows(page, &mut items, limit, hidden_author_pubkeys) {
            VisibleRows::Done(next_cursor) => return Ok(Page { items, next_cursor }),
            VisibleRows::Continue(next_cursor) => current_cursor = Some(next_cursor),
        }
    }
    // 非表示の著者の行が続いて、上限まで読んでも `limit` 件に届かなかった。集まった分と、読み進めた位置を返す
    // (呼び出し側は、その位置から続きを取得できる)。
    Ok(Page {
        items,
        next_cursor: current_cursor,
    })
}

pub(crate) fn filter_channel_rows<T>(
    rows: Vec<T>,
    allowed_channels: &BTreeSet<String>,
    channel_id: impl Fn(&T) -> &str,
) -> Vec<T> {
    rows.into_iter()
        .filter(|row| allowed_channels.contains(channel_id(row)))
        .collect()
}

pub(crate) fn object_projection_row_is_hidden(
    row: &ObjectProjectionRow,
    hidden_author_pubkeys: &BTreeSet<String>,
) -> bool {
    hidden_author_pubkeys.contains(row.author_pubkey.as_str())
        || row.repost_of.as_ref().is_some_and(|snapshot| {
            hidden_author_pubkeys.contains(snapshot.source_author_pubkey.as_str())
        })
}

pub(crate) fn bookmarked_post_row_is_hidden(
    row: &BookmarkedPostRow,
    hidden_author_pubkeys: &BTreeSet<String>,
) -> bool {
    hidden_author_pubkeys.contains(row.author_pubkey.as_str())
        || row.repost_of.as_ref().is_some_and(|snapshot| {
            hidden_author_pubkeys.contains(snapshot.source_author_pubkey.as_str())
        })
}

pub(crate) fn profile_timeline_item_is_hidden(
    item: &ProfileTimelineItem,
    hidden_author_pubkeys: &BTreeSet<String>,
) -> bool {
    match item {
        ProfileTimelineItem::Post(post) => {
            hidden_author_pubkeys.contains(post.author_pubkey.as_str())
        }
        ProfileTimelineItem::Repost(repost) => {
            hidden_author_pubkeys.contains(repost.author_pubkey.as_str())
                || hidden_author_pubkeys.contains(repost.repost_of.source_author_pubkey.as_str())
        }
    }
}

// 互換パス(REFACTORING.md「互換パスと sunset 条件」参照)。
// epoch 導入前に保存されたプライベートチャンネル capability(epoch_id が空)を "legacy"
// epoch として扱い、epoch なし時代のレプリカ ID で読む(private_channel_replica_for_epoch /
// joined_private_channel_state_from_capability)。現行ビルドが空の epoch_id を新規に書くことはない。
// 撤去条件(WP-C8 で確定): 正式リリースの節目で削除可(preview データの保全は保証しない
// 方針)。削除時は空の epoch_id を持つ capability をエラーで拒否すること(黙って読めなく
// ならないようにする)。
pub(crate) fn legacy_epoch_id() -> &'static str {
    "legacy"
}

pub(crate) fn private_channel_is_epoch_aware(audience_kind: &ChannelAudienceKind) -> bool {
    let _ = audience_kind;
    true
}

pub(crate) fn initial_private_channel_epoch_id(
    audience_kind: &ChannelAudienceKind,
    now_ms: i64,
    owner_pubkey: &str,
) -> String {
    let _ = audience_kind;
    format!("epoch-{now_ms}-{}", short_id_suffix(owner_pubkey))
}

pub(crate) fn next_private_channel_epoch_id(owner_pubkey: &str) -> String {
    format!(
        "epoch-{}-{}",
        Utc::now().timestamp_millis(),
        short_id_suffix(owner_pubkey)
    )
}

pub(crate) fn private_channel_replica_for_epoch(channel_id: &str, epoch_id: &str) -> ReplicaId {
    if epoch_id == legacy_epoch_id() {
        return private_channel_replica_id(channel_id);
    }
    private_channel_epoch_replica_id(channel_id, epoch_id)
}

pub(crate) fn current_private_channel_replica_id(state: &JoinedPrivateChannelState) -> ReplicaId {
    private_channel_replica_for_epoch(state.channel_id.as_str(), state.current_epoch_id.as_str())
}

pub(crate) fn private_channel_epoch_capabilities(
    state: &JoinedPrivateChannelState,
) -> Vec<PrivateChannelEpochCapability> {
    let mut items = vec![PrivateChannelEpochCapability {
        epoch_id: state.current_epoch_id.clone(),
        namespace_secret_hex: state.current_epoch_secret_hex.clone(),
    }];
    for epoch in &state.archived_epochs {
        if items.iter().any(|item| item.epoch_id == epoch.epoch_id) {
            continue;
        }
        items.push(epoch.clone());
    }
    items
}

pub(crate) fn joined_private_channel_state_from_capability(
    capability: PrivateChannelCapability,
) -> Result<JoinedPrivateChannelState> {
    let current_epoch_id = if capability.current_epoch_id.trim().is_empty() {
        legacy_epoch_id().to_string()
    } else {
        capability.current_epoch_id
    };
    let current_epoch_secret_hex = if capability.current_epoch_secret_hex.trim().is_empty() {
        capability.namespace_secret_hex.clone()
    } else {
        capability.current_epoch_secret_hex
    };
    if current_epoch_secret_hex.trim().is_empty() {
        anyhow::bail!("private channel capability is missing current epoch secret");
    }
    let owner_pubkey = if capability.owner_pubkey.trim().is_empty() {
        capability.creator_pubkey.clone()
    } else {
        capability.owner_pubkey
    };
    Ok(JoinedPrivateChannelState {
        topic_id: capability.topic_id,
        channel_id: ChannelId::new(capability.channel_id),
        label: capability.label.trim().to_string(),
        creator_pubkey: capability.creator_pubkey,
        owner_pubkey,
        joined_via_pubkey: capability.joined_via_pubkey,
        audience_kind: capability.audience_kind,
        current_epoch_id,
        current_epoch_secret_hex,
        archived_epochs: capability.archived_epochs,
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn merged_private_channel_state_from_epoch_join(
    existing: Option<JoinedPrivateChannelState>,
    topic_id: &str,
    channel_id: ChannelId,
    label: &str,
    creator_pubkey: &str,
    owner_pubkey: &str,
    joined_via_pubkey: Option<&str>,
    audience_kind: ChannelAudienceKind,
    epoch_id: &str,
    namespace_secret_hex: &str,
) -> JoinedPrivateChannelState {
    let mut archived_epochs = existing
        .as_ref()
        .map(|state| state.archived_epochs.clone())
        .unwrap_or_default();
    archived_epochs.retain(|epoch| epoch.epoch_id != epoch_id);
    if let Some(existing_state) = existing.as_ref()
        && existing_state.current_epoch_id != epoch_id
        && !archived_epochs
            .iter()
            .any(|epoch| epoch.epoch_id == existing_state.current_epoch_id)
    {
        archived_epochs.push(PrivateChannelEpochCapability {
            epoch_id: existing_state.current_epoch_id.clone(),
            namespace_secret_hex: existing_state.current_epoch_secret_hex.clone(),
        });
    }
    JoinedPrivateChannelState {
        topic_id: topic_id.to_string(),
        channel_id,
        label: label.to_string(),
        creator_pubkey: creator_pubkey.to_string(),
        owner_pubkey: owner_pubkey.to_string(),
        joined_via_pubkey: joined_via_pubkey.map(str::to_string),
        audience_kind,
        current_epoch_id: epoch_id.to_string(),
        current_epoch_secret_hex: namespace_secret_hex.to_string(),
        archived_epochs,
    }
}

pub(crate) fn archive_private_channel_epoch(
    state: &mut JoinedPrivateChannelState,
    epoch_id: &str,
    namespace_secret_hex: &str,
) {
    if state
        .archived_epochs
        .iter()
        .any(|epoch| epoch.epoch_id == epoch_id)
    {
        return;
    }
    state.archived_epochs.push(PrivateChannelEpochCapability {
        epoch_id: epoch_id.to_string(),
        namespace_secret_hex: namespace_secret_hex.to_string(),
    });
}

pub(crate) fn active_private_channel_participants(
    participants: &[PrivateChannelParticipantDocV1],
    epoch_id: &str,
) -> Vec<PrivateChannelParticipantDocV1> {
    participants
        .iter()
        .filter(|participant| participant.epoch_id == epoch_id && participant.left_at.is_none())
        .cloned()
        .collect()
}
