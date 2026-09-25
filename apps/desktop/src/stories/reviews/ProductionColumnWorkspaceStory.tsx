import { useMemo, useState } from 'react';

import { DesktopShellPage } from '@/shell/DesktopShellPage';
import {
  createDesktopShellStore,
  DesktopShellStoreContext,
  timelineStorageKeyForChannel,
} from '@/shell/store';
import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import { DEVELOPER_MODE_STORAGE_KEY } from '@/lib/developerMode';
import type { DesktopTheme } from '@/lib/theme';
import type {
  DirectMessageConversationView,
  GameRoomView,
  JoinedPrivateChannelView,
  LiveSessionView,
  NotificationView,
} from '@/lib/api';
import { createDefaultMetaverseRoomState } from '@/components/extended/metaverse/DomeSceneModel';
import { setColumnDraft } from '@/shell/slices/columnDrafts';
import { columnIdentityId, defaultColumnSpan } from '@/shell/slices/workspace';
import { captureSavedWorkspaceLayout } from '@/shell/savedWorkspaceLayouts';

const DEMO_SCOPE = { topicId: 'kukuri:topic:demo', channelId: null };
const FRIENDS_SCOPE = { topicId: 'kukuri:topic:demo', channelId: 'channel-review' };
const IROH_SCOPE = { topicId: 'kukuri:topic:iroh', channelId: null };
const REVIEW_CHANNEL: JoinedPrivateChannelView = {
  topic_id: DEMO_SCOPE.topicId,
  channel_id: FRIENDS_SCOPE.channelId,
  label: 'Core friends',
  creator_pubkey: 'f'.repeat(64),
  owner_pubkey: 'f'.repeat(64),
  joined_via_pubkey: null,
  audience_kind: 'invite_only',
  is_owner: true,
  current_epoch_id: 'review-epoch',
  archived_epoch_ids: [],
  sharing_state: 'open',
  rotation_required: false,
  participant_count: 4,
  stale_participant_count: 0,
};

type ProductionColumnWorkspaceStoryProps = {
  initialControlCenterOpen?: boolean;
  scenario?:
    | 'default-overview'
    | 'scoped-drafts'
    | 'wide-surfaces'
    | 'activity-surfaces'
    | 'explore-status';
  metaverseSpan?: 1 | 3 | 4;
  streamSpan?: 1 | 2;
  communityNodeUnavailable?: boolean;
  seedSavedLayout?: 'active' | 'dirty';
};

const REVIEW_TIMESTAMP = 1_787_420_800_000;
const REVIEW_LIVE_SESSION: LiveSessionView = {
  session_id: 'live-review',
  host_pubkey: 'f'.repeat(64),
  title: 'Wave 5 launch stream',
  description: 'A production Stream Column using its two-span layout.',
  status: 'Live',
  started_at: REVIEW_TIMESTAMP,
  ended_at: null,
  viewer_count: 18,
  joined_by_me: true,
  channel_id: null,
  audience_label: 'Public',
};
const REVIEW_METAVERSE_ROOM: GameRoomView = {
  room_id: 'metaverse-review',
  host_pubkey: 'f'.repeat(64),
  title: 'Wave 5 atrium',
  description: 'A production Metaverse Column with a container-aware HUD.',
  status: 'Waiting',
  phase_label: 'fixed-dome-v1',
  scores: [],
  room_kind: 'metaverse_room',
  metaverse: createDefaultMetaverseRoomState(8),
  manifest_blob_hash: 'mock-metaverse-review',
  updated_at: REVIEW_TIMESTAMP,
  channel_id: null,
  audience_label: 'Public',
};

// Notifications Column の review seed。auto-read の churn を避けるため既読で固定する。
const REVIEW_NOTIFICATIONS: NotificationView[] = [
  {
    notification_id: 'notification-review-reply',
    kind: 'reply',
    actor_pubkey: 'a'.repeat(64),
    actor_name: 'alice',
    actor_display_name: 'Alice',
    topic_id: DEMO_SCOPE.topicId,
    object_id: 'post-review-reply',
    thread_root_object_id: 'post-review-root',
    preview_text: 'Replied to your launch note with rollout questions.',
    created_at: REVIEW_TIMESTAMP - 120_000,
    received_at: REVIEW_TIMESTAMP - 120_000,
    read_at: REVIEW_TIMESTAMP,
  },
  {
    notification_id: 'notification-review-mention',
    kind: 'mention',
    actor_pubkey: 'b'.repeat(64),
    actor_name: 'bob',
    actor_display_name: 'Bob',
    topic_id: DEMO_SCOPE.topicId,
    object_id: 'post-review-mention',
    preview_text: 'Mentioned you in the launch checklist thread.',
    created_at: REVIEW_TIMESTAMP - 300_000,
    received_at: REVIEW_TIMESTAMP - 300_000,
    read_at: REVIEW_TIMESTAMP,
  },
  {
    notification_id: 'notification-review-followed',
    kind: 'followed',
    actor_pubkey: 'c'.repeat(64),
    actor_name: 'carol',
    actor_display_name: 'Carol',
    created_at: REVIEW_TIMESTAMP - 600_000,
    received_at: REVIEW_TIMESTAMP - 600_000,
    read_at: REVIEW_TIMESTAMP,
  },
];

// Messages Column の review seed。mutual な peer の会話 2 件を固定表示する。
function reviewDirectMessageConversation(
  peerPubkey: string,
  peerName: string,
  peerDisplayName: string,
  preview: string,
  lastMessageAt: number
): DirectMessageConversationView {
  const dmId = ['f'.repeat(64), peerPubkey].sort().join(':');
  return {
    dm_id: dmId,
    peer_pubkey: peerPubkey,
    peer_name: peerName,
    peer_display_name: peerDisplayName,
    peer_picture_asset: null,
    updated_at: lastMessageAt,
    last_message_at: lastMessageAt,
    last_message_id: `dm-message-${peerName}`,
    last_message_preview: preview,
    status: {
      peer_pubkey: peerPubkey,
      dm_id: dmId,
      mutual: true,
      send_enabled: true,
      pending_outbox_count: 0,
      pending_outbox_has_more: false,
    },
  };
}

const REVIEW_DIRECT_MESSAGES: DirectMessageConversationView[] = [
  reviewDirectMessageConversation(
    'a'.repeat(64),
    'alice',
    'Alice',
    'Release notes look ready — shipping tonight?',
    REVIEW_TIMESTAMP - 90_000
  ),
  reviewDirectMessageConversation(
    'b'.repeat(64),
    'bob',
    'Bob',
    'Sent the Wave 6 review capture.',
    REVIEW_TIMESTAMP - 480_000
  ),
];

function scenarioHash(scenario: NonNullable<ProductionColumnWorkspaceStoryProps['scenario']>) {
  switch (scenario) {
    case 'default-overview':
      return '#/timeline?topic=kukuri%3Atopic%3Ademo';
    case 'wide-surfaces':
      return '#/game?topic=kukuri%3Atopic%3Ademo&roomId=metaverse-review';
    case 'activity-surfaces':
      return '#/notifications?topic=kukuri%3Atopic%3Ademo';
    case 'explore-status':
      return '#/explore?topic=kukuri%3Atopic%3Ademo';
    default:
      return '#/timeline?topic=kukuri%3Atopic%3Ademo&channel=channel-review';
  }
}

function createReviewStore({
  initialControlCenterOpen = false,
  scenario = 'scoped-drafts',
  metaverseSpan = 3,
  streamSpan = 2,
  seedSavedLayout,
}: ProductionColumnWorkspaceStoryProps) {
  window.localStorage.setItem(DEVELOPER_MODE_STORAGE_KEY, 'true');
  window.history.replaceState(null, '', scenarioHash(scenario));
  const store = createDesktopShellStore();
  const publicColumnId = columnIdentityId('timeline', DEMO_SCOPE);
  const friendsColumnId = columnIdentityId('timeline', FRIENDS_SCOPE);
  const irohColumnId = columnIdentityId('timeline', IROH_SCOPE);
  const streamColumnId = columnIdentityId('stream', DEMO_SCOPE, REVIEW_LIVE_SESSION.session_id);
  const metaverseColumnId = columnIdentityId(
    'metaverse',
    DEMO_SCOPE,
    REVIEW_METAVERSE_ROOM.room_id
  );
  const notificationsColumnId = columnIdentityId('notifications', DEMO_SCOPE);
  const messagesColumnId = columnIdentityId('messages', DEMO_SCOPE);
  const exploreColumnId = columnIdentityId('explore', DEMO_SCOPE);
  const profileColumnId = columnIdentityId('profile', DEMO_SCOPE);
  const publicTimelineColumn = {
    id: publicColumnId,
    kind: 'timeline' as const,
    scope: DEMO_SCOPE,
    pinned: true,
    preferredDesktopSpan: defaultColumnSpan('timeline'),
  };
  const columnsByScenario = {
    'default-overview': [
      publicTimelineColumn,
      {
        id: profileColumnId,
        kind: 'profile' as const,
        scope: DEMO_SCOPE,
        pinned: true,
        preferredDesktopSpan: defaultColumnSpan('profile'),
      },
      {
        id: exploreColumnId,
        kind: 'explore' as const,
        scope: DEMO_SCOPE,
        pinned: true,
        preferredDesktopSpan: defaultColumnSpan('explore'),
      },
      {
        id: notificationsColumnId,
        kind: 'notifications' as const,
        scope: DEMO_SCOPE,
        pinned: true,
        preferredDesktopSpan: defaultColumnSpan('notifications'),
      },
      {
        id: messagesColumnId,
        kind: 'messages' as const,
        scope: DEMO_SCOPE,
        pinned: true,
        preferredDesktopSpan: defaultColumnSpan('messages'),
      },
    ],
    'wide-surfaces': [
      publicTimelineColumn,
      {
        id: streamColumnId,
        kind: 'stream' as const,
        scope: DEMO_SCOPE,
        entityId: REVIEW_LIVE_SESSION.session_id,
        pinned: true,
        preferredDesktopSpan: streamSpan,
      },
      {
        id: metaverseColumnId,
        kind: 'metaverse' as const,
        scope: DEMO_SCOPE,
        entityId: REVIEW_METAVERSE_ROOM.room_id,
        pinned: true,
        preferredDesktopSpan: metaverseSpan,
      },
    ],
    'activity-surfaces': [
      {
        id: notificationsColumnId,
        kind: 'notifications' as const,
        scope: DEMO_SCOPE,
        pinned: true,
        preferredDesktopSpan: defaultColumnSpan('notifications'),
      },
      {
        id: messagesColumnId,
        kind: 'messages' as const,
        scope: DEMO_SCOPE,
        pinned: true,
        preferredDesktopSpan: defaultColumnSpan('messages'),
      },
      publicTimelineColumn,
    ],
    'explore-status': [
      {
        id: exploreColumnId,
        kind: 'explore' as const,
        scope: DEMO_SCOPE,
        pinned: true,
        preferredDesktopSpan: defaultColumnSpan('explore'),
      },
      publicTimelineColumn,
    ],
    'scoped-drafts': [
      { ...publicTimelineColumn, preferredDesktopSpan: 1 as const },
      {
        id: friendsColumnId,
        kind: 'timeline' as const,
        scope: FRIENDS_SCOPE,
        pinned: true,
        preferredDesktopSpan: 1 as const,
      },
      {
        id: irohColumnId,
        kind: 'timeline' as const,
        scope: IROH_SCOPE,
        pinned: true,
        preferredDesktopSpan: 1 as const,
      },
    ],
  };
  const activeColumnIdByScenario = {
    'default-overview': publicColumnId,
    'wide-surfaces': metaverseColumnId,
    'activity-surfaces': notificationsColumnId,
    'explore-status': exploreColumnId,
    'scoped-drafts': friendsColumnId,
  };
  store.setState((current) => ({
    joinedChannelsByTopic: {
      ...current.joinedChannelsByTopic,
      [DEMO_SCOPE.topicId]: [REVIEW_CHANNEL],
    },
    workspaceState: {
      ...current.workspaceState,
      activeColumnId: activeColumnIdByScenario[scenario],
      controlCenterOpen: initialControlCenterOpen,
      columns: columnsByScenario[scenario],
    },
    liveSessionsByScopeKey: {
      ...current.liveSessionsByScopeKey,
      [timelineStorageKeyForChannel(DEMO_SCOPE.topicId, DEMO_SCOPE.channelId)]: [
        REVIEW_LIVE_SESSION,
      ],
    },
    gameRoomsByScopeKey: {
      ...current.gameRoomsByScopeKey,
      [timelineStorageKeyForChannel(DEMO_SCOPE.topicId, DEMO_SCOPE.channelId)]: [
        REVIEW_METAVERSE_ROOM,
      ],
    },
    selectedGameRoomId:
      scenario === 'wide-surfaces' ? REVIEW_METAVERSE_ROOM.room_id : current.selectedGameRoomId,
    ...(scenario === 'activity-surfaces'
      ? {
          notifications: REVIEW_NOTIFICATIONS,
          notificationPanelState: { status: 'ready' as const, error: null },
          directMessages: REVIEW_DIRECT_MESSAGES,
        }
      : {}),
    columnDraftsByKey: setColumnDraft(
      setColumnDraft(
        current.columnDraftsByKey,
        { columnId: publicColumnId, action: 'post', scope: DEMO_SCOPE },
        (draft) => ({ ...draft, content: 'Public launch note stays with this Column.' })
      ),
      { columnId: friendsColumnId, action: 'post', scope: FRIENDS_SCOPE },
      (draft) => ({
        ...draft,
        content: 'Friends-only release note — this Draft cannot move to Public.',
        expanded: true,
      })
    ),
  }));
  if (seedSavedLayout) {
    const workspaceState = store.getState().workspaceState;
    const layout = captureSavedWorkspaceLayout(
      'review-daily-layout',
      'Daily workspace',
      workspaceState
    );
    store.setState({
      savedWorkspaceLayouts: [layout],
      workspaceState: {
        ...workspaceState,
        activeLayoutId: layout.id,
        columns:
          seedSavedLayout === 'dirty'
            ? workspaceState.columns.map((column, index) =>
                index === 0 ? { ...column, pinned: !column.pinned } : column
              )
            : workspaceState.columns,
      },
    });
  }
  return store;
}

export function ProductionColumnWorkspaceStory({
  initialControlCenterOpen = false,
  scenario = 'scoped-drafts',
  metaverseSpan = 3,
  streamSpan = 2,
  communityNodeUnavailable = false,
  seedSavedLayout,
}: ProductionColumnWorkspaceStoryProps) {
  const [store] = useState(() =>
    createReviewStore({
      initialControlCenterOpen,
      scenario,
      metaverseSpan,
      streamSpan,
      seedSavedLayout,
    })
  );
  const api = useMemo(() => {
    const mock = createDesktopMockApi({
      seedLiveSessions: { [DEMO_SCOPE.topicId]: [REVIEW_LIVE_SESSION] },
      seedGameRooms: { [DEMO_SCOPE.topicId]: [REVIEW_METAVERSE_ROOM] },
      notifications: scenario === 'activity-surfaces' ? REVIEW_NOTIFICATIONS : [],
    });
    return {
      ...mock,
      listJoinedPrivateChannels: async (topic: string) =>
        topic === DEMO_SCOPE.topicId ? [REVIEW_CHANNEL] : [],
      // Messages Column を空 state にしないため、seed した会話一覧を loader へも返す。
      listDirectMessages: async () =>
        scenario === 'activity-surfaces' ? REVIEW_DIRECT_MESSAGES : mock.listDirectMessages(),
      // Community Node unavailable review: 全ノードを timeout / retrying に固定し、
      // Control Center trigger の状態と Explore Column の inline Notice を同時に見せる。
      getCommunityNodeStatuses: async () => {
        const statuses = await mock.getCommunityNodeStatuses();
        if (!communityNodeUnavailable) return statuses;
        return statuses.map((status) => ({
          ...status,
          last_error: 'community node timeout',
          session_phase: 'retrying' as const,
        }));
      },
    };
  }, [communityNodeUnavailable, scenario]);
  const [theme, setTheme] = useState<DesktopTheme>('dark');

  return (
    <DesktopShellStoreContext.Provider value={store}>
      <DesktopShellPage api={api} theme={theme} onThemeChange={setTheme} />
    </DesktopShellStoreContext.Provider>
  );
}
