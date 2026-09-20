import { useMemo, type ChangeEvent, type FormEvent } from 'react';
import { Settings } from 'lucide-react';

import { AuthorAvatar } from '@/components/core/AuthorAvatar';
import { AuthorDetailCard } from '@/components/core/AuthorDetailCard';
import { MediaFetchFailure } from '@/components/core/MediaFetchFailure';
import { AuthorTrustDisplayExceptionField } from '@/components/core/AuthorTrustDisplayExceptionField';
import { CommunityNodeAdvisoryPanel } from '@/components/core/CommunityNodeAdvisoryPanel';
import { AuthorIdentityButton } from '@/components/core/AuthorIdentityButton';
import { ComposerDraftPreviewList } from '@/components/core/ComposerDraftPreviewList';
import { ThreadPanel } from '@/components/core/ThreadPanel';
import { TimelineFeed } from '@/components/core/TimelineFeed';
import type { AuthorDetailView } from '@/components/core/types';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card } from '@/components/ui/card';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { Notice } from '@/components/ui/notice';
import { Textarea } from '@/components/ui/textarea';

import type {
  DesktopApi,
  NotificationView,
  PostView,
  ReactionKeyInput,
} from '@/lib/api';
import { formatLocalizedTime } from '@/i18n/format';
import type { SupportedLocale } from '@/i18n';
import { clipboardImageFiles } from '@/lib/attachments';
import { type InternalSmartReference } from '@/lib/internalLinks';
import { eligibleTrustRelationNodes } from '@/lib/api/communityIndex';
import { copyTextToClipboard } from '@/lib/utils';
import { useDesktopShellFieldSetter, useDesktopShellStore } from '@/shell/store';
import { activeWorkspaceScope } from '@/shell/slices/workspace';
import {
  authorDisplayLabel,
  authorViewFromDirectMessageConversation,
  formatCount,
  resolveProfilePictureSrc,
  shortPubkey,
  strongestRelationshipLabel,
} from '@/shell/presentation';
import {
  hasAdultContentLabel,
  isGatingContentAdvisory,
  selectPrimaryImageAttachment,
  selectVideoManifestAttachment,
  selectVideoPosterAttachment,
} from '@/shell/media';
import { useDesktopShellViewModels } from '@/shell/useDesktopShellViewModels';
import { advisorySubjectKey } from '@/shell/contentAdvisories';
import { useShallow } from 'zustand/react/shallow';
import type {
  OpenAuthorDetail,
  OpenDirectMessagePane,
  OpenThread,
  Translate,
} from '@/shell/actions/shared';

type ViewModels = ReturnType<typeof useDesktopShellViewModels>;

type NotificationItemView = NotificationView & {
  actorLabel: string;
  actorPicture: string | null;
  contextLabel: string;
  kindLabel: string;
  previewText: string;
  receivedLabel: string;
  unread: boolean;
};

export type DesktopShellMessagesSurfaceProps = {
  t: Translate;
  locale: SupportedLocale;
  viewModels: Pick<
    ViewModels,
    | 'directMessageDraftViews'
    | 'selectedDirectMessagePeerLabel'
    | 'selectedDirectMessagePeerPicture'
    | 'selectedDirectMessageStatus'
    | 'selectedDirectMessageTimeline'
    | 'localDirectMessageAuthorPicture'
  >;
  openDirectMessageList: (mode?: 'push' | 'replace') => void;
  openDirectMessagePane: OpenDirectMessagePane;
  openAuthorDetail: OpenAuthorDetail;
  handleDeleteDirectMessageMessage: (peerPubkey: string, messageId: string) => Promise<void>;
  handleDirectMessageAttachmentSelection: (event: ChangeEvent<HTMLInputElement>) => Promise<void>;
  handleDirectMessageAttachmentPaste?: (files: File[]) => Promise<void>;
  handleRemoveDirectMessageDraftAttachment: (itemId: string) => void;
  handleSendDirectMessage: (event: FormEvent<HTMLFormElement>) => Promise<void>;
  surfaceKind?: 'messages' | 'conversation';
  peerPubkey?: string;
  showComposer?: boolean;
};

export function DesktopShellMessagesSurface({
  t,
  locale,
  viewModels,
  openDirectMessageList,
  openDirectMessagePane,
  openAuthorDetail,
  handleDeleteDirectMessageMessage,
  handleDirectMessageAttachmentSelection,
  handleDirectMessageAttachmentPaste,
  handleRemoveDirectMessageDraftAttachment,
  handleSendDirectMessage,
  surfaceKind,
  peerPubkey,
  showComposer = true,
}: DesktopShellMessagesSurfaceProps) {
  const {
    directMessageAttachmentInputKey,
    directMessageComposer,
    directMessageError,
    directMessageSending,
    directMessages,
    directMessageStatusByPeer,
    directMessageTimelineByPeer,
    knownAuthorsByPubkey,
    localProfile,
    mediaObjectUrls,
    selectedDirectMessagePeerPubkey,
    syncStatus,
    unsupportedVideoManifests,
    mediaRetryingHashes,
  } = useDesktopShellStore(
    useShallow((s) => ({
      directMessageAttachmentInputKey: s.directMessageAttachmentInputKey,
      directMessageComposer: s.directMessageComposer,
      directMessageError: s.directMessageError,
      directMessageSending: s.directMessageSending,
      directMessages: s.directMessages,
      directMessageStatusByPeer: s.directMessageStatusByPeer,
      directMessageTimelineByPeer: s.directMessageTimelineByPeer,
      knownAuthorsByPubkey: s.knownAuthorsByPubkey,
      localProfile: s.localProfile,
      mediaObjectUrls: s.mediaObjectUrls,
      selectedDirectMessagePeerPubkey: s.selectedDirectMessagePeerPubkey,
      syncStatus: s.syncStatus,
      unsupportedVideoManifests: s.unsupportedVideoManifests,
      mediaRetryingHashes: s.mediaRetryingHashes,
    }))
  );
  const setDirectMessageComposer = useDesktopShellFieldSetter('directMessageComposer');
  const profileAuthorLabel = authorDisplayLabel(
    syncStatus.local_author_pubkey,
    localProfile?.display_name,
    localProfile?.name
  );
  const conversationPeerPubkey = peerPubkey ?? selectedDirectMessagePeerPubkey;
  const conversation = directMessages.find(
    (item) => item.peer_pubkey === conversationPeerPubkey
  );
  const conversationTimeline = conversationPeerPubkey
    ? directMessageTimelineByPeer[conversationPeerPubkey] ?? []
    : [];
  const conversationStatus = conversationPeerPubkey
    ? directMessageStatusByPeer[conversationPeerPubkey] ?? conversation?.status ?? null
    : null;
  const conversationAuthor = conversationPeerPubkey
    ? knownAuthorsByPubkey[conversationPeerPubkey] ?? null
    : null;
  const conversationLabel = conversationPeerPubkey
    ? authorDisplayLabel(
        conversationPeerPubkey,
        conversation?.peer_display_name,
        conversation?.peer_name
      )
    : null;
  const conversationPicture = resolveProfilePictureSrc(conversationAuthor, mediaObjectUrls);
  const activeConversation = conversationPeerPubkey === selectedDirectMessagePeerPubkey;

  return (
    <>
      {surfaceKind !== 'conversation' ? (
        <Card className='shell-workspace-card'>
        <div className='panel-header'>
          <div>
            <h3>{t('shell:messages.title')}</h3>
            <small>{t('shell:messages.conversationCount', { count: directMessages.length })}</small>
          </div>
          {selectedDirectMessagePeerPubkey ? (
            <Button variant='secondary' type='button' onClick={() => void openDirectMessageList('replace')}>
              {t('shell:messages.all')}
            </Button>
          ) : null}
        </div>
        {directMessageError ? <Notice tone='destructive'>{directMessageError}</Notice> : null}
        {directMessages.length === 0 ? (
          <p className='empty'>{t('shell:messages.empty')}</p>
        ) : (
          <ul className='post-list'>
            {directMessages.map((conversation) => {
              const label = authorDisplayLabel(
                conversation.peer_pubkey,
                conversation.peer_display_name,
                conversation.peer_name
              );
              const knownAuthor =
                knownAuthorsByPubkey[conversation.peer_pubkey] ??
                authorViewFromDirectMessageConversation(conversation);
              const picture = resolveProfilePictureSrc(knownAuthor, mediaObjectUrls);
              const selected = conversation.peer_pubkey === selectedDirectMessagePeerPubkey;
              return (
                <li key={conversation.peer_pubkey}>
                  <article className='post-card'>
                    <div className='post-meta'>
                      <AuthorIdentityButton
                        label={label}
                        picture={picture}
                        avatarTestId={`dm-conversation-avatar-${conversation.peer_pubkey}`}
                        onClick={() =>
                          void openAuthorDetail(conversation.peer_pubkey, {
                            historyMode: 'push',
                            preserveDirectMessageContext: true,
                            directMessagePeerPubkey: selectedDirectMessagePeerPubkey,
                          })
                        }
                      />
                      <span>
                        {conversation.last_message_at
                          ? formatLocalizedTime(conversation.last_message_at, locale)
                          : t('common:fallbacks.noEvents')}
                      </span>
                    </div>
                    <div className='post-body'>
                      <strong className='post-title'>
                        {t('shell:messages.latest', {
                          preview: conversation.last_message_preview ?? t('common:fallbacks.none'),
                        })}
                      </strong>
                    </div>
                    <div className='post-actions'>
                      <Button
                        variant={selected ? 'primary' : 'secondary'}
                        type='button'
                        onClick={() => void openDirectMessagePane(conversation.peer_pubkey)}
                      >
                        {t('shell:messages.open')}
                      </Button>
                    </div>
                  </article>
                </li>
              );
            })}
          </ul>
        )}
        </Card>
      ) : null}

      {conversationPeerPubkey && surfaceKind !== 'messages' ? (
        <>
          {activeConversation && directMessageError ? (
            <Notice tone='destructive'>{directMessageError}</Notice>
          ) : null}
          <Card className='shell-workspace-card'>
            {conversationTimeline.length === 0 ? (
              <p className='empty'>{t('shell:messages.noMessages')}</p>
            ) : (
              <ul className='post-list'>
                {conversationTimeline.map((message) => {
                  const image = selectPrimaryImageAttachment(message.attachments);
                  const poster = selectVideoPosterAttachment(message.attachments);
                  const video = selectVideoManifestAttachment(message.attachments);
                  const imageSrc = image ? mediaObjectUrls[image.hash] ?? null : null;
                  const posterSrc = poster ? mediaObjectUrls[poster.hash] ?? null : null;
                  const videoSrc = video ? mediaObjectUrls[video.hash] ?? null : null;
                  const videoUnsupported = Boolean(video && unsupportedVideoManifests[video.hash]);
                  const authorPubkey = message.outgoing
                    ? syncStatus.local_author_pubkey
                    : conversationPeerPubkey;
                  const authorLabel = message.outgoing
                    ? profileAuthorLabel
                    : conversationLabel ?? conversationPeerPubkey;
                  const authorPicture = message.outgoing
                    ? viewModels.localDirectMessageAuthorPicture
                    : conversationPicture;
                  return (
                    <li key={message.message_id}>
                      <article className='post-card'>
                        <div className='post-meta'>
                          <AuthorIdentityButton
                            label={authorLabel}
                            picture={authorPicture}
                            avatarTestId={`dm-message-avatar-${message.message_id}`}
                            onClick={() =>
                              void openAuthorDetail(authorPubkey, {
                                historyMode: 'push',
                                preserveDirectMessageContext: true,
                                directMessagePeerPubkey: conversationPeerPubkey,
                              })
                            }
                          />
                          <span>{formatLocalizedTime(message.created_at, locale)}</span>
                          <span className='reply-chip'>
                            {t(
                              message.delivered
                                ? 'shell:messages.delivered'
                                : 'shell:messages.pending'
                            )}
                          </span>
                        </div>
                        {message.text ? (
                          <div className='post-body'>
                            <strong className='post-title'>{message.text}</strong>
                          </div>
                        ) : null}
                        {image ? (
                          imageSrc ? (
                            <div className='draft-preview-frame'>
                              <img
                                className='draft-preview-image'
                                src={imageSrc}
                                alt={t('common:media.imageAlt')}
                              />
                            </div>
                          ) : mediaObjectUrls[image.hash] === null ? (
                            <MediaFetchFailure
                              hashes={[image.hash]}
                              retrying={mediaRetryingHashes[image.hash] === true}
                              testId={`dm-media-fetch-failure-${message.message_id}`}
                            />
                          ) : (
                            <small>{t('common:media.syncingImage')}</small>
                          )
                        ) : null}
                        {video ? (
                          videoSrc && !videoUnsupported ? (
                            <video
                              className='post-card-video'
                              controls
                              playsInline
                              poster={posterSrc ?? undefined}
                              src={videoSrc}
                            />
                          ) : posterSrc ? (
                            <div className='draft-preview-frame'>
                              <img
                                className='draft-preview-image'
                                src={posterSrc}
                                alt={t('common:media.videoPosterAlt')}
                              />
                            </div>
                          ) : [video, poster].every(
                              (attachment) =>
                                attachment === null || mediaObjectUrls[attachment.hash] === null
                            ) ? (
                            <MediaFetchFailure
                              hashes={[video, poster]
                                .filter((attachment) => attachment !== null)
                                .map((attachment) => attachment.hash)}
                              retrying={[video, poster].some(
                                (attachment) =>
                                  attachment !== null &&
                                  mediaRetryingHashes[attachment.hash] === true
                              )}
                              testId={`dm-video-fetch-failure-${message.message_id}`}
                            />
                          ) : (
                            <small>{t('common:media.syncingPoster')}</small>
                          )
                        ) : null}
                        <div className='post-actions'>
                          <Button
                            variant='secondary'
                            type='button'
                            onClick={() =>
                              void handleDeleteDirectMessageMessage(
                                conversationPeerPubkey,
                                message.message_id
                              )
                            }
                          >
                            {t('common:actions.clear')}
                          </Button>
                        </div>
                      </article>
                    </li>
                  );
                })}
              </ul>
            )}
          </Card>

          {activeConversation && showComposer ? <Card className='shell-workspace-card'>
            {conversationStatus && !conversationStatus.send_enabled ? (
              <Notice tone='warning'>
                {t('shell:messages.mutualRequired')}
              </Notice>
            ) : null}
            <form className='composer' onSubmit={(event) => void handleSendDirectMessage(event)}>
              <Textarea
                value={directMessageComposer}
                onChange={(event) => setDirectMessageComposer(event.target.value)}
                onPaste={(event) => {
                  if (
                    directMessageSending ||
                    conversationStatus?.send_enabled === false ||
                    !handleDirectMessageAttachmentPaste
                  ) {
                    return;
                  }
                  const images = clipboardImageFiles(event.clipboardData);
                  if (images.length === 0) {
                    return;
                  }
                  event.preventDefault();
                  void handleDirectMessageAttachmentPaste(images);
                }}
                placeholder={t('common:composer.writeMessage')}
                disabled={
                  directMessageSending || conversationStatus?.send_enabled === false
                }
              />
              <Label className='file-field file-field-compact'>
                <span>{t('common:fallbacks.attachment')}</span>
                <Input
                  key={directMessageAttachmentInputKey}
                  aria-label={t('common:fallbacks.attachment')}
                  type='file'
                  accept='image/*,video/*'
                  disabled={
                    directMessageSending || conversationStatus?.send_enabled === false
                  }
                  onChange={(event) => {
                    void handleDirectMessageAttachmentSelection(event);
                  }}
                />
              </Label>
              <ComposerDraftPreviewList
                items={viewModels.directMessageDraftViews}
                onRemove={handleRemoveDirectMessageDraftAttachment}
              />
              <div className='topic-diagnostic topic-diagnostic-secondary'>
                <span>
                  {t('shell:messages.pendingOutbox', {
                    count: formatCount(conversationStatus?.pending_outbox_count ?? 0),
                  })}
                </span>
              </div>
              <Button
                type='submit'
                disabled={
                  directMessageSending || conversationStatus?.send_enabled === false
                }
              >
                {directMessageSending
                  ? t('shell:messages.sending')
                  : t('common:actions.send')}
              </Button>
            </form>
          </Card> : null}
        </>
      ) : null}
    </>
  );
}

export function DesktopShellMessagesWorkspace(props: DesktopShellMessagesSurfaceProps) {
  return <DesktopShellMessagesSurface {...props} />;
}

export type DesktopShellNotificationsSurfaceProps = {
  t: Translate;
  locale: SupportedLocale;
  handleOpenNotification: (notification: NotificationView) => Promise<void>;
  /** #962: 通知の受信設定(設定 > 通知)を開く。一覧の状態や既読は変えない。 */
  onOpenNotificationSettings: () => void;
};

export function DesktopShellNotificationsSurface({
  t,
  locale,
  handleOpenNotification,
  onOpenNotificationSettings,
}: DesktopShellNotificationsSurfaceProps) {
  const {
    knownAuthorsByPubkey,
    mediaObjectUrls,
    notifications,
    notificationAutoReadError,
    notificationPanelState,
    adultContentEnabled,
    timelineContentAdvisories,
  } = useDesktopShellStore(
    useShallow((s) => ({
      knownAuthorsByPubkey: s.knownAuthorsByPubkey,
      mediaObjectUrls: s.mediaObjectUrls,
      notifications: s.notifications,
      notificationAutoReadError: s.notificationAutoReadError,
      notificationPanelState: s.notificationPanelState,
      adultContentEnabled: s.adultContentEnabled,
      timelineContentAdvisories: s.timelineContentAdvisories,
    }))
  );
  const notificationItems = useMemo<NotificationItemView[]>(
    () =>
      notifications.map((notification) => {
        const knownAuthor = knownAuthorsByPubkey[notification.actor_pubkey] ?? null;
        const actorLabel = authorDisplayLabel(
          notification.actor_pubkey,
          notification.actor_display_name,
          notification.actor_name
        );
        const actorPicture = knownAuthor
          ? resolveProfilePictureSrc(knownAuthor, mediaObjectUrls)
          : notification.actor_picture_asset
            ? mediaObjectUrls[notification.actor_picture_asset.hash] ?? null
            : null;
        const contextLabel =
          notification.kind === 'direct_message'
            ? t('shell:notifications.context.directMessage')
            : notification.topic_id && notification.channel_id
              ? t('shell:notifications.context.topicChannel', {
                  channel: notification.channel_id,
                  topic: notification.topic_id,
                })
              : notification.topic_id
                ? t('shell:notifications.context.topic', {
                    topic: notification.topic_id,
                  })
                : t('shell:notifications.context.authorActivity');
        // #1056: 採用 node の content advisory が対象投稿に付いていれば、自己申告と同じく伏せる。
        const advisoryGated =
          Boolean(notification.object_id) &&
          (timelineContentAdvisories[advisorySubjectKey('post_id', notification.object_id ?? '')] ?? [])
            .some((entry) => isGatingContentAdvisory(entry.advisory));
        const adultPreviewGated =
          Boolean(notification.object_id) &&
          !adultContentEnabled &&
          (notification.content_labels == null ||
            hasAdultContentLabel(notification.content_labels) ||
            advisoryGated);
        const previewText = adultPreviewGated
          ? notification.content_labels == null
            ? t('shell:notifications.preview.noContent')
            : hasAdultContentLabel(notification.content_labels)
              ? t('common:feed.adultContentHidden')
              : t('common:feed.advisoryContentHidden')
          : notification.preview_text ??
            (notification.kind === 'followed'
              ? t('shell:notifications.preview.followed')
              : notification.kind === 'direct_message'
                ? t('shell:notifications.preview.noMessage')
                : t('shell:notifications.preview.noContent'));

        return {
          ...notification,
          actorLabel,
          actorPicture,
          contextLabel,
          kindLabel: t(`shell:notifications.kinds.${notification.kind}`),
          previewText,
          receivedLabel: formatLocalizedTime(notification.received_at, locale),
          unread: !notification.read_at,
        };
      }),
    [
      adultContentEnabled,
      knownAuthorsByPubkey,
      locale,
      mediaObjectUrls,
      notifications,
      t,
      timelineContentAdvisories,
    ]
  );

  return (
    <>
      {notificationPanelState.status === 'loading' ? (
        <Notice>{t('shell:notifications.loading')}</Notice>
      ) : null}
      {notificationPanelState.status === 'error' && notificationPanelState.error ? (
        <Notice tone='destructive'>{notificationPanelState.error}</Notice>
      ) : null}
      {notificationAutoReadError ? <Notice tone='warning'>{notificationAutoReadError}</Notice> : null}

      {/* #962: 受信設定(設定 > 通知)への導線。header は要約と更新で埋まるため本文先頭に置く。 */}
      <div className='flex justify-end'>
        <Button variant='ghost' size='sm' type='button' onClick={onOpenNotificationSettings}>
          <Settings className='size-4' aria-hidden='true' />
          {t('shell:notifications.settings')}
        </Button>
      </div>

      <Card className='shell-workspace-card'>
        {notificationPanelState.status === 'ready' && notificationItems.length === 0 ? (
          <p className='empty-state'>{t('shell:notifications.empty')}</p>
        ) : null}
        {notificationItems.length > 0 ? (
          <ul className='notification-list' aria-label={t('shell:notifications.title')}>
            {notificationItems.map((notification) => (
              <li key={notification.notification_id}>
                <button
                  className='notification-item'
                  data-unread={notification.unread}
                  type='button'
                  onClick={() => void handleOpenNotification(notification)}
                >
                  <div className='notification-item-header'>
                    <div className='notification-item-author'>
                      <AuthorAvatar
                        label={notification.actorLabel}
                        picture={notification.actorPicture}
                        testId={`notification-avatar-${notification.notification_id}`}
                      />
                      <div className='notification-item-copy'>
                        <span className='notification-item-author-label'>
                          {notification.actorLabel}
                        </span>
                        <div className='notification-item-badges'>
                          <Badge tone={notification.unread ? 'accent' : 'neutral'}>
                            {notification.kindLabel}
                          </Badge>
                          {notification.unread ? (
                            <Badge tone='warning'>{t('shell:notifications.unread')}</Badge>
                          ) : null}
                        </div>
                      </div>
                    </div>
                    <span className='notification-item-time'>{notification.receivedLabel}</span>
                  </div>
                  <div className='notification-item-body'>
                    <p className='notification-item-preview'>{notification.previewText}</p>
                    <small className='notification-item-context'>{notification.contextLabel}</small>
                  </div>
                </button>
              </li>
            ))}
          </ul>
        ) : null}
      </Card>
    </>
  );
}

export function DesktopShellNotificationsWorkspace(props: DesktopShellNotificationsSurfaceProps) {
  return <DesktopShellNotificationsSurface {...props} />;
}

export type DesktopShellDetailSurfaceStackProps = {
  api: DesktopApi;
  t: Translate;
  viewModels: Pick<
    ViewModels,
    | 'authorDetailView'
    | 'buildPostCardView'
    | 'selectedAuthorTimelinePostViews'
    | 'threadPanelState'
    | 'threadPostViews'
  >;
  loadMoreThread: (topic: string, threadId: string) => Promise<void>;
  loadReactionCatalogData: () => Promise<void>;
  openAuthorDetail: OpenAuthorDetail;
  openDirectMessagePane: OpenDirectMessagePane;
  openThread: OpenThread;
  beginColumnReply: (post: PostView) => void;
  handleSimpleRepost: (post: PostView) => Promise<void>;
  beginColumnQuoteRepost: (post: PostView) => void;
  handleRetryLocalPost: (post: PostView) => void;
  handleRestoreLocalPost: (post: PostView) => void;
  handleWithdrawPost: (post: PostView) => Promise<void>;
  handleToggleReaction: (post: PostView, reactionKey: ReactionKeyInput) => Promise<void>;
  handleBookmarkCustomReaction: (
    asset: Parameters<import('@/lib/api').DesktopApi['bookmarkCustomReaction']>[0]
  ) => Promise<void>;
  handleActivateReference: (reference: InternalSmartReference) => Promise<void>;
  handleCopyPostLink: (link: string) => void;
  handleRelationshipAction: (authorPubkey: string, following: boolean) => Promise<void>;
  handleMuteAction: (authorPubkey: string, muted: boolean) => Promise<void>;
  handleBlockAction: (authorPubkey: string, blocking: boolean) => Promise<void>;
  handleOpenOriginalTopic: (topicId: string) => Promise<void>;
  openCommunityNodeSettings: () => void;
  surfaceKind: 'thread' | 'profile';
  entityId?: string;
  topicId?: string;
};

export function DesktopShellDetailSurfaceStack({
  api,
  t,
  viewModels,
  loadMoreThread,
  loadReactionCatalogData,
  openAuthorDetail,
  openDirectMessagePane,
  openThread,
  beginColumnReply,
  handleSimpleRepost,
  beginColumnQuoteRepost,
  handleRetryLocalPost,
  handleRestoreLocalPost,
  handleWithdrawPost,
  handleToggleReaction,
  handleBookmarkCustomReaction,
  handleActivateReference,
  handleCopyPostLink,
  handleRelationshipAction,
  handleMuteAction,
  handleBlockAction,
  handleOpenOriginalTopic,
  openCommunityNodeSettings,
  surfaceKind,
  entityId,
  topicId,
}: DesktopShellDetailSurfaceStackProps) {
  const {
    activeTopic,
    bookmarkedReactionAssets,
    communityNodeConfig,
    communityNodeManifests,
    communityNodeStatuses,
    focusedObjectId,
    authorErrorsByPubkey,
    authorTimelinesByPubkey,
    knownAuthorsByPubkey,
    mediaObjectUrls,
    ownedReactionAssets,
    recentReactions,
    selectedAuthor,
    selectedAuthorPubkey,
    selectedThread,
    syncStatus,
    threadLoadingMoreById,
    threadNextCursorById,
    threadsById,
  } = useDesktopShellStore(
    useShallow((s) => ({
      activeTopic: activeWorkspaceScope(s.workspaceState).topicId,
      bookmarkedReactionAssets: s.bookmarkedReactionAssets,
      communityNodeConfig: s.communityNodeConfig,
      communityNodeManifests: s.communityNodeManifests,
      communityNodeStatuses: s.communityNodeStatuses,
      focusedObjectId: s.focusedObjectId,
      authorErrorsByPubkey: s.authorErrorsByPubkey,
      authorTimelinesByPubkey: s.authorTimelinesByPubkey,
      knownAuthorsByPubkey: s.knownAuthorsByPubkey,
      mediaObjectUrls: s.mediaObjectUrls,
      ownedReactionAssets: s.ownedReactionAssets,
      recentReactions: s.recentReactions,
      selectedAuthor: s.selectedAuthor,
      selectedAuthorPubkey: s.selectedAuthorPubkey,
      selectedThread: s.selectedThread,
      syncStatus: s.syncStatus,
      threadLoadingMoreById: s.threadLoadingMoreById,
      threadNextCursorById: s.threadNextCursorById,
      threadsById: s.threadsById,
    }))
  );
  // #1061: 著者ごとの例外を設定・解除したとき、表示中の判断を差し替える。
  const setAuthorTrustGates = useDesktopShellFieldSetter('authorTrustGates');
  const effectiveThreadId = surfaceKind === 'thread' && entityId ? entityId : selectedThread;
  const effectiveAuthorPubkey =
    surfaceKind === 'profile' && entityId ? entityId : selectedAuthorPubkey;
  const effectiveTopicId = topicId ?? activeTopic;
  const selectedThreadHasMore = effectiveThreadId
    ? Boolean(threadNextCursorById[effectiveThreadId])
    : false;
  const selectedThreadLoadingMore = effectiveThreadId
    ? (threadLoadingMoreById[effectiveThreadId] ?? false)
    : false;
  const effectiveThreadPostViews = useMemo(
    () => {
      const posts = effectiveThreadId ? threadsById[effectiveThreadId] ?? [] : [];
      return posts.map((post) => viewModels.buildPostCardView(post, 'thread'));
    },
    [effectiveThreadId, threadsById, viewModels]
  );
  const effectiveAuthor = effectiveAuthorPubkey
    ? knownAuthorsByPubkey[effectiveAuthorPubkey] ??
      (effectiveAuthorPubkey === selectedAuthorPubkey ? selectedAuthor : null)
    : null;
  const effectiveAuthorTimelinePostViews = useMemo(
    () => {
      const posts = effectiveAuthorPubkey
        ? authorTimelinesByPubkey[effectiveAuthorPubkey] ??
          (effectiveAuthorPubkey === selectedAuthorPubkey
            ? viewModels.selectedAuthorTimelinePostViews.map((item) => item.post)
            : [])
        : [];
      return posts.map((post) => viewModels.buildPostCardView(post, 'timeline'));
    },
    [authorTimelinesByPubkey, effectiveAuthorPubkey, selectedAuthorPubkey, viewModels]
  );
  const effectiveAuthorDetailView = useMemo<AuthorDetailView>(() => {
    if (!effectiveAuthor) {
      return {
        author: null,
        displayLabel: t('common:fallbacks.authorDetail'),
        summary: null,
        authorError: effectiveAuthorPubkey
          ? authorErrorsByPubkey[effectiveAuthorPubkey] ?? null
          : null,
      };
    }
    return {
      author: effectiveAuthor,
      displayLabel: authorDisplayLabel(
        effectiveAuthor.author_pubkey,
        effectiveAuthor.display_name,
        effectiveAuthor.name
      ),
      pictureSrc: resolveProfilePictureSrc(effectiveAuthor, mediaObjectUrls),
      summary: {
        label: strongestRelationshipLabel(effectiveAuthor),
        following: effectiveAuthor.following,
        followedBy: effectiveAuthor.followed_by,
        mutual: effectiveAuthor.mutual,
        friendOfFriend: effectiveAuthor.friend_of_friend,
        muted: effectiveAuthor.muted,
        blocking: effectiveAuthor.blocking,
        blockedBy: effectiveAuthor.blocked_by,
        viaPubkeys: effectiveAuthor.friend_of_friend_via_pubkeys.map(shortPubkey),
        isSelf: effectiveAuthor.author_pubkey === syncStatus.local_author_pubkey,
        canFollow: effectiveAuthor.author_pubkey !== syncStatus.local_author_pubkey,
        followActionLabel: effectiveAuthor.following ? 'Unfollow' : 'Follow',
        muteActionLabel: effectiveAuthor.muted ? 'Unmute' : 'Mute',
        blockActionLabel: effectiveAuthor.blocking ? 'Unblock' : 'Block',
      },
      canMessage:
        effectiveAuthor.author_pubkey !== syncStatus.local_author_pubkey &&
        effectiveAuthor.mutual,
      authorError: authorErrorsByPubkey[effectiveAuthor.author_pubkey] ?? null,
    };
  }, [
    authorErrorsByPubkey,
    effectiveAuthor,
    effectiveAuthorPubkey,
    mediaObjectUrls,
    syncStatus.local_author_pubkey,
    t,
  ]);
  // 信頼・関係の候補は、認証済み・必須同意承認済み・通信エラーなし・community_local_trust 提供中の
  // ノードに限る(#705)。索引側の適格判定と同じ境界。
  const trustRelationNodeBaseUrls = useMemo(
    () => eligibleTrustRelationNodes(communityNodeConfig, communityNodeStatuses, communityNodeManifests),
    [communityNodeConfig, communityNodeManifests, communityNodeStatuses]
  );
  const fetchReportManifest = (baseUrl: string) =>
    api.fetchCommunityNodeManifest(baseUrl);
  const submitReport = (request: import('@/lib/api').SubmitCommunityNodeReportRequest) =>
    api.submitCommunityNodeReport(request);
  const threadContent = effectiveThreadId ? (
    <ThreadPanel
      state={{ ...viewModels.threadPanelState, selectedThreadId: effectiveThreadId }}
      posts={effectiveThreadPostViews}
      hasMore={selectedThreadHasMore}
      loadingMore={selectedThreadLoadingMore}
      onLoadMore={() => void loadMoreThread(effectiveTopicId, effectiveThreadId)}
      onOpenAuthor={(authorPubkey) =>
        void openAuthorDetail(authorPubkey, {
          fromThread: true,
          threadId: effectiveThreadId,
        })
      }
      onOpenThread={(threadId) => void openThread(threadId)}
      onOpenThreadInTopic={(threadId, topicId) => void openThread(threadId, { topic: topicId })}
      onReply={beginColumnReply}
      onRepost={(post) => void handleSimpleRepost(post)}
      onQuoteRepost={beginColumnQuoteRepost}
      onRetryLocalPost={handleRetryLocalPost}
      onRestoreLocalPost={handleRestoreLocalPost}
      onWithdraw={(post) => void handleWithdrawPost(post)}
      localAuthorPubkey={syncStatus.local_author_pubkey}
      mediaObjectUrls={mediaObjectUrls}
      ownedReactionAssets={ownedReactionAssets}
      bookmarkedReactionAssets={bookmarkedReactionAssets}
      recentReactions={recentReactions}
      onToggleReaction={(post, reactionKey) => void handleToggleReaction(post, reactionKey)}
      onBookmarkCustomReaction={(asset) => void handleBookmarkCustomReaction(asset)}
      onReactionPickerOpen={() => void loadReactionCatalogData()}
      onActivateReference={(reference) => void handleActivateReference(reference)}
      onCopyPostLink={handleCopyPostLink}
      focusedPostObjectId={effectiveThreadId === selectedThread ? focusedObjectId : null}
      onSubmitReport={submitReport}
      onCopyReportContact={(value) => void copyTextToClipboard(value)}
      onFetchReportManifest={fetchReportManifest}
      onMuteReportAuthor={(authorPubkey) => handleMuteAction(authorPubkey, false)}
    />
  ) : null;
  const authorContent = effectiveAuthorPubkey ? (
    <div className='shell-main-stack'>
      <AuthorDetailCard
        view={effectiveAuthorDetailView}
        localAuthorPubkey={syncStatus.local_author_pubkey}
        onToggleRelationship={(authorPubkey, following) =>
          void handleRelationshipAction(authorPubkey, following)
        }
        onToggleMute={(authorPubkey, muted) => void handleMuteAction(authorPubkey, muted)}
        onToggleBlock={(authorPubkey, blocking) => void handleBlockAction(authorPubkey, blocking)}
        onOpenDirectMessage={(authorPubkey) => void openDirectMessagePane(authorPubkey)}
        onSubmitReport={submitReport}
        onCopyReportContact={(value) => void copyTextToClipboard(value)}
        onFetchReportManifest={fetchReportManifest}
        trustDisplayException={
          effectiveAuthorPubkey ? (
            <AuthorTrustDisplayExceptionField
              authorPubkey={effectiveAuthorPubkey}
              loadAlwaysVisible={async (pubkey) =>
                (await api.listAuthorTrustDisplayExceptions()).includes(pubkey)
              }
              setAlwaysVisible={async (pubkey, alwaysVisible) => {
                const gate = await api.setAuthorTrustDisplayException(pubkey, alwaysVisible);
                // #1061: 例外は判断を作り直す。表示中の投稿へ即時に反映する(AC-5)。
                setAuthorTrustGates((current) => ({ ...current, [gate.author_pubkey]: gate }));
                return gate.always_visible;
              }}
            />
          ) : null
        }
        communityNodeAdvisory={
          <CommunityNodeAdvisoryPanel
            api={api}
            targetPubkey={effectiveAuthorPubkey}
            nodeBaseUrls={trustRelationNodeBaseUrls}
            onOpenCommunityNodeSettings={openCommunityNodeSettings}
          />
        }
      />
      <Card className='shell-workspace-card'>
        <TimelineFeed
          posts={effectiveAuthorTimelinePostViews}
          emptyCopy={t('profile:feed.noAuthorPosts')}
          onOpenAuthor={(authorPubkey) => void openAuthorDetail(authorPubkey)}
          onOpenThread={(threadId) => void openThread(threadId)}
          onOpenThreadInTopic={(threadId, topicId) => void openThread(threadId, { topic: topicId })}
          onReply={beginColumnReply}
          readOnly={true}
          onOpenOriginalTopic={(topicId) => void handleOpenOriginalTopic(topicId)}
          onActivateReference={(reference) => void handleActivateReference(reference)}
          onCopyPostLink={handleCopyPostLink}
          onSubmitReport={submitReport}
          onCopyReportContact={(value) => void copyTextToClipboard(value)}
          onFetchReportManifest={fetchReportManifest}
          onMuteReportAuthor={(authorPubkey) => handleMuteAction(authorPubkey, false)}
        />
      </Card>
    </div>
  ) : null;

  return surfaceKind === 'thread' ? threadContent : authorContent;
}
