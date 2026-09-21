use std::collections::{BTreeSet, HashMap};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use async_trait::async_trait;
use kukuri_core::{
    BlobHash, BlockEdge, EnvelopeId, FollowEdge, KukuriEnvelope, Profile, ReplicaId,
};

use crate::models::{
    AuthorRelationshipProjectionRow, BlobCacheStatus, BookmarkedCustomReactionRow,
    BookmarkedPostRow, ContentObservationRow, DirectMessageConversationRow,
    DirectMessageMessageRow, DirectMessageOutboxRow, DirectMessageTombstoneRow,
    DomeConnectionProjectionRow, DomeHostingProjectionRow, GameRoomProjectionRow,
    LiveSessionProjectionRow, MutedAuthorRow, NotificationRow, ObjectProjectionRow, Page,
    PostWithdrawalRow, ReactionProjectionRow, TimelineCursor,
};

pub(crate) const CONTENT_OBSERVATION_RETENTION_MS: i64 = 90 * 24 * 60 * 60 * 1000;

#[async_trait]
pub trait Store: Send + Sync {
    async fn put_envelope(&self, envelope: KukuriEnvelope) -> Result<()>;
    async fn get_envelope(&self, envelope_id: &EnvelopeId) -> Result<Option<KukuriEnvelope>>;
    async fn list_topic_timeline(
        &self,
        topic_id: &str,
        cursor: Option<TimelineCursor>,
        limit: usize,
    ) -> Result<Page<KukuriEnvelope>>;
    async fn list_thread(
        &self,
        topic_id: &str,
        thread_root_object_id: &EnvelopeId,
        cursor: Option<TimelineCursor>,
        limit: usize,
    ) -> Result<Page<KukuriEnvelope>>;
    async fn upsert_profile(&self, profile: Profile) -> Result<()>;
    async fn get_profile(&self, pubkey: &str) -> Result<Option<Profile>>;
    async fn get_profiles(&self, pubkeys: &[String]) -> Result<HashMap<String, Profile>> {
        let mut profiles = HashMap::new();
        for pubkey in pubkeys {
            if let Some(profile) = self.get_profile(pubkey.as_str()).await? {
                profiles.insert(pubkey.clone(), profile);
            }
        }
        Ok(profiles)
    }
    async fn upsert_follow_edge(&self, edge: FollowEdge) -> Result<()>;
    async fn list_follow_edges_by_subject(&self, subject_pubkey: &str) -> Result<Vec<FollowEdge>>;
    async fn list_follow_edges_by_target(&self, target_pubkey: &str) -> Result<Vec<FollowEdge>>;
    async fn upsert_block_edge(&self, edge: BlockEdge) -> Result<()>;
    async fn list_block_edges_by_subject(&self, subject_pubkey: &str) -> Result<Vec<BlockEdge>>;
    async fn list_block_edges_by_target(&self, target_pubkey: &str) -> Result<Vec<BlockEdge>>;
}

// ProjectionStore はドメイン別 sub-trait の supertrait 合成(WP-H1)。
// 実装側(sqlite / memory)はサブモジュール毎に対応する sub-trait を直接 impl し、
// blanket impl により全 sub-trait を実装する型が自動的に ProjectionStore になる。
// 注入点(`dyn ProjectionStore`)は従来どおり全ドメインを提供する。

/// object projection とタイムライン / スレッド読み出し(実装: sqlite/projections.rs)。
#[async_trait]
pub trait ObjectProjectionStore: Send + Sync {
    async fn put_object_projection(&self, row: ObjectProjectionRow) -> Result<()>;
    async fn put_object_projections(&self, rows: Vec<ObjectProjectionRow>) -> Result<()> {
        put_object_projections_one_by_one(self, rows).await
    }
    async fn get_object_projection(
        &self,
        object_id: &EnvelopeId,
    ) -> Result<Option<ObjectProjectionRow>>;
    /// #1239: `author_pubkey` が `topic_id` に書いた、`source_object_id` の repost を新しい順に最大 `limit` 件返す。
    /// 自分の既存の repost の検索に使う。docs の replica を走査せず、projection の索引だけで引く。
    async fn find_author_reposts_of(
        &self,
        topic_id: &str,
        author_pubkey: &str,
        source_object_id: &EnvelopeId,
        limit: usize,
    ) -> Result<Vec<ObjectProjectionRow>>;
    /// #858: 成人向けラベル付き投稿の添付として観測済みの blob hash を記録する
    /// (insert-only)。projection 書き込み時は実装側が自動で記録するが、projection を
    /// 経由しない表示経路(profile timeline 等)からも明示的に記録できるようにする。
    async fn mark_adult_media_hashes(&self, hashes: &[BlobHash]) -> Result<()>;
    /// #858: 対象 hash が成人向けラベル付き投稿の添付として観測済みか。
    /// blob 取得ゲート(`blob_media_payload`)の判定に使う。
    async fn is_adult_media_hash(&self, hash: &BlobHash) -> Result<bool>;
    async fn list_topic_timeline(
        &self,
        topic_id: &str,
        cursor: Option<TimelineCursor>,
        limit: usize,
    ) -> Result<Page<ObjectProjectionRow>>;
    async fn list_topic_timeline_filtered(
        &self,
        topic_id: &str,
        allowed_channels: &BTreeSet<String>,
        cursor: Option<TimelineCursor>,
        limit: usize,
    ) -> Result<Page<ObjectProjectionRow>> {
        scan_projection_pages_filtered(
            cursor,
            limit,
            |cursor, page_size| self.list_topic_timeline(topic_id, cursor, page_size),
            |row| allowed_channels.contains(row.channel_id.as_str()),
        )
        .await
    }
    async fn list_thread(
        &self,
        topic_id: &str,
        thread_root_object_id: &EnvelopeId,
        cursor: Option<TimelineCursor>,
        limit: usize,
    ) -> Result<Page<ObjectProjectionRow>>;
    async fn list_thread_filtered(
        &self,
        topic_id: &str,
        thread_root_object_id: &EnvelopeId,
        allowed_channel: Option<&str>,
        cursor: Option<TimelineCursor>,
        limit: usize,
    ) -> Result<Page<ObjectProjectionRow>> {
        scan_projection_pages_filtered(
            cursor,
            limit,
            |cursor, page_size| {
                self.list_thread(topic_id, thread_root_object_id, cursor, page_size)
            },
            |row| allowed_channel.is_none_or(|channel_id| row.channel_id == channel_id),
        )
        .await
    }
    async fn rebuild_object_projections(&self, rows: Vec<ObjectProjectionRow>) -> Result<()>;
}

/// 内容をどのコミュニティノード経由で観測したかを保持する端末内記録。
#[async_trait]
pub trait ContentObservationStore: Send + Sync {
    /// 対象が端末内に存在する場合だけ記録し、保存した場合は true を返す。
    async fn put_content_observation(&self, row: ContentObservationRow) -> Result<bool>;
    async fn list_content_observations(
        &self,
        subject_kind: &str,
        subject_id: &str,
    ) -> Result<Vec<ContentObservationRow>> {
        let now_millis = i64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis())?;
        self.list_content_observations_at(subject_kind, subject_id, now_millis)
            .await
    }
    /// 保持期間の契約試験で基準時刻を固定するための読み取り境界。
    async fn list_content_observations_at(
        &self,
        subject_kind: &str,
        subject_id: &str,
        now_millis: i64,
    ) -> Result<Vec<ContentObservationRow>>;
}

/// 著者署名付き投稿撤回の端末内最小 ledger。
#[async_trait]
pub trait PostWithdrawalStore: Send + Sync {
    /// 世代、撤回時刻、envelope ID の順で新しい事象だけを採用する。
    async fn put_post_withdrawal(&self, row: PostWithdrawalRow) -> Result<bool>;
    async fn get_post_withdrawal(
        &self,
        target_object_id: &EnvelopeId,
    ) -> Result<Option<PostWithdrawalRow>>;
}

/// `put_object_projections` の既定動作: 1 件ずつ `put_object_projection` を呼ぶ。
pub(crate) async fn put_object_projections_one_by_one<S>(
    store: &S,
    rows: Vec<ObjectProjectionRow>,
) -> Result<()>
where
    S: ObjectProjectionStore + ?Sized,
{
    for row in rows {
        store.put_object_projection(row).await?;
    }
    Ok(())
}

/// filtered 系デフォルト実装の共通ループ: ページを順に読みながら述語に合う行を
/// limit 件まで集める(バックエンド側で絞り込めない場合のフォールバック動作)。
pub(crate) async fn scan_projection_pages_filtered<F, Fut>(
    cursor: Option<TimelineCursor>,
    limit: usize,
    mut fetch_page: F,
    keep: impl Fn(&ObjectProjectionRow) -> bool,
) -> Result<Page<ObjectProjectionRow>>
where
    F: FnMut(Option<TimelineCursor>, usize) -> Fut,
    Fut: std::future::Future<Output = Result<Page<ObjectProjectionRow>>>,
{
    if limit == 0 {
        return Ok(Page {
            items: Vec::new(),
            next_cursor: cursor,
        });
    }

    let mut current_cursor = cursor;
    let mut items = Vec::new();
    let page_size = limit.max(20);
    loop {
        let page = fetch_page(current_cursor.clone(), page_size).await?;
        let next_cursor = page.next_cursor.clone();
        for row in page.items {
            if keep(&row) {
                items.push(row);
                if items.len() >= limit {
                    return Ok(Page { items, next_cursor });
                }
            }
        }
        if next_cursor.is_none() {
            return Ok(Page { items, next_cursor });
        }
        current_cursor = next_cursor;
    }
}

/// ライブセッション / ゲームルーム / live presence(実装: sqlite/live_game.rs)。
#[async_trait]
pub trait LiveGameProjectionStore: Send + Sync {
    async fn upsert_live_session_cache(&self, row: LiveSessionProjectionRow) -> Result<()>;
    async fn list_topic_live_sessions(
        &self,
        topic_id: &str,
    ) -> Result<Vec<LiveSessionProjectionRow>>;
    async fn upsert_game_room_cache(&self, row: GameRoomProjectionRow) -> Result<()>;
    async fn list_topic_game_rooms(&self, topic_id: &str) -> Result<Vec<GameRoomProjectionRow>>;
    async fn upsert_dome_connection_projection(
        &self,
        row: DomeConnectionProjectionRow,
    ) -> Result<()>;
    async fn get_dome_connection_projection(
        &self,
        context_id: &str,
    ) -> Result<Option<DomeConnectionProjectionRow>>;
    async fn upsert_dome_hosting_projection(&self, row: DomeHostingProjectionRow) -> Result<()>;
    async fn get_dome_hosting_projection(
        &self,
        instance_id: &str,
    ) -> Result<Option<DomeHostingProjectionRow>>;
    async fn upsert_live_presence(
        &self,
        topic_id: &str,
        channel_id: &str,
        session_id: &str,
        author_pubkey: &str,
        expires_at: i64,
        updated_at: i64,
    ) -> Result<()>;
    async fn clear_topic_live_presence(&self, topic_id: &str) -> Result<()>;
    async fn clear_expired_live_presence(&self, now_ms: i64) -> Result<()>;
}

/// profile cache / 作者関係 / ミュート(実装: sqlite/social.rs)。
#[async_trait]
pub trait SocialProjectionStore: Send + Sync {
    async fn upsert_profile_cache(&self, profile: Profile) -> Result<()>;
    async fn get_author_relationship(
        &self,
        local_author_pubkey: &str,
        author_pubkey: &str,
    ) -> Result<Option<AuthorRelationshipProjectionRow>>;
    async fn list_author_relationships(
        &self,
        local_author_pubkey: &str,
        author_pubkeys: &[String],
    ) -> Result<HashMap<String, AuthorRelationshipProjectionRow>> {
        list_author_relationships_one_by_one(self, local_author_pubkey, author_pubkeys).await
    }
    async fn rebuild_author_relationships(
        &self,
        local_author_pubkey: &str,
        rows: Vec<AuthorRelationshipProjectionRow>,
    ) -> Result<()>;
    async fn put_muted_author(&self, row: MutedAuthorRow) -> Result<()>;
    async fn get_muted_author(&self, author_pubkey: &str) -> Result<Option<MutedAuthorRow>>;
    async fn list_muted_authors(&self) -> Result<Vec<MutedAuthorRow>>;
    async fn remove_muted_author(&self, author_pubkey: &str) -> Result<()>;
    /// 背景で小分けに進む docs の読み出しの進み具合(#1239)。無ければ `None`。
    async fn get_sync_checkpoint(&self, checkpoint_key: &str) -> Result<Option<String>>;
    /// 背景で小分けに進む docs の読み出しの進み具合を書く(#1239)。同じ key は上書きする。
    async fn put_sync_checkpoint(&self, checkpoint_key: &str, value: &str) -> Result<()>;
    /// author が署名つきの envelope で申告した docs author の id(#1239、ADR 0053 §6)。無ければ `None`。
    async fn get_author_docs_author(&self, author_pubkey: &str) -> Result<Option<String>>;
    /// author が署名つきの envelope で申告した docs author の id を書く(#1239)。同じ author は上書きする。
    async fn put_author_docs_author(&self, author_pubkey: &str, docs_author: &str) -> Result<()>;
}

/// `list_author_relationships` の既定動作: 1 件ずつ `get_author_relationship` を呼ぶ。
pub(crate) async fn list_author_relationships_one_by_one<S>(
    store: &S,
    local_author_pubkey: &str,
    author_pubkeys: &[String],
) -> Result<HashMap<String, AuthorRelationshipProjectionRow>>
where
    S: SocialProjectionStore + ?Sized,
{
    let mut relationships = HashMap::new();
    for author_pubkey in author_pubkeys {
        if let Some(relationship) = store
            .get_author_relationship(local_author_pubkey, author_pubkey.as_str())
            .await?
        {
            relationships.insert(author_pubkey.clone(), relationship);
        }
    }
    Ok(relationships)
}

/// blob の取得状態(実装: sqlite/bookmarks.rs。意味的に独立させた最小 trait)。
#[async_trait]
pub trait BlobCacheStore: Send + Sync {
    async fn mark_blob_status(&self, hash: &BlobHash, status: BlobCacheStatus) -> Result<()>;
    async fn mark_blob_statuses(&self, rows: Vec<(BlobHash, BlobCacheStatus)>) -> Result<()> {
        mark_blob_statuses_one_by_one(self, rows).await
    }
}

/// `mark_blob_statuses` の既定動作: 1 件ずつ `mark_blob_status` を呼ぶ。
pub(crate) async fn mark_blob_statuses_one_by_one<S>(
    store: &S,
    rows: Vec<(BlobHash, BlobCacheStatus)>,
) -> Result<()>
where
    S: BlobCacheStore + ?Sized,
{
    for (hash, status) in rows {
        store.mark_blob_status(&hash, status).await?;
    }
    Ok(())
}

/// リアクション cache / カスタムリアクション / ブックマーク(実装: sqlite/bookmarks.rs)。
#[async_trait]
pub trait ReactionBookmarkStore: Send + Sync {
    async fn upsert_reaction_cache(&self, row: ReactionProjectionRow) -> Result<()>;
    async fn get_reaction_cache(
        &self,
        source_replica_id: &ReplicaId,
        target_object_id: &EnvelopeId,
        reaction_id: &EnvelopeId,
    ) -> Result<Option<ReactionProjectionRow>>;
    async fn list_reaction_cache_for_target(
        &self,
        source_replica_id: &ReplicaId,
        target_object_id: &EnvelopeId,
    ) -> Result<Vec<ReactionProjectionRow>>;
    async fn list_reaction_cache_for_targets(
        &self,
        source_replica_id: &ReplicaId,
        target_object_ids: &[EnvelopeId],
    ) -> Result<HashMap<String, Vec<ReactionProjectionRow>>> {
        list_reaction_cache_for_targets_one_by_one(self, source_replica_id, target_object_ids).await
    }
    async fn list_recent_reaction_cache_by_author(
        &self,
        author_pubkey: &str,
    ) -> Result<Vec<ReactionProjectionRow>>;
    async fn put_bookmarked_custom_reaction(&self, row: BookmarkedCustomReactionRow) -> Result<()>;
    async fn list_bookmarked_custom_reactions(&self) -> Result<Vec<BookmarkedCustomReactionRow>>;
    async fn remove_bookmarked_custom_reaction(&self, asset_id: &str) -> Result<()>;
    async fn put_bookmarked_post(&self, row: BookmarkedPostRow) -> Result<()>;
    async fn list_bookmarked_posts(&self) -> Result<Vec<BookmarkedPostRow>>;
    async fn remove_bookmarked_post(&self, source_object_id: &EnvelopeId) -> Result<()>;
}

/// `list_reaction_cache_for_targets` の既定動作: 対象毎に `list_reaction_cache_for_target` を呼ぶ。
pub(crate) async fn list_reaction_cache_for_targets_one_by_one<S>(
    store: &S,
    source_replica_id: &ReplicaId,
    target_object_ids: &[EnvelopeId],
) -> Result<HashMap<String, Vec<ReactionProjectionRow>>>
where
    S: ReactionBookmarkStore + ?Sized,
{
    let mut reactions = HashMap::new();
    for target_object_id in target_object_ids {
        reactions.insert(
            target_object_id.as_str().to_string(),
            store
                .list_reaction_cache_for_target(source_replica_id, target_object_id)
                .await?,
        );
    }
    Ok(reactions)
}

/// ダイレクトメッセージの会話 / メッセージ / outbox / tombstone(実装: sqlite/direct_messages.rs)。
#[async_trait]
pub trait DirectMessageStore: Send + Sync {
    async fn upsert_direct_message_conversation(
        &self,
        row: DirectMessageConversationRow,
    ) -> Result<()>;
    async fn get_direct_message_conversation_by_peer(
        &self,
        peer_pubkey: &str,
    ) -> Result<Option<DirectMessageConversationRow>>;
    async fn get_direct_message_conversation_by_dm_id(
        &self,
        dm_id: &str,
    ) -> Result<Option<DirectMessageConversationRow>>;
    async fn list_direct_message_conversations(&self) -> Result<Vec<DirectMessageConversationRow>>;
    async fn put_direct_message_message(&self, row: DirectMessageMessageRow) -> Result<()>;
    async fn get_direct_message_message(
        &self,
        dm_id: &str,
        message_id: &str,
    ) -> Result<Option<DirectMessageMessageRow>>;
    async fn list_direct_message_messages(
        &self,
        dm_id: &str,
        cursor: Option<TimelineCursor>,
        limit: usize,
    ) -> Result<Page<DirectMessageMessageRow>>;
    async fn set_direct_message_acked_at(
        &self,
        dm_id: &str,
        message_id: &str,
        acked_at: i64,
    ) -> Result<()>;
    async fn put_direct_message_outbox(&self, row: DirectMessageOutboxRow) -> Result<()>;
    async fn get_direct_message_outbox(
        &self,
        dm_id: &str,
        message_id: &str,
    ) -> Result<Option<DirectMessageOutboxRow>>;
    async fn list_direct_message_outbox(&self) -> Result<Vec<DirectMessageOutboxRow>>;
    async fn touch_direct_message_outbox_attempt(
        &self,
        dm_id: &str,
        message_id: &str,
        attempted_at: i64,
    ) -> Result<()>;
    async fn remove_direct_message_outbox(&self, dm_id: &str, message_id: &str) -> Result<()>;
    async fn put_direct_message_tombstone(&self, row: DirectMessageTombstoneRow) -> Result<()>;
    async fn list_direct_message_tombstones(
        &self,
        dm_id: &str,
    ) -> Result<Vec<DirectMessageTombstoneRow>>;
    async fn has_direct_message_tombstone(&self, dm_id: &str, message_id: &str) -> Result<bool>;
    async fn delete_direct_message_message_local(
        &self,
        dm_id: &str,
        message_id: &str,
    ) -> Result<()>;
    async fn clear_direct_message_local(&self, dm_id: &str) -> Result<()>;
}

/// 通知(実装: sqlite/notifications.rs)。
#[async_trait]
pub trait NotificationStore: Send + Sync {
    async fn put_notification_if_absent(&self, row: NotificationRow) -> Result<bool>;
    async fn list_notifications(&self) -> Result<Vec<NotificationRow>>;
    async fn mark_notification_read(&self, notification_id: &str, read_at: i64) -> Result<()>;
    async fn mark_all_notifications_read(&self, read_at: i64) -> Result<()>;
    async fn count_unread_notifications(&self) -> Result<usize>;
}

/// 全ドメインを提供する projection store(sub-trait の supertrait 合成)。
///
/// 注入点はこれまでどおり `dyn ProjectionStore` を使える。個別ドメインだけが必要な
/// 場面では対応する sub-trait を直接使う。
pub trait ProjectionStore:
    ObjectProjectionStore
    + ContentObservationStore
    + PostWithdrawalStore
    + LiveGameProjectionStore
    + SocialProjectionStore
    + BlobCacheStore
    + ReactionBookmarkStore
    + DirectMessageStore
    + NotificationStore
{
}

impl<T> ProjectionStore for T where
    T: ObjectProjectionStore
        + ContentObservationStore
        + PostWithdrawalStore
        + LiveGameProjectionStore
        + SocialProjectionStore
        + BlobCacheStore
        + ReactionBookmarkStore
        + DirectMessageStore
        + NotificationStore
{
}
