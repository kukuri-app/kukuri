import {
  type AuthorSocialView,
  type BlobMediaPayload,
  type BookmarkedCustomReactionView,
  type BookmarkedPostView,
  type CommunityNodeConfig,
  type CommunityNodeNodeStatus,
  type ContentAdvisory,
  type CustomReactionAssetView,
  type DirectMessageConversationView,
  type DirectMessageMessageView,
  type DirectMessageStatusView,
  type DiscoveryConfig,
  type GameRoomView,
  type JoinedPrivateChannelView,
  type LiveSessionView,
  type MetaverseRoomEventView,
  type NotificationView,
  type PostView,
  type Profile,
  type RecentReactionView,
  type SocialConnectionKind,
  type SyncStatus,
  type TimelineView,
} from '@/lib/api';

import {
  cloneAuthorView,
  cloneNotification,
  compareAuthorViews,
  withDefaultAuthorView,
  withSocialPostDefaults,
  withGameRoomDefaults,
  withLiveSessionDefaults,
  type DesktopMockApiOptions,
} from './desktopMockModel';

type ConsentItems = NonNullable<CommunityNodeNodeStatus['consent_state']>['items'];

/// mock 実装が共有する可変状態とヘルパ(WP-H7 PR2)。
/// ドメイン別ファイル(mocks/api/*)がこの runtime を受け取り、
/// const レコード・ヘルパは分割代入で bare 参照のまま使い、
/// 再代入されるスカラー(sequence / myProfile / notifications /
/// recentReactions / discoveryConfig / communityNode*)だけ runtime.X で読み書きする。
export interface MockRuntime {
  options: DesktopMockApiOptions | undefined;
  assistPeerIds: string[];
  // in-place で変異する const レコード(分割代入で使う)
  postsByTopic: Record<string, TimelineView['items']>;
  authorProfileTimelines: Record<string, TimelineView['items']>;
  liveSessionsByTopic: Record<string, LiveSessionView[]>;
  gameRoomsByTopic: Record<string, GameRoomView[]>;
  metaverseRoomEventsByRoom: Record<string, MetaverseRoomEventView[]>;
  metaverseAssetPayloads: Record<string, BlobMediaPayload>;
  joinedChannelsByTopic: Record<string, JoinedPrivateChannelView[]>;
  syncStatus: SyncStatus;
  authorSocialViews: Record<string, AuthorSocialView>;
  directMessageMessagesByPeer: Record<string, DirectMessageMessageView[]>;
  openedDirectMessagePeers: Set<string>;
  ownedCustomReactionAssets: CustomReactionAssetView[];
  bookmarkedCustomReactionAssets: BookmarkedCustomReactionView[];
  bookmarkedPosts: BookmarkedPostView[];
  // 再代入されるスカラー(runtime.X で読み書きする)
  sequence: number;
  myProfile: Profile;
  notifications: NotificationView[];
  recentReactions: RecentReactionView[];
  discoveryConfig: DiscoveryConfig;
  communityNodeConfig: CommunityNodeConfig;
  communityNodeStatuses: CommunityNodeNodeStatus[];
  // #1056: 一括照会で返す content advisory と、その発行元(manifest node_id)。
  contentAdvisories: ContentAdvisory[];
  contentAdvisoryIssuerNodeId: string;
  // ヘルパ
  mockConsentItems: (accepted: boolean) => ConsentItems;
  mutedAuthorPubkeys: () => Set<string>;
  withCurrentRelationship: (post: PostView) => PostView;
  isVisiblePost: (post: PostView) => boolean;
  visibleTimelineItems: (items: PostView[]) => PostView[];
  listConnections: (kind: SocialConnectionKind) => AuthorSocialView[];
  directMessageStatusFor: (pubkey: string) => DirectMessageStatusView;
  directMessageConversationFor: (pubkey: string) => DirectMessageConversationView;
}

export function createMockRuntime(options?: DesktopMockApiOptions): MockRuntime {
  const assistPeerIds = options?.assistPeerIds ?? [];
  const starterTopics = [
    'kukuri:topic:general',
    'kukuri:topic:dev',
    'kukuri:topic:test',
  ];
  const effectivePeerIds = Array.from(new Set(['peer-a', ...assistPeerIds]));
  const postsByTopic: Record<string, TimelineView['items']> = Object.fromEntries(
    Object.entries(options?.seedPosts ?? {}).map(([topic, posts]) => [
      topic,
      posts.map((post) => withSocialPostDefaults({ ...post, origin_topic_id: post.origin_topic_id ?? topic })),
    ])
  );
  const authorProfileTimelines: Record<string, TimelineView['items']> = Object.fromEntries(
    Object.entries(options?.authorProfileTimelines ?? {}).map(([pubkey, posts]) => [
      pubkey,
      posts.map((post) => withSocialPostDefaults(post)),
    ])
  );
  for (const [topic, posts] of Object.entries(postsByTopic)) {
    for (const post of posts) {
      if (post.channel_id) {
        continue;
      }
      const current = authorProfileTimelines[post.author_pubkey] ?? [];
      if (current.some((item) => item.object_id === post.object_id)) {
        continue;
      }
      authorProfileTimelines[post.author_pubkey] = [
        withSocialPostDefaults({
          ...post,
          origin_topic_id: post.origin_topic_id ?? topic,
          channel_id: null,
          audience_label: 'Public',
        }),
        ...current,
      ].sort((left, right) => right.created_at - left.created_at || right.object_id.localeCompare(left.object_id));
    }
  }
  const liveSessionsByTopic: Record<string, LiveSessionView[]> = Object.fromEntries(
    Object.entries(options?.seedLiveSessions ?? {}).map(([topic, sessions]) => [
      topic,
      sessions.map((session) => withLiveSessionDefaults(session)),
    ])
  );
  const gameRoomsByTopic: Record<string, GameRoomView[]> = Object.fromEntries(
    Object.entries(options?.seedGameRooms ?? {}).map(([topic, rooms]) => [
      topic,
      rooms.map((room) => withGameRoomDefaults(room)),
    ])
  );
  const metaverseRoomEventsByRoom: Record<string, MetaverseRoomEventView[]> = {};
  const metaverseAssetPayloads: Record<string, BlobMediaPayload> = {};
  const joinedChannelsByTopic: Record<string, JoinedPrivateChannelView[]> = {};
  const mockConsentItems = (accepted: boolean): ConsentItems => [
    {
      policy_slug: 'terms_of_service',
      policy_version: 1,
      title: 'Terms of Service',
      body: 'You must follow the community node terms of service.',
      required: true,
      accepted_at: accepted ? Math.floor(Date.now() / 1000) : null,
      previously_accepted_version: accepted ? 1 : null,
    },
    {
      policy_slug: 'privacy_policy',
      policy_version: 1,
      title: 'Privacy Policy',
      body: 'You must acknowledge the community node privacy policy.',
      required: true,
      accepted_at: accepted ? Math.floor(Date.now() / 1000) : null,
      previously_accepted_version: accepted ? 1 : null,
    },
  ];
  const syncStatus: SyncStatus = {
    connected: true,
    delivery_state: 'Live',
    last_sync_ts: 1,
    peer_count: effectivePeerIds.length,
    pending_events: 0,
    status_detail: 'Connected to all configured peers',
    last_error: options?.globalLastError ?? null,
    configured_peers: ['peer-a'],
    subscribed_topics: [...starterTopics],
    active_path: 'direct_p2p',
    fallback_peer_ids: [],
    topic_diagnostics: starterTopics.map((topic) => ({
      topic,
      joined: true,
      delivery_state: 'Live',
      peer_count: effectivePeerIds.length,
      connected_peers: ['peer-a'],
      docs_assist_peer_ids: assistPeerIds,
      configured_peer_ids: ['peer-a'],
      missing_peer_ids: [],
      active_path: 'direct_p2p',
      rendezvous_peer_ids: [],
      fallback_peer_ids: [],
      last_received_at: topic === 'kukuri:topic:general' ? 1 : null,
      last_docs_activity_at: topic === 'kukuri:topic:general' ? 1 : null,
      status_detail: 'Connected to all configured peers for this topic',
      last_error: topic === 'kukuri:topic:general' ? options?.topicLastError ?? null : null,
    })),
    local_author_pubkey: 'f'.repeat(64),
    discovery: {
      mode: 'seeded_dht',
      connect_mode: 'direct_only',
      active_path: 'direct_p2p',
      fallback_peer_ids: [],
      env_locked: false,
      configured_seed_peer_ids: [],
      bootstrap_seed_peer_ids: [],
      manual_ticket_peer_ids: [],
      connected_peer_ids: ['peer-a'],
      docs_assist_peer_ids: assistPeerIds,
      blob_assist_peer_ids: assistPeerIds,
      local_endpoint_id: 'local-endpoint-a',
      last_discovery_error: null,
    },
    gossip_disabled_topics: [],
    gossip_disabled_channels: [],
  };
  const authorSocialViews: Record<string, AuthorSocialView> = Object.fromEntries(
    Object.entries(options?.authorSocialViews ?? {}).map(([pubkey, view]) => [
      pubkey,
      withDefaultAuthorView(pubkey, view),
    ])
  );
  const directMessageMessagesByPeer: Record<string, DirectMessageMessageView[]> = {};
  const openedDirectMessagePeers = new Set<string>();
  const ownedCustomReactionAssets: CustomReactionAssetView[] = [];
  const bookmarkedCustomReactionAssets: BookmarkedCustomReactionView[] = [];
  const bookmarkedPosts: BookmarkedPostView[] = [];

  function mutedAuthorPubkeys(): Set<string> {
    return new Set(
      Object.values(authorSocialViews)
        .filter((view) => view.muted)
        .map((view) => view.author_pubkey)
    );
  }

  function withCurrentRelationship(post: PostView): PostView {
    const author = authorSocialViews[post.author_pubkey];
    if (!author) {
      return withSocialPostDefaults(post);
    }
    return withSocialPostDefaults({
      ...post,
      following: author.following ?? post.following,
      followed_by: author.followed_by ?? post.followed_by,
      mutual: author.mutual ?? post.mutual,
      friend_of_friend: author.friend_of_friend ?? post.friend_of_friend,
    });
  }

  function isVisiblePost(post: PostView): boolean {
    const muted = mutedAuthorPubkeys();
    return (
      !muted.has(post.author_pubkey) &&
      !(post.repost_of && muted.has(post.repost_of.source_author_pubkey))
    );
  }

  function visibleTimelineItems(items: PostView[]): PostView[] {
    return items.map(withCurrentRelationship).filter(isVisiblePost);
  }

  function listConnections(kind: SocialConnectionKind): AuthorSocialView[] {
    const items = Object.values(authorSocialViews)
      .filter((view) => {
        if (kind === 'following') {
          return view.following;
        }
        if (kind === 'followed') {
          return view.followed_by;
        }
        if (kind === 'blocking') {
          return view.blocking;
        }
        if (kind === 'blocked_by') {
          return view.blocked_by;
        }
        return view.muted;
      })
      .map(cloneAuthorView);
    items.sort(compareAuthorViews);
    return items;
  }

  function directMessageStatusFor(pubkey: string): DirectMessageStatusView {
    const author = withDefaultAuthorView(pubkey, authorSocialViews[pubkey]);
    return {
      peer_pubkey: pubkey,
      dm_id: [syncStatus.local_author_pubkey, pubkey].sort().join(':'),
      mutual: author.mutual,
      send_enabled: author.mutual,
      pending_outbox_count: 0,
      pending_outbox_has_more: false,
    };
  }

  function directMessageConversationFor(pubkey: string): DirectMessageConversationView {
    const messages = directMessageMessagesByPeer[pubkey] ?? [];
    const latest = [...messages].sort(
      (left, right) => right.created_at - left.created_at || right.message_id.localeCompare(left.message_id)
    )[0];
    const author = withDefaultAuthorView(pubkey, authorSocialViews[pubkey]);
    return {
      dm_id: directMessageStatusFor(pubkey).dm_id,
      peer_pubkey: pubkey,
      peer_name: author.name ?? null,
      peer_display_name: author.display_name ?? null,
      peer_picture_asset: author.picture_asset ?? null,
      updated_at: latest?.created_at ?? 0,
      last_message_at: latest?.created_at ?? null,
      last_message_id: latest?.message_id ?? null,
      last_message_preview:
        latest?.text?.trim() ||
        (latest?.attachments.some((attachment) => attachment.role === 'video_manifest')
          ? '[Video]'
          : latest?.attachments.length
            ? '[Image]'
            : null),
      status: directMessageStatusFor(pubkey),
    };
  }

  return {
    options,
    assistPeerIds,
    postsByTopic,
    authorProfileTimelines,
    liveSessionsByTopic,
    gameRoomsByTopic,
    metaverseRoomEventsByRoom,
    metaverseAssetPayloads,
    joinedChannelsByTopic,
    syncStatus,
    authorSocialViews,
    directMessageMessagesByPeer,
    openedDirectMessagePeers,
    ownedCustomReactionAssets,
    bookmarkedCustomReactionAssets,
    bookmarkedPosts,
    sequence: 0,
    discoveryConfig: {
      mode: 'seeded_dht',
      connect_mode: 'direct_only',
      env_locked: false,
      seed_peers: [],
    },
    communityNodeConfig: {
      nodes: [
        {
          base_url: 'https://api.kukuri.app',
          resolved_urls: {
            public_base_url: 'https://api.kukuri.app',
            connectivity_urls: ['https://api.kukuri.app'],
            seed_peers: [],
          },
        },
      ],
    },
    contentAdvisories: [],
    contentAdvisoryIssuerNodeId: '',
    communityNodeStatuses: [
      {
        base_url: 'https://api.kukuri.app',
        auth_state: { authenticated: true, expires_at: Date.now() + 60_000 },
        consent_state: { all_required_accepted: true, items: mockConsentItems(true) },
        // #857: mock の既定ノードはローカル同意済みとして扱う。
        local_consent: {
          records: mockConsentItems(true).map((item) => ({
            policy_slug: item.policy_slug,
            policy_version: item.policy_version,
            accepted_at: Math.floor(Date.now() / 1000),
            language: 'en',
            app_version: 'mock',
          })),
          withdrawn_at: null,
        },
        consent_update_pending: false,
        resolved_urls: {
          public_base_url: 'https://api.kukuri.app',
          connectivity_urls: ['https://api.kukuri.app'],
          seed_peers: [],
        },
        last_error: null,
        invite_code_saved: false,
        admission_rejection: null,
        session_phase: 'ready',
        retry_after: null,
        restart_required: false,
      },
    ],
    myProfile: {
      pubkey: syncStatus.local_author_pubkey,
      name: null,
      display_name: null,
      about: null,
      picture_asset: null,
      updated_at: 0,
      ...options?.myProfile,
    },
    notifications: (options?.notifications ?? []).map(cloneNotification),
    recentReactions: [],
    mockConsentItems,
    mutedAuthorPubkeys,
    withCurrentRelationship,
    isVisiblePost,
    visibleTimelineItems,
    listConnections,
    directMessageStatusFor,
    directMessageConversationFor,
  };
}
