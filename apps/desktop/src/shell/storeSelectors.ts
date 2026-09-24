import type { DesktopShellStore } from '@/shell/store';
import {
  activeWorkspaceColumn,
  activeWorkspaceScope,
  primarySectionForColumn,
} from '@/shell/slices/workspace';
import type { ShellChromeProjection, ShellChromeState } from '@/components/shell/types';
import type { WorkspaceState } from '@/shell/slices/workspace';

const selectedChannelProjectionCache = new WeakMap<
  WorkspaceState,
  Record<string, string | null>
>();
const chromeProjectionCache = new WeakMap<
  WorkspaceState,
  { chrome: ShellChromeState; projection: ShellChromeProjection }
>();

function activeTopic(s: DesktopShellStore) {
  return activeWorkspaceScope(s.workspaceState).topicId;
}

function selectedChannelIdByTopic(s: DesktopShellStore) {
  const cached = selectedChannelProjectionCache.get(s.workspaceState);
  if (cached) return cached;
  const scope = activeWorkspaceScope(s.workspaceState);
  const projection = { [scope.topicId]: scope.channelId };
  selectedChannelProjectionCache.set(s.workspaceState, projection);
  return projection;
}

function projectedChromeState(s: DesktopShellStore) {
  const cached = chromeProjectionCache.get(s.workspaceState);
  if (cached?.chrome === s.shellChromeState) return cached.projection;
  const column = activeWorkspaceColumn(s.workspaceState);
  const projection = {
    ...s.shellChromeState,
    activePrimarySection: primarySectionForColumn(column),
    timelineView: column.kind === 'timeline' ? column.timelineView ?? 'feed' : 'feed',
  };
  chromeProjectionCache.set(s.workspaceState, { chrome: s.shellChromeState, projection });
  return projection;
}

/// useDesktopShellActions.ts が読むフィールド(購読スライス。WP-H6 PR2)。
export const selectShellActionsSlice = (s: DesktopShellStore) => ({
  activeTopic: activeTopic(s),
  bookmarkedPosts: s.bookmarkedPosts,
  bookmarkMembershipById: s.bookmarkMembershipById,
  channelAudienceInput: s.channelAudienceInput,
  channelLabelInput: s.channelLabelInput,
  communityNodeInput: s.communityNodeInput,
  composeChannelByTopic: s.composeChannelByTopic,
  discoverySeedInput: s.discoverySeedInput,
  gameDescription: s.gameDescription,
  gameDrafts: s.gameDrafts,
  gameParticipantsInput: s.gameParticipantsInput,
  gameRoomsByScopeKey: s.gameRoomsByScopeKey,
  gameTitle: s.gameTitle,
  inviteTokenInput: s.inviteTokenInput,
  joinedChannelsByTopic: s.joinedChannelsByTopic,
  liveDescription: s.liveDescription,
  liveTitle: s.liveTitle,
  localProfile: s.localProfile,
  peerTicket: s.peerTicket,
  profileDraft: s.profileDraft,
  selectedAuthorPubkey: s.selectedAuthorPubkey,
  selectedChannelIdByTopic: selectedChannelIdByTopic(s),
  selectedThread: s.selectedThread,
  shellChromeState: projectedChromeState(s),
  syncStatus: s.syncStatus,
  topicInput: s.topicInput,
  trackedTopics: s.trackedTopics,
  workspaceState: s.workspaceState,
});

/// useDesktopShellData.ts が読むフィールド(購読スライス。WP-H6 PR2)。
export const selectShellDataSlice = (s: DesktopShellStore) => ({
  activeTopic: activeTopic(s),
  authorTimelinesByPubkey: s.authorTimelinesByPubkey,
  adultContentEnabled: s.adultContentEnabled,
  bookmarkedReactionAssets: s.bookmarkedReactionAssets,
  communityIndexResolvedPosts: s.communityIndexResolvedPosts,
  advisoryGatedMediaHashes: s.advisoryGatedMediaHashes,
  bookmarkedPosts: s.bookmarkedPosts,
  communityNodeConfig: s.communityNodeConfig,
  communityNodeConfigError: s.communityNodeConfigError,
  communityNodeConfigLoaded: s.communityNodeConfigLoaded,
  communityNodeStatuses: s.communityNodeStatuses,
  communityNodeStatusesLoaded: s.communityNodeStatusesLoaded,
  communityNodeStatusError: s.communityNodeStatusError,
  timelineAdvisoryLookup: s.timelineAdvisoryLookup,
  timelineContentAdvisories: s.timelineContentAdvisories,
  authorTrustGates: s.authorTrustGates,
  directMessageTimelineByPeer: s.directMessageTimelineByPeer,
  directMessages: s.directMessages,
  gameRoomsByScopeKey: s.gameRoomsByScopeKey,
  liveSessionsByScopeKey: s.liveSessionsByScopeKey,
  joinedChannelsByTopic: s.joinedChannelsByTopic,
  knownAuthorsByPubkey: s.knownAuthorsByPubkey,
  localProfile: s.localProfile,
  mediaObjectUrls: s.mediaObjectUrls,
  notifications: s.notifications,
  ownedReactionAssets: s.ownedReactionAssets,
  profileTimeline: s.profileTimeline,
  recentReactions: s.recentReactions,
  selectedAuthorPubkey: s.selectedAuthorPubkey,
  selectedAuthorTimeline: s.selectedAuthorTimeline,
  selectedChannelIdByTopic: selectedChannelIdByTopic(s),
  selectedDirectMessagePeerPubkey: s.selectedDirectMessagePeerPubkey,
  selectedThread: s.selectedThread,
  shellChromeState: projectedChromeState(s),
  syncStatus: s.syncStatus,
  threadsById: s.threadsById,
  timelineScopeByTopic: s.timelineScopeByTopic,
  timelinesByKey: s.timelinesByKey,
  trackedTopics: s.trackedTopics,
  workspaceState: s.workspaceState,
  visibleListColumnIds: s.visibleListColumnIds,
});

/// useDesktopShellRouting.ts が読むフィールド(購読スライス。WP-H6 PR2)。
export const selectShellRoutingSlice = (s: DesktopShellStore) => ({
  activeTopic: activeTopic(s),
  channelPanelStateByTopic: s.channelPanelStateByTopic,
  developerModeEnabled: s.developerModeEnabled,
  directMessagePaneOpen: s.directMessagePaneOpen,
  focusedObjectId: s.focusedObjectId,
  gamePanelStateByScopeKey: s.gamePanelStateByScopeKey,
  gameRoomsByScopeKey: s.gameRoomsByScopeKey,
  joinedChannelsByTopic: s.joinedChannelsByTopic,
  lastNonNotificationsRoute: s.lastNonNotificationsRoute,
  livePanelStateByScopeKey: s.livePanelStateByScopeKey,
  liveSessionsByScopeKey: s.liveSessionsByScopeKey,
  selectedAuthor: s.selectedAuthor,
  selectedAuthorPubkey: s.selectedAuthorPubkey,
  selectedChannelIdByTopic: selectedChannelIdByTopic(s),
  selectedDirectMessagePeerPubkey: s.selectedDirectMessagePeerPubkey,
  selectedGameRoomId: s.selectedGameRoomId,
  selectedLiveSessionId: s.selectedLiveSessionId,
  selectedThread: s.selectedThread,
  shellChromeState: projectedChromeState(s),
  threadsById: s.threadsById,
  trackedTopics: s.trackedTopics,
});

/// useDesktopShellViewModels.ts が読むフィールド(購読スライス。WP-H6 PR2)。
export const selectShellViewModelsSlice = (s: DesktopShellStore) => ({
  activeTopic: activeTopic(s),
  adultContentEnabled: s.adultContentEnabled,
  authorError: s.authorError,
  bookmarkedPosts: s.bookmarkedPosts,
  bookmarkedReactionAssets: s.bookmarkedReactionAssets,
  channelPanelStateByTopic: s.channelPanelStateByTopic,
  communityNodeConfig: s.communityNodeConfig,
  communityNodeEditorDirty: s.communityNodeEditorDirty,
  communityNodeError: s.communityNodeError,
  communityNodeInput: s.communityNodeInput,
  communityNodeManifests: s.communityNodeManifests,
  communityNodePolicies: s.communityNodePolicies,
  communityNodeStatuses: s.communityNodeStatuses,
  composeChannelByTopic: s.composeChannelByTopic,
  developerModeEnabled: s.developerModeEnabled,
  directMessageDraftMediaItems: s.directMessageDraftMediaItems,
  directMessageStatusByPeer: s.directMessageStatusByPeer,
  directMessageTimelineByPeer: s.directMessageTimelineByPeer,
  directMessages: s.directMessages,
  discoveryConfig: s.discoveryConfig,
  discoveryEditorDirty: s.discoveryEditorDirty,
  discoveryError: s.discoveryError,
  discoverySeedInput: s.discoverySeedInput,
  error: s.error,
  gameDrafts: s.gameDrafts,
  gamePanelStateByScopeKey: s.gamePanelStateByScopeKey,
  gameRoomsByScopeKey: s.gameRoomsByScopeKey,
  joinedChannelsByTopic: s.joinedChannelsByTopic,
  knownAuthorsByPubkey: s.knownAuthorsByPubkey,
  livePanelStateByScopeKey: s.livePanelStateByScopeKey,
  livePendingBySessionId: s.livePendingBySessionId,
  liveSessionsByScopeKey: s.liveSessionsByScopeKey,
  localPeerTicket: s.localPeerTicket,
  localProfile: s.localProfile,
  mediaObjectUrls: s.mediaObjectUrls,
  timelineAdvisoryLookup: s.timelineAdvisoryLookup,
  timelineContentAdvisories: s.timelineContentAdvisories,
  authorTrustGates: s.authorTrustGates,
  ownedReactionAssets: s.ownedReactionAssets,
  peerTicket: s.peerTicket,
  profileDraft: s.profileDraft,
  profileTimeline: s.profileTimeline,
  reactionPanelState: s.reactionPanelState,
  selectedAuthor: s.selectedAuthor,
  selectedAuthorTimeline: s.selectedAuthorTimeline,
  selectedChannelIdByTopic: selectedChannelIdByTopic(s),
  selectedDirectMessagePeerPubkey: s.selectedDirectMessagePeerPubkey,
  selectedThread: s.selectedThread,
  shellChromeState: projectedChromeState(s),
  socialConnections: s.socialConnections,
  syncStatus: s.syncStatus,
  syncStatusRead: s.syncStatusRead,
  threadsById: s.threadsById,
  timelineScopeByTopic: s.timelineScopeByTopic,
  timelinesByKey: s.timelinesByKey,
  trackedTopics: s.trackedTopics,
  unsupportedVideoManifests: s.unsupportedVideoManifests,
  workspaceState: s.workspaceState,
});

/// DesktopShellPage が読むフィールド(購読スライス。WP-H6 PR2)。
export const selectShellPageSlice = (s: DesktopShellStore) => ({
  workspaceState: s.workspaceState,
  trackedTopics: s.trackedTopics,
  activeTopic: activeTopic(s),
  topicInput: s.topicInput,
  selectedThread: s.selectedThread,
  focusedObjectId: s.focusedObjectId,
  syncStatus: s.syncStatus,
  selectedAuthorPubkey: s.selectedAuthorPubkey,
  selectedDirectMessagePeerPubkey: s.selectedDirectMessagePeerPubkey,
  selectedChannelIdByTopic: selectedChannelIdByTopic(s),
  notifications: s.notifications,
  notificationStatus: s.notificationStatus,
  selectedLiveSessionId: s.selectedLiveSessionId,
  selectedGameRoomId: s.selectedGameRoomId,
  developerModeEnabled: s.developerModeEnabled,
  shellChromeState: projectedChromeState(s),
  communityNodeConfig: s.communityNodeConfig,
  communityNodeStatuses: s.communityNodeStatuses,
  communityNodeManifests: s.communityNodeManifests,
});
