import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from 'react';
import { useTranslation } from 'react-i18next';

import {
  CommunityIndexingRequestDialog,
  type CommunityIndexingTarget,
} from '@/components/core/CommunityIndexingRequestDialog';
import { DesktopShellTesterFeedbackDialog } from './page/DesktopShellTesterFeedbackDialog';
import { type SettingsSection } from '@/components/shell/types';

import { runtimeApi } from '@/lib/api';
import {
  eligibleCommunityIndexNodes,
} from '@/lib/api/communityIndex';
import i18n from '@/i18n';
import { changeDesktopLocale } from '@/i18n/changeLocale';
import { getResolvedLocale } from '@/i18n/format';
import {
  buildTopicLink,
  type InternalSmartReference,
} from '@/lib/internalLinks';
import { CLIPBOARD_COPY_EVENT, copyTextToClipboard } from '@/lib/utils';
import {
  SHELL_WORKSPACE_ID,
  type DesktopShellPageProps,
  PUBLIC_CHANNEL_REF,
  PUBLIC_TIMELINE_SCOPE,
  useDesktopShellFieldSetter,
  useDesktopShellStore,
  useDesktopShellStoreApi,
} from '@/shell/store';
import {
  messageFromError,
  privateComposeTarget,
  privateTimelineScope,
} from '@/shell/presentation';
import { selectShellPageSlice } from '@/shell/storeSelectors';
import { setRecordEntry } from '@/shell/stateUpdates';
import { useDesktopShellData } from '@/shell/useDesktopShellData';
import { useDesktopShellRouting } from '@/shell/useDesktopShellRouting';
import { useDesktopShellActions } from '@/shell/useDesktopShellActions';
import { useDeveloperModeBridge } from '@/shell/useDeveloperModeBridge';
import { useOsNotificationBridge } from '@/shell/useOsNotificationBridge';
import { useOsNotificationActivation } from '@/shell/useOsNotificationActivation';
import { selectUpdateAvailable, useAppUpdateStore } from '@/shell/useAppUpdateStore';
import { useAppUpdateScheduler } from '@/shell/useAppUpdateScheduler';
import { useDesktopShellViewModels } from '@/shell/useDesktopShellViewModels';
import {
  DesktopShellDetailSurfaceStack,
  DesktopShellMessagesSurface,
  DesktopShellNotificationsSurface,
} from '@/shell/page/DesktopShellAuxiliaryPanels';
import { DesktopShellOverlays } from '@/shell/page/DesktopShellOverlays';
import { CommunityNodeOnboarding } from '@/shell/page/CommunityNodeOnboarding';
import { DesktopShellColumnWorkspace } from '@/shell/page/DesktopShellColumnWorkspace';
import { DesktopShellControlCenter } from '@/shell/page/DesktopShellControlCenter';
import { DesktopShellPrimarySurface } from '@/shell/page/DesktopShellPrimaryWorkspace';
import { DesktopShellSettingsDrawer } from '@/shell/page/DesktopShellSettingsDrawer';
import { useFocusScroll } from '@/shell/page/useFocusScroll';
import { useSharePreview } from '@/shell/page/useSharePreview';
import { useShellDialogs } from '@/shell/page/useShellDialogs';
import { usePrivateChannelEntries } from '@/shell/page/usePrivateChannelEntries';
import { useShallow } from 'zustand/react/shallow';
import {
  activateColumn,
  childConversationPeer,
  columnIdentityId,
  openTransientColumn,
  setColumnTimelineView,
  setTimelineColumnTopic,
  type ColumnKind,
  type ColumnState,
  type ColumnTimelineView,
} from '@/shell/slices/workspace';
import { routeStateForColumn } from '@/shell/routing/initialWorkspaceRoute';
import { PostRecoveryProviders } from '@/components/core/PostRecoveryProviders';

const CLIPBOARD_TOAST_TIMEOUT_MS = 2200;
export function DesktopShellPage({
  api = runtimeApi,
  theme,
  onThemeChange,
}: DesktopShellPageProps) {
  const storeApi = useDesktopShellStoreApi();
  const { t, i18n: i18nInstance } = useTranslation([
    'common',
    'shell',
    'settings',
    'profile',
    'channels',
    'live',
    'game',
  ]);
  const locale = getResolvedLocale(i18nInstance.resolvedLanguage);
  const translate = useCallback((key: string, options?: Record<string, unknown>) => {
    return i18n.t(key, options) as string;
  }, []);
  const {
    workspaceState,
    trackedTopics,
    topicInput,
    notifications,
    selectedAuthorPubkey,
    selectedDirectMessagePeerPubkey,
    selectedLiveSessionId,
    selectedGameRoomId,
    developerModeEnabled,
    shellChromeState,
    communityNodeConfig,
    communityNodeStatuses,
    communityNodeManifests,
  } = useDesktopShellStore(useShallow(selectShellPageSlice));
  const [profileAvatarPreviewUrl, setProfileAvatarPreviewUrl] = useState<string | null>(null);
  const [clipboardToastId, setClipboardToastId] = useState(0);
  const [indexingTarget, setIndexingTarget] = useState<CommunityIndexingTarget | null>(null);
  const [testerFeedbackOpen, setTesterFeedbackOpen] = useState(false);
  const [localeSaveFailed, setLocaleSaveFailed] = useState(false);
  const clipboardToastTimeoutRef = useRef<number | null>(null);
  const dialogs = useShellDialogs({
    activePrimarySection: shellChromeState.activePrimarySection,
  });

  useEffect(
    () => () => {
      if (profileAvatarPreviewUrl) {
        URL.revokeObjectURL(profileAvatarPreviewUrl);
      }
    },
    [profileAvatarPreviewUrl]
  );

  useEffect(
    () => () => {
      if (clipboardToastTimeoutRef.current !== null) {
        window.clearTimeout(clipboardToastTimeoutRef.current);
      }
    },
    []
  );

  const showClipboardToast = useCallback(() => {
    setClipboardToastId((current) => current + 1);
    if (clipboardToastTimeoutRef.current !== null) {
      window.clearTimeout(clipboardToastTimeoutRef.current);
    }
    clipboardToastTimeoutRef.current = window.setTimeout(() => {
      setClipboardToastId(0);
      clipboardToastTimeoutRef.current = null;
    }, CLIPBOARD_TOAST_TIMEOUT_MS);
  }, []);

  useEffect(() => {
    const handleClipboardCopy = () => {
      showClipboardToast();
    };
    window.addEventListener(CLIPBOARD_COPY_EVENT, handleClipboardCopy as EventListener);
    return () => {
      window.removeEventListener(CLIPBOARD_COPY_EVENT, handleClipboardCopy as EventListener);
    };
  }, [showClipboardToast]);

  const setTopicInput = useDesktopShellFieldSetter('topicInput');
  const setTrackedTopics = useDesktopShellFieldSetter('trackedTopics');
  const setNotificationAutoReadError = useDesktopShellFieldSetter('notificationAutoReadError');
  const setNotificationPanelState = useDesktopShellFieldSetter('notificationPanelState');
  const setDirectMessages = useDesktopShellFieldSetter('directMessages');
  const setDirectMessageTimelineByPeer = useDesktopShellFieldSetter('directMessageTimelineByPeer');
  const setDirectMessageStatusByPeer = useDesktopShellFieldSetter('directMessageStatusByPeer');
  const setDirectMessageError = useDesktopShellFieldSetter('directMessageError');
  const setShellChromeState = useDesktopShellFieldSetter('shellChromeState');
  const setVisibleListColumnIds = useDesktopShellFieldSetter('visibleListColumnIds');
  const updateAvailable = useAppUpdateStore(selectUpdateAvailable);
  const checkForUpdate = useAppUpdateStore((state) => state.checkForUpdate);
  const setComposeChannelByTopic = useDesktopShellFieldSetter('composeChannelByTopic');
  const setTimelineScopeByTopic = useDesktopShellFieldSetter('timelineScopeByTopic');
  const setSelectedLiveSessionId = useDesktopShellFieldSetter('selectedLiveSessionId');
  const setSelectedGameRoomId = useDesktopShellFieldSetter('selectedGameRoomId');
  const setWorkspaceState = useDesktopShellFieldSetter('workspaceState');
  const draftSequenceRef = useRef(0);
  // 表示中 Column id(背景 Timeline refresh 用、Issue #765)。runtime 情報なので store には置かない。
  const visibleColumnIdsRef = useRef<string[]>([]);
  const mediaFetchAttemptRef = useRef(new Map<string, number>());
  const remoteObjectUrlRef = useRef(new Map<string, string>());
  const draftPreviewUrlRef = useRef(new Map<string, string>());
  const directMessageDraftPreviewUrlRef = useRef(new Map<string, string>());
  const loadTopicsRequestRef = useRef(new Map<string, number>());

  const pendingRouteUrlRef = useRef<string | null>(null);
  const controlCenterTriggerRef = useRef<HTMLButtonElement | null>(null);

  const {
    loadTopics,
    retryCommunityNode,
    refreshVisibleTimelineAfterPublish,
    refreshTimelineFeed,
    refreshVisibleShellData,
    refreshConnectivityStatus,
    loadProfileSection,
    loadAuthorSection,
    loadMoreProfileTimeline,
    loadMoreAuthorTimeline,
    loadBookmarksSection,
    navigateBookmarkPage,
    loadReactionCatalogData,
    loadNotificationsSection,
    loadMoreTimeline,
    loadMoreThread,
    rememberDraftPreview,
    releaseDraftPreview,
    rememberDirectMessageDraftPreview,
    releaseDirectMessageDraftPreview,
    releaseAllDirectMessageDraftPreviews,
    buildImageDraftItem: buildComposerImageDraftItem,
    buildVideoDraftItem: buildComposerVideoDraftItem,
    gatedAdultMediaHashes,
    retryMediaFetch,
    reloadPostElements,
  } = useDesktopShellData({
    api,
    translate,
    loadTopicsRequestRef,
    remoteObjectUrlRef,
    draftPreviewUrlRef,
    directMessageDraftPreviewUrlRef,
    mediaFetchAttemptRef,
    draftSequenceRef,
    visibleColumnIdsRef,
  });

  const {
    syncRoute,
    setSettingsOpen,
    focusPrimarySection,
    focusTimelineView,
    openDirectMessageList,
    openDirectMessagePane,
    openThread,
    openAuthorDetail,
    openProfileOverview, openOwnProfileColumn,
    openProfileEditor,
    openProfileConnections,
  } = useDesktopShellRouting({
    api,
    translate,
    loadTopics,
    settingsTriggerRef: controlCenterTriggerRef,
    pendingRouteUrlRef,
  });

  const shellActions = useDesktopShellActions({
    api,
    translate,
    loadTopics,
    refreshProfile: loadProfileSection,
    refreshBookmarks: (removedPostId) => {
      const current = storeApi.getState();
      if (removedPostId && current.bookmarkedPosts.length === 1 &&
        current.bookmarkedPosts[0]?.post.object_id === removedPostId && current.bookmarksNewerCursor) {
        return loadBookmarksSection({ cursor: current.bookmarksNewerCursor, before: true });
      }
      return loadBookmarksSection({ preserveCurrent: true });
    },
    refreshVisibleTimelineAfterPublish,
    syncRoute,
    openDirectMessagePane,
    openAuthorDetail,
    openThread,
    setLiveCreateDialogOpen: dialogs.setLiveCreateDialogOpen,
    setGameCreateDialogOpen: dialogs.setGameCreateDialogOpen,
    setProfileAvatarPreviewUrl,
    setProfileAvatarInputKey: dialogs.setProfileAvatarInputKey,
    releaseDraftPreview,
    rememberDraftPreview,
    releaseDirectMessageDraftPreview,
    releaseAllDirectMessageDraftPreviews,
    rememberDirectMessageDraftPreview,
    buildImageDraftItem: buildComposerImageDraftItem,
    buildVideoDraftItem: buildComposerVideoDraftItem,
  });

  const viewModels = useDesktopShellViewModels({
    t,
    translate,
    locale,
    theme,
    profileAvatarPreviewUrl, gatedMediaHashes: gatedAdultMediaHashes,
  });

  const {
    liveSessionListItems,
    topicNavItems,
    activeGameRooms,
  } = viewModels;
  useOsNotificationBridge();
  useDeveloperModeBridge(developerModeEnabled);
  const { handleOpenNotification } = shellActions;
  const handleActivateOsNotification = useCallback(
    async (notification: Parameters<typeof handleOpenNotification>[0]) => {
      setSettingsOpen(false);
      setWorkspaceState((current) => ({ ...current, controlCenterOpen: false }));
      await handleOpenNotification(notification);
    },
    [setSettingsOpen, setWorkspaceState, handleOpenNotification]
  );
  useOsNotificationActivation(notifications, handleActivateOsNotification);
  const syncTopicContext = useCallback(
    async (topic: string, channelId: string | null) => {
      const nextTopics = trackedTopics.includes(topic) ? trackedTopics : [...trackedTopics, topic];
      if (!trackedTopics.includes(topic)) {
        setTrackedTopics(nextTopics);
      }
      setTimelineScopeByTopic(setRecordEntry(topic, privateTimelineScope(channelId)));
      setComposeChannelByTopic(setRecordEntry(topic, privateComposeTarget(channelId)));
      const scope = { topicId: topic, channelId };
      const columnId = columnIdentityId('timeline', scope);
      setWorkspaceState((current) =>
        openTransientColumn(current, {
          id: columnId,
          kind: 'timeline',
          scope,
          pinned: false,
        })
      );
      await loadTopics(nextTopics, topic, null);
    },
    [
      loadTopics,
      setComposeChannelByTopic,
      setTimelineScopeByTopic,
      setTrackedTopics,
      setWorkspaceState,
      trackedTopics,
    ]
  );
  const handleCopyInternalLink = useCallback((link: string) => {
    void copyTextToClipboard(link);
  }, []);
  const sharePreview = useSharePreview({
    api,
    importChannelAccessToken: shellActions.handleImportChannelAccessToken,
    translate,
  });
  const handleOpenSharePreview = sharePreview.openPreview;
  const handleActivateReference = useCallback(
    async (reference: InternalSmartReference) => {
      if (reference.kind === 'share_token') {
        await handleOpenSharePreview(reference.token);
        return;
      }
      if (reference.kind === 'topic') {
        await syncTopicContext(reference.topic, null);
        setSelectedLiveSessionId(null);
        setSelectedGameRoomId(null);
        syncRoute('push', {
          activeTopic: reference.topic,
          composeTarget: PUBLIC_CHANNEL_REF,
          focusedObjectId: null,
          primarySection: 'timeline',
          selectedAuthorPubkey: null,
          selectedDirectMessagePeerPubkey: null,
          selectedGameRoomId: null,
          selectedLiveSessionId: null,
          selectedThread: null,
          timelineScope: PUBLIC_TIMELINE_SCOPE,
          timelineView: 'feed',
        });
        return;
      }
      if (reference.kind === 'post') {
        if (!trackedTopics.includes(reference.topic)) {
          setTrackedTopics([...trackedTopics, reference.topic]);
        }
        await openThread(reference.threadId, {
          focusObjectId: reference.focusObjectId ?? reference.threadId,
          topic: reference.topic,
        });
        return;
      }
      if ((reference.kind === 'live' || reference.kind === 'game') && !developerModeEnabled) {
        // WIP機能が隠れている間は live/game リンクを topic timeline へ落とす。
        await syncTopicContext(reference.topic, reference.channelId);
        setSelectedLiveSessionId(null);
        setSelectedGameRoomId(null);
        syncRoute('push', {
          activeTopic: reference.topic,
          composeTarget: privateComposeTarget(reference.channelId),
          focusedObjectId: null,
          primarySection: 'timeline',
          selectedAuthorPubkey: null,
          selectedDirectMessagePeerPubkey: null,
          selectedGameRoomId: null,
          selectedLiveSessionId: null,
          selectedThread: null,
          timelineScope: privateTimelineScope(reference.channelId),
        });
        return;
      }
      if (reference.kind === 'live') {
        await syncTopicContext(reference.topic, reference.channelId);
        setSelectedLiveSessionId(reference.sessionId);
        setSelectedGameRoomId(null);
        const scope = { topicId: reference.topic, channelId: reference.channelId };
        setWorkspaceState((current) =>
          openTransientColumn(current, {
            id: columnIdentityId('stream', scope, reference.sessionId),
            kind: 'stream',
            scope,
            entityId: reference.sessionId,
            parentColumnId: current.activeColumnId,
            pinned: false,
          })
        );
        syncRoute('push', {
          activeTopic: reference.topic,
          composeTarget: privateComposeTarget(reference.channelId),
          focusedObjectId: null,
          primarySection: 'live',
          selectedAuthorPubkey: null,
          selectedDirectMessagePeerPubkey: null,
          selectedGameRoomId: null,
          selectedLiveSessionId: reference.sessionId,
          selectedThread: null,
          timelineScope: privateTimelineScope(reference.channelId),
        });
        return;
      }
      await syncTopicContext(reference.topic, reference.channelId);
      setSelectedGameRoomId(reference.roomId);
      setSelectedLiveSessionId(null);
      const scope = { topicId: reference.topic, channelId: reference.channelId };
      setWorkspaceState((current) =>
        openTransientColumn(current, {
          id: columnIdentityId('game', scope, reference.roomId),
          kind: 'game',
          scope,
          entityId: reference.roomId,
          parentColumnId: current.activeColumnId,
          pinned: false,
        })
      );
      syncRoute('push', {
        activeTopic: reference.topic,
        composeTarget: privateComposeTarget(reference.channelId),
        focusedObjectId: null,
        primarySection: 'game',
        selectedAuthorPubkey: null,
        selectedDirectMessagePeerPubkey: null,
        selectedGameRoomId: reference.roomId,
        selectedLiveSessionId: null,
        selectedThread: null,
        timelineScope: privateTimelineScope(reference.channelId),
      });
    },
    [
      developerModeEnabled,
      handleOpenSharePreview,
      openThread,
      setSelectedGameRoomId,
      setSelectedLiveSessionId,
      setWorkspaceState,
      setTrackedTopics,
      syncRoute,
      syncTopicContext,
      trackedTopics,
    ]
  );
  const eligibleIndexingNodes = useMemo(
    () =>
      eligibleCommunityIndexNodes(
        communityNodeConfig,
        communityNodeStatuses,
        communityNodeManifests
      ),
    [communityNodeConfig, communityNodeManifests, communityNodeStatuses]
  );
  const handleOpenSettingsSection = useCallback((section: SettingsSection) => {
    if (section === 'community-node') setIndexingTarget(null);
    setSettingsOpen(true, false);
    setShellChromeState((current) => ({
      ...current,
      settingsOpen: true,
      activeSettingsSection: section,
    }));
    syncRoute('push', {
      settingsOpen: true,
      settingsSection: section,
    });
  }, [setSettingsOpen, setShellChromeState, syncRoute]);
  const handleOpenCommunityNodeSettings = useCallback(() => handleOpenSettingsSection('community-node'), [handleOpenSettingsSection]);
  const handleOpenConnectivitySettings = useCallback(() => handleOpenSettingsSection('connectivity'), [handleOpenSettingsSection]);
  const handleOpenTimelineSection = useCallback(() => focusPrimarySection('timeline'), [focusPrimarySection]);
  const handleOpenExploreSection = useCallback(() => focusPrimarySection('explore'), [focusPrimarySection]);
  const liveFocusKey =
    shellChromeState.activePrimarySection === 'live' ? selectedLiveSessionId : null;
  useFocusScroll({
    focusKey: liveFocusKey,
    readinessKey: liveSessionListItems.length,
    selector: liveFocusKey ? `[data-live-session-id=${JSON.stringify(liveFocusKey)}]` : null,
  });
  const gameFocusKey =
    shellChromeState.activePrimarySection === 'game' ? selectedGameRoomId : null;
  useFocusScroll({
    focusKey: gameFocusKey,
    readinessKey: activeGameRooms.length,
    selector: gameFocusKey ? `[data-game-room-id=${JSON.stringify(gameFocusKey)}]` : null,
  });
  useAppUpdateScheduler(checkForUpdate);
  const renderMessagesSurface = (
    surfaceKind: 'messages' | 'conversation',
    peerPubkey: string | undefined,
    sourceColumnId: string
  ) => (
    <DesktopShellMessagesSurface
      t={t}
      locale={locale}
      viewModels={viewModels}
      openDirectMessageList={openDirectMessageList}
      openDirectMessagePane={(nextPeerPubkey, options) =>
        openDirectMessagePane(nextPeerPubkey, {
          ...options,
          parentColumnId: options?.parentColumnId ?? sourceColumnId,
        })
      }
      openAuthorDetail={(authorPubkey, options) =>
        openAuthorDetail(authorPubkey, {
          ...options,
          // Messages Column の操作は route の DM 選択を外すため、開いている Conversation を正とする(#1053)。
          directMessagePeerPubkey:
            options?.preserveDirectMessageContext && surfaceKind === 'messages'
              ? childConversationPeer(workspaceState, sourceColumnId) ?? options.directMessagePeerPubkey
              : options?.directMessagePeerPubkey,
          // A selected DM can also be projected inside the Messages Column. Preserve
          // its logical Conversation parent so opening the author does not replace it.
          parentColumnId:
            options?.parentColumnId ??
            (options?.preserveDirectMessageContext ? undefined : sourceColumnId),
        })
      }
      handleDeleteDirectMessageMessage={shellActions.handleDeleteDirectMessageMessage}
      handleDirectMessageAttachmentSelection={shellActions.handleDirectMessageAttachmentSelection}
      handleRemoveDirectMessageDraftAttachment={shellActions.handleRemoveDirectMessageDraftAttachment}
      handleSendDirectMessage={shellActions.handleSendDirectMessage}
      surfaceKind={surfaceKind}
      peerPubkey={peerPubkey}
      showComposer={false}
    />
  );
  const renderNotificationsSurface = (column: ColumnState) => (
    <DesktopShellNotificationsSurface
      t={t}
      locale={locale}
      handleOpenNotification={(notification) =>
        shellActions.handleOpenNotification(notification, column.id)
      }
      onOpenNotificationSettings={() => handleOpenSettingsSection('notifications')}
    />
  );
  const renderDetailSurface = (
    surfaceKind: 'thread' | 'profile',
    entityId?: string,
    topicId?: string,
    sourceColumnId?: string,
    channelId?: string | null
  ) => (
    <DesktopShellDetailSurfaceStack
      api={api}
      t={t}
      viewModels={viewModels}
      loadMoreThread={loadMoreThread}
      refreshThread={(topic, threadId) => refreshVisibleShellData(topic, threadId, 'apply')}
      loadMoreAuthorTimeline={loadMoreAuthorTimeline}
      refreshAuthor={(pubkey) => loadAuthorSection(pubkey, { resetToLatest: true })}
      loadReactionCatalogData={loadReactionCatalogData}
      openAuthorDetail={(authorPubkey, options) =>
        openAuthorDetail(authorPubkey, {
          ...options,
          parentColumnId: options?.parentColumnId ?? sourceColumnId,
        })
      }
      openDirectMessagePane={(peerPubkey, options) =>
        openDirectMessagePane(peerPubkey, {
          ...options,
          parentColumnId: options?.parentColumnId ?? sourceColumnId,
        })
      }
      openThread={(threadId, options) =>
        openThread(threadId, {
          ...options,
          channelId: options?.channelId ?? channelId,
          parentColumnId: options?.parentColumnId ?? sourceColumnId,
          topic: options?.topic ?? topicId,
        })
      }
      beginColumnReply={shellActions.beginColumnReply}
      handleSimpleRepost={shellActions.handleSimpleRepost}
      beginColumnQuoteRepost={shellActions.beginColumnQuoteRepost}
      handleRetryLocalPost={shellActions.handleRetryLocalPost}
      handleRestoreLocalPost={shellActions.handleRestoreLocalPost}
      handleWithdrawPost={shellActions.handleWithdrawPost}
      handleToggleReaction={shellActions.handleToggleReaction}
      handleBookmarkCustomReaction={shellActions.handleBookmarkCustomReaction}
      handleActivateReference={handleActivateReference}
      handleCopyPostLink={handleCopyInternalLink}
      handleRelationshipAction={shellActions.handleRelationshipAction}
      handleMuteAction={shellActions.handleMuteAction}
      handleBlockAction={shellActions.handleBlockAction}
      handleOpenOriginalTopic={shellActions.handleOpenOriginalTopic}
      openCommunityNodeSettings={handleOpenCommunityNodeSettings}
      surfaceKind={surfaceKind}
      entityId={entityId}
      topicId={topicId}
    />
  );
  const renderPrimarySurface = (column: ColumnState) => (
    <DesktopShellPrimarySurface
      t={t}
      api={api}
      metaverseActions={shellActions.metaverseActions}
      locale={locale}
      column={column}
      profileAvatarInputKey={dialogs.profileAvatarInputKey}
      messagesWorkspace={null}
      notificationsWorkspace={null}
      viewModels={viewModels}
      openCommunityNodeSettings={handleOpenCommunityNodeSettings}
      openConnectivitySettings={handleOpenConnectivitySettings}
      openTimelineSection={handleOpenTimelineSection}
      openExploreSection={handleOpenExploreSection}
      selectTimelineView={selectColumnTimelineView}
      retryBookmarks={() => void loadBookmarksSection({ preserveCurrent: true })}
      navigateBookmarkPage={navigateBookmarkPage}
      requestIndexing={setIndexingTarget}
      communityNodePanelView={viewModels.communityNodePanelView}
      onFetchCommunityNodeConsents={shellActions.handleFetchCommunityNodeConsents}
      onAcceptCommunityNodeConsents={shellActions.handleAcceptCommunityNodeConsents}
      onRetryCommunityNode={retryCommunityNode}
      loadReactionCatalogData={loadReactionCatalogData}
      refreshTimelineFeed={refreshTimelineFeed}
      refreshProfile={() => loadProfileSection({ resetToLatest: true })}
      loadMoreProfileTimeline={loadMoreProfileTimeline}
      loadMoreTimeline={loadMoreTimeline}
      openAuthorDetail={(authorPubkey, options) =>
        openAuthorDetail(authorPubkey, {
          ...options,
          parentColumnId: options?.parentColumnId ?? column.id,
        })
      }
      openThread={(threadId, options) =>
        openThread(threadId, {
          ...options,
          channelId: options?.channelId ?? column.scope?.channelId,
          parentColumnId: options?.parentColumnId ?? column.id,
          topic: options?.topic ?? column.scope?.topicId,
        })
      }
      beginColumnReply={shellActions.beginColumnReply}
      handleSimpleRepost={shellActions.handleSimpleRepost}
      beginColumnQuoteRepost={shellActions.beginColumnQuoteRepost}
      handleRetryLocalPost={shellActions.handleRetryLocalPost}
      handleRestoreLocalPost={shellActions.handleRestoreLocalPost}
      handleToggleReaction={shellActions.handleToggleReaction}
      handleBookmarkCustomReaction={shellActions.handleBookmarkCustomReaction}
      handleToggleBookmarkedPost={shellActions.handleToggleBookmarkedPost}
      handleWithdrawPost={shellActions.handleWithdrawPost}
      handleActivateReference={handleActivateReference}
      handleCopyInternalLink={handleCopyInternalLink}
      handleJoinLiveSession={shellActions.handleJoinLiveSession}
      handleLeaveLiveSession={shellActions.handleLeaveLiveSession}
      handleEndLiveSession={shellActions.handleEndLiveSession}
      handleCreateGameRoom={shellActions.handleCreateGameRoom}
      updateGameDraft={shellActions.updateGameDraft}
      handleUpdateGameRoom={shellActions.handleUpdateGameRoom}
      openProfileOverview={openProfileOverview}
      openProfileEditor={openProfileEditor}
      openProfileConnections={openProfileConnections}
      handleProfileFieldChange={shellActions.handleProfileFieldChange}
      onProfilePictureSelect={(file) => {
        dialogs.setProfileAvatarCropFile(file);
        dialogs.setProfileAvatarCropOpen(true);
      }}
      handleClearProfileAvatar={shellActions.handleClearProfileAvatar}
      handleSaveProfile={shellActions.handleSaveProfile}
      resetProfileDraft={shellActions.resetProfileDraft}
      handleRelationshipAction={shellActions.handleRelationshipAction}
      handleMuteAction={shellActions.handleMuteAction}
      handleBlockAction={shellActions.handleBlockAction}
      handleOpenOriginalTopic={shellActions.handleOpenOriginalTopic}
    />
  );
  // route だけを Column の canonical target へ同期する(Column 内操作の active 化用、Issue #1053)。
  const syncWorkspaceColumnRoute = (column: ColumnState, preserveAuthorPane = true) => {
    const route = routeStateForColumn(column);
    if (!route) return;
    syncRoute('push', {
      ...route,
      selectedAuthorPubkey:
        preserveAuthorPane &&
        column.kind === 'conversation' &&
        column.entityId === selectedDirectMessagePeerPubkey
          ? selectedAuthorPubkey
          : route.selectedAuthorPubkey,
    });
  };
  const activateWorkspaceColumn = async (
    column: ColumnState,
    preserveAuthorPane = true
  ) => {
    syncWorkspaceColumnRoute(column, preserveAuthorPane);
    if (column.scope) {
      const nextTopics = trackedTopics.includes(column.scope.topicId)
        ? trackedTopics
        : [...trackedTopics, column.scope.topicId];
      if (nextTopics !== trackedTopics) setTrackedTopics(nextTopics);
      await loadTopics(nextTopics, column.scope.topicId, column.kind === 'thread' ? column.entityId ?? null : null);
    }
    if (column.kind === 'thread' && column.entityId) {
      await openThread(column.entityId, {
        historyMode: 'replace',
        topic: column.scope?.topicId,
        channelId: column.scope?.channelId,
      });
      return;
    }
    if (column.kind === 'profile' && column.entityId) {
      await openAuthorDetail(column.entityId, { historyMode: 'replace' });
      return;
    }
    if (column.kind === 'conversation' && column.entityId) {
      await openDirectMessagePane(column.entityId, { historyMode: 'replace' });
      return;
    }
    if (column.kind === 'stream') {
      setSelectedLiveSessionId(column.entityId ?? null);
      setSelectedGameRoomId(null);
    } else if (column.kind === 'game' || column.kind === 'metaverse') {
      setSelectedGameRoomId(column.entityId ?? null);
      setSelectedLiveSessionId(null);
    }
  };
  // Timeline header の view / topic 切替。正本を更新し、操作した Column を active にして route を同期する。
  // 操作時点の route は active 化の push が未 commit のことがあるため判定に使わない(Issue #1053)。
  const selectColumnTimelineView = (column: ColumnState, view: ColumnTimelineView) => {
    setWorkspaceState((current) =>
      activateColumn(setColumnTimelineView(current, column.id, view), column.id)
    );
    focusTimelineView(view);
  };
  const selectColumnTimelineTopic = async (column: ColumnState, topicId: string) => {
    const scope = { topicId, channelId: null };
    const nextColumn = { ...column, scope };
    setWorkspaceState((current) =>
      activateColumn(setTimelineColumnTopic(current, column.id, topicId), column.id)
    );
    setTimelineScopeByTopic(setRecordEntry(topicId, privateTimelineScope(null)));
    setComposeChannelByTopic(setRecordEntry(topicId, privateComposeTarget(null)));
    await activateWorkspaceColumn(nextColumn);
  };
  const refreshNotificationsColumn = useCallback(() => {
    setNotificationAutoReadError(null);
    setNotificationPanelState({
      status: 'loading',
      error: null,
    });
    void loadNotificationsSection({
      markAsRead: shellChromeState.activePrimarySection === 'notifications',
    }).catch(() => undefined);
  }, [
    loadNotificationsSection,
    setNotificationAutoReadError,
    setNotificationPanelState,
    shellChromeState.activePrimarySection,
  ]);
  const refreshConversationColumn = useCallback(
    async (peerPubkey: string) => {
      try {
        const [conversation, timeline, status] = await Promise.all([
          api.openDirectMessage(peerPubkey),
          api.listDirectMessageMessages(peerPubkey, null, 100),
          api.getDirectMessageStatus(peerPubkey),
        ]);
        setDirectMessages((current) => [
          conversation,
          ...current.filter((entry) => entry.peer_pubkey !== conversation.peer_pubkey),
        ]);
        setDirectMessageTimelineByPeer(setRecordEntry(peerPubkey, timeline.items));
        setDirectMessageStatusByPeer(setRecordEntry(peerPubkey, status));
        setDirectMessageError(null);
      } catch (refreshError) {
        setDirectMessageError(
          messageFromError(refreshError, translate('common:errors.failedToOpenDirectMessage'))
        );
      }
    },
    [
      api,
      setDirectMessageError,
      setDirectMessages,
      setDirectMessageStatusByPeer,
      setDirectMessageTimelineByPeer,
      translate,
    ]
  );
  const clearConversationColumn = useCallback(
    async (peerPubkey: string) => {
      try {
        await api.clearDirectMessage(peerPubkey);
        await refreshConversationColumn(peerPubkey);
      } catch (clearError) {
        setDirectMessageError(
          messageFromError(clearError, translate('common:errors.failedToClearDirectMessages'))
        );
      }
    },
    [api, refreshConversationColumn, setDirectMessageError, translate]
  );
  const columnTitles: Record<ColumnKind, string> = {
    timeline: t('shell:primarySections.timeline'),
    notifications: t('shell:primarySections.notifications'),
    thread: t('shell:context.thread'),
    profile: t('shell:primarySections.profile'),
    explore: t('shell:primarySections.explore'),
    messages: t('shell:primarySections.messages'),
    conversation: 'Conversation',
    stream: t('shell:primarySections.live'),
    game: t('shell:primarySections.game'),
    metaverse: 'Metaverse',
  };
  const channelEntries = usePrivateChannelEntries({ dialogs, shellActions, activateWorkspaceColumn });
  const workspace = (
    <DesktopShellColumnWorkspace
      scopeLabel={viewModels.activeComposeAudienceLabel}
      locale={locale}
      onVisibleColumnsChange={(columnIds) => {
        visibleColumnIdsRef.current = columnIds;
        const visible = [workspaceState.activeColumnId,
          ...columnIds.filter((id) => id !== workspaceState.activeColumnId)].slice(0, 8);
        setVisibleListColumnIds((current) =>
          current.length === visible.length && current.every((id, index) => id === visible[index])
            ? current : visible
        );
      }}
      mentionCandidates={viewModels.mentionCandidates}
      onColumnAttachmentSelection={shellActions.handleColumnDraftAttachmentSelection}
      onColumnAttachmentPaste={shellActions.handleColumnDraftAttachmentPaste}
      onRemoveColumnAttachment={shellActions.handleRemoveColumnDraftAttachment}
      onSubmitColumnDraft={shellActions.handleSubmitColumnDraft}
      onEndLiveSession={shellActions.handleEndLiveSession}
      onJoinLiveSession={shellActions.handleJoinLiveSession}
      onLeaveLiveSession={shellActions.handleLeaveLiveSession}
      onOpenGameCreate={() => dialogs.setGameCreateDialogOpen(true)}
      onOpenKeyboardHelp={() => handleOpenSettingsSection('keyboard')}
      onOpenLiveCreate={() => dialogs.setLiveCreateDialogOpen(true)}
      timelineViewItems={viewModels.timelineViewItems}
      onSelectTimelineTopic={(column, topicId) =>
        void selectColumnTimelineTopic(column, topicId)
      }
      onSelectTimelineView={selectColumnTimelineView}
      onOpenChannelManager={channelEntries.openChannelManagerForColumn}
      onOpenChannelSettings={channelEntries.openChannelSettingsForColumn}
      onRefreshNotifications={refreshNotificationsColumn}
      onRefreshProfile={() => loadProfileSection({ resetToLatest: true })}
      onRefreshConversation={(peerPubkey) => void refreshConversationColumn(peerPubkey)}
      onClearConversation={(peerPubkey) => void clearConversationColumn(peerPubkey)}
      onOpenConversationAuthor={(peerPubkey, parentColumnId) =>
        void openAuthorDetail(peerPubkey, {
          historyMode: 'push',
          parentColumnId,
          preserveDirectMessageContext: true,
          directMessagePeerPubkey: peerPubkey,
        })
      }
      onActivateColumn={(column, preserveAuthorPane) =>
        void activateWorkspaceColumn(column, preserveAuthorPane)
      }
      onSyncColumnRoute={syncWorkspaceColumnRoute}
      renderPrimarySurface={renderPrimarySurface}
      renderMessagesSurface={(column) =>
        renderMessagesSurface('messages', undefined, column.id)
      }
      renderConversationSurface={(column) =>
        renderMessagesSurface('conversation', column.entityId, column.id)
      }
      renderNotificationsSurface={renderNotificationsSurface}
      renderThreadSurface={(column) =>
        renderDetailSurface(
          'thread',
          column.entityId,
          column.scope?.topicId,
          column.id,
          column.scope?.channelId
        )
      }
      renderProfileSurface={(column) =>
        renderDetailSurface(
          'profile',
          column.entityId,
          column.scope?.topicId,
          column.id,
          column.scope?.channelId
        )
      }
      titles={columnTitles}
    />
  );

  return (
    <PostRecoveryProviders mediaRetry={retryMediaFetch} postReload={reloadPostElements}>
      <div className='shell-phase1' data-workspace-layout='column'>
        <a className='shell-skip-link' href={`#${SHELL_WORKSPACE_ID}`}>
          {t('shell:workspace.skipToWorkspace')}
        </a>
        <main
          id={SHELL_WORKSPACE_ID}
          className='shell-column-workspace-main'
          tabIndex={-1}
          aria-label={t('shell:workspace.primaryLabel')}
        >
          {workspace}
        </main>
        <DesktopShellControlCenter
          api={api}
          onAcceptCommunityNodeConsents={shellActions.handleAcceptCommunityNodeConsents}
          triggerRef={controlCenterTriggerRef}
          topicItems={topicNavItems}
          topicInput={topicInput}
          titles={columnTitles}
          updateAvailable={updateAvailable}
          onTopicInputChange={setTopicInput}
          onAddTopic={shellActions.handleAddTopic}
          onOpenChannelManager={() => dialogs.setChannelDialogOpen(true)}
          onOpenChannelManagerForTopic={channelEntries.openChannelManagerForTopic}
          onActivateColumn={activateWorkspaceColumn}
          onOpenSettings={handleOpenSettingsSection}
          onOpenProfile={openOwnProfileColumn}
          onOpenTesterFeedback={() => setTesterFeedbackOpen(true)}
          onSelectTopic={(topic) => void shellActions.handleSelectTopic(topic)}
          onSelectChannel={(topic, channelId) => {
            shellActions.handleSelectPrivateChannel(topic, channelId);
          }}
          onOpenChannelSettings={channelEntries.openChannelSettingsDialog}
          onLeaveChannel={(topic, channelId) => dialogs.openLeaveChannelDialog(topic, channelId)}
          onRemoveTopic={(topic) => void shellActions.handleRemoveTopic(topic)}
          onCopyTopicLink={(topic) => handleCopyInternalLink(buildTopicLink(topic))}
          onRequestTopicIndexing={(topic) =>
            setIndexingTarget({ kind: 'public_topic', topicId: topic })
          }
          onToggleTopicGossip={(topic, enabled) =>
            void shellActions.handleToggleTopicGossip(topic, enabled)
          }
          onToggleChannelGossip={(topic, channelId, enabled) =>
            void shellActions.handleToggleChannelGossip(topic, channelId, enabled)
          }
        />
      </div>
      <DesktopShellOverlays
        actions={shellActions}
        dialogs={dialogs}
        t={t}
        viewModels={viewModels}
        handleCopyInternalLink={handleCopyInternalLink}
        sharePreview={sharePreview}
        clipboardToastId={clipboardToastId}
        onRequestPrivateIndexing={setIndexingTarget}
        onOpenChannelSettings={channelEntries.openChannelSettingsDialog}
      />
      <CommunityNodeOnboarding api={api} onAccept={shellActions.handleAcceptCommunityNodeConsents}
        onOpenSettings={handleOpenCommunityNodeSettings} onRetry={retryCommunityNode} />

      <CommunityIndexingRequestDialog
        api={api}
        target={indexingTarget}
        eligibleNodeBaseUrls={eligibleIndexingNodes}
        onOpenChange={(open) => {
          if (!open) setIndexingTarget(null);
        }}
        onOpenCommunityNodeSettings={handleOpenCommunityNodeSettings}
      />

      <DesktopShellTesterFeedbackDialog
        api={api}
        open={testerFeedbackOpen}
        onOpenChange={setTesterFeedbackOpen}
        onOpenCommunityNodeSettings={handleOpenCommunityNodeSettings}
      />

      <DesktopShellSettingsDrawer
        onRefreshDiagnostics={() => { void refreshConnectivityStatus(); }}
        api={api}
        onThemeChange={onThemeChange}
        onLocaleChange={(nextLocale) => {
          setLocaleSaveFailed(!changeDesktopLocale(nextLocale));
        }}
        localeSaveFailed={localeSaveFailed}
        syncRoute={syncRoute}
        setSettingsOpen={setSettingsOpen}
        focusPrimarySection={focusPrimarySection}
        openProfileConnections={openProfileConnections}
        viewModels={viewModels}
        handleImportPeer={shellActions.handleImportPeer}
        handleSaveDiscoverySeeds={shellActions.handleSaveDiscoverySeeds}
        handleSaveCommunityNodes={shellActions.handleSaveCommunityNodes}
        handleSetCommunityNodeTrustPriority={shellActions.handleSetCommunityNodeTrustPriority}
        handleClearCommunityNodes={shellActions.handleClearCommunityNodes}
        handleAuthenticateCommunityNode={shellActions.handleAuthenticateCommunityNode}
        handleSetCommunityNodeInviteCode={shellActions.handleSetCommunityNodeInviteCode}
        handleFetchCommunityNodeConsents={shellActions.handleFetchCommunityNodeConsents}
        handleAcceptCommunityNodeConsents={shellActions.handleAcceptCommunityNodeConsents}
        handleWithdrawCommunityNodeConsents={shellActions.handleWithdrawCommunityNodeConsents}
        handleRefreshCommunityNode={shellActions.handleRefreshCommunityNode}
        handleClearCommunityNodeToken={shellActions.handleClearCommunityNodeToken}
        handleCreateCustomReactionAsset={shellActions.handleCreateCustomReactionAsset}
        handleRemoveBookmarkedCustomReaction={shellActions.handleRemoveBookmarkedCustomReaction}
      />
    </PostRecoveryProviders>
  );
}
