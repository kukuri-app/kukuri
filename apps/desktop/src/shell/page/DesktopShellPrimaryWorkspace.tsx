import { type FormEvent, type ReactNode, useMemo } from 'react';
import { Link2 } from 'lucide-react';

import { BookmarksEmptyState, BookmarksListFrame } from '@/components/core/BookmarksEmptyState';
import { TimelineFeed } from '@/components/core/TimelineFeed';
import { CommunityIndexWorkspace } from '@/components/core/CommunityIndexWorkspace';
import type { CommunityIndexingTarget } from '@/components/core/CommunityIndexingRequestDialog';
import { MetaverseRoomPanel } from '@/components/extended/MetaverseRoomPanel';
import type { CommunityNodePanelView } from '@/components/settings/types';
import { AuthorTrustGateNotice } from '@/components/core/AuthorTrustGateNotice';
import { useAuthorTrustGateReveal } from '@/components/core/useAuthorTrustGateReveal';
import { GameRoomPanel } from '@/components/extended/GameRoomPanel';
import type { MetaverseRoomActions } from '@/components/extended/metaverse/MetaverseRoomActions';
import { ProfileConnectionsPanel } from '@/components/extended/ProfileConnectionsPanel';
import { ProfileEditorPanel } from '@/components/extended/ProfileEditorPanel';
import { ProfileOverviewPanel } from '@/components/extended/ProfileOverviewPanel';
import { Button } from '@/components/ui/button';
import { IconButton } from '@/components/ui/icon-button';
import { Notice } from '@/components/ui/notice';
import { SmartReferenceText } from '@/components/core/SmartReferenceText';
import type { ProfileConnectionsView } from '@/components/shell/types';
import {
  primarySectionForColumn,
  type ColumnState,
  type ColumnTimelineView,
} from '@/shell/slices/workspace';

import type {
  CommunityNodeConsentDocumentRef,
  DesktopApi,
  PostView,
  ReactionKeyInput,
} from '@/lib/api';
import { formatLocalizedTime } from '@/i18n/format';
import type { SupportedLocale } from '@/i18n';
import { buildLiveLink, type InternalSmartReference } from '@/lib/internalLinks';
import { copyTextToClipboard } from '@/lib/utils';
import { consentPendingCommunityNodes, eligibleCommunityIndexNodes } from '@/lib/api/communityIndex';
import { communityIndexAvailability, type CommunityNodeAvailability } from '@/lib/api/communityNodeAvailability';
import type { FetchCommunityNodePolicyView } from '@/shell/actions/useCommunityNodePolicyDialog';
import type { SubmitCommunityNodeReportRequest } from '@/lib/api';
import {
  timelineStorageKeyForChannel,
  type GameEditorDraft,
  useDesktopShellFieldSetter,
  useDesktopShellStore,
} from '@/shell/store';
import {
  authorDisplayLabel,
  formatCount,
  createGameEditorDraft,
  localizeAudienceLabel,
  privateTimelineScope,
  resolveProfilePictureSrc,
  translateLiveStatus,
} from '@/shell/presentation';
import { useDesktopShellViewModels } from '@/shell/useDesktopShellViewModels';
import type { OpenAuthorDetail, OpenThread, Translate } from '@/shell/actions/shared';
import { useShallow } from 'zustand/react/shallow';
import { DEFAULT_ASYNC_PANEL_STATE } from '@/shell/slices/shared';

type ViewModels = ReturnType<typeof useDesktopShellViewModels>;

export type DesktopShellPrimarySurfaceProps = {
  t: Translate;
  api: DesktopApi;
  metaverseActions: MetaverseRoomActions;
  locale: SupportedLocale;
  column: ColumnState;
  profileAvatarInputKey: number;
  messagesWorkspace: ReactNode;
  notificationsWorkspace: ReactNode;
  viewModels: Pick<
    ViewModels,
    | 'activeComposeAudienceLabel'
    | 'activeGamePanelState'
    | 'activeGameRooms'
    | 'activeLivePanelState'
    | 'activeSocialConnectionViews'
    | 'activeTimelinePostViews'
    | 'activeTimelineScope'
    | 'bookmarkedTimelinePostViews'
    | 'buildPostCardView'
    | 'gatedMediaHashes'
    | 'gameDraftViews'
    | 'liveSessionListItems'
    | 'primarySectionItems'
    | 'profileEditorFields'
    | 'profileEditorHasPicture'
    | 'profileEditorPictureSrc'
    | 'profileTimelinePostViews'
    | 'selectedAuthorTimelinePostViews'
    | 'timelineViewItems'
  >;
  openCommunityNodeSettings: () => void;
  /** #960: 見つけるの空状態から既存の接続診断・タイムライン・索引登録申請へ移る。 */
  openConnectivitySettings?: () => void;
  openTimelineSection?: () => void;
  /** #994: 空状態の CTA。見つける section へ移る / 同じ Column の表示をフィードへ戻す / ブックマーク再取得。 */
  openExploreSection?: () => void;
  selectTimelineView?: (column: ColumnState, view: ColumnTimelineView) => void;
  retryBookmarks?: () => void;
  requestIndexing?: (target: CommunityIndexingTarget) => void;
  communityNodePanelView?: CommunityNodePanelView;
  onFetchCommunityNodeConsents?: FetchCommunityNodePolicyView;
  onAcceptCommunityNodeConsents?: (
    baseUrl: string,
    documents: CommunityNodeConsentDocumentRef[],
    language?: string
  ) => Promise<void>;
  onRetryCommunityNode: (availability: CommunityNodeAvailability) => Promise<void>;
  loadReactionCatalogData: () => Promise<void>;
  refreshProfile: () => Promise<void>;
  refreshTimelineFeed: (
    topic: string,
    currentThread: string | null,
    channelId?: string | null
  ) => Promise<void>;
  loadMoreTimeline: (topic: string, channelId?: string | null) => Promise<void>;
  openAuthorDetail: OpenAuthorDetail;
  openThread: OpenThread;
  beginColumnReply: (post: PostView) => void;
  handleSimpleRepost: (post: PostView) => Promise<void>;
  beginColumnQuoteRepost: (post: PostView) => void;
  handleRetryLocalPost: (post: PostView) => void;
  handleRestoreLocalPost: (post: PostView) => void;
  handleToggleReaction: (post: PostView, reactionKey: ReactionKeyInput) => Promise<void>;
  handleBookmarkCustomReaction: (
    asset: Parameters<DesktopApi['bookmarkCustomReaction']>[0]
  ) => Promise<void>;
  handleToggleBookmarkedPost: (post: PostView) => Promise<void>;
  handleWithdrawPost: (post: PostView) => Promise<void>;
  handleActivateReference: (reference: InternalSmartReference) => Promise<void>;
  handleCopyInternalLink: (link: string) => void;
  handleJoinLiveSession: (sessionId: string) => Promise<void>;
  handleLeaveLiveSession: (sessionId: string) => Promise<void>;
  handleEndLiveSession: (sessionId: string) => Promise<void>;
  handleCreateGameRoom: (event: FormEvent<HTMLFormElement>) => Promise<void>;
  updateGameDraft: (roomId: string, update: (draft: GameEditorDraft) => GameEditorDraft) => void;
  handleUpdateGameRoom: (roomId: string) => Promise<void>;
  openProfileOverview: () => void;
  openProfileEditor: () => void;
  openProfileConnections: (view: ProfileConnectionsView) => void;
  handleProfileFieldChange: (field: 'displayName' | 'name' | 'about', value: string) => void;
  onProfilePictureSelect: (file: File) => void;
  handleClearProfileAvatar: () => void;
  handleSaveProfile: (event: FormEvent<HTMLFormElement>) => Promise<void>;
  resetProfileDraft: () => void;
  handleRelationshipAction: (authorPubkey: string, following: boolean) => Promise<void>;
  handleMuteAction: (authorPubkey: string, muted: boolean) => Promise<void>;
  handleBlockAction: (authorPubkey: string, blocking: boolean) => Promise<void>;
  handleOpenOriginalTopic: (topicId: string) => Promise<void>;
};

export function DesktopShellPrimarySurface({
  t,
  api,
  metaverseActions,
  locale,
  column,
  profileAvatarInputKey,
  messagesWorkspace,
  notificationsWorkspace,
  viewModels,
  openCommunityNodeSettings,
  openConnectivitySettings,
  openTimelineSection,
  openExploreSection,
  selectTimelineView,
  retryBookmarks,
  requestIndexing,
  communityNodePanelView,
  onFetchCommunityNodeConsents,
  onAcceptCommunityNodeConsents,
  onRetryCommunityNode,
  loadReactionCatalogData,
  refreshTimelineFeed,
  refreshProfile,
  loadMoreTimeline,
  openAuthorDetail,
  openThread,
  beginColumnReply,
  handleSimpleRepost,
  beginColumnQuoteRepost,
  handleRetryLocalPost,
  handleRestoreLocalPost,
  handleToggleReaction,
  handleBookmarkCustomReaction,
  handleToggleBookmarkedPost,
  handleWithdrawPost,
  handleActivateReference,
  handleCopyInternalLink,
  handleJoinLiveSession,
  handleLeaveLiveSession,
  handleEndLiveSession,
  handleCreateGameRoom,
  updateGameDraft,
  handleUpdateGameRoom,
  openProfileOverview,
  openProfileEditor,
  openProfileConnections,
  handleProfileFieldChange,
  onProfilePictureSelect,
  handleClearProfileAvatar,
  handleSaveProfile,
  resetProfileDraft,
  handleRelationshipAction,
  handleMuteAction,
  handleBlockAction,
  handleOpenOriginalTopic,
}: DesktopShellPrimarySurfaceProps) {
  const {
    bookmarkedPosts,
    bookmarksPanelState,
    bookmarkedReactionAssets,
    knownAuthorsByPubkey,
    gameCreatePending,
    gameDescription,
    gameError,
    gameParticipantsInput,
    gameSavingByRoomId,
    gameTitle,
    liveError,
    localProfile,
    adultContentEnabled,
    mediaObjectUrls,
    unsupportedVideoManifests,
    communityNodeManifests,
    communityNodeConfig,
    communityIndexNodeBaseUrl,
    communityNodeStatuses,
    communityIndexNodePreference,
    communityNodeConfigLoaded,
    communityNodeStatusesLoaded,
    communityNodeConfigError,
    communityNodeStatusError,
    patchState,
    ownedReactionAssets,
    pendingTimelineCountsByKey,
    profileDirty,
    profileError,
    profilePanelState,
    profileHasLoaded,
    profileSaving,
    recentReactions,
    selectedLiveSessionId,
    selectedThread,
    shellChromeState,
    socialConnections,
    socialConnectionsPanelState,
    syncStatus,
    timelineLoadingMoreByKey,
    timelineNextCursorByKey,
    timelineUnavailableByKey,
    timelinesByKey,
    joinedChannelsByTopic,
    liveSessionsByScopeKey,
    authorTrustGates,
    livePanelStateByScopeKey,
    livePendingBySessionId,
    gameRoomsByScopeKey,
    gamePanelStateByScopeKey,
    gameDrafts,
  } = useDesktopShellStore(
    useShallow((s) => ({
      bookmarkedPosts: s.bookmarkedPosts,
      bookmarksPanelState: s.bookmarksPanelState,
      bookmarkedReactionAssets: s.bookmarkedReactionAssets,
      knownAuthorsByPubkey: s.knownAuthorsByPubkey,
      gameCreatePending: s.gameCreatePending,
      gameDescription: s.gameDescription,
      gameError: s.gameError,
      gameParticipantsInput: s.gameParticipantsInput,
      gameSavingByRoomId: s.gameSavingByRoomId,
      gameTitle: s.gameTitle,
      liveError: s.liveError,
      localProfile: s.localProfile,
      adultContentEnabled: s.adultContentEnabled,
      mediaObjectUrls: s.mediaObjectUrls,
      unsupportedVideoManifests: s.unsupportedVideoManifests,
      communityNodeManifests: s.communityNodeManifests,
      communityNodeConfig: s.communityNodeConfig,
      communityIndexNodeBaseUrl: s.communityIndexNodeBaseUrl,
      communityNodeStatuses: s.communityNodeStatuses,
      communityIndexNodePreference: s.communityIndexNodePreference,
      communityNodeConfigLoaded: s.communityNodeConfigLoaded,
      communityNodeStatusesLoaded: s.communityNodeStatusesLoaded,
      communityNodeConfigError: s.communityNodeConfigError,
      communityNodeStatusError: s.communityNodeStatusError,
      patchState: s.patchState,
      ownedReactionAssets: s.ownedReactionAssets,
      pendingTimelineCountsByKey: s.pendingTimelineCountsByKey,
      profileDirty: s.profileDirty,
      profileError: s.profileError,
      profilePanelState: s.profilePanelState,
      profileHasLoaded: s.profileHasLoaded,
      profileSaving: s.profileSaving,
      recentReactions: s.recentReactions,
      selectedLiveSessionId: s.selectedLiveSessionId,
      selectedThread: s.selectedThread,
      shellChromeState: s.shellChromeState,
      socialConnections: s.socialConnections,
      socialConnectionsPanelState: s.socialConnectionsPanelState,
      syncStatus: s.syncStatus,
      timelineLoadingMoreByKey: s.timelineLoadingMoreByKey,
      timelineNextCursorByKey: s.timelineNextCursorByKey,
      timelineUnavailableByKey: s.timelineUnavailableByKey,
      timelinesByKey: s.timelinesByKey,
      joinedChannelsByTopic: s.joinedChannelsByTopic,
      liveSessionsByScopeKey: s.liveSessionsByScopeKey,
      authorTrustGates: s.authorTrustGates,
      livePanelStateByScopeKey: s.livePanelStateByScopeKey,
      livePendingBySessionId: s.livePendingBySessionId,
      gameRoomsByScopeKey: s.gameRoomsByScopeKey,
      gamePanelStateByScopeKey: s.gamePanelStateByScopeKey,
      gameDrafts: s.gameDrafts,
    }))
  );
  // #1061: 配信一覧の折りたたみと、この一覧だけの「表示する」。
  const liveTrustGate = useAuthorTrustGateReveal(authorTrustGates);
  const setGameTitle = useDesktopShellFieldSetter('gameTitle');
  const setGameDescription = useDesktopShellFieldSetter('gameDescription');
  const setGameParticipantsInput = useDesktopShellFieldSetter('gameParticipantsInput');
  const profileAuthorLabel = authorDisplayLabel(
    syncStatus.local_author_pubkey,
    localProfile?.display_name,
    localProfile?.name
  );
  const bookmarkedPostIds = useMemo(
    () => new Set(bookmarkedPosts.map((item) => item.post.object_id)),
    [bookmarkedPosts]
  );
  const submitReport = (request: SubmitCommunityNodeReportRequest) =>
    api.submitCommunityNodeReport(request);
  const fetchReportManifest = (baseUrl: string) =>
    api.fetchCommunityNodeManifest(baseUrl);
  // #1192: 権利侵害申請モーダルで提示する権利侵害申出ポリシーの取得(認証不要の公開カタログ)。
  const fetchNodePolicies = (baseUrl: string, language?: string) =>
    api.fetchCommunityNodePolicies(baseUrl, language);
  const muteReportAuthor = async (authorPubkey: string) => {
    await handleMuteAction(authorPubkey, false);
  };
  const copyReportContact = (value: string) => void copyTextToClipboard(value);
  const surfaceTopic = column.scope?.topicId ?? '';
  const surfaceChannelId = column.scope?.channelId ?? null;
  const surfaceTimelineScope = privateTimelineScope(surfaceChannelId);
  // Thread は「この surface(Column)の scope」で開く。非 active Column の投稿本文クリックは
  // Column の activate(route 同期)を伴わないため、global の選択 channel に依存すると別 channel の
  // scope で Thread が開き返信先がずれる。topic が異なる場合は public として開く。
  const openThreadInSurfaceScope = (threadId: string) =>
    void openThread(threadId, { topic: surfaceTopic, channelId: surfaceChannelId });
  const openThreadInTopicFromSurface = (threadId: string, topicId: string) =>
    void openThread(threadId, {
      topic: topicId,
      channelId: topicId === surfaceTopic ? surfaceChannelId : null,
    });
  const activeTimelineKey = timelineStorageKeyForChannel(surfaceTopic, surfaceChannelId);
  const surfaceJoinedChannels = useMemo(
    () => joinedChannelsByTopic[surfaceTopic] ?? [],
    [joinedChannelsByTopic, surfaceTopic]
  );
  const surfaceTimelinePostViews = useMemo(
    () =>
      (timelinesByKey[activeTimelineKey] ?? []).map((post) =>
        viewModels.buildPostCardView(post, 'timeline', surfaceJoinedChannels)
      ),
    [activeTimelineKey, surfaceJoinedChannels, timelinesByKey, viewModels]
  );
  const surfaceScopeKey = timelineStorageKeyForChannel(surfaceTopic, surfaceChannelId);
  const surfaceLiveSessions = liveSessionsByScopeKey[surfaceScopeKey] ?? [];
  const surfaceGameRooms = useMemo(
    () => gameRoomsByScopeKey[surfaceScopeKey] ?? [],
    [gameRoomsByScopeKey, surfaceScopeKey]
  );
  const surfaceLivePanelState =
    livePanelStateByScopeKey[surfaceScopeKey] ?? DEFAULT_ASYNC_PANEL_STATE;
  const surfaceGamePanelState =
    gamePanelStateByScopeKey[surfaceScopeKey] ?? DEFAULT_ASYNC_PANEL_STATE;
  const surfaceAudienceLabel = surfaceChannelId
    ? surfaceJoinedChannels.find((channel) => channel.channel_id === surfaceChannelId)?.label ??
      surfaceChannelId
    : 'Public';
  const surfaceLiveSessionListItems = surfaceLiveSessions.map((session) => ({
    session,
    isOwner: session.host_pubkey === syncStatus.local_author_pubkey,
    pending: Boolean(livePendingBySessionId[session.session_id]),
  }));
  const surfaceGameDraftViews = Object.fromEntries(
    surfaceGameRooms.map((room) => {
      const draft = gameDrafts[room.room_id] ?? createGameEditorDraft(room);
      return [
        room.room_id,
        {
          status: draft.status,
          phaseLabel: draft.phase_label,
          scores: draft.scores,
        },
      ];
    })
  );
  const activeTimelinePendingCount = pendingTimelineCountsByKey[activeTimelineKey] ?? 0;
  const activeTimelineHasMore = Boolean(timelineNextCursorByKey[activeTimelineKey]);
  const activeTimelineLoadingMore = timelineLoadingMoreByKey[activeTimelineKey] ?? false;
  const activeTimelineUnavailable = timelineUnavailableByKey[activeTimelineKey] ?? 0;
  const metaverseRooms = useMemo(
    () => surfaceGameRooms.filter((room) => room.room_kind === 'metaverse_room'),
    [surfaceGameRooms]
  );
  const scoreGameRooms = useMemo(
    () => surfaceGameRooms.filter((room) => room.room_kind === 'score_game'),
    [surfaceGameRooms]
  );
  const profileMode = shellChromeState.profileMode;
  const profileConnectionsView = shellChromeState.profileConnectionsView;
  const activeSurfaceSection = primarySectionForColumn(column);
  const activeTimelineView = column.kind === 'timeline' ? column.timelineView ?? 'feed' : 'feed';
  const eligibleIndexNodeBaseUrls = useMemo(
    () =>
      eligibleCommunityIndexNodes(
        communityNodeConfig,
        communityNodeStatuses,
        communityNodeManifests
      ),
    [communityNodeConfig, communityNodeManifests, communityNodeStatuses]
  );
  // #857: 未同意・撤回・再同意待ちの node は、Node 機能の利用直前に同意モーダルを提示する。
  const consentPendingNodeBaseUrls = useMemo(
    () => consentPendingCommunityNodes(communityNodeConfig, communityNodeStatuses),
    [communityNodeConfig, communityNodeStatuses]
  );
  const indexAvailability = communityIndexAvailability({
    config: communityNodeConfig, statuses: communityNodeStatuses, manifests: communityNodeManifests,
    preference: communityIndexNodePreference, configLoaded: communityNodeConfigLoaded,
    statusesLoaded: communityNodeStatusesLoaded, statusError: Boolean(communityNodeStatusError || communityNodeConfigError),
  });
  const showCommunityNodeUnavailableNotice =
    activeSurfaceSection === 'explore' &&
    communityNodeConfig.nodes.length > 0 &&
    eligibleIndexNodeBaseUrls.length === 0 &&
    communityNodeStatuses.some((status) => Boolean(status.last_error));

  return (
    <div className='shell-main-stack'>
      {showCommunityNodeUnavailableNotice ? (
        <Notice
          tone='warning'
          className='flex flex-col gap-3 md:flex-row md:items-start md:justify-between'
          data-testid='community-node-unavailable-notice'
        >
          <div className='space-y-1'>
            <p className='font-semibold'>{t('shell:workspace.communityNodeUnavailableTitle')}</p>
            <p>{t('shell:workspace.communityNodeUnavailableBody')}</p>
          </div>
          <Button
            variant='secondary'
            type='button'
            onClick={openCommunityNodeSettings}
          >
            {t('shell:workspace.communityNodeUnavailableAction')}
          </Button>
        </Notice>
      ) : null}
      <section className='shell-section'>
        {activeSurfaceSection === 'timeline' ? (
          <>
            <div className='shell-column-content'>
              {activeTimelineView === 'feed' ? (
                <TimelineFeed
                  posts={surfaceTimelinePostViews}
                  emptyCopy={t('shell:workspace.noPosts')}
                  onOpenAuthor={(authorPubkey) => void openAuthorDetail(authorPubkey)}
                  onOpenThread={openThreadInSurfaceScope}
                  onOpenThreadInTopic={openThreadInTopicFromSurface}
                  onReply={beginColumnReply}
                  onRepost={(post) => void handleSimpleRepost(post)}
                  onQuoteRepost={beginColumnQuoteRepost}
                  onRetryLocalPost={handleRetryLocalPost}
                  onRestoreLocalPost={handleRestoreLocalPost}
                  localAuthorPubkey={syncStatus.local_author_pubkey}
                  mediaObjectUrls={mediaObjectUrls}
                  ownedReactionAssets={ownedReactionAssets}
                  bookmarkedReactionAssets={bookmarkedReactionAssets}
                  recentReactions={recentReactions}
                  onToggleReaction={(post, reactionKey) => void handleToggleReaction(post, reactionKey)}
                  onBookmarkCustomReaction={(asset) => void handleBookmarkCustomReaction(asset)}
                  onReactionPickerOpen={() => void loadReactionCatalogData()}
                  showBookmarkAction={true}
                  bookmarkedPostIds={bookmarkedPostIds}
                  onToggleBookmark={(post) => void handleToggleBookmarkedPost(post)}
                  onWithdraw={(post) => void handleWithdrawPost(post)}
                  onActivateReference={(reference) => void handleActivateReference(reference)}
                  onCopyPostLink={handleCopyInternalLink}
                  hasMore={activeTimelineHasMore}
                  loadingMore={activeTimelineLoadingMore}
                  unavailableCount={activeTimelineUnavailable}
                  onLoadMore={() => void loadMoreTimeline(surfaceTopic, surfaceChannelId)}
                  pendingCount={activeTimelinePendingCount}
                  onApplyPending={() =>
                    void refreshTimelineFeed(surfaceTopic, selectedThread, surfaceChannelId)
                  }
                  onSubmitReport={submitReport}
                  onCopyReportContact={copyReportContact}
                  onFetchReportManifest={fetchReportManifest}
                  onFetchNodePolicies={fetchNodePolicies}
                  onMuteReportAuthor={muteReportAuthor}
                />
              ) : (
                <BookmarksListFrame
                  status={bookmarksPanelState.status}
                  error={bookmarksPanelState.error}
                  onRetry={() => retryBookmarks?.()}
                >
                  {(bookmarksDisplayedStatus) => (
                    <TimelineFeed
                      posts={viewModels.bookmarkedTimelinePostViews}
                      emptyCopy={t('shell:workspace.noBookmarks')}
                      emptyState={
                        bookmarksDisplayedStatus === 'ready' ? (
                          <BookmarksEmptyState
                            onShowTimeline={
                              selectTimelineView ? () => selectTimelineView(column, 'feed') : undefined
                            }
                          />
                        ) : null
                      }
                      onOpenAuthor={(authorPubkey) => void openAuthorDetail(authorPubkey)}
                      onOpenThread={openThreadInSurfaceScope}
                      onOpenThreadInTopic={openThreadInTopicFromSurface}
                      onReply={beginColumnReply}
                      onRepost={(post) => void handleSimpleRepost(post)}
                      onQuoteRepost={beginColumnQuoteRepost}
                      onRetryLocalPost={handleRetryLocalPost}
                      onRestoreLocalPost={handleRestoreLocalPost}
                      localAuthorPubkey={syncStatus.local_author_pubkey}
                      mediaObjectUrls={mediaObjectUrls}
                      ownedReactionAssets={ownedReactionAssets}
                      bookmarkedReactionAssets={bookmarkedReactionAssets}
                      recentReactions={recentReactions}
                      onToggleReaction={(post, reactionKey) => void handleToggleReaction(post, reactionKey)}
                      onBookmarkCustomReaction={(asset) => void handleBookmarkCustomReaction(asset)}
                      onReactionPickerOpen={() => void loadReactionCatalogData()}
                      showBookmarkAction={true}
                      bookmarkedPostIds={bookmarkedPostIds}
                      onToggleBookmark={(post) => void handleToggleBookmarkedPost(post)}
                      onWithdraw={(post) => void handleWithdrawPost(post)}
                      onActivateReference={(reference) => void handleActivateReference(reference)}
                      onCopyPostLink={handleCopyInternalLink}
                      onSubmitReport={submitReport}
                      onCopyReportContact={copyReportContact}
                      onFetchReportManifest={fetchReportManifest}
                      onFetchNodePolicies={fetchNodePolicies}
                      onMuteReportAuthor={muteReportAuthor}
                    />
                  )}
                </BookmarksListFrame>
              )}
            </div>
          </>
        ) : null}

        {activeSurfaceSection === 'explore' ? (
          <CommunityIndexWorkspace
            api={api}
            mode='explore'
            activeTopic={surfaceTopic}
            activeTimelineScope={surfaceTimelineScope}
            activeChannelLabel={surfaceChannelId
              ? surfaceJoinedChannels.find((channel) => channel.channel_id === surfaceChannelId)?.label ?? null
              : null}
            eligibleNodeBaseUrls={eligibleIndexNodeBaseUrls}
            consentPendingNodeBaseUrls={consentPendingNodeBaseUrls}
            selectedNodeBaseUrl={communityIndexNodeBaseUrl}
            configuredNodeBaseUrls={communityNodeConfig.nodes.map((node) => node.base_url)}
            nodeStatuses={communityNodeStatuses}
            availability={indexAvailability}
            onAcceptConsents={onAcceptCommunityNodeConsents}
            onRetryNode={(recovery) => onRetryCommunityNode({ ...indexAvailability, recovery: recovery ?? indexAvailability.recovery })}
            onAutomaticNode={() => patchState({ communityIndexNodePreference: { mode: 'auto' } })}
            onOpenCommunityNodeSettings={openCommunityNodeSettings}
            onOpenConnectivitySettings={openConnectivitySettings}
            onOpenTimeline={openTimelineSection}
            onRequestIndexing={requestIndexing}
            knownAuthorsByPubkey={knownAuthorsByPubkey}
            mediaObjectUrls={mediaObjectUrls}
            adultContentEnabled={adultContentEnabled}
            gatedMediaHashes={viewModels.gatedMediaHashes}
            unsupportedVideoManifests={unsupportedVideoManifests}
            locale={locale}
            onResolvedPostsChange={(posts) =>
              patchState({ communityIndexResolvedPosts: posts })
            }
            onAdvisoryGatedMediaHashesChange={(hashes) =>
              patchState({ advisoryGatedMediaHashes: hashes })
            }
            onOpenAuthor={(pubkey) => void openAuthorDetail(pubkey)}
            onOpenThread={openThreadInSurfaceScope}
            onOpenThreadInTopic={openThreadInTopicFromSurface}
            onReply={beginColumnReply}
            onRepost={(post) => void handleSimpleRepost(post)}
            onQuoteRepost={beginColumnQuoteRepost}
            localAuthorPubkey={syncStatus.local_author_pubkey}
            localProfile={localProfile}
            ownedReactionAssets={ownedReactionAssets}
            bookmarkedReactionAssets={bookmarkedReactionAssets}
            recentReactions={recentReactions}
            onToggleReaction={handleToggleReaction}
            onBookmarkCustomReaction={(asset) => void handleBookmarkCustomReaction(asset)}
            onReactionPickerOpen={() => void loadReactionCatalogData()}
            showBookmarkAction={true}
            bookmarkedPostIds={bookmarkedPostIds}
            onToggleBookmark={handleToggleBookmarkedPost}
            onWithdraw={handleWithdrawPost}
            onActivateReference={(reference) => void handleActivateReference(reference)}
            onCopyPostLink={handleCopyInternalLink}
          />
        ) : null}

        {activeSurfaceSection === 'live' ? (
          <div className='shell-stream-layout'>
            <section className='shell-column-content'>
              <div className='panel-header'>
                <div>
                  <h3>{t('live:title')}</h3>
                  <small>{t('live:summary', { count: surfaceLiveSessionListItems.length })}</small>
                </div>
              </div>
              {surfaceLivePanelState.status === 'loading' ? (
                <Notice>{t('live:loading')}</Notice>
              ) : null}
              {surfaceLivePanelState.status === 'error' &&
              (liveError ?? surfaceLivePanelState.error) ? (
                <Notice tone='destructive'>{liveError ?? surfaceLivePanelState.error}</Notice>
              ) : null}
            </section>
            <div className='shell-column-content'>
              {surfaceLiveSessionListItems.length === 0 &&
              surfaceLivePanelState.status === 'ready' ? (
                <p className='empty-state'>{t('live:empty')}</p>
              ) : null}
              <ul className='post-list'>
                {surfaceLiveSessionListItems.map(({ session, isOwner, pending }) => {
                  // #1061: 採用 CN の評価で主催者が非表示推奨なら、配信を折りたたむ。
                  const trustGate = liveTrustGate.gateFor(session.host_pubkey);
                  if (trustGate) {
                    return (
                      <li key={session.session_id}>
                        <AuthorTrustGateNotice
                          gate={trustGate}
                          onReveal={() => liveTrustGate.reveal(trustGate.authorPubkey)}
                          onOpenAuthor={(pubkey) => void openAuthorDetail(pubkey)}
                        />
                      </li>
                    );
                  }
                  return (
                  <li key={session.session_id}>
                    <article
                      className={`post-card${
                        selectedLiveSessionId === session.session_id ? ' post-card-targeted' : ''
                      }`}
                      aria-busy={pending}
                      data-live-session-id={session.session_id}
                      tabIndex={selectedLiveSessionId === session.session_id ? -1 : undefined}
                    >
                      <div className='post-meta'>
                        <span>{session.title}</span>
                        <span>{translateLiveStatus(session.status)}</span>
                        <span className='reply-chip'>
                          {localizeAudienceLabel(session.audience_label)}
                        </span>
                      </div>
                      <div className='post-body'>
                        <strong className='post-title post-copy-wrap'>
                          <SmartReferenceText
                            text={session.description || t('common:fallbacks.noDescription')}
                            className='post-copy-wrap'
                            onActivateReference={(reference) => void handleActivateReference(reference)}
                          />
                        </strong>
                      </div>
                      <div className='topic-diagnostic topic-diagnostic-secondary'>
                        <span>{t('common:labels.viewers')}: {formatCount(session.viewer_count)}</span>
                        <span>
                          {t('common:labels.started')}: {formatLocalizedTime(session.started_at, locale)}
                        </span>
                      </div>
                      {session.ended_at ? (
                        <div className='topic-diagnostic topic-diagnostic-secondary'>
                          <span>
                            {t('common:labels.ended')}: {formatLocalizedTime(session.ended_at, locale)}
                          </span>
                        </div>
                      ) : null}
                      <div className='post-actions'>
                        {session.joined_by_me ? (
                          <Button
                            variant='secondary'
                            type='button'
                            disabled={pending}
                            onClick={() => void handleLeaveLiveSession(session.session_id)}
                          >
                            {t('common:actions.leave')}
                          </Button>
                        ) : (
                          <Button
                            variant='secondary'
                            type='button'
                            disabled={pending || session.status === 'Ended'}
                            onClick={() => void handleJoinLiveSession(session.session_id)}
                          >
                            {t('common:actions.join')}
                          </Button>
                        )}
                        {isOwner ? (
                          <Button
                            variant='secondary'
                            type='button'
                            disabled={pending || session.status === 'Ended'}
                            onClick={() => void handleEndLiveSession(session.session_id)}
                          >
                            {t('common:actions.end')}
                          </Button>
                        ) : null}
                        <IconButton
                          variant='secondary'
                          className='post-action-button'
                          type='button'
                          label={t('common:actions.copyLink')}
                          onClick={() =>
                            handleCopyInternalLink(
                              buildLiveLink(surfaceTopic, session.session_id, session.channel_id ?? null)
                            )
                          }
                        >
                          <Link2 className='size-4' aria-hidden='true' />
                        </IconButton>
                      </div>
                    </article>
                  </li>
                  );
                })}
              </ul>
            </div>
          </div>
        ) : null}

        {activeSurfaceSection === 'game' && column.kind === 'game' ? (
          <GameRoomPanel
            status={surfaceGamePanelState.status}
            error={gameError ?? surfaceGamePanelState.error}
            audienceLabel={surfaceAudienceLabel}
            title={gameTitle}
            description={gameDescription}
            participantsInput={gameParticipantsInput}
            createPending={gameCreatePending}
            rooms={scoreGameRooms}
            trustGates={authorTrustGates}
            onOpenAuthor={(pubkey) => void openAuthorDetail(pubkey)}
            drafts={surfaceGameDraftViews}
            savingByRoomId={gameSavingByRoomId}
            localAuthorPubkey={syncStatus.local_author_pubkey}
            onTitleChange={setGameTitle}
            onDescriptionChange={setGameDescription}
            onParticipantsChange={setGameParticipantsInput}
            onSubmit={handleCreateGameRoom}
            onDraftStatusChange={(roomId, status) =>
              updateGameDraft(roomId, (draft) => ({ ...draft, status }))
            }
            onDraftPhaseChange={(roomId, phaseLabel) =>
              updateGameDraft(roomId, (draft) => ({ ...draft, phase_label: phaseLabel }))
            }
            onDraftScoreChange={(roomId, participantId, score) =>
              updateGameDraft(roomId, (draft) => ({
                ...draft,
                scores: { ...draft.scores, [participantId]: score },
              }))
            }
            onSaveRoom={(roomId) => void handleUpdateGameRoom(roomId)}
          />
        ) : activeSurfaceSection === 'game' ? (
          <MetaverseRoomPanel
            loadError={surfaceGamePanelState.error}
            catalogReady={surfaceGamePanelState.status === 'ready'}
            actions={metaverseActions}
            activeTopic={surfaceTopic}
            rooms={metaverseRooms}
            syncStatus={syncStatus}
            locale={locale}
            localProfile={localProfile}
            knownAuthorsByPubkey={knownAuthorsByPubkey}
            mediaObjectUrls={mediaObjectUrls}
            initialSelectedRoomId={column.entityId}
            activeChannel={surfaceChannelId
              ? surfaceJoinedChannels.find((channel) => channel.channel_id === surfaceChannelId) ?? null
              : null}
            communityNodePanelView={communityNodePanelView}
            onFetchCommunityNodeConsents={onFetchCommunityNodeConsents}
            onAcceptCommunityNodeConsents={onAcceptCommunityNodeConsents}
            onOpenCommunityNodeSettings={openCommunityNodeSettings}
          />
        ) : null}

        {activeSurfaceSection === 'notifications' ? notificationsWorkspace : null}

        {activeSurfaceSection === 'messages' ? messagesWorkspace : null}

        {activeSurfaceSection === 'profile' ? (
          <>
            {profileMode === 'edit' ? (
              <ProfileEditorPanel
                authorLabel={profileAuthorLabel}
                status={profilePanelState.status}
                saving={profileSaving}
                dirty={profileDirty}
                error={profileError ?? profilePanelState.error}
                fields={viewModels.profileEditorFields}
                picturePreviewSrc={viewModels.profileEditorPictureSrc}
                hasPicture={viewModels.profileEditorHasPicture}
                pictureInputKey={profileAvatarInputKey}
                onFieldChange={handleProfileFieldChange}
                onPictureSelect={(event) => {
                  const file = event.target.files?.[0] ?? null;
                  if (file) {
                    onProfilePictureSelect(file);
                  }
                }}
                onPictureClear={handleClearProfileAvatar}
                onBack={openProfileOverview}
                onSave={handleSaveProfile}
                onReset={resetProfileDraft}
              />
            ) : profileMode === 'connections' ? (
              <ProfileConnectionsPanel
                activeView={profileConnectionsView}
                items={viewModels.activeSocialConnectionViews}
                localAuthorPubkey={syncStatus.local_author_pubkey}
                status={socialConnectionsPanelState.status}
                error={socialConnectionsPanelState.error}
                onSelectView={openProfileConnections}
                onToggleRelationship={(authorPubkey, following) =>
                  void handleRelationshipAction(authorPubkey, following)
                }
                onToggleMute={(authorPubkey, muted) => void handleMuteAction(authorPubkey, muted)}
                onToggleBlock={(authorPubkey, blocking) =>
                  void handleBlockAction(authorPubkey, blocking)
                }
                onBack={openProfileOverview}
                onOpenTimeline={openTimelineSection}
                onOpenExplore={openExploreSection}
              />
            ) : (
              <ProfileOverviewPanel
                authorLabel={profileAuthorLabel}
                username={localProfile?.name ?? null}
                about={localProfile?.about ?? null}
                picture={resolveProfilePictureSrc(localProfile, mediaObjectUrls)}
                status={profilePanelState.status}
                error={profileError ?? profilePanelState.error}
                postCount={profileHasLoaded ? viewModels.profileTimelinePostViews.length : null}
                followingCount={socialConnections.following.length}
                followedCount={socialConnections.followed.length}
                mutedCount={socialConnections.muted.length}
                blockingCount={socialConnections.blocking.length}
                onEdit={openProfileEditor}
                onOpenFollowing={() => openProfileConnections('following')}
                onOpenFollowed={() => openProfileConnections('followed')}
                onOpenMuted={() => openProfileConnections('muted')}
                onOpenBlocking={() => openProfileConnections('blocking')}
              />
            )}
            {profileMode !== 'connections' && profilePanelState.status === 'error' ? (
              <Button variant='secondary' onClick={() => void refreshProfile()}>
                {t('common:actions.retry')}
              </Button>
            ) : null}
            {profileMode !== 'connections' &&
            profileHasLoaded ? (
              <div className='shell-column-content'>
                <TimelineFeed
                  posts={viewModels.profileTimelinePostViews}
                  emptyCopy={t('profile:feed.noOwnPosts')}
                  onOpenAuthor={(authorPubkey) => void openAuthorDetail(authorPubkey)}
                  onOpenThread={openThreadInSurfaceScope}
                  onOpenThreadInTopic={openThreadInTopicFromSurface}
                  onReply={beginColumnReply}
                  readOnly={true}
                  onOpenOriginalTopic={(topicId) => void handleOpenOriginalTopic(topicId)}
                  onActivateReference={(reference) => void handleActivateReference(reference)}
                  onCopyPostLink={handleCopyInternalLink}
                />
              </div>
            ) : null}
          </>
        ) : null}
      </section>
    </div>
  );
}
