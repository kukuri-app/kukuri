import { setRecordEntry } from '@/shell/stateUpdates';
import type {
  CustomReactionCropRect,
  DesktopApi,
  NotificationView,
  PostView,
  ReactionKeyInput,
} from '@/lib/api';
import { fileToCreateAttachment } from '@/lib/attachments';

import {
  mergeKnownAuthors,
  messageFromError,
  patchReactionStateIntoPosts,
  privateComposeTarget,
  privateTimelineScope,
  publishedTopicIdForPost,
} from '@/shell/presentation';

import type {
  ActionsBaseParams,
  DerivedSetter,
  NavigationActions,
  Setter,
} from './shared';

type MessageReactionSocialParams = ActionsBaseParams &
  NavigationActions & {
    refreshProfile: () => Promise<void>;
    refreshBookmarks: (removedPostId?: string) => Promise<void>;
    activeTopic: string;
    bookmarkedPostIds: ReadonlySet<string>;
    selectedAuthorPubkey: string | null;
    selectedThread: string | null;
    trackedTopics: string[];
    clearAuxiliaryPanels: () => void;
    setTrackedTopics: Setter<'trackedTopics'>;
    setActiveTopic: DerivedSetter<string>;
    setSelectedChannelIdByTopic: DerivedSetter<Record<string, string | null>>;
    setTimelineScopeByTopic: Setter<'timelineScopeByTopic'>;
    setComposeChannelByTopic: Setter<'composeChannelByTopic'>;
    setTimelinesByKey: Setter<'timelinesByKey'>;
    setThreadsById: Setter<'threadsById'>;
    setProfileTimeline: Setter<'profileTimeline'>;
    setSelectedAuthorTimeline: Setter<'selectedAuthorTimeline'>;
    setKnownAuthorsByPubkey: Setter<'knownAuthorsByPubkey'>;
    setOwnedReactionAssets: Setter<'ownedReactionAssets'>;
    setBookmarkedReactionAssets: Setter<'bookmarkedReactionAssets'>;
    setBookmarkMembershipById: Setter<'bookmarkMembershipById'>;
    setRecentReactions: Setter<'recentReactions'>;
    setSelectedAuthor: Setter<'selectedAuthor'>;
    setAuthorError: Setter<'authorError'>;
    setDirectMessageError: Setter<'directMessageError'>;
    setReactionPanelState: Setter<'reactionPanelState'>;
    setReactionCreatePending: Setter<'reactionCreatePending'>;
    setShellChromeState: Setter<'shellChromeState'>;
    setError: Setter<'error'>;
  };

export function createMessageReactionSocialActions({
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
}: MessageReactionSocialParams) {
  async function handleDeleteDirectMessageMessage(peerPubkey: string, messageId: string) {
    try {
      await api.deleteDirectMessageMessage(peerPubkey, messageId);
      await openDirectMessagePane(peerPubkey, { historyMode: 'replace' });
    } catch (deleteError) {
      setDirectMessageError(
        messageFromError(deleteError, translate('common:errors.failedToDeleteDirectMessage'))
      );
    }
  }

  async function handleClearDirectMessage(peerPubkey: string) {
    try {
      await api.clearDirectMessage(peerPubkey);
      await openDirectMessagePane(peerPubkey, { historyMode: 'replace' });
    } catch (clearError) {
      setDirectMessageError(
        messageFromError(clearError, translate('common:errors.failedToClearDirectMessages'))
      );
    }
  }

  async function handleOpenNotification(
    notification: NotificationView,
    parentColumnId?: string,
    isCurrent: () => boolean = () => true
  ) {
    if (notification.kind === 'direct_message') {
      await openDirectMessagePane(notification.actor_pubkey, {
        historyMode: 'push',
        parentColumnId,
      });
      return;
    }

    if (notification.kind === 'followed') {
      clearAuxiliaryPanels();
      setShellChromeState((current) => ({
        ...current,
      }));
      syncRoute('push', {
        primarySection: 'timeline',
        timelineView: 'feed',
        selectedAuthorPubkey: null,
        selectedDirectMessagePeerPubkey: null,
        selectedThread: null,
      });
      await openAuthorDetail(notification.actor_pubkey, {
        historyMode: 'replace',
        parentColumnId,
      });
      return;
    }

    const targetTopic = notification.topic_id?.trim() || activeTopic;
    const nextTopics = trackedTopics.includes(targetTopic)
      ? trackedTopics
      : [...trackedTopics, targetTopic];
    const nextChannelId = notification.channel_id ?? null;
    const threadTargetId = notification.thread_root_object_id ?? notification.object_id ?? null;

    setTrackedTopics(nextTopics);
    setActiveTopic(targetTopic);
    setSelectedChannelIdByTopic(setRecordEntry(targetTopic, nextChannelId));
    setTimelineScopeByTopic(setRecordEntry(targetTopic, privateTimelineScope(nextChannelId)));
    setComposeChannelByTopic(setRecordEntry(targetTopic, privateComposeTarget(nextChannelId)));
    clearAuxiliaryPanels();
    setShellChromeState((current) => ({
      ...current,
    }));

    const initialHash = window.location.hash;
    await loadTopics(nextTopics, targetTopic, threadTargetId);
    if (!isCurrent() || initialHash !== window.location.hash) return;

    if (threadTargetId) {
      await openThread(threadTargetId, {
        isCurrent,
        focusObjectId: notification.object_id ?? null,
        channelId: nextChannelId,
        historyMode: 'replace',
        parentColumnId,
        topic: targetTopic,
      });
      return;
    }

    syncRoute('replace', {
      activeTopic: targetTopic,
      composeTarget: privateComposeTarget(nextChannelId),
      primarySection: 'timeline',
      selectedAuthorPubkey: null,
      selectedDirectMessagePeerPubkey: null,
      selectedThread: null,
      timelineScope: privateTimelineScope(nextChannelId),
      timelineView: 'feed',
    });
  }

  function patchReactionState(reactionState: Parameters<typeof patchReactionStateIntoPosts>[1]) {
    setTimelinesByKey((current) =>
      Object.fromEntries(
        Object.entries(current).map(([key, posts]) => [
          key,
          patchReactionStateIntoPosts(posts, reactionState),
        ])
      )
    );
    setThreadsById((current) =>
      Object.fromEntries(
        Object.entries(current).map(([topic, posts]) => [
          topic,
          patchReactionStateIntoPosts(posts, reactionState),
        ])
      )
    );
    setProfileTimeline((current) => patchReactionStateIntoPosts(current, reactionState));
    setSelectedAuthorTimeline((current) => patchReactionStateIntoPosts(current, reactionState));
  }

  async function handleToggleReaction(post: PostView, reactionKey: ReactionKeyInput) {
    const topicId = publishedTopicIdForPost(post);
    if (!topicId) {
      setError(translate('common:errors.failedToPublish'));
      return;
    }
    try {
      const nextState = await api.toggleReaction(
        topicId,
        post.object_id,
        reactionKey,
        post.channel_id ? { kind: 'private_channel', channel_id: post.channel_id } : { kind: 'public' }
      );
      patchReactionState(nextState);
      try {
        setRecentReactions(await api.listRecentReactions(8));
      } catch {
        // Keep the toggled state even if the quick-reaction history refresh misses.
      }
      setError(null);
    } catch (reactionError) {
      setError(messageFromError(reactionError, translate('common:errors.failedToPublish')));
    }
  }

  async function handleCreateCustomReactionAsset(
    file: File,
    cropRect: CustomReactionCropRect,
    searchKey: string
  ) {
    setReactionCreatePending(true);
    try {
      const upload = await fileToCreateAttachment(file, 'image_original');
      const asset = await api.createCustomReactionAsset(upload, cropRect, searchKey);
      setOwnedReactionAssets((current) => [
        asset,
        ...current.filter((item) => item.asset_id !== asset.asset_id),
      ]);
      setReactionPanelState({
        status: 'ready',
        error: null,
      });
    } catch (reactionError) {
      setReactionPanelState({
        status: 'error',
        error: messageFromError(reactionError, translate('common:errors.failedToPublish')),
      });
    } finally {
      setReactionCreatePending(false);
    }
  }

  async function handleBookmarkCustomReaction(asset: Parameters<DesktopApi['bookmarkCustomReaction']>[0]) {
    try {
      const bookmarked = await api.bookmarkCustomReaction(asset);
      setBookmarkedReactionAssets((current) => [
        bookmarked,
        ...current.filter((item) => item.asset_id !== bookmarked.asset_id),
      ]);
      setReactionPanelState({
        status: 'ready',
        error: null,
      });
    } catch (bookmarkError) {
      setReactionPanelState({
        status: 'error',
        error: messageFromError(bookmarkError, translate('common:errors.failedToPublish')),
      });
    }
  }

  async function handleRemoveBookmarkedCustomReaction(assetId: string) {
    try {
      await api.removeBookmarkedCustomReaction(assetId);
      setBookmarkedReactionAssets((current) => current.filter((item) => item.asset_id !== assetId));
      setReactionPanelState({
        status: 'ready',
        error: null,
      });
    } catch (bookmarkError) {
      setReactionPanelState({
        status: 'error',
        error: messageFromError(bookmarkError, translate('common:errors.failedToPublish')),
      });
    }
  }

  async function handleToggleBookmarkedPost(post: PostView) {
    const topicId = publishedTopicIdForPost(post);
    if (!topicId) {
      setError(translate('common:errors.failedToUpdateBookmark'));
      return;
    }
    try {
      const removing = bookmarkedPostIds.has(post.object_id);
      if (removing) {
        await api.removeBookmarkedPost(post.object_id);
        setBookmarkMembershipById((current) => ({ ...current, [post.object_id]: false }));
      } else {
        const bookmarked = await api.bookmarkPost(
          topicId,
          post.object_id,
          post.channel_id
            ? { kind: 'private_channel', channel_id: post.channel_id }
            : { kind: 'public' }
        );
        setBookmarkMembershipById((current) => ({ ...current, [bookmarked.post.object_id]: true }));
      }
      await refreshBookmarks(removing ? post.object_id : undefined).catch(() => undefined);
      setError(null);
    } catch (bookmarkError) {
      setError(messageFromError(bookmarkError, translate('common:errors.failedToUpdateBookmark')));
    }
  }

  async function handleWithdrawPost(post: PostView) {
    const topicId = publishedTopicIdForPost(post);
    if (!topicId) {
      setError(translate('common:errors.failedToPublish'));
      return;
    }
    try {
      await api.withdrawPost(
        topicId,
        post.object_id,
        post.channel_id
          ? { kind: 'private_channel', channel_id: post.channel_id }
          : { kind: 'public' }
      );
      await Promise.all([loadTopics(trackedTopics, activeTopic, selectedThread), refreshProfile()]);
      setError(null);
    } catch (withdrawError) {
      setError(messageFromError(withdrawError, translate('common:errors.failedToPublish')));
    }
  }

  async function handleRelationshipAction(authorPubkey: string, following: boolean) {
    try {
      const nextView = following
        ? await api.unfollowAuthor(authorPubkey)
        : await api.followAuthor(authorPubkey);
      setKnownAuthorsByPubkey((current) => mergeKnownAuthors(current, [nextView]));
      if (selectedAuthorPubkey === authorPubkey) {
        setSelectedAuthor(nextView);
        setAuthorError(null);
      }
      await Promise.all([loadTopics(trackedTopics, activeTopic, selectedThread), refreshProfile()]);
    } catch (relationshipError) {
      setAuthorError(
        relationshipError instanceof Error
          ? relationshipError.message
          : translate('common:errors.failedToUpdateFollowState')
      );
    }
  }

  async function handleMuteAction(authorPubkey: string, muted: boolean) {
    try {
      const nextView = muted
        ? await api.unmuteAuthor(authorPubkey)
        : await api.muteAuthor(authorPubkey);
      setKnownAuthorsByPubkey((current) => mergeKnownAuthors(current, [nextView]));
      if (selectedAuthorPubkey === authorPubkey) {
        setSelectedAuthor(nextView);
        setAuthorError(null);
      }
      await Promise.all([loadTopics(trackedTopics, activeTopic, selectedThread), refreshProfile()]);
    } catch (muteError) {
      setAuthorError(
        muteError instanceof Error
          ? muteError.message
          : translate('common:errors.failedToUpdateMuteState')
      );
    }
  }

  async function handleBlockAction(authorPubkey: string, blocking: boolean) {
    try {
      const nextView = blocking
        ? await api.unblockAuthor(authorPubkey)
        : await api.blockAuthor(authorPubkey);
      setKnownAuthorsByPubkey((current) => mergeKnownAuthors(current, [nextView]));
      if (selectedAuthorPubkey === authorPubkey) {
        setSelectedAuthor(nextView);
        setAuthorError(null);
      }
      await Promise.all([loadTopics(trackedTopics, activeTopic, selectedThread), refreshProfile()]);
    } catch (blockError) {
      setAuthorError(
        blockError instanceof Error
          ? blockError.message
          : translate('common:errors.failedToUpdateBlockState', { defaultValue: 'Failed to update block state' })
      );
    }
  }

  return {
    handleDeleteDirectMessageMessage,
    handleClearDirectMessage,
    handleOpenNotification,
    handleToggleReaction,
    handleCreateCustomReactionAsset,
    handleBookmarkCustomReaction,
    handleRemoveBookmarkedCustomReaction,
    handleToggleBookmarkedPost,
    handleWithdrawPost,
    handleRelationshipAction,
    handleMuteAction,
    handleBlockAction,
  };
}
