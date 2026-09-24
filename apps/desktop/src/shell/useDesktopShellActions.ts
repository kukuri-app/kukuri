import {
  useEffect,
  useMemo,
  useRef,
  type ChangeEvent,
  type Dispatch,
  type FormEvent,
  type SetStateAction,
} from 'react';

import type { DesktopApi, PostView } from '@/lib/api';
import { formatUnsupportedAttachmentMessage } from '@/lib/attachments';

import {
  PUBLIC_CHANNEL_REF,
  timelineStorageKeyForChannel,
  type DraftMediaItem,
  useDesktopShellFieldSetter,
  useDesktopShellStore,
  useDesktopShellStoreApi,
} from '@/shell/store';
import { setRecordEntry } from '@/shell/stateUpdates';
import { privateTimelineScope, publishedTopicIdForPost } from '@/shell/presentation';
import { selectShellActionsSlice } from '@/shell/storeSelectors';
import {
  columnDraftKey,
  removeColumnDraft,
  setColumnDraft,
  type ColumnDraftTarget,
} from '@/shell/slices/columnDrafts';
import {
  activeWorkspaceScope,
  columnIdentityId,
  openTransientColumn,
} from '@/shell/slices/workspace';
import { createComposeInteractionsActions } from './actions/composeInteractions';
import { createDirectMessageActions } from './actions/directMessages';
import { createLiveGameActions } from './actions/liveGame';
import { createMessageReactionSocialActions } from './actions/messageReactionSocial';
import { createMetaverseRoomActions } from './actions/metaverse';
import { createOptimisticPostActions } from './actions/optimisticPosts';
import { createProfileTopicChannelActions } from './actions/profileTopicChannel';
import { useShallow } from 'zustand/react/shallow';
import type {
  OpenAuthorDetail,
  OpenDirectMessagePane,
  OpenThread,
  SyncRoute,
  Translate,
} from './actions/shared';

type UseDesktopShellActionsArgs = {
  api: DesktopApi;
  translate: Translate;
  loadTopics: (topics: string[], activeTopic: string, currentThread: string | null) => Promise<void>;
  refreshProfile: () => Promise<void>;
  refreshBookmarks: (removedPostId?: string) => Promise<void>;
  refreshVisibleTimelineAfterPublish: (
    topic: string,
    currentThread: string | null,
    scopeChannelId?: string | null
  ) => Promise<void>;
  syncRoute: SyncRoute;
  openDirectMessagePane: OpenDirectMessagePane;
  openAuthorDetail: OpenAuthorDetail;
  openThread: OpenThread;
  setLiveCreateDialogOpen: Dispatch<SetStateAction<boolean>>;
  setGameCreateDialogOpen: Dispatch<SetStateAction<boolean>>;
  setProfileAvatarPreviewUrl: Dispatch<SetStateAction<string | null>>;
  setProfileAvatarInputKey: Dispatch<SetStateAction<number>>;
  releaseDraftPreview: (itemId: string) => void;
  rememberDraftPreview: (item: DraftMediaItem) => void;
  releaseDirectMessageDraftPreview: (itemId: string) => void;
  releaseAllDirectMessageDraftPreviews: () => void;
  rememberDirectMessageDraftPreview: (item: DraftMediaItem) => void;
  buildImageDraftItem: (file: File) => Promise<DraftMediaItem>;
  buildVideoDraftItem: (file: File) => Promise<DraftMediaItem>;
};

function repostTargetFromSnapshot(post: PostView): PostView | null {
  const source = post.repost_of;
  if (!source) return null;

  return {
    object_id: source.source_object_id,
    envelope_id: source.source_object_id,
    author_pubkey: source.source_author_pubkey,
    author_name: source.source_author_name ?? null,
    author_display_name: source.source_author_display_name ?? null,
    author_picture_asset: source.source_author_picture_asset ?? null,
    following: false,
    followed_by: false,
    mutual: false,
    friend_of_friend: false,
    content: source.content,
    content_status: post.content_status,
    attachments: source.attachments.map((attachment) => ({ ...attachment })),
    created_at: post.created_at,
    reply_to: source.reply_to ?? null,
    root_id: source.root_id ?? null,
    object_kind: source.source_object_kind,
    published_topic_id: source.source_topic_id,
    origin_topic_id: source.source_topic_id,
    repost_of: null,
    repost_commentary: null,
    is_threadable: true,
    channel_id: null,
    audience_label: 'Public',
    local_id: null,
    local_state: null,
    local_error: null,
    server_object_id: null,
    local_draft: null,
    local_draft_media_items: [],
  };
}

export function useDesktopShellActions({
  api,
  translate,
  loadTopics,
  refreshProfile,
  refreshBookmarks,
  refreshVisibleTimelineAfterPublish,
  syncRoute,
  openDirectMessagePane,
  openAuthorDetail,
  openThread,
  setLiveCreateDialogOpen,
  setGameCreateDialogOpen,
  setProfileAvatarPreviewUrl,
  setProfileAvatarInputKey,
  releaseDraftPreview,
  rememberDraftPreview,
  releaseDirectMessageDraftPreview,
  releaseAllDirectMessageDraftPreviews,
  rememberDirectMessageDraftPreview,
  buildImageDraftItem,
  buildVideoDraftItem,
}: UseDesktopShellActionsArgs) {
  const storeApi = useDesktopShellStoreApi();
  const state = useDesktopShellStore(useShallow(selectShellActionsSlice));
  const nextActiveTopic = state.activeTopic;
  const nextSelectedChannelId = state.selectedChannelIdByTopic[nextActiveTopic] ?? null;
  const nextJoinedChannels = state.joinedChannelsByTopic[nextActiveTopic] ?? [];
  const activeComposeChannel = useMemo(
    () => state.composeChannelByTopic[nextActiveTopic] ?? PUBLIC_CHANNEL_REF,
    [nextActiveTopic, state.composeChannelByTopic]
  );
  const {
    trackedTopics,
    activeTopic,
    topicInput,
    selectedThread,
    selectedChannelIdByTopic,
    channelLabelInput,
    channelAudienceInput,
    inviteTokenInput,
    gameDrafts,
    liveTitle,
    liveDescription,
    gameTitle,
    gameDescription,
    gameParticipantsInput,
    peerTicket,
    discoverySeedInput,
    communityNodeInput,
    localProfile,
    profileDraft,
    selectedAuthorPubkey,
  } = state;
  const activePrivateChannel =
    nextJoinedChannels.find((channel) => channel.channel_id === nextSelectedChannelId) ?? null;
  const bookmarkedPostIds = new Set([
    ...Object.entries(state.bookmarkMembershipById).filter(([, value]) => value).map(([id]) => id),
    ...state.bookmarkedPosts.map((item) => item.post.object_id),
  ]);
  const activeScope = activeWorkspaceScope(state.workspaceState);
  const activeGameRooms =
    state.gameRoomsByScopeKey[
      timelineStorageKeyForChannel(activeScope.topicId, activeScope.channelId)
    ] ?? [];
  const localAuthorPubkey = state.syncStatus.local_author_pubkey;
  const metaverseActions = useMemo(
    () =>
      createMetaverseRoomActions({
        api,
        activeTopic,
        activeComposeChannel,
        onRefresh: () => loadTopics(trackedTopics, activeTopic, selectedThread),
      }),
    [api, activeComposeChannel, activeTopic, loadTopics, selectedThread, trackedTopics]
  );

  const setTrackedTopics = useDesktopShellFieldSetter('trackedTopics');
  const setTopicInput = useDesktopShellFieldSetter('topicInput');
  const setTimelinesByKey = useDesktopShellFieldSetter('timelinesByKey');
  const setJoinedChannelsByTopic = useDesktopShellFieldSetter('joinedChannelsByTopic');
  const setTimelineScopeByTopic = useDesktopShellFieldSetter('timelineScopeByTopic');
  const setComposeChannelByTopic = useDesktopShellFieldSetter('composeChannelByTopic');
  const setSelectedThread = useDesktopShellFieldSetter('selectedThread');
  const setThreadsById = useDesktopShellFieldSetter('threadsById');
  const setPeerTicket = useDesktopShellFieldSetter('peerTicket');
  const setDiscoveryConfig = useDesktopShellFieldSetter('discoveryConfig');
  const setDiscoverySeedInput = useDesktopShellFieldSetter('discoverySeedInput');
  const setDiscoveryEditorDirty = useDesktopShellFieldSetter('discoveryEditorDirty');
  const setDiscoveryError = useDesktopShellFieldSetter('discoveryError');
  const setCommunityNodeConfig = useDesktopShellFieldSetter('communityNodeConfig');
  const setCommunityNodeStatuses = useDesktopShellFieldSetter('communityNodeStatuses');
  const setCommunityNodePolicies = useDesktopShellFieldSetter('communityNodePolicies');
  const setCommunityNodeInput = useDesktopShellFieldSetter('communityNodeInput');
  const setCommunityNodeEditorDirty = useDesktopShellFieldSetter('communityNodeEditorDirty');
  const setCommunityNodeError = useDesktopShellFieldSetter('communityNodeError');
  const setKnownAuthorsByPubkey = useDesktopShellFieldSetter('knownAuthorsByPubkey');
  const setOwnedReactionAssets = useDesktopShellFieldSetter('ownedReactionAssets');
  const setBookmarkedReactionAssets = useDesktopShellFieldSetter('bookmarkedReactionAssets');
  const setBookmarkMembershipById = useDesktopShellFieldSetter('bookmarkMembershipById');
  const setRecentReactions = useDesktopShellFieldSetter('recentReactions');
  const setProfileDraft = useDesktopShellFieldSetter('profileDraft');
  const setProfileDirty = useDesktopShellFieldSetter('profileDirty');
  const setProfileError = useDesktopShellFieldSetter('profileError');
  const setProfilePanelState = useDesktopShellFieldSetter('profilePanelState');
  const setProfileSaving = useDesktopShellFieldSetter('profileSaving');
  const setLocalProfile = useDesktopShellFieldSetter('localProfile');
  const setProfileTimeline = useDesktopShellFieldSetter('profileTimeline');
  const setSelectedAuthorPubkey = useDesktopShellFieldSetter('selectedAuthorPubkey');
  const setSelectedAuthor = useDesktopShellFieldSetter('selectedAuthor');
  const setSelectedAuthorTimeline = useDesktopShellFieldSetter('selectedAuthorTimeline');
  const setAuthorError = useDesktopShellFieldSetter('authorError');
  const setDirectMessagePaneOpen = useDesktopShellFieldSetter('directMessagePaneOpen');
  const setSelectedDirectMessagePeerPubkey = useDesktopShellFieldSetter(
    'selectedDirectMessagePeerPubkey'
  );
  const setDirectMessages = useDesktopShellFieldSetter('directMessages');
  const setDirectMessageTimelineByPeer = useDesktopShellFieldSetter('directMessageTimelineByPeer');
  const setDirectMessageComposer = useDesktopShellFieldSetter('directMessageComposer');
  const setDirectMessageDraftMediaItems = useDesktopShellFieldSetter('directMessageDraftMediaItems');
  const setDirectMessageAttachmentInputKey = useDesktopShellFieldSetter(
    'directMessageAttachmentInputKey'
  );
  const setDirectMessageError = useDesktopShellFieldSetter('directMessageError');
  const setDirectMessageSending = useDesktopShellFieldSetter('directMessageSending');
  const setColumnDraftsByKey = useDesktopShellFieldSetter('columnDraftsByKey');
  const setLiveTitle = useDesktopShellFieldSetter('liveTitle');
  const setLiveDescription = useDesktopShellFieldSetter('liveDescription');
  const setLiveError = useDesktopShellFieldSetter('liveError');
  const setLivePendingBySessionId = useDesktopShellFieldSetter('livePendingBySessionId');
  const setLiveCreatePending = useDesktopShellFieldSetter('liveCreatePending');
  const setChannelLabelInput = useDesktopShellFieldSetter('channelLabelInput');
  const setChannelAudienceInput = useDesktopShellFieldSetter('channelAudienceInput');
  const setInviteTokenInput = useDesktopShellFieldSetter('inviteTokenInput');
  const setInviteOutput = useDesktopShellFieldSetter('inviteOutput');
  const setInviteOutputLabel = useDesktopShellFieldSetter('inviteOutputLabel');
  const setChannelError = useDesktopShellFieldSetter('channelError');
  const setChannelPanelStateByTopic = useDesktopShellFieldSetter('channelPanelStateByTopic');
  const setChannelActionPending = useDesktopShellFieldSetter('channelActionPending');
  const setGameTitle = useDesktopShellFieldSetter('gameTitle');
  const setGameDescription = useDesktopShellFieldSetter('gameDescription');
  const setGameParticipantsInput = useDesktopShellFieldSetter('gameParticipantsInput');
  const setGameError = useDesktopShellFieldSetter('gameError');
  const setGameDrafts = useDesktopShellFieldSetter('gameDrafts');
  const setGameSavingByRoomId = useDesktopShellFieldSetter('gameSavingByRoomId');
  const setGameCreatePending = useDesktopShellFieldSetter('gameCreatePending');
  const setReactionPanelState = useDesktopShellFieldSetter('reactionPanelState');
  const setReactionCreatePending = useDesktopShellFieldSetter('reactionCreatePending');
  const setShellChromeState = useDesktopShellFieldSetter('shellChromeState');
  const setWorkspaceState = useDesktopShellFieldSetter('workspaceState');
  const setError = useDesktopShellFieldSetter('error');

  const setActiveTopic = (value: string | ((current: string) => string)) => {
    const currentScope = activeWorkspaceScope(storeApi.getState().workspaceState);
    const topicId = typeof value === 'function' ? value(currentScope.topicId) : value;
    const scope = { topicId, channelId: null };
    setWorkspaceState((current) =>
      openTransientColumn(current, {
        id: columnIdentityId('timeline', scope),
        kind: 'timeline',
        scope,
        pinned: false,
      })
    );
  };

  const setSelectedChannelIdByTopic = (
    value:
      | Record<string, string | null>
      | ((current: Record<string, string | null>) => Record<string, string | null>)
  ) => {
    const currentScope = activeWorkspaceScope(storeApi.getState().workspaceState);
    const currentProjection = { [currentScope.topicId]: currentScope.channelId };
    const nextProjection = typeof value === 'function' ? value(currentProjection) : value;
    const topicId = Object.keys(nextProjection).find(
      (topic) => nextProjection[topic] !== currentProjection[topic]
    ) ?? currentScope.topicId;
    const scope = { topicId, channelId: nextProjection[topicId] ?? null };
    setWorkspaceState((current) =>
      openTransientColumn(current, {
        id: columnIdentityId('timeline', scope),
        kind: 'timeline',
        scope,
        pinned: false,
      })
    );
  };

  const {
    cloneDraftMediaItems,
    createOptimisticPost,
    findKnownPost,
    insertOptimisticPost,
    submitOptimisticPost,
  } = createOptimisticPostActions({
    api,
    activeTopic,
    localAuthorPubkey,
    localProfile,
    refreshVisibleTimelineAfterPublish,
    releaseDraftPreview,
    storeApi,
    translate,
    setProfileTimeline,
    setSelectedAuthorTimeline,
    setThreadsById,
    setTimelinesByKey,
  });

  function clearThreadContext() {
    setSelectedThread(null);
    setSelectedAuthorPubkey(null);
    setSelectedAuthor(null);
    setSelectedAuthorTimeline([]);
    setAuthorError(null);
  }

  function clearAuxiliaryPanels() {
    clearThreadContext();
    setDirectMessagePaneOpen(false);
    setSelectedDirectMessagePeerPubkey(null);
    setDirectMessageError(null);
  }

  const {
    handleProfileFieldChange,
    handleProfileAvatarFile,
    handleClearProfileAvatar,
    resetProfileDraft,
    handleSelectPrivateChannel,
    handleSaveProfile,
    handleAddTopic,
    handleSelectTopic,
    handleOpenOriginalTopic,
    handleRemoveTopic,
    handleToggleTopicGossip,
    handleToggleChannelGossip,
    handleCreatePrivateChannel,
    handleLeavePrivateChannel,
    handleShareChannelAccess,
    handleJoinChannelAccess,
    handleImportChannelAccessToken,
    handleSaveDiscoverySeeds,
    handleSaveCommunityNodes,
    handleSetCommunityNodeTrustPriority,
    handleClearCommunityNodes,
    handleAuthenticateCommunityNode,
    handleSetCommunityNodeInviteCode,
    handleClearCommunityNodeToken,
    handleRefreshCommunityNode,
    handleFetchCommunityNodeConsents,
    handleAcceptCommunityNodeConsents,
    handleWithdrawCommunityNodeConsents,
  } = createProfileTopicChannelActions({
    api,
    getState: storeApi.getState,
    translate,
    loadTopics,
    refreshProfile,
    syncRoute,
    activePrivateChannel,
    activeTopic,
    channelAudienceInput,
    channelLabelInput,
    communityNodeInput,
    discoverySeedInput,
    inviteTokenInput,
    localProfile,
    profileDraft,
    selectedChannelIdByTopic,
    selectedThread,
    topicInput,
    trackedTopics,
    clearThreadContext,
    setProfileAvatarPreviewUrl,
    setProfileAvatarInputKey,
    setTrackedTopics,
    setActiveTopic,
    setTopicInput,
    setTimelineScopeByTopic,
    setComposeChannelByTopic,
    setSelectedChannelIdByTopic,
    setShellChromeState,
    setProfileDraft,
    setProfileDirty,
    setProfileError,
    setProfilePanelState,
    setProfileSaving,
    setLocalProfile,
    setChannelLabelInput,
    setChannelAudienceInput,
    setInviteTokenInput,
    setInviteOutput,
    setInviteOutputLabel,
    setChannelError,
    setChannelPanelStateByTopic,
    setChannelActionPending,
    setJoinedChannelsByTopic,
    setCommunityNodeConfig,
    setCommunityNodeStatuses,
    setCommunityNodePolicies,
    setCommunityNodeInput,
    setCommunityNodeEditorDirty,
    setCommunityNodeError,
    setDiscoveryConfig,
    setDiscoverySeedInput,
    setDiscoveryEditorDirty,
    setDiscoveryError,
  });

  const notificationNavigationRef = useRef(0);
  useEffect(() => () => { notificationNavigationRef.current += 1; }, []);
  const {
    handleDeleteDirectMessageMessage,
    handleClearDirectMessage,
    handleOpenNotification: openNotification,
    handleToggleReaction,
    handleCreateCustomReactionAsset,
    handleBookmarkCustomReaction,
    handleRemoveBookmarkedCustomReaction,
    handleToggleBookmarkedPost,
    handleWithdrawPost,
    handleRelationshipAction,
    handleMuteAction,
    handleBlockAction,
  } = createMessageReactionSocialActions({
    api,
    translate,
    loadTopics,
    refreshProfile,
    refreshBookmarks,
    syncRoute,
    openDirectMessagePane,
    openAuthorDetail,
    openThread,
    activeTopic,
    bookmarkedPostIds,
    selectedAuthorPubkey,
    selectedThread,
    trackedTopics,
    clearAuxiliaryPanels,
    setTrackedTopics,
    setActiveTopic,
    setSelectedChannelIdByTopic,
    setTimelineScopeByTopic,
    setComposeChannelByTopic,
    setTimelinesByKey,
    setThreadsById,
    setProfileTimeline,
    setSelectedAuthorTimeline,
    setKnownAuthorsByPubkey,
    setOwnedReactionAssets,
    setBookmarkedReactionAssets,
    setBookmarkMembershipById,
    setRecentReactions,
    setSelectedAuthor,
    setAuthorError,
    setDirectMessageError,
    setReactionPanelState,
    setReactionCreatePending,
    setShellChromeState,
    setError,
  });

  const handleOpenNotification = (
    notification: Parameters<typeof openNotification>[0], parentColumnId?: string
  ) => {
    const request = ++notificationNavigationRef.current;
    return openNotification(notification, parentColumnId, () => request === notificationNavigationRef.current);
  };

  const {
    handleImportPeer,
    handleCreateLiveSession,
    handleJoinLiveSession,
    handleLeaveLiveSession,
    handleEndLiveSession,
    handleCreateGameRoom,
    updateGameDraft,
    handleUpdateGameRoom,
  } = createLiveGameActions({
    api,
    translate,
    loadTopics,
    syncRoute,
    activeComposeChannel,
    activeGameRooms,
    activeTopic,
    gameDescription,
    gameDrafts,
    gameParticipantsInput,
    gameTitle,
    liveDescription,
    liveTitle,
    peerTicket,
    selectedThread,
    trackedTopics,
    setPeerTicket,
    setLiveTitle,
    setLiveDescription,
    setLiveError,
    setLivePendingBySessionId,
    setLiveCreatePending,
    setShellChromeState,
    setGameTitle,
    setGameDescription,
    setGameParticipantsInput,
    setGameError,
    setGameDrafts,
    setGameSavingByRoomId,
    setGameCreatePending,
    setError,
    setLiveCreateDialogOpen,
    setGameCreateDialogOpen,
  });

  const {
    handleDirectMessageAttachmentSelection,
    handleDirectMessageAttachmentPaste,
    handleRemoveDirectMessageDraftAttachment,
    handleSimpleRepost,
    handleRetryLocalPost,
  } = createComposeInteractionsActions({
    activeTopic,
    buildImageDraftItem,
    buildVideoDraftItem,
    createOptimisticPost,
    insertOptimisticPost,
    releaseAllDirectMessageDraftPreviews,
    releaseDirectMessageDraftPreview,
    rememberDirectMessageDraftPreview,
    submitOptimisticPost: async (post) => {
      await submitOptimisticPost(post);
    },
    syncRoute,
    translate,
    setDirectMessageAttachmentInputKey,
    setDirectMessageDraftMediaItems,
    setDirectMessageError,
    setError,
    setSelectedThread,
    setShellChromeState,
  });

  const { handleSendDirectMessage, sendDirectMessageDraft } = createDirectMessageActions({
    api,
    translate,
    getState: storeApi.getState,
    openDirectMessagePane,
    releaseAllDirectMessageDraftPreviews,
    setDirectMessageTimelineByPeer,
    setDirectMessages,
    setDirectMessageComposer,
    setDirectMessageDraftMediaItems,
    setDirectMessageAttachmentInputKey,
    setDirectMessageError,
    setDirectMessageSending,
  });

  async function prepareColumnDraftAttachments(
    target: ColumnDraftTarget,
    files: File[],
    resetInput: boolean
  ) {
    if (files.length === 0) return;
    const nextItems: DraftMediaItem[] = [];
    // #965: 非対応ファイルは読み込まずに名前だけ集め、1 つの理由文にまとめる。
    const rejectedNames: string[] = [];
    let imageFailed = false;
    let posterFailed = false;
    for (const file of files) {
      try {
        if (file.type.startsWith('image/')) {
          nextItems.push(await buildImageDraftItem(file));
        } else if (file.type.startsWith('video/')) {
          nextItems.push(await buildVideoDraftItem(file));
        } else {
          rejectedNames.push(file.name);
        }
      } catch {
        if (file.type.startsWith('image/')) {
          imageFailed = true;
        } else {
          posterFailed = true;
        }
      }
    }
    const failures = [
      formatUnsupportedAttachmentMessage(translate, rejectedNames),
      imageFailed ? translate('common:errors.failedToPrepareImageAttachment') : null,
      posterFailed ? translate('common:errors.failedToGenerateVideoPoster') : null,
    ].filter((message): message is string => message !== null);
    nextItems.forEach(rememberDraftPreview);
    setColumnDraftsByKey((current) =>
      setColumnDraft(current, target, (draft) => ({
        ...draft,
        mediaItems: [...draft.mediaItems, ...nextItems],
        error: failures.length > 0 ? failures.join(' ') : null,
        attachmentInputKey: resetInput ? draft.attachmentInputKey + 1 : draft.attachmentInputKey,
      }))
    );
  }

  async function handleColumnDraftAttachmentSelection(
    target: ColumnDraftTarget,
    event: ChangeEvent<HTMLInputElement>
  ) {
    await prepareColumnDraftAttachments(target, Array.from(event.target.files ?? []), true);
  }

  async function handleColumnDraftAttachmentPaste(target: ColumnDraftTarget, files: File[]) {
    await prepareColumnDraftAttachments(
      target,
      files.filter((file) => file.type.startsWith('image/')),
      false
    );
  }

  function handleRemoveColumnDraftAttachment(target: ColumnDraftTarget, itemId: string) {
    releaseDraftPreview(itemId);
    setColumnDraftsByKey((current) =>
      setColumnDraft(current, target, (draft) => ({
        ...draft,
        mediaItems: draft.mediaItems.filter((item) => item.id !== itemId),
      }))
    );
  }

  async function handleSubmitColumnDraft(
    target: ColumnDraftTarget,
    event: FormEvent<HTMLFormElement>
  ) {
    event.preventDefault();
    const draft = storeApi.getState().columnDraftsByKey[columnDraftKey(target)];
    if (!draft || draft.pending) return;
    const trimmedContent = draft.content.trim();
    const draftMediaSnapshot = cloneDraftMediaItems(draft.mediaItems);
    const attachments = draftMediaSnapshot.flatMap((item) => item.attachments);
    // #964: 空の下書きは無言で無視せず、送信できない理由を composer に表示する。
    if (!draft.repostTarget && !trimmedContent && attachments.length === 0) {
      setColumnDraftsByKey((current) =>
        setColumnDraft(current, target, (currentDraft) => ({
          ...currentDraft,
          error: translate('common:composer.emptyDraft'),
        }))
      );
      return;
    }
    setColumnDraftsByKey((current) =>
      setColumnDraft(current, target, (currentDraft) => ({
        ...currentDraft,
        pending: true,
        error: null,
      }))
    );

    if (target.action === 'message') {
      if (!target.peerPubkey) return;
      const sent = await sendDirectMessageDraft(
        target.peerPubkey,
        trimmedContent,
        draft.mediaItems,
        () => draft.mediaItems.forEach((item) => releaseDraftPreview(item.id))
      );
      if (sent) {
        setColumnDraftsByKey((current) => removeColumnDraft(current, target));
      } else {
        const message =
          storeApi.getState().directMessageError ??
          translate('common:errors.failedToSendDirectMessage');
        setColumnDraftsByKey((current) =>
          setColumnDraft(current, target, (currentDraft) => ({
            ...currentDraft,
            pending: false,
            error: message,
          }))
        );
      }
      return;
    }

    if (!target.scope) return;
    const repostSourceTopic = draft.repostTarget
      ? publishedTopicIdForPost(draft.repostTarget)
      : null;
    if (draft.repostTarget && !repostSourceTopic) {
      setColumnDraftsByKey((current) =>
        setColumnDraft(current, target, (currentDraft) => ({
          ...currentDraft,
          pending: false,
          error: translate('common:errors.failedToPublish'),
        }))
      );
      return;
    }
    const replyObjectId =
      target.action === 'reply'
        ? draft.replyTarget?.object_id ?? target.threadId ?? null
        : null;
    const currentState = storeApi.getState();
    const targetTimeline =
      currentState.timelinesByKey[
        timelineStorageKeyForChannel(target.scope.topicId, target.scope.channelId)
      ] ?? [];
    const targetThread = target.threadId
      ? currentState.threadsById[target.threadId] ?? []
      : [];
    const replyPost = replyObjectId
      ? [...targetThread, ...targetTimeline].find(
          (post) => post.object_id === replyObjectId
        ) ?? draft.replyTarget
      : null;
    const createdAt = Math.floor(Date.now() / 1000);
    const localId = `local-post:${Date.now()}:${Math.random().toString(16).slice(2)}`;
    const optimisticPost = createOptimisticPost({
      createdAt,
      localId,
      draft: draft.repostTarget
        ? {
            kind: 'repost',
            topic: target.scope.topicId,
            content: trimmedContent,
            source_topic: repostSourceTopic!,
            source_object_id: draft.repostTarget.object_id,
            channel_ref: PUBLIC_CHANNEL_REF,
          }
        : {
            kind: 'post',
            topic: target.scope.topicId,
            content: trimmedContent,
            reply_to: replyObjectId,
            channel_ref: target.scope.channelId
              ? { kind: 'private_channel', channel_id: target.scope.channelId }
              : PUBLIC_CHANNEL_REF,
            attachments,
            content_labels: draft.adultLabeled ? ['adult'] : [],
          },
      draftMedia: draftMediaSnapshot,
      replyPost,
      repostPost: draft.repostTarget,
    });
    insertOptimisticPost(optimisticPost);
    const published = await submitOptimisticPost(optimisticPost);
    if (published) {
      setColumnDraftsByKey((current) => removeColumnDraft(current, target));
    } else {
      const failedPost = storeApi
        .getState()
        .timelinesByKey[
          timelineStorageKeyForChannel(target.scope.topicId, target.scope.channelId)
        ]?.find((post) => post.local_id === localId);
      setColumnDraftsByKey((current) =>
        setColumnDraft(current, target, (currentDraft) => ({
          ...currentDraft,
          pending: false,
          error: failedPost?.local_error ?? translate('common:errors.failedToPublish'),
        }))
      );
    }
  }

  function beginColumnReply(post: PostView) {
    const topicId = publishedTopicIdForPost(post) ?? activeTopic;
    const scope = { topicId, channelId: post.channel_id ?? null };
    const threadId = post.root_id ?? post.object_id;
    const timelineColumnId = columnIdentityId('timeline', scope);
    const threadColumnId = columnIdentityId('thread', scope, threadId);
    const target: ColumnDraftTarget = {
      columnId: threadColumnId,
      action: 'reply',
      scope,
      threadId,
    };
    setColumnDraftsByKey((current) =>
      setColumnDraft(current, target, (draft) => ({
        ...draft,
        expanded: true,
        replyTarget: post,
        repostTarget: null,
        error: null,
      }))
    );
    setWorkspaceState((current) =>
      openTransientColumn(current, {
        id: threadColumnId,
        kind: 'thread',
        scope,
        entityId: threadId,
        parentColumnId: timelineColumnId,
        pinned: false,
      })
    );
    setActiveTopic(topicId);
    setSelectedChannelIdByTopic(setRecordEntry(topicId, scope.channelId));
    setTimelineScopeByTopic(setRecordEntry(topicId, privateTimelineScope(scope.channelId)));
    // 返信元投稿の channel を route / selection にも載せ、Thread Column の scope を global 選択に依存させない。
    void openThread(threadId, { topic: topicId, channelId: scope.channelId });
  }

  function beginColumnQuoteRepost(post: PostView) {
    const topicId = publishedTopicIdForPost(post) ?? activeTopic;
    const scope = { topicId, channelId: null };
    const columnId = columnIdentityId('timeline', scope);
    const target: ColumnDraftTarget = { columnId, action: 'post', scope };
    setColumnDraftsByKey((current) =>
      setColumnDraft(current, target, (draft) => ({
        ...draft,
        expanded: true,
        replyTarget: null,
        repostTarget: post,
        error: null,
      }))
    );
    setWorkspaceState((current) =>
      openTransientColumn(current, {
        id: columnId,
        kind: 'timeline',
        scope,
        pinned: false,
      })
    );
    syncRoute('replace', {
      activeTopic: topicId,
      primarySection: 'timeline',
      selectedThread: null,
      timelineScope: privateTimelineScope(null),
      timelineView: 'feed',
    });
  }

  function restoreLocalPostToColumn(post: PostView) {
    const localDraft = post.local_draft;
    if (!localDraft) return;
    const mediaItems = cloneDraftMediaItems(post.local_draft_media_items ?? []) as DraftMediaItem[];
    mediaItems.forEach((item) => rememberDraftPreview(item));
    const channelId =
      localDraft.kind === 'repost'
        ? null
        : localDraft.channel_ref?.kind === 'private_channel'
          ? localDraft.channel_ref.channel_id
          : null;
    const scope = { topicId: localDraft.topic, channelId };
    const threadId = localDraft.reply_to ? post.root_id ?? localDraft.reply_to : undefined;
    const kind = threadId ? 'thread' : 'timeline';
    const columnId = columnIdentityId(kind, scope, threadId);
    const target: ColumnDraftTarget = {
      columnId,
      action: threadId ? 'reply' : 'post',
      scope,
      ...(threadId ? { threadId } : {}),
    };
    setColumnDraftsByKey((current) =>
      setColumnDraft(current, target, (draft) => ({
        ...draft,
        content: localDraft.content,
        mediaItems,
        replyTarget: localDraft.reply_to ? findKnownPost(localDraft.reply_to) : null,
        repostTarget:
          localDraft.kind === 'repost' && localDraft.source_object_id
            ? (findKnownPost(localDraft.source_object_id) ?? repostTargetFromSnapshot(post))
            : null,
        expanded: true,
        error: post.local_error ?? null,
        pending: false,
        attachmentInputKey: draft.attachmentInputKey + 1,
      }))
    );
    setWorkspaceState((current) =>
      openTransientColumn(current, {
        id: columnId,
        kind,
        scope,
        entityId: threadId,
        parentColumnId: threadId ? columnIdentityId('timeline', scope) : undefined,
        pinned: false,
      })
    );
    if (threadId) {
      void openThread(threadId, { topic: scope.topicId, channelId: scope.channelId });
    } else {
      syncRoute('replace', {
        activeTopic: scope.topicId,
        primarySection: 'timeline',
        selectedThread: null,
        timelineScope: privateTimelineScope(scope.channelId),
        timelineView: 'feed',
      });
    }
  }

  return {
    metaverseActions,
    handleProfileFieldChange,
    handleProfileAvatarFile,
    handleClearProfileAvatar,
    resetProfileDraft,
    handleSaveProfile,
    handleAddTopic,
    handleSelectTopic,
    handleOpenOriginalTopic,
    handleRemoveTopic,
    handleToggleTopicGossip,
    handleToggleChannelGossip,
    handleSelectPrivateChannel,
    handleCreatePrivateChannel,
    handleLeavePrivateChannel,
    handleShareChannelAccess,
    handleJoinChannelAccess,
    handleImportChannelAccessToken,
    handleDirectMessageAttachmentSelection,
    handleDirectMessageAttachmentPaste,
    handleRemoveDirectMessageDraftAttachment,
    handleSendDirectMessage,
    handleColumnDraftAttachmentSelection,
    handleColumnDraftAttachmentPaste,
    handleRemoveColumnDraftAttachment,
    handleSubmitColumnDraft,
    handleDeleteDirectMessageMessage,
    handleClearDirectMessage,
    handleOpenNotification,
    handleToggleReaction,
    handleCreateCustomReactionAsset,
    handleBookmarkCustomReaction,
    handleRemoveBookmarkedCustomReaction,
    handleToggleBookmarkedPost,
    handleWithdrawPost,
    beginColumnReply,
    beginColumnQuoteRepost,
    handleSimpleRepost,
    handleRetryLocalPost,
    handleRestoreLocalPost: restoreLocalPostToColumn,
    handleRelationshipAction,
    handleMuteAction,
    handleBlockAction,
    handleSaveDiscoverySeeds,
    handleSaveCommunityNodes,
    handleSetCommunityNodeTrustPriority,
    handleClearCommunityNodes,
    handleAuthenticateCommunityNode,
    handleSetCommunityNodeInviteCode,
    handleClearCommunityNodeToken,
    handleRefreshCommunityNode,
    handleFetchCommunityNodeConsents,
    handleAcceptCommunityNodeConsents,
    handleWithdrawCommunityNodeConsents,
    handleImportPeer,
    handleCreateLiveSession,
    handleJoinLiveSession,
    handleLeaveLiveSession,
    handleEndLiveSession,
    handleCreateGameRoom,
    updateGameDraft,
    handleUpdateGameRoom,
  };
}
