import {
  type AuthorSocialView,
  type BookmarkedPostView,
  type ChannelAccessTokenPreview,
  type GameRoomView,
  type JoinedPrivateChannelView,
  type LiveSessionView,
  type NotificationView,
  type PostView,
  type PrivateChannelInvitePreview,
  type Profile,
  type ReactionKeyInput,
  type ReactionStateView,
  type RecentReactionView,
  type SyncStatus,
  type TimelineScope,
} from '@/lib/api';

// seed 入力は wire 契約(PostView / GameRoomView)より緩い専用型で受け、
// 既定値は with*Defaults がバックエンドのルールをミラーして補完する。
// 生成型 PostView は reaction 系を必須(wire 常在)にするが、seed 入力では
// 省略でき withSocialPostDefaults が [] を補う。is_threadable も同様に任意。
export type SeedPostInput = Omit<
  PostView,
  'is_threadable' | 'reaction_summary' | 'my_reactions'
> & {
  is_threadable?: boolean;
  reaction_summary?: PostView['reaction_summary'];
  my_reactions?: PostView['my_reactions'];
};
export type SeedGameRoomInput = Omit<GameRoomView, 'room_kind' | 'manifest_blob_hash'> & {
  room_kind?: GameRoomView['room_kind'];
  manifest_blob_hash?: string;
};

export type DesktopMockApiOptions = {
  globalLastError?: string | null;
  topicLastError?: string | null;
  assistPeerIds?: string[];
  seedPosts?: Record<string, SeedPostInput[]>;
  authorProfileTimelines?: Record<string, SeedPostInput[]>;
  seedLiveSessions?: Record<string, LiveSessionView[]>;
  seedGameRooms?: Record<string, SeedGameRoomInput[]>;
  notifications?: NotificationView[];
  myProfile?: Partial<Profile>;
  authorSocialViews?: Record<string, Partial<AuthorSocialView>>;
  myProfileError?: string | null;
  invitePreview?: PrivateChannelInvitePreview;
  /**
   * mock 上で新しく作った投稿の created_at の基準 (Unix 秒)。未指定なら 0 で、
   * 従来どおり連番がそのまま created_at になる。告知素材の撮影 (#1039) が、
   * 撮影中に作った投稿を seed の投稿と同じ日時帯に並べるために使う。
   */
  clockBase?: number;
};

export function parseMockChannelAccessTokenPreview(
  token: string,
  options: DesktopMockApiOptions,
  localAuthorPubkey: string
): ChannelAccessTokenPreview {
  if (token.startsWith('{')) {
    const parsed = JSON.parse(token) as {
      envelope?: {
        kind?: string;
        pubkey?: string;
        content?: string;
        id?: string;
      };
    };
    const content = parsed.envelope?.content
      ? (JSON.parse(parsed.envelope.content) as Record<string, string | number | null>)
      : null;
    if (!content || !parsed.envelope?.kind) {
      throw new Error('unrecognized private channel access token');
    }
    if (parsed.envelope.kind === 'channel-invite') {
      return {
        kind: 'invite',
        topic_id: String(content.topic_id ?? 'kukuri:topic:general'),
        channel_id: String(content.channel_id ?? 'channel-imported'),
        channel_label: String(content.channel_label ?? 'Imported'),
        owner_pubkey: String(content.owner_pubkey ?? localAuthorPubkey),
        inviter_pubkey: parsed.envelope.pubkey ?? localAuthorPubkey,
        sponsor_pubkey: null,
        epoch_id: String(content.epoch_id ?? 'epoch-imported-1'),
      };
    }
    if (parsed.envelope.kind === 'channel-friend-grant') {
      const ownerPubkey = String(content.owner_pubkey ?? localAuthorPubkey);
      return {
        kind: 'grant',
        topic_id: String(content.topic_id ?? 'kukuri:topic:general'),
        channel_id: String(content.channel_id ?? 'channel-friends'),
        channel_label: String(content.channel_label ?? 'Friends'),
        owner_pubkey: ownerPubkey,
        inviter_pubkey: null,
        sponsor_pubkey: ownerPubkey,
        epoch_id: String(content.epoch_id ?? 'epoch-1'),
      };
    }
    if (parsed.envelope.kind === 'channel-share') {
      return {
        kind: 'share',
        topic_id: String(content.topic_id ?? 'kukuri:topic:general'),
        channel_id: String(content.channel_id ?? 'channel-friends-plus'),
        channel_label: String(content.channel_label ?? 'Friends+'),
        owner_pubkey: String(content.owner_pubkey ?? localAuthorPubkey),
        inviter_pubkey: null,
        sponsor_pubkey: String(content.sponsor_pubkey ?? parsed.envelope.pubkey ?? localAuthorPubkey),
        epoch_id: String(content.epoch_id ?? 'epoch-plus-1'),
      };
    }
    throw new Error('unrecognized private channel access token');
  }

  if (token.startsWith('grant:')) {
    return {
      kind: 'grant',
      topic_id: 'kukuri:topic:general',
      channel_id: 'channel-friends',
      channel_label: 'Friends',
      owner_pubkey: localAuthorPubkey,
      inviter_pubkey: null,
      sponsor_pubkey: localAuthorPubkey,
      epoch_id: 'epoch-1',
    };
  }
  if (token.startsWith('share:')) {
    return {
      kind: 'share',
      topic_id: 'kukuri:topic:general',
      channel_id: 'channel-friends-plus',
      channel_label: 'Friends+',
      owner_pubkey: localAuthorPubkey,
      inviter_pubkey: null,
      sponsor_pubkey: 'sponsor-pubkey-1234',
      epoch_id: 'epoch-plus-1',
    };
  }

  const invitePreview = options.invitePreview ?? {
    channel_id: 'channel-imported',
    topic_id: 'kukuri:topic:general',
    channel_label: 'Imported',
    inviter_pubkey: localAuthorPubkey,
    owner_pubkey: localAuthorPubkey,
    epoch_id: 'epoch-imported-1',
    expires_at: null,
    namespace_secret_hex: 'a'.repeat(64),
  };
  return {
    kind: 'invite',
    topic_id: invitePreview.topic_id,
    channel_id: invitePreview.channel_id,
    channel_label: invitePreview.channel_label,
    owner_pubkey: invitePreview.owner_pubkey,
    inviter_pubkey: invitePreview.inviter_pubkey,
    sponsor_pubkey: null,
    epoch_id: invitePreview.epoch_id,
  };
}

export function withSocialPostDefaults(post: SeedPostInput): PostView {
  return {
    ...post,
    author_name: post.author_name ?? null,
    author_display_name: post.author_display_name ?? null,
    author_picture_asset: post.author_picture_asset ? { ...post.author_picture_asset } : null,
    following: post.following ?? false,
    followed_by: post.followed_by ?? false,
    mutual: post.mutual ?? false,
    friend_of_friend: post.friend_of_friend ?? false,
    published_topic_id: post.published_topic_id ?? post.origin_topic_id ?? null,
    origin_topic_id: post.origin_topic_id ?? null,
    reply_preview: post.reply_preview
      ? {
          ...post.reply_preview,
          author: {
            ...post.reply_preview.author,
            picture_asset: post.reply_preview.author.picture_asset
              ? { ...post.reply_preview.author.picture_asset }
              : null,
          },
          attachments: post.reply_preview.attachments.map((attachment) => ({ ...attachment })),
        }
      : null,
    repost_of: post.repost_of ?? null,
    repost_commentary: post.repost_commentary ?? null,
    // Rust 側のルール(timeline_runtime_support.rs)をミラー:
    // repost かつ commentary 無しのときだけ thread を開けない。
    is_threadable:
      post.is_threadable ?? (post.object_kind !== 'repost' || Boolean(post.repost_commentary)),
    channel_id: post.channel_id ?? null,
    audience_label: post.audience_label ?? (post.channel_id ? 'Private channel' : 'Public'),
    attachments: [...post.attachments],
    reaction_summary: [...(post.reaction_summary ?? [])],
    my_reactions: [...(post.my_reactions ?? [])],
    local_id: post.local_id ?? null,
    local_state: post.local_state ?? null,
    local_error: post.local_error ?? null,
    server_object_id: post.server_object_id ?? null,
    local_draft: post.local_draft
      ? {
          ...post.local_draft,
          channel_ref: post.local_draft.channel_ref ?? null,
          attachments: [...(post.local_draft.attachments ?? [])],
        }
      : null,
    local_draft_media_items: post.local_draft_media_items
      ? post.local_draft_media_items.map((item) => ({
          ...item,
          attachments: [...item.attachments],
        }))
      : null,
  };
}

export function withLiveSessionDefaults(session: LiveSessionView): LiveSessionView {
  return {
    ...session,
    channel_id: session.channel_id ?? null,
    audience_label: session.audience_label ?? (session.channel_id ? 'Private channel' : 'Public'),
  };
}

export function withGameRoomDefaults(room: SeedGameRoomInput): GameRoomView {
  return {
    ...room,
    channel_id: room.channel_id ?? null,
    audience_label: room.audience_label ?? (room.channel_id ? 'Private channel' : 'Public'),
    scores: room.scores.map((score) => ({ ...score })),
    room_kind: room.room_kind ?? 'score_game',
    metaverse: room.metaverse
      ? {
          ...room.metaverse,
          chat_history: [...(room.metaverse.chat_history ?? [])],
        }
      : null,
    manifest_blob_hash: room.manifest_blob_hash ?? `mock-${room.room_id}`,
  };
}

export function cloneBookmarkedPost(view: BookmarkedPostView): BookmarkedPostView {
  return {
    bookmarked_at: view.bookmarked_at,
    post: withSocialPostDefaults({
      ...view.post,
      attachments: view.post.attachments.map((attachment) => ({ ...attachment })),
      repost_of: view.post.repost_of
        ? {
            ...view.post.repost_of,
            attachments: view.post.repost_of.attachments.map((attachment) => ({ ...attachment })),
          }
        : null,
      reply_preview: view.post.reply_preview
        ? {
            ...view.post.reply_preview,
            author: {
              ...view.post.reply_preview.author,
              picture_asset: view.post.reply_preview.author.picture_asset
                ? { ...view.post.reply_preview.author.picture_asset }
                : null,
            },
            attachments: view.post.reply_preview.attachments.map((attachment) => ({
              ...attachment,
            })),
          }
        : null,
      reaction_summary: [...(view.post.reaction_summary ?? [])],
      my_reactions: [...(view.post.my_reactions ?? [])],
    }),
  };
}

export function withJoinedChannelDefaults(channel: JoinedPrivateChannelView): JoinedPrivateChannelView {
  return {
    ...channel,
    owner_pubkey: channel.owner_pubkey ?? channel.creator_pubkey,
    joined_via_pubkey: channel.joined_via_pubkey ?? null,
    audience_kind: channel.audience_kind ?? 'invite_only',
    is_owner: channel.is_owner ?? true,
    current_epoch_id: channel.current_epoch_id ?? 'legacy',
    archived_epoch_ids: [...(channel.archived_epoch_ids ?? [])],
    sharing_state: channel.sharing_state ?? 'open',
    rotation_required: channel.rotation_required ?? false,
    participant_count: channel.participant_count ?? 0,
    stale_participant_count: channel.stale_participant_count ?? 0,
  };
}

export function withDefaultAuthorView(
  pubkey: string,
  view?: Partial<AuthorSocialView>
): AuthorSocialView {
  return {
    author_pubkey: pubkey,
    name: null,
    display_name: null,
    about: null,
    picture_asset: null,
    updated_at: null,
    following: false,
    followed_by: false,
    mutual: false,
    friend_of_friend: false,
    friend_of_friend_via_pubkeys: [],
    muted: false,
    blocking: false,
    blocked_by: false,
    ...view,
  };
}

export function cloneAuthorView(view: AuthorSocialView): AuthorSocialView {
  return {
    ...view,
    picture_asset: view.picture_asset ? { ...view.picture_asset } : null,
    friend_of_friend_via_pubkeys: [...view.friend_of_friend_via_pubkeys],
  };
}

export function cloneNotification(view: NotificationView): NotificationView {
  return {
    ...view,
    actor_picture_asset: view.actor_picture_asset ? { ...view.actor_picture_asset } : null,
  };
}

export function cloneSyncStatus(syncStatus: SyncStatus): SyncStatus {
  return {
    ...syncStatus,
    configured_peers: [...syncStatus.configured_peers],
    subscribed_topics: [...syncStatus.subscribed_topics],
    topic_diagnostics: syncStatus.topic_diagnostics.map((diagnostic) => ({
      ...diagnostic,
      connected_peers: [...diagnostic.connected_peers],
      docs_assist_peer_ids: [...diagnostic.docs_assist_peer_ids],
      configured_peer_ids: [...diagnostic.configured_peer_ids],
      missing_peer_ids: [...diagnostic.missing_peer_ids],
    })),
    discovery: {
      ...syncStatus.discovery,
      configured_seed_peer_ids: [...syncStatus.discovery.configured_seed_peer_ids],
      bootstrap_seed_peer_ids: [...syncStatus.discovery.bootstrap_seed_peer_ids],
      manual_ticket_peer_ids: [...syncStatus.discovery.manual_ticket_peer_ids],
      connected_peer_ids: [...syncStatus.discovery.connected_peer_ids],
      docs_assist_peer_ids: [...syncStatus.discovery.docs_assist_peer_ids],
      blob_assist_peer_ids: [...syncStatus.discovery.blob_assist_peer_ids],
    },
  };
}

export function filterChannelScopedItems<T extends { channel_id?: string | null }>(
  items: T[],
  scope: TimelineScope
) {
  if (scope.kind === 'channel') {
    return items.filter((item) => item.channel_id === scope.channel_id);
  }
  return items.filter((item) => !item.channel_id);
}

export function compareAuthorViews(left: AuthorSocialView, right: AuthorSocialView) {
  const normalize = (value?: string | null) => value?.trim().toLocaleLowerCase() ?? '';
  const leftDisplay = normalize(left.display_name);
  const rightDisplay = normalize(right.display_name);
  if (leftDisplay !== rightDisplay) {
    if (!leftDisplay) {
      return 1;
    }
    if (!rightDisplay) {
      return -1;
    }
    return leftDisplay.localeCompare(rightDisplay);
  }
  const leftName = normalize(left.name);
  const rightName = normalize(right.name);
  if (leftName !== rightName) {
    if (!leftName) {
      return 1;
    }
    if (!rightName) {
      return -1;
    }
    return leftName.localeCompare(rightName);
  }
  return left.author_pubkey.localeCompare(right.author_pubkey);
}

export function normalizedReactionKey(reactionKey: ReactionKeyInput): string {
  return reactionKey.kind === 'emoji'
    ? `emoji:${reactionKey.emoji.trim()}`
    : `custom_asset:${reactionKey.asset.asset_id}`;
}

export function reactionStateForPost(post: PostView): ReactionStateView {
  return {
    target_object_id: post.object_id,
    source_replica_id: post.channel_id ?? 'public',
    reaction_summary: [...(post.reaction_summary ?? [])],
    my_reactions: [...(post.my_reactions ?? [])],
  };
}

function recentReactionFromInput(
  reactionKey: ReactionKeyInput,
  updatedAt: number
): RecentReactionView {
  return reactionKey.kind === 'emoji'
    ? {
        reaction_key_kind: 'emoji',
        normalized_reaction_key: `emoji:${reactionKey.emoji.trim()}`,
        emoji: reactionKey.emoji.trim(),
        custom_asset: null,
        updated_at: updatedAt,
      }
    : {
        reaction_key_kind: 'custom_asset',
        normalized_reaction_key: `custom_asset:${reactionKey.asset.asset_id}`,
        emoji: null,
        custom_asset: { ...reactionKey.asset },
        updated_at: updatedAt,
      };
}

export function pushRecentReaction(
  current: RecentReactionView[],
  reactionKey: ReactionKeyInput,
  updatedAt: number
): RecentReactionView[] {
  const next = recentReactionFromInput(reactionKey, updatedAt);
  return [
    next,
    ...current.filter((item) => item.normalized_reaction_key !== next.normalized_reaction_key),
  ].slice(0, 8);
}
