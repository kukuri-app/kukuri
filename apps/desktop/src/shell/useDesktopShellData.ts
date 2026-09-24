import { useSessionProjectionRefresh } from '@/shell/data/useSessionProjectionRefresh';
import {
  startTransition,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  type MutableRefObject,
} from 'react';

import type {
  DesktopApi,
  DirectMessageMessageView,
  GameRoomView,
  JoinedPrivateChannelView,
  PostView,
} from '@/lib/api';

import { removeRecordEntry, setRecordEntry, setTimelineCursorEntry, updateRecordEntry } from '@/shell/stateUpdates';
import { useConnectivityStatusRefresh } from '@/shell/data/useConnectivityStatusRefresh';
import { useAdultGatedMediaHashes } from '@/shell/data/useAdultGatedMediaHashes';
import { useCommunityNodeRecovery } from '@/shell/actions/useCommunityNodeRecovery';
import { useDesktopShellDataEffects } from '@/shell/data/useDesktopShellDataEffects';
import {
  activeWorkspaceScope,
  columnIdentityId,
  openTransientColumn,
} from '@/shell/slices/workspace';
import { useDraftMediaHelpers } from '@/shell/data/useDraftMediaHelpers';
import { useNotificationLoaders } from '@/shell/data/loaders/useNotificationLoaders';
import { useDesktopShellSectionLoaders } from '@/shell/data/loaders/useDesktopShellSectionLoaders';
import { useQueuedLoadTopics } from '@/shell/data/useQueuedLoadTopics';
import {
  hasReadPastHeadPage,
  mergeRefreshedVisiblePosts,
  mergeUniquePosts,
  postIdentityKey,
  uniquePostsByIdentity,
  windowHeadCursor,
} from '@/shell/data/timelineMerge';
import { usePreviewableMediaAttachments } from '@/shell/data/usePreviewableMediaAttachments';
import {
  adoptingContentAdvisoryNodes,
  useTimelineContentAdvisoryLookup,
} from '@/shell/data/useTimelineContentAdvisoryLookup';
import { useAuthorTrustGateLookup } from '@/shell/data/useAuthorTrustGateLookup';
import {
  activeTimelineStorageKey,
  PUBLIC_TIMELINE_SCOPE,
  timelineScopeStorageKey,
  timelineStorageKeyForChannel,
  useDesktopShellFieldSetter,
  useDesktopShellStore,
  useDesktopShellStoreApi,
} from '@/shell/store';
import { THREAD_TIMELINE_LIMIT, VISIBLE_TIMELINE_LIMIT } from '@/shell/pagination';
import { useShallow } from 'zustand/react/shallow';
import {
  mergeKnownAuthors,
  messageFromError,
  privateTimelineScope,
} from '@/shell/presentation';
import { selectShellDataSlice } from '@/shell/storeSelectors';

type UseDesktopShellDataArgs = {
  api: DesktopApi;
  translate: (key: string, options?: Record<string, unknown>) => string;
  loadTopicsRequestRef: MutableRefObject<Map<string, number>>;
  remoteObjectUrlRef: MutableRefObject<Map<string, string>>;
  draftPreviewUrlRef: MutableRefObject<Map<string, string>>;
  directMessageDraftPreviewUrlRef: MutableRefObject<Map<string, string>>;
  mediaFetchAttemptRef: MutableRefObject<Map<string, number>>;
  draftSequenceRef: MutableRefObject<number>;
  /// 表示中(viewport 内)の Column id 列。背景 Timeline Column の定期 refresh(Issue #765)に使う。
  visibleColumnIdsRef?: MutableRefObject<string[]>;
};

const EMPTY_POSTS: PostView[] = [];
const EMPTY_GAME_ROOMS: GameRoomView[] = [];
const EMPTY_JOINED_CHANNELS: JoinedPrivateChannelView[] = [];
const EMPTY_DIRECT_MESSAGE_TIMELINE: DirectMessageMessageView[] = [];

export function useDesktopShellData({
  api,
  translate,
  loadTopicsRequestRef,
  remoteObjectUrlRef,
  draftPreviewUrlRef,
  directMessageDraftPreviewUrlRef,
  mediaFetchAttemptRef,
  draftSequenceRef,
  visibleColumnIdsRef,
}: UseDesktopShellDataArgs) {
  const storeApi = useDesktopShellStoreApi();
  const state = useDesktopShellStore(useShallow(selectShellDataSlice));
  const {
    trackedTopics,
    activeTopic,
    adultContentEnabled,
    communityIndexResolvedPosts,
    advisoryGatedMediaHashes,
    bookmarkedPosts,
    communityNodeConfig,
    communityNodeStatuses,
    timelineAdvisoryLookup,
    timelineContentAdvisories,
    selectedThread,
    gameRoomsByScopeKey,
    joinedChannelsByTopic,
    selectedChannelIdByTopic,
    mediaObjectUrls,
    localProfile,
    knownAuthorsByPubkey,
    profileTimeline,
    selectedAuthorTimeline,
    threadsById,
    ownedReactionAssets,
    bookmarkedReactionAssets,
    recentReactions,
    notifications,
    shellChromeState,
  } = state;
  const selectedDirectMessageTimeline =
    state.directMessageTimelineByPeer[state.selectedDirectMessagePeerPubkey ?? ''] ??
    EMPTY_DIRECT_MESSAGE_TIMELINE;
  const activeTimelineKey = activeTimelineStorageKey(state, activeTopic);
  const activePublicTimeline =
    state.timelinesByKey[timelineScopeStorageKey(activeTopic, PUBLIC_TIMELINE_SCOPE)] ?? EMPTY_POSTS;
  const activeTimeline = state.timelinesByKey[activeTimelineKey] ?? EMPTY_POSTS;
  const activeScope = activeWorkspaceScope(state.workspaceState);
  const activeGameRooms =
    gameRoomsByScopeKey[timelineScopeStorageKey(activeScope.topicId, privateTimelineScope(activeScope.channelId))] ??
    EMPTY_GAME_ROOMS;
  const activeJoinedChannels = joinedChannelsByTopic[activeTopic] ?? EMPTY_JOINED_CHANNELS;
  const selectedPrivateChannelId = selectedChannelIdByTopic[activeTopic] ?? null;
  const selectedAuthorPubkey = state.selectedAuthorPubkey;
  const thread = selectedThread ? threadsById[selectedThread] ?? EMPTY_POSTS : EMPTY_POSTS;
  const visibleRefreshInFlightRef = useRef(false);

  const setTimelinesByKey = useDesktopShellFieldSetter('timelinesByKey');
  const setTimelineNextCursorByKey = useDesktopShellFieldSetter('timelineNextCursorByKey');
  const setTimelineWindowHeadCursorByKey = useDesktopShellFieldSetter('timelineWindowHeadCursorByKey');
  const setTimelineLoadingMoreByKey = useDesktopShellFieldSetter('timelineLoadingMoreByKey');
  const setTimelineUnavailableByKey = useDesktopShellFieldSetter('timelineUnavailableByKey');
  const setPendingTimelineSnapshotsByKey = useDesktopShellFieldSetter(
    'pendingTimelineSnapshotsByKey'
  );
  const setPendingTimelineCountsByKey = useDesktopShellFieldSetter('pendingTimelineCountsByKey');
  const setPendingTimelineNextCursorByKey = useDesktopShellFieldSetter(
    'pendingTimelineNextCursorByKey'
  );
  const setPendingTimelineUnavailableByKey = useDesktopShellFieldSetter(
    'pendingTimelineUnavailableByKey'
  );
  const setJoinedChannelsByTopic = useDesktopShellFieldSetter('joinedChannelsByTopic');
  const setChannelPanelStateByTopic = useDesktopShellFieldSetter('channelPanelStateByTopic');
  const setWorkspaceState = useDesktopShellFieldSetter('workspaceState');
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
  const setTimelineScopeByTopic = useDesktopShellFieldSetter('timelineScopeByTopic');
  const setComposeChannelByTopic = useDesktopShellFieldSetter('composeChannelByTopic');
  const setThreadsById = useDesktopShellFieldSetter('threadsById');
  const setBookmarkMembershipById = useDesktopShellFieldSetter('bookmarkMembershipById');
  const setThreadNextCursorById = useDesktopShellFieldSetter('threadNextCursorById');
  const setThreadWindowHeadCursorById = useDesktopShellFieldSetter('threadWindowHeadCursorById');
  const setThreadLoadingMoreById = useDesktopShellFieldSetter('threadLoadingMoreById');
  const setThreadUnavailableById = useDesktopShellFieldSetter('threadUnavailableById');
  const setCommunityNodeStatuses = useDesktopShellFieldSetter('communityNodeStatuses');
  const setMediaObjectUrls = useDesktopShellFieldSetter('mediaObjectUrls');
  const setSyncStatus = useDesktopShellFieldSetter('syncStatus');
  const setLocalProfile = useDesktopShellFieldSetter('localProfile');
  const setKnownAuthorsByPubkey = useDesktopShellFieldSetter('knownAuthorsByPubkey');
  const setOwnedReactionAssets = useDesktopShellFieldSetter('ownedReactionAssets');
  const setBookmarkedReactionAssets = useDesktopShellFieldSetter('bookmarkedReactionAssets');
  const setRecentReactions = useDesktopShellFieldSetter('recentReactions');
  const setProfileDraft = useDesktopShellFieldSetter('profileDraft');
  const setGameDrafts = useDesktopShellFieldSetter('gameDrafts');
  const setReactionPanelState = useDesktopShellFieldSetter('reactionPanelState');
  const setError = useDesktopShellFieldSetter('error');

  useEffect(() => {
    if (shellChromeState.activePrimarySection !== 'game') {
      return;
    }
    const missingHostPubkeys = Array.from(
      new Set(
        activeGameRooms
          .map((room) => room.host_pubkey)
          .filter(
            (pubkey) =>
              pubkey &&
              pubkey !== localProfile?.pubkey &&
              pubkey !== state.syncStatus.local_author_pubkey &&
              !knownAuthorsByPubkey[pubkey]
          )
      )
    );
    if (missingHostPubkeys.length === 0) {
      return;
    }
    let disposed = false;
    void Promise.all(
      missingHostPubkeys.map((pubkey) => api.getAuthorSocialView(pubkey).catch(() => null))
    ).then((authors) => {
      if (disposed) {
        return;
      }
      const resolvedAuthors = authors.filter((author) => author !== null);
      if (resolvedAuthors.length > 0) {
        setKnownAuthorsByPubkey((current) => mergeKnownAuthors(current, resolvedAuthors));
      }
    });
    return () => {
      disposed = true;
    };
  }, [
    activeGameRooms,
    api,
    knownAuthorsByPubkey,
    localProfile?.pubkey,
    setKnownAuthorsByPubkey,
    shellChromeState.activePrimarySection,
    state.syncStatus.local_author_pubkey,
  ]);

  // #1056: 開いている Timeline Column と各表示経路の投稿を、採用 node へ一括照会する対象にする。
  // 「見つける」の解決済み投稿は index 応答の advisory(#1055)で扱うため対象外。
  // `buildPostCardView` を通る投稿源(Timeline / Thread / Profile Column、ブックマーク等)は
  // すべて照会対象に含める。含めない投稿は照会済みにならず、スケルトンのまま残るため。
  const workspaceColumns = state.workspaceState.columns;
  const timelinesByKey = state.timelinesByKey;
  const authorTimelinesByPubkey = state.authorTimelinesByPubkey;
  const advisoryLookupPosts = useMemo(() => {
    const columnPosts = workspaceColumns.flatMap((column) => {
      if (column.kind === 'timeline' && column.scope) {
        return (
          timelinesByKey[
            timelineStorageKeyForChannel(column.scope.topicId, column.scope.channelId)
          ] ?? EMPTY_POSTS
        );
      }
      if (column.kind === 'thread' && column.entityId) {
        return threadsById[column.entityId] ?? EMPTY_POSTS;
      }
      if (column.kind === 'profile' && column.entityId) {
        return authorTimelinesByPubkey[column.entityId] ?? EMPTY_POSTS;
      }
      return EMPTY_POSTS;
    });
    return [
      ...activeTimeline,
      ...activePublicTimeline,
      ...profileTimeline,
      ...selectedAuthorTimeline,
      ...thread,
      ...bookmarkedPosts.map((item) => item.post),
      ...columnPosts,
    ];
  }, [
    activePublicTimeline,
    activeTimeline,
    authorTimelinesByPubkey,
    bookmarkedPosts,
    profileTimeline,
    selectedAuthorTimeline,
    thread,
    threadsById,
    timelinesByKey,
    workspaceColumns,
  ]);
  const visibleBookmarkIds = useMemo(() => [
    ...new Set([...advisoryLookupPosts, ...communityIndexResolvedPosts].map((post) => post.object_id)),
  ].slice(0, 1600), [advisoryLookupPosts, communityIndexResolvedPosts]);
  const bookmarkAccountRef = useRef(state.syncStatus.local_author_pubkey);
  useEffect(() => {
    let cancelled = false;
    if (bookmarkAccountRef.current !== state.syncStatus.local_author_pubkey) {
      bookmarkAccountRef.current = state.syncStatus.local_author_pubkey;
      setBookmarkMembershipById({});
    }
    const pages = Array.from(
      { length: Math.ceil(visibleBookmarkIds.length / 200) },
      (_, index) => visibleBookmarkIds.slice(index * 200, (index + 1) * 200)
    );
    void Promise.all(pages.map((ids) => api.bookmarkedPostIds(ids))).then((results) => {
      if (cancelled) return;
      const found = new Set(results.flat());
      setBookmarkMembershipById(Object.fromEntries(
        visibleBookmarkIds.map((id) => [id, found.has(id)])
      ));
    }).catch(() => undefined);
    return () => { cancelled = true; };
  }, [api, visibleBookmarkIds, setBookmarkMembershipById, state.syncStatus.local_author_pubkey]);
  // 設定・状態の取得に失敗した場合も、手元の状態で確定させる(照会中のまま止めない)。
  const communityNodeConfigLoaded =
    state.communityNodeConfigLoaded || Boolean(state.communityNodeConfigError);
  const communityNodeStatusesLoaded =
    state.communityNodeStatusesLoaded || Boolean(state.communityNodeStatusError);
  const adoptingNodes = useMemo(
    () =>
      adoptingContentAdvisoryNodes(communityNodeConfig, communityNodeStatuses, {
        configLoaded: communityNodeConfigLoaded,
        statusesLoaded: communityNodeStatusesLoaded,
      }),
    [
      communityNodeConfig,
      communityNodeConfigLoaded,
      communityNodeStatuses,
      communityNodeStatusesLoaded,
    ]
  );
  useTimelineContentAdvisoryLookup({
    api,
    posts: advisoryLookupPosts,
    notifications,
    adoptingNodes,
  });
  // #1061: live / game 一覧の主催者も折りたたみ判断の対象にする。
  const trustGateHostPubkeys = useMemo(() => {
    const hosts = new Set<string>();
    for (const sessions of Object.values(state.liveSessionsByScopeKey)) {
      for (const session of sessions) {
        const pubkey = session.host_pubkey?.trim();
        if (pubkey) hosts.add(pubkey);
      }
    }
    for (const rooms of Object.values(gameRoomsByScopeKey)) {
      for (const room of rooms) {
        const pubkey = room.host_pubkey?.trim();
        if (pubkey) hosts.add(pubkey);
      }
    }
    return [...hosts];
  }, [gameRoomsByScopeKey, state.liveSessionsByScopeKey]);
  // #1061: 表示中の著者を採用 CN へ一括照会し、折りたたみ判断を state へ置く。
  useAuthorTrustGateLookup({
    api,
    posts: advisoryLookupPosts,
    hostPubkeys: trustGateHostPubkeys,
    config: communityNodeConfig,
    statuses: communityNodeStatuses,
    statusesLoaded: communityNodeStatusesLoaded,
  });

  // #858 / #1107: 表示設定 OFF の間にゲート対象となる添付 hash(引用 snapshot 含む)。
  // 取得対象からの除外、表示済み object URL の破棄、カードの代替表示が同じ集合を使う。
  const adultGatedPosts = useMemo(
    () => [...advisoryLookupPosts, ...communityIndexResolvedPosts],
    [advisoryLookupPosts, communityIndexResolvedPosts]
  );
  const gatedAdultMediaHashes = useAdultGatedMediaHashes({
    adultContentEnabled,
    posts: adultGatedPosts,
    timelineContentAdvisories,
    additionalHashes: advisoryGatedMediaHashes,
  });

  const previewableMediaAttachments = usePreviewableMediaAttachments({
    activeTimeline,
    activePublicTimeline,
    // #1107: ブックマークや非 active な Column も描画されるため、取得対象に含める。
    additionalTimelinePosts: advisoryLookupPosts,
    communityIndexResolvedPosts,
    gatedMediaHashes: gatedAdultMediaHashes,
    timelineContentAdvisories,
    timelineAdvisoryLookup,
    profileTimeline,
    selectedAuthorTimeline,
    thread,
    selectedDirectMessageTimeline,
    ownedReactionAssets,
    bookmarkedReactionAssets,
    recentReactions,
    localProfile,
    knownAuthorsByPubkey,
    notifications,
    adultContentEnabled,
  });

  const clearPendingTimeline = useCallback(
    (key: string) => {
      setPendingTimelineSnapshotsByKey((current) => {
        if (!current[key]) {
          return current;
        }
        const next = { ...current };
        delete next[key];
        return next;
      });
      setPendingTimelineCountsByKey((current) => {
        if (!current[key]) {
          return current;
        }
        const next = { ...current };
        delete next[key];
        return next;
      });
      setPendingTimelineNextCursorByKey(removeRecordEntry(key));
      setPendingTimelineUnavailableByKey(removeRecordEntry(key));
    },
    [
      setPendingTimelineCountsByKey,
      setPendingTimelineNextCursorByKey,
      setPendingTimelineSnapshotsByKey,
      setPendingTimelineUnavailableByKey,
    ]
  );

  const applyPendingTimeline = useCallback(
    (
      topic: string,
      scope = storeApi.getState().timelineScopeByTopic[topic] ??
        privateTimelineScope(
          activeWorkspaceScope(storeApi.getState().workspaceState).topicId === topic
            ? activeWorkspaceScope(storeApi.getState().workspaceState).channelId
            : null
        )
    ) => {
      const key = timelineScopeStorageKey(topic, scope);
      const currentState = storeApi.getState();
      const pendingItems = currentState.pendingTimelineSnapshotsByKey[key];
      if (!pendingItems || pendingItems.length === 0) {
        return false;
      }
      const currentTimelinePosts = currentState.timelinesByKey[key] ?? EMPTY_POSTS;
      const pendingCursor = currentState.pendingTimelineNextCursorByKey[key] ?? null;
      const pendingUnavailable = currentState.pendingTimelineUnavailableByKey[key];
      // refresh と同じ判定で、表示中の古い行を残すかを決める(#1239、#1274)。refresh が読み進めた位置を残したとき、
      // 保留の続きの位置は保留中の先頭のページより先にある。判定が refresh と食い違うと、表示と続きの位置が
      // 食い違う(行が消える、または順序が崩れる)。
      const lastPending = pendingItems.filter((post) => !post.local_state).at(-1);
      const preserveOlderPages = hasReadPastHeadPage(
        currentTimelinePosts,
        pendingItems,
        pendingCursor,
        lastPending ? { created_at: lastPending.created_at, object_id: lastPending.object_id } : null,
        'desc'
      );
      startTransition(() => {
        setTimelinesByKey(updateRecordEntry(key, (prev) => mergeRefreshedVisiblePosts(
            prev ?? EMPTY_POSTS,
            pendingItems,
            preserveOlderPages
          )));
        setTimelineNextCursorByKey(setTimelineCursorEntry(key, pendingCursor));
        setTimelineWindowHeadCursorByKey(setRecordEntry(key, null));
        // 読んだ範囲を捨てるときは、保留した先頭のページの数に置き換える(#1239 AC-4、独立監査 B2)。
        if (!preserveOlderPages) {
          setTimelineUnavailableByKey(setRecordEntry(key, pendingUnavailable ?? 0));
        }
      });
      clearPendingTimeline(key);
      return true;
    },
    [
      clearPendingTimeline,
      setTimelineNextCursorByKey,
      setTimelineWindowHeadCursorByKey,
      setTimelineUnavailableByKey,
      setTimelinesByKey,
      storeApi,
    ]
  );

  const refreshVisibleShellData = useCallback(
    async (
      topic: string,
      currentThread: string | null,
      mode: 'apply' | 'buffer' = 'buffer',
      scopeChannelId?: string | null
    ) => {
      const requestState = storeApi.getState();
      const selectedChannelId =
        scopeChannelId === undefined
          ? activeWorkspaceScope(requestState.workspaceState).topicId === topic
            ? activeWorkspaceScope(requestState.workspaceState).channelId
            : null
          : scopeChannelId;
      const timelineScope = privateTimelineScope(selectedChannelId);
      const timelineKey = timelineScopeStorageKey(topic, timelineScope);
      const timelineHeadCursor = !(requestState.timelinesByKey[timelineKey]?.length)
        ? requestState.timelineWindowHeadCursorByKey[timelineKey] ?? null : null;
      const publicTimelineKey = timelineScopeStorageKey(topic, PUBLIC_TIMELINE_SCOPE);
      const publicHeadCursor = !(requestState.timelinesByKey[publicTimelineKey]?.length)
        ? requestState.timelineWindowHeadCursorByKey[publicTimelineKey] ?? null : null;
      const threadHeadCursor = currentThread && !(requestState.threadsById[currentThread]?.length)
        ? requestState.threadWindowHeadCursorById[currentThread] ?? null : null;
      const requestId = (loadTopicsRequestRef.current.get(timelineKey) ?? 0) + 1;
      loadTopicsRequestRef.current.set(timelineKey, requestId);

      const [
        timelineResult,
        publicTimelineResult,
        joinedChannelsResult,
        threadViewResult,
      ] = await Promise.allSettled([
        api.listTimeline(topic, timelineHeadCursor, VISIBLE_TIMELINE_LIMIT, timelineScope),
        selectedChannelId === null
          ? Promise.resolve(null)
          : api.listTimeline(topic, publicHeadCursor, VISIBLE_TIMELINE_LIMIT, PUBLIC_TIMELINE_SCOPE),
        api.listJoinedPrivateChannels(topic),
        currentThread
          ? api.listThread(topic, currentThread, threadHeadCursor, THREAD_TIMELINE_LIMIT)
          : Promise.resolve(null),
      ]);

      if (requestId !== loadTopicsRequestRef.current.get(timelineKey)) {
        return;
      }

      const firstCoreFailure = [
        timelineResult,
        publicTimelineResult,
        joinedChannelsResult,
        threadViewResult,
      ].find((result) => result.status === 'rejected');

      startTransition(() => {
        const currentState = storeApi.getState();

        if (timelineResult.status === 'fulfilled') {
          const timeline = timelineResult.value;
          const normalizedTimelineItems = uniquePostsByIdentity(timeline.items);
          const baselinePosts = currentState.timelinesByKey[timelineKey] ?? EMPTY_POSTS;
          const preserveTimelinePages =
            mode === 'buffer' &&
            hasReadPastHeadPage(
              baselinePosts,
              normalizedTimelineItems,
              currentState.timelineNextCursorByKey[timelineKey],
              timeline.next_cursor,
              'desc'
            );
          const resolvedTimelineCursor = preserveTimelinePages
            ? (currentState.timelineNextCursorByKey[timelineKey] ?? null)
            : (timeline.next_cursor ?? null);
          const visiblePostIds = new Set(baselinePosts.map((post) => postIdentityKey(post)));
          const authoritativeIds = new Set(
            baselinePosts
              .filter((post) => !post.local_state)
              .map((post) => postIdentityKey(post))
          );
          const hasAuthoritativeBaseline = authoritativeIds.size > 0;
          const pendingTimelineItems = normalizedTimelineItems.filter(
            (post) => !visiblePostIds.has(postIdentityKey(post))
          );
          const pendingCount = pendingTimelineItems.length;
          const shouldBuffer = mode === 'buffer' && hasAuthoritativeBaseline && pendingCount > 0;

          if (shouldBuffer) {
            setPendingTimelineSnapshotsByKey(setRecordEntry(timelineKey, normalizedTimelineItems));
            setPendingTimelineCountsByKey(setRecordEntry(timelineKey, pendingCount));
            setPendingTimelineNextCursorByKey(setRecordEntry(timelineKey, resolvedTimelineCursor));
            setPendingTimelineUnavailableByKey(
              setRecordEntry(timelineKey, timeline.unavailable_count ?? 0)
            );
          } else {
            setTimelinesByKey(updateRecordEntry(timelineKey, (prev) => mergeRefreshedVisiblePosts(
                prev ?? EMPTY_POSTS,
                normalizedTimelineItems,
                preserveTimelinePages
              )));
            setTimelineNextCursorByKey(setTimelineCursorEntry(timelineKey, resolvedTimelineCursor));
            if (!preserveTimelinePages) {
              setTimelineWindowHeadCursorByKey(setRecordEntry(timelineKey, timelineHeadCursor));
            }
            // 読んだ範囲を残すときは、その範囲の数も残す(#1239 AC-4)。
            if (!preserveTimelinePages) {
              setTimelineUnavailableByKey(
                setRecordEntry(timelineKey, timeline.unavailable_count ?? 0)
              );
            }
            clearPendingTimeline(timelineKey);
          }
        }

        if (publicTimelineResult.status === 'fulfilled' && publicTimelineResult.value) {
          const publicTimeline = publicTimelineResult.value;
          const baselinePublicTimeline =
            currentState.timelinesByKey[publicTimelineKey] ?? EMPTY_POSTS;
          // 公開の scope の列も、選択中の scope と同じ判定で古い行と続きの位置を残す(#1239、#1274)。
          const preservePublicTimelinePages =
            mode === 'buffer' &&
            hasReadPastHeadPage(
              baselinePublicTimeline,
              publicTimeline.items,
              currentState.timelineNextCursorByKey[publicTimelineKey],
              publicTimeline.next_cursor,
              'desc'
            );
          const resolvedPublicTimelineCursor = preservePublicTimelinePages
            ? (currentState.timelineNextCursorByKey[publicTimelineKey] ?? null)
            : (publicTimeline.next_cursor ?? null);
          setTimelinesByKey(updateRecordEntry(publicTimelineKey, (prev) => mergeRefreshedVisiblePosts(
              prev ?? EMPTY_POSTS,
              publicTimeline.items,
              preservePublicTimelinePages
            )));
          setTimelineNextCursorByKey(
            setTimelineCursorEntry(publicTimelineKey, resolvedPublicTimelineCursor)
          );
          if (!preservePublicTimelinePages) {
            setTimelineWindowHeadCursorByKey(setRecordEntry(publicTimelineKey, publicHeadCursor));
          }
          if (!preservePublicTimelinePages) {
            setTimelineUnavailableByKey(
              setRecordEntry(publicTimelineKey, publicTimeline.unavailable_count ?? 0)
            );
          }
        }

        if (joinedChannelsResult.status === 'fulfilled') {
          setJoinedChannelsByTopic(setRecordEntry(topic, joinedChannelsResult.value));
          setChannelPanelStateByTopic(setRecordEntry(topic, {
              status: 'ready',
              error: null,
            }));
        } else {
          setChannelPanelStateByTopic(setRecordEntry(topic, {
              status: 'error',
              error: messageFromError(
                joinedChannelsResult.reason,
                translate('common:errors.failedToLoadPrivateChannels')
              ),
            }));
        }

        if (currentThread && !(mode === 'buffer' &&
          currentState.threadWindowHeadCursorById[currentThread] &&
          (currentState.threadsById[currentThread]?.length ?? 0) > 0)) {
          if (threadViewResult.status === 'fulfilled') {
            const threadView = threadViewResult.value;
            const incomingThreadItems = threadView?.items ?? [];
            const currentThreadPosts = currentState.threadsById[currentThread] ?? EMPTY_POSTS;
            const preserveThreadPages =
              mode === 'buffer' &&
              hasReadPastHeadPage(
                currentThreadPosts,
                incomingThreadItems,
                currentState.threadNextCursorById[currentThread],
                threadView?.next_cursor,
                'asc'
              );
            const resolvedThreadCursor = preserveThreadPages
              ? (currentState.threadNextCursorById[currentThread] ?? null)
              : (threadView?.next_cursor ?? null);
            setThreadsById((current) => ({
              ...current,
              [currentThread]: mergeRefreshedVisiblePosts(
                current[currentThread] ?? [],
                incomingThreadItems,
                preserveThreadPages
              ),
            }));
            setThreadNextCursorById(setTimelineCursorEntry(currentThread, resolvedThreadCursor));
            if (!preserveThreadPages) {
              setThreadWindowHeadCursorById(setRecordEntry(currentThread, threadHeadCursor));
            }
            if (!preserveThreadPages) {
              setThreadUnavailableById(
                setRecordEntry(currentThread, threadView?.unavailable_count ?? 0)
              );
            }
          }
        }

        setError(
          firstCoreFailure && firstCoreFailure.status === 'rejected'
            ? messageFromError(firstCoreFailure.reason, translate('common:errors.failedToLoadTopic'))
            : null
        );
      });
    },
    [
      api,
      clearPendingTimeline,
      loadTopicsRequestRef,
      setError,
      setChannelPanelStateByTopic,
      setJoinedChannelsByTopic,
      setPendingTimelineCountsByKey,
      setPendingTimelineNextCursorByKey,
      setPendingTimelineSnapshotsByKey,
      setPendingTimelineUnavailableByKey,
      setThreadsById,
      setThreadNextCursorById,
      setThreadWindowHeadCursorById,
      setThreadUnavailableById,
      setTimelineNextCursorByKey,
      setTimelineWindowHeadCursorByKey,
      setTimelineUnavailableByKey,
      setTimelinesByKey,
      storeApi,
      translate,
    ]
  );

  const loadMoreTimeline = useCallback(
    async (topic: string, scopeChannelId?: string | null) => {
      const currentState = storeApi.getState();
      const selectedChannelId =
        scopeChannelId === undefined
          ? activeWorkspaceScope(currentState.workspaceState).topicId === topic
            ? activeWorkspaceScope(currentState.workspaceState).channelId
            : null
          : scopeChannelId;
      const timelineScope = privateTimelineScope(selectedChannelId);
      const timelineKey = timelineScopeStorageKey(topic, timelineScope);
      const cursor = currentState.timelineNextCursorByKey[timelineKey] ?? null;
      if (!cursor || currentState.timelineLoadingMoreByKey[timelineKey]) {
        return;
      }
      setTimelineLoadingMoreByKey(setRecordEntry(timelineKey, true));
      try {
        const timeline = await api.listTimeline(
          topic,
          cursor,
          VISIBLE_TIMELINE_LIMIT,
          timelineScope
        );
        startTransition(() => {
          const latest = storeApi.getState();
          const current = latest.timelinesByKey[timelineKey] ?? EMPTY_POSTS;
          const merged = mergeUniquePosts(current, timeline.items);
          setTimelinesByKey(setRecordEntry(timelineKey, merged));
          setTimelineWindowHeadCursorByKey(setRecordEntry(timelineKey,
            windowHeadCursor(current, merged, latest.timelineWindowHeadCursorByKey[timelineKey] ?? null)));
          setTimelineNextCursorByKey(setTimelineCursorEntry(timelineKey, timeline.next_cursor ?? null));
          setTimelineUnavailableByKey(setRecordEntry(timelineKey, timeline.unavailable_count ?? 0));
        });
      } finally {
        setTimelineLoadingMoreByKey(setRecordEntry(timelineKey, false));
      }
    },
    [
      api,
      setTimelineLoadingMoreByKey,
      setTimelineNextCursorByKey,
      setTimelineUnavailableByKey,
      setTimelinesByKey,
      setTimelineWindowHeadCursorByKey,
      storeApi,
    ]
  );

  const loadMoreThread = useCallback(
    async (topic: string, threadId: string) => {
      const currentState = storeApi.getState();
      const cursor = currentState.threadNextCursorById[threadId] ?? null;
      if (!cursor || currentState.threadLoadingMoreById[threadId]) {
        return;
      }
      setThreadLoadingMoreById(setRecordEntry(threadId, true));
      try {
        const threadView = await api.listThread(topic, threadId, cursor, THREAD_TIMELINE_LIMIT);
        startTransition(() => {
          const latest = storeApi.getState();
          const current = latest.threadsById[threadId] ?? EMPTY_POSTS;
          const merged = mergeUniquePosts(current, threadView.items);
          setThreadsById(setRecordEntry(threadId, merged));
          setThreadWindowHeadCursorById(setRecordEntry(threadId,
            windowHeadCursor(current, merged, latest.threadWindowHeadCursorById[threadId] ?? null)));
          setThreadNextCursorById(setTimelineCursorEntry(threadId, threadView.next_cursor ?? null));
          setThreadUnavailableById(setRecordEntry(threadId, threadView.unavailable_count ?? 0));
        });
      } finally {
        setThreadLoadingMoreById(setRecordEntry(threadId, false));
      }
    },
    [
      api,
      setThreadsById,
      setThreadLoadingMoreById,
      setThreadNextCursorById,
      setThreadWindowHeadCursorById,
      setThreadUnavailableById,
      storeApi,
    ]
  );

  const loadReactionCatalogData = useCallback(async () => {
    try {
      const [ownedAssets, bookmarkedAssets, recent] = await Promise.all([
        api.listMyCustomReactionAssets(),
        api.listBookmarkedCustomReactions(),
        api.listRecentReactions(8),
      ]);
      startTransition(() => {
        setOwnedReactionAssets(ownedAssets);
        setBookmarkedReactionAssets(bookmarkedAssets);
        setRecentReactions(recent);
        setReactionPanelState({ status: 'ready', error: null });
      });
    } catch (error) {
      setReactionPanelState({
        status: 'error',
        error: messageFromError(error, translate('common:errors.failedToLoadSettings')),
      });
    }
  }, [
    api,
    setBookmarkedReactionAssets,
    setOwnedReactionAssets,
    setReactionPanelState,
    setRecentReactions,
    translate,
  ]);

  const { refreshNotificationStatus, refreshNotificationsFromEvent, loadNotificationsSection,
    navigateNotificationPage } = useNotificationLoaders({
    api, activePrimarySection: shellChromeState.activePrimarySection, translate,
  });
  const {
    loadLiveSection,
    loadGameSection,
    loadShellSections,
    loadProfileSection,
    loadAuthorSection,
    loadMoreProfileTimeline,
    loadMoreAuthorTimeline,
    loadBookmarksSection,
    navigateBookmarkPage,
    loadMessagesSection,
    loadCommunityIndexCapability,
  } = useDesktopShellSectionLoaders({
    api,
    loadReactionCatalogData,
    loadNotificationsSection,
    storeApi,
    translate,
  });
  const visibleListLoads = useRef(new Set<string>());
  useEffect(() => {
    const visible = new Set(state.visibleListColumnIds);
    for (const id of visibleListLoads.current) {
      if (!visible.has(id)) visibleListLoads.current.delete(id);
    }
    for (const column of state.workspaceState.columns) {
      if (!visible.has(column.id) || visibleListLoads.current.has(column.id)) continue;
      visibleListLoads.current.add(column.id);
      if (column.id === state.workspaceState.activeColumnId) continue;
      if (column.kind === 'timeline' && column.timelineView === 'bookmarks') {
        if (!storeApi.getState().bookmarkedPosts.length) void loadBookmarksSection({ preserveCurrent: true });
      } else if (column.kind === 'timeline' && column.scope) {
        const key = timelineStorageKeyForChannel(column.scope.topicId, column.scope.channelId);
        if (!storeApi.getState().timelinesByKey[key]?.length) {
          void refreshVisibleShellData(column.scope.topicId, null, 'apply', column.scope.channelId);
        }
      } else if (column.kind === 'thread' && column.entityId && column.scope) {
        if (!storeApi.getState().threadsById[column.entityId]?.length) {
          void refreshVisibleShellData(column.scope.topicId, column.entityId, 'apply', column.scope.channelId);
        }
      } else if (column.kind === 'profile') {
        if (column.entityId) {
          if (!storeApi.getState().authorTimelinesByPubkey[column.entityId]?.length) {
            void loadAuthorSection(column.entityId);
          }
        } else if (!storeApi.getState().profileHasLoaded || storeApi.getState().profileTimelineEvicted) {
          void loadProfileSection();
        }
      }
    }
  }, [state.visibleListColumnIds, state.workspaceState.columns, state.workspaceState.activeColumnId, storeApi,
    loadBookmarksSection, loadAuthorSection, loadProfileSection, refreshVisibleShellData]);
  useSessionProjectionRefresh(storeApi, loadLiveSection, loadGameSection, visibleColumnIdsRef);

  const runLoadTopics = useCallback(
    async (_currentTopics: string[], currentActiveTopic: string, currentThread: string | null) => {
      await refreshVisibleShellData(currentActiveTopic, currentThread, 'apply');
      await loadShellSections(currentActiveTopic);
    },
    [loadShellSections, refreshVisibleShellData]
  );

  const refreshConnectivityStatus = useConnectivityStatusRefresh(
    api,
    setSyncStatus,
    setCommunityNodeStatuses
  );

  const queuedLoadTopics = useQueuedLoadTopics(runLoadTopics);
  const retryCommunityNode = useCommunityNodeRecovery(api, refreshConnectivityStatus, loadCommunityIndexCapability);
  const loadTopics = useCallback(
    async (topics: string[], currentActiveTopic: string, currentThread: string | null) => {
      await queuedLoadTopics(topics, currentActiveTopic, currentThread);
      await refreshConnectivityStatus();
    },
    [queuedLoadTopics, refreshConnectivityStatus]
  );

  const refreshVisibleTimelineAfterPublish = useCallback(
    async (topic: string, currentThread: string | null, scopeChannelId?: string | null) => {
      const state = storeApi.getState();
      const tasks = [refreshVisibleShellData(topic, currentThread, 'apply', scopeChannelId)];
      if (!scopeChannelId) {
        if (state.workspaceState.columns.some((column) => column.kind === 'profile' && !column.entityId)) {
          tasks.push(loadProfileSection());
        }
        const ownAuthor = state.syncStatus.local_author_pubkey;
        if (ownAuthor && state.workspaceState.columns.some(
          (column) => column.kind === 'profile' && column.entityId === ownAuthor
        )) {
          tasks.push(loadAuthorSection(ownAuthor));
        }
      }
      await Promise.all(tasks);
    },
    [loadAuthorSection, loadProfileSection, refreshVisibleShellData, storeApi]
  );

  const refreshTimelineFeed = useCallback(
    async (topic: string, currentThread: string | null, scopeChannelId?: string | null) => {
      const timelineScope = privateTimelineScope(
        scopeChannelId === undefined
          ? activeWorkspaceScope(storeApi.getState().workspaceState).topicId === topic
            ? activeWorkspaceScope(storeApi.getState().workspaceState).channelId
            : null
          : scopeChannelId
      );
      if (applyPendingTimeline(topic, timelineScope)) {
        return;
      }
      await refreshVisibleShellData(topic, currentThread, 'apply', scopeChannelId);
    },
    [applyPendingTimeline, refreshVisibleShellData, storeApi]
  );

  const { retryMediaFetch } = useDesktopShellDataEffects({
    api,
    storeApi,
    trackedTopics,
    activeTopic,
    selectedThread,
    activeGameRooms,
    activeJoinedChannels,
    selectedPrivateChannelId,
    mediaObjectUrls,
    shellChromeState,
    selectedAuthorPubkey,
    previewableMediaAttachments,
    gatedAdultMediaHashes,
    remoteObjectUrlRef,
    draftPreviewUrlRef,
    directMessageDraftPreviewUrlRef,
    mediaFetchAttemptRef,
    visibleRefreshInFlightRef,
    visibleColumnIdsRef,
    loadTopics,
    loadProfileSection,
    loadAuthorSection,
    loadMessagesSection,
    loadNotificationsSection,
    loadCommunityIndexCapability,
    refreshVisibleShellData,
    refreshConnectivityStatus,
    refreshNotificationStatus,
    refreshNotificationsFromEvent,
    setCommunityNodeStatuses,
    setSyncStatus,
    setLocalProfile,
    setProfileDraft,
    setGameDrafts,
    setSelectedChannelIdByTopic,
    setComposeChannelByTopic,
    setTimelineScopeByTopic,
    setMediaObjectUrls,
  });
  const reloadPostElements = useCallback(
    async (post: PostView, bodyObjectId?: string | null, manual = true) => {
      let mediaRetry: Promise<void> = Promise.resolve();
      if (manual && bodyObjectId == null) {
        const currentUrls = storeApi.getState().mediaObjectUrls;
        const relatedAttachments = [
          ...post.attachments,
          ...(post.reply_preview?.attachments ?? []),
          ...(post.repost_of?.attachments ?? []),
        ];
        const accepted = retryMediaFetch(
          relatedAttachments
            .map((attachment) => attachment.hash)
            .filter(
              (hash, index, hashes) =>
                currentUrls[hash] === null && hashes.indexOf(hash) === index
            )
        );
        if (accepted.length > 0) {
          mediaRetry = new Promise<void>((resolve) => {
            let observedInFlight = false;
            let settled = false;
            const finish = () => {
              if (settled) return;
              settled = true;
              window.clearTimeout(timeoutId);
              unsubscribe();
              resolve();
            };
            const check = () => {
              const retrying = storeApi.getState().mediaRetryingHashes;
              if (accepted.some((hash) => retrying[hash])) observedInFlight = true;
              if (observedInFlight && accepted.every((hash) => !retrying[hash])) finish();
            };
            const unsubscribe = storeApi.subscribe(check);
            const timeoutId = window.setTimeout(finish, 35_000);
            check();
          });
        }
      }
      const [updated] = await Promise.all([
        api.retryPostElements(post.object_id, bodyObjectId ?? null, manual),
        mediaRetry,
      ]);
      return updated;
    },
    [api, retryMediaFetch, storeApi]
  );

  const {
    rememberDraftPreview,
    releaseDraftPreview,
    releaseAllDraftPreviews,
    rememberDirectMessageDraftPreview,
    releaseDirectMessageDraftPreview,
    releaseAllDirectMessageDraftPreviews,
    buildImageDraftItem,
    buildVideoDraftItem,
  } = useDraftMediaHelpers({
    draftPreviewUrlRef,
    directMessageDraftPreviewUrlRef,
    draftSequenceRef,
  });

  return {
    gatedAdultMediaHashes,
    retryMediaFetch,
    reloadPostElements,
    loadTopics,
    retryCommunityNode,
    refreshConnectivityStatus,
    refreshVisibleShellData,
    refreshVisibleTimelineAfterPublish,
    refreshTimelineFeed,
    loadProfileSection,
    loadAuthorSection,
    loadMoreProfileTimeline,
    loadMoreAuthorTimeline,
    loadBookmarksSection,
    navigateBookmarkPage,
    applyPendingTimeline,
    loadReactionCatalogData,
    loadNotificationsSection,
    navigateNotificationPage,
    loadMoreTimeline,
    loadMoreThread,
    rememberDraftPreview,
    releaseDraftPreview,
    releaseAllDraftPreviews,
    rememberDirectMessageDraftPreview,
    releaseDirectMessageDraftPreview,
    releaseAllDirectMessageDraftPreviews,
    buildImageDraftItem,
    buildVideoDraftItem,
  };
}
