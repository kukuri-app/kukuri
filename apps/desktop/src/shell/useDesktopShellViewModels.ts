import {
  useMemo,
} from 'react';

import type {
  MentionCandidate,
  ThreadPanelState,
  TopicDiagnosticSummary,
} from '@/components/core/types';
import type { TimelineWorkspaceView } from '@/components/shell/types';
import type {
  GameRoomView,
  JoinedPrivateChannelView,
  LiveSessionView,
  PostView,
  TopicSyncStatus,
} from '@/lib/api';
import type { DesktopTheme } from '@/lib/theme';
import { useChannelsViewModels } from '@/shell/viewModels/useChannelsViewModels';
import { useMessagesViewModels } from '@/shell/viewModels/useMessagesViewModels';
import { useProfileViewModels } from '@/shell/viewModels/useProfileViewModels';
import { useSettingsViewModels } from '@/shell/viewModels/useSettingsViewModels';
import { connectivityGuidance } from '@/shell/connectivityGuidance';
import { useTimelineViewModels } from '@/shell/viewModels/useTimelineViewModels';

import { useShallow } from 'zustand/react/shallow';
import {
  PRIMARY_SECTION_ITEMS,
  SETTINGS_SECTION_COPY,
} from '@/shell/routes';
import {
  type SupportedLocale,
} from '@/i18n';
import {
  activeTimelineStorageKey,
  DEFAULT_ASYNC_PANEL_STATE,
  timelineStorageKeyForChannel,
  useDesktopShellStore,
} from '@/shell/store';
import { activeWorkspaceScope } from '@/shell/slices/workspace';
import {
  audienceLabelForChannelRef,
  authorDisplayLabel,
  formatLastReceivedLabel,
  resolveProfilePictureSrc,
} from '@/shell/presentation';
import { selectShellViewModelsSlice } from '@/shell/storeSelectors';
import { EXTENDED_FEATURES_AVAILABLE } from '@/lib/platform';

type UseDesktopShellViewModelsArgs = {
  t: (key: string, options?: Record<string, unknown>) => string;
  translate: (key: string, options?: Record<string, unknown>) => string;
  locale: SupportedLocale;
  theme: DesktopTheme;
  profileAvatarPreviewUrl: string | null;
  /// #1107: 表示設定 OFF の間ゲートする添付 blob hash(`useDesktopShellData` が計算する)。
  gatedMediaHashes?: readonly string[];
};

const EMPTY_POSTS: PostView[] = [];
const EMPTY_LIVE_SESSIONS: LiveSessionView[] = [];
const EMPTY_GAME_ROOMS: GameRoomView[] = [];
const EMPTY_JOINED_CHANNELS: JoinedPrivateChannelView[] = [];

export function useDesktopShellViewModels({
  t,
  translate,
  locale,
  theme,
  profileAvatarPreviewUrl,
  gatedMediaHashes,
}: UseDesktopShellViewModelsArgs) {
  const state = useDesktopShellStore(useShallow(selectShellViewModelsSlice));
  const activeTopic = state.activeTopic;
  const activeScope = activeWorkspaceScope(state.workspaceState);
  const activeScopeKey = timelineStorageKeyForChannel(
    activeScope.topicId,
    activeScope.channelId
  );
  const selectedPrivateChannelId = state.selectedChannelIdByTopic[activeTopic] ?? null;
  const activeJoinedChannels = state.joinedChannelsByTopic[activeTopic] ?? EMPTY_JOINED_CHANNELS;
  const activeTimeline = state.timelinesByKey[activeTimelineStorageKey(state, activeTopic)] ?? EMPTY_POSTS;
  const activeLiveSessions = state.liveSessionsByScopeKey[activeScopeKey] ?? EMPTY_LIVE_SESSIONS;
  const activeGameRooms = state.gameRoomsByScopeKey[activeScopeKey] ?? EMPTY_GAME_ROOMS;
  const activeTimelineScope = state.timelineScopeByTopic[activeTopic] ?? { kind: 'public' as const };
  const activeComposeChannel =
    state.composeChannelByTopic[activeTopic] ?? { kind: 'public' as const };
  const activeComposeAudienceLabel = audienceLabelForChannelRef(
    activeComposeChannel,
    activeJoinedChannels
  );
  const activePrivateChannel =
    selectedPrivateChannelId
      ? activeJoinedChannels.find((channel) => channel.channel_id === selectedPrivateChannelId) ?? null
      : null;
  const activeSocialConnections =
    state.socialConnections[state.shellChromeState.profileConnectionsView];
  const activeSocialConnectionViews =
    state.socialConnections[state.shellChromeState.profileConnectionsView] ?? [];
  const activeChannelPanelState =
    state.channelPanelStateByTopic[activeTopic] ?? DEFAULT_ASYNC_PANEL_STATE;
  const activeLivePanelState =
    state.livePanelStateByScopeKey[activeScopeKey] ?? DEFAULT_ASYNC_PANEL_STATE;
  const activeGamePanelState =
    state.gamePanelStateByScopeKey[activeScopeKey] ?? DEFAULT_ASYNC_PANEL_STATE;
  const topicDiagnostics = Object.fromEntries(
    state.syncStatus.topic_diagnostics.map((diagnostic) => [diagnostic.topic, diagnostic])
  ) as Record<string, TopicSyncStatus>;
  const {
    syncStatus,
    livePendingBySessionId,
    gameDrafts,
    profileDraft,
    localProfile,
    mediaObjectUrls,
    communityNodeStatuses,
    threadsById,
    trackedTopics,
    joinedChannelsByTopic,
    selectedChannelIdByTopic,
    directMessageDraftMediaItems,
    selectedThread,
    selectedAuthor,
    knownAuthorsByPubkey,
    authorError,
    error,
    localPeerTicket,
    peerTicket,
    discoveryConfig,
    discoveryError,
    discoverySeedInput,
    discoveryEditorDirty,
    communityNodeConfig,
    communityNodeError,
    communityNodeInput,
    communityNodeManifests,
    communityNodePolicies,
    communityNodeEditorDirty,
    reactionPanelState,
    ownedReactionAssets,
    bookmarkedReactionAssets,
    bookmarkedPosts,
    profileTimeline,
    selectedAuthorTimeline,
    unsupportedVideoManifests,
    directMessages,
  } = state;
  const thread = selectedThread ? threadsById[selectedThread] ?? EMPTY_POSTS : EMPTY_POSTS;
  const {
    activeTimelinePostViews,
    bookmarkedTimelinePostViews,
    profileTimelinePostViews,
    selectedAuthorTimelinePostViews,
    threadPostViews,
    buildPostCardView,
  } = useTimelineViewModels({
    activeJoinedChannels,
    activeTimeline,
    adultContentEnabled: state.adultContentEnabled,
    bookmarkedPosts,
    developerModeEnabled: state.developerModeEnabled,
    knownAuthorsByPubkey,
    localAuthorPubkey: syncStatus.local_author_pubkey,
    locale,
    localProfile,
    mediaObjectUrls,
    profileTimeline,
    selectedAuthorTimeline,
    thread,
    unsupportedVideoManifests,
    timelineContentAdvisories: state.timelineContentAdvisories,
    timelineAdvisoryLookup: state.timelineAdvisoryLookup,
    communityNodeManifests: state.communityNodeManifests,
    gatedMediaHashes,
    authorTrustGates: state.authorTrustGates,
  });

  const {
    channelAudienceOptions,
    privateChannelListItems,
    liveSessionListItems,
    gameDraftViews,
  } = useChannelsViewModels({
    activeGameRooms,
    activeJoinedChannels,
    activeLiveSessions,
    gameDrafts,
    livePendingBySessionId,
    localAuthorPubkey: syncStatus.local_author_pubkey,
    selectedPrivateChannelId,
    t,
  });

  const {
    profileEditorFields,
    profileEditorPictureSrc,
    profileEditorHasPicture,
    authorDetailView,
  } = useProfileViewModels({
    authorError,
    knownAuthorsByPubkey,
    localAuthorPubkey: syncStatus.local_author_pubkey,
    localProfile,
    mediaObjectUrls,
    profileAvatarPreviewUrl,
    profileDraft,
    selectedAuthor,
    t,
  });

  const {
    selectedDirectMessageConversation,
    selectedDirectMessageTimeline,
    selectedDirectMessageStatus,
    selectedDirectMessagePeerAuthor,
    selectedDirectMessagePeerLabel,
    selectedDirectMessagePeerPicture,
    localDirectMessageAuthorPicture,
    directMessageDraftViews,
  } = useMessagesViewModels({
    directMessageDraftMediaItems,
    directMessages,
    directMessageStatusByPeer: state.directMessageStatusByPeer,
    directMessageTimelineByPeer: state.directMessageTimelineByPeer,
    knownAuthorsByPubkey,
    locale,
    localProfile,
    mediaObjectUrls,
    selectedDirectMessagePeerPubkey: state.selectedDirectMessagePeerPubkey,
    translate,
  });

  const {
    communityNodeStatusByBaseUrl,
    connectivityPanelView,
    appearancePanelView,
    discoveryPanelView,
    communityNodePanelView,
    reactionsPanelView,
  } = useSettingsViewModels({
    syncStatusRead: state.syncStatusRead,
    bookmarkedReactionAssets,
    communityNodeConfig,
    communityNodeEditorDirty,
    communityNodeError,
    communityNodeInput,
    communityNodeManifests,
    communityNodePolicies,
    communityNodeStatuses,
    discoveryConfig,
    discoveryEditorDirty,
    discoveryError,
    discoverySeedInput,
    error,
    locale,
    localPeerTicket,
    ownedReactionAssets,
    peerTicket,
    reactionPanelState,
    syncStatus,
    t,
    theme,
    topicDiagnostics,
    trackedTopics,
  });

  const gossipDisabledTopics = useMemo(
    () => new Set(syncStatus.gossip_disabled_topics ?? []),
    [syncStatus.gossip_disabled_topics]
  );
  const gossipDisabledChannels = useMemo(
    () => new Set(syncStatus.gossip_disabled_channels ?? []),
    [syncStatus.gossip_disabled_channels]
  );
  const topicNavItems = useMemo<TopicDiagnosticSummary[]>(
    () =>
      trackedTopics.map((topic) => ({
        topic,
        active: topic === activeTopic,
        publicActive: topic === activeTopic && (selectedChannelIdByTopic[topic] ?? null) === null,
        removable: trackedTopics.length > 1,
        connectionLabel: connectivityGuidance(syncStatus, state.syncStatusRead, t, {
          id: topic, diagnostic: topicDiagnostics[topic],
        }).label,
        peerCount: state.syncStatusRead.loaded && !gossipDisabledTopics.has(topic)
          ? topicDiagnostics[topic]?.peer_count ?? null : null,
        lastReceivedLabel: formatLastReceivedLabel(topicDiagnostics[topic]?.last_received_at, locale),
        lastReceivedAt: topicDiagnostics[topic]?.last_received_at ?? null,
        gossipJoined: !gossipDisabledTopics.has(topic),
        channels: (joinedChannelsByTopic[topic] ?? []).map((channel) => ({
          channelId: channel.channel_id,
          label: channel.label,
          audienceKind: channel.audience_kind,
          active: topic === activeTopic && selectedChannelIdByTopic[topic] === channel.channel_id,
          gossipJoined:
            !gossipDisabledTopics.has(topic) &&
            !gossipDisabledChannels.has(`${topic}::${channel.channel_id}`),
        })),
      })),
    [
      activeTopic,
      gossipDisabledChannels,
      gossipDisabledTopics,
      joinedChannelsByTopic,
      locale,
      selectedChannelIdByTopic,
      state.syncStatusRead,
      syncStatus,
      t,
      topicDiagnostics,
      trackedTopics,
    ]
  );
  const followingConnections = state.socialConnections.following;
  const mentionCandidates = useMemo<MentionCandidate[]>(() => {
    const seen = new Set<string>();
    const result: MentionCandidate[] = [];
    for (const view of [...followingConnections, ...Object.values(knownAuthorsByPubkey)]) {
      const pubkey = view.author_pubkey;
      if (!pubkey || pubkey === syncStatus.local_author_pubkey || seen.has(pubkey)) {
        continue;
      }
      seen.add(pubkey);
      result.push({
        pubkey,
        label: authorDisplayLabel(pubkey, view.display_name, view.name),
        displayName: view.display_name ?? null,
        name: view.name ?? null,
        about: view.about ?? null,
        picture: resolveProfilePictureSrc(view, mediaObjectUrls),
      });
    }
    return result;
  }, [followingConnections, knownAuthorsByPubkey, mediaObjectUrls, syncStatus.local_author_pubkey]);
  const threadPanelState = useMemo<ThreadPanelState>(
    () => ({
      selectedThreadId: selectedThread,
      summary: selectedThread
        ? t('shell:context.threadSummary', { count: thread.length })
        : t('shell:context.threadEmpty'),
      emptyCopy: t('shell:context.threadEmpty'),
    }),
    [selectedThread, t, thread.length]
  );
  const primarySectionItems = useMemo(
    () =>
      PRIMARY_SECTION_ITEMS.filter(
        (item) =>
          (state.developerModeEnabled && EXTENDED_FEATURES_AVAILABLE) ||
          (item.id !== 'live' && item.id !== 'game')
      ).map((item) => ({
        ...item,
        label: t(`shell:primarySections.${item.id}`),
      })),
    [state.developerModeEnabled, t]
  );
  const timelineViewItems = useMemo<Array<{ id: TimelineWorkspaceView; label: string }>>(
    () => [
      { id: 'feed', label: t('shell:workspace.feed') },
      { id: 'bookmarks', label: t('shell:workspace.bookmarks') },
    ],
    [t]
  );
  const settingsSectionCopy = useMemo(
    () =>
      SETTINGS_SECTION_COPY.map((section) => ({
        ...section,
        label: t(`shell:settingsSections.${section.id}.label`),
        description: t(`shell:settingsSections.${section.id}.description`),
      })),
    [t]
  );

  return {
    channelAudienceOptions,
    privateChannelListItems,
    liveSessionListItems,
    gameDraftViews,
    profileEditorFields,
    profileEditorPictureSrc,
    profileEditorHasPicture,
    communityNodeStatusByBaseUrl,
    topicDiagnostics,
    activeTimelinePostViews,
    bookmarkedTimelinePostViews,
    profileTimelinePostViews,
    selectedAuthorTimelinePostViews,
    threadPostViews,
    buildPostCardView,
    gatedMediaHashes,
    topicNavItems,
    mentionCandidates,
    directMessageDraftViews,
    threadPanelState,
    authorDetailView,
    connectivityPanelView,
    appearancePanelView,
    discoveryPanelView,
    communityNodePanelView,
    reactionsPanelView,
    primarySectionItems,
    timelineViewItems,
    settingsSectionCopy,
    activeComposeChannel,
    activeComposeAudienceLabel,
    activeTimelineScope,
    activeJoinedChannels,
    activeLiveSessions,
    activeGameRooms,
    activeSocialConnections,
    activeSocialConnectionViews,
    activeChannelPanelState,
    activeLivePanelState,
    activeGamePanelState,
    selectedDirectMessageConversation,
    selectedDirectMessageTimeline,
    selectedDirectMessageStatus,
    selectedDirectMessagePeerLabel,
    selectedDirectMessagePeerPicture,
    selectedDirectMessagePeerAuthor,
    localDirectMessageAuthorPicture,
    directMessages,
    activePrivateChannel,
  };
}
