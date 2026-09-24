import { startTransition, useCallback, useRef } from 'react';

import type { BookmarkCursor, DesktopApi } from '@/lib/api';
import type { LoadNotificationsSection } from '@/shell/data/loaders/useNotificationLoaders';
import { VISIBLE_TIMELINE_LIMIT } from '@/shell/pagination';
import {
  authorViewFromDirectMessageConversation,
  communityNodesToDraftNodes,
  mergeKnownAuthors,
  messageFromError,
  privateTimelineScope,
  profileInputFromProfile,
  seedPeersToEditorValue,
} from '@/shell/presentation';
import { sameTimelineCursor, setRecordEntry, setTimelineCursorEntry } from '@/shell/stateUpdates';
import {
  hasReadPastHeadPage,
  mergeRefreshedVisiblePosts,
  mergeUniquePosts,
  windowHeadCursor,
} from '@/shell/data/timelineMerge';
import {
  timelineStorageKeyForChannel,
  useDesktopShellFieldSetter,
  type DesktopShellStoreApi,
} from '@/shell/store';
import {
  activeWorkspaceColumn,
  activeWorkspaceScope,
  primarySectionForColumn,
} from '@/shell/slices/workspace';

type UseDesktopShellSectionLoadersArgs = {
  api: DesktopApi;
  loadReactionCatalogData: () => Promise<void>;
  loadNotificationsSection: LoadNotificationsSection;
  storeApi: DesktopShellStoreApi;
  translate: (key: string, options?: Record<string, unknown>) => string;
};

export function useDesktopShellSectionLoaders({
  api,
  loadReactionCatalogData,
  loadNotificationsSection,
  storeApi,
  translate,
}: UseDesktopShellSectionLoadersArgs) {
  const profileRequestId = useRef(0);
  const bookmarkRequestId = useRef(0);
  const communityNodeRequestId = useRef(0);
  const authorRequestIds = useRef(new Map<string, number>());
  const setAuthorError = useDesktopShellFieldSetter('authorError');
  const setAuthorErrorsByPubkey = useDesktopShellFieldSetter('authorErrorsByPubkey');
  const setAuthorTimelinesByPubkey = useDesktopShellFieldSetter('authorTimelinesByPubkey');
  const setAuthorTimelineNextCursorByPubkey = useDesktopShellFieldSetter(
    'authorTimelineNextCursorByPubkey'
  );
  const setAuthorTimelineWindowHeadCursorByPubkey = useDesktopShellFieldSetter('authorTimelineWindowHeadCursorByPubkey');
  const setAuthorTimelineLoadingMoreByPubkey = useDesktopShellFieldSetter(
    'authorTimelineLoadingMoreByPubkey'
  );
  const setAuthorTimelineLoadMoreErrorsByPubkey = useDesktopShellFieldSetter(
    'authorTimelineLoadMoreErrorsByPubkey'
  );
  const setBookmarkedPosts = useDesktopShellFieldSetter('bookmarkedPosts');
  const setBookmarksNewerCursor = useDesktopShellFieldSetter('bookmarksNewerCursor');
  const setBookmarksOlderCursor = useDesktopShellFieldSetter('bookmarksOlderCursor');
  const setBookmarksLoadingPage = useDesktopShellFieldSetter('bookmarksLoadingPage');
  const setBookmarksPageRequestCursor = useDesktopShellFieldSetter('bookmarksPageRequestCursor');
  const setBookmarksPageRequestBefore = useDesktopShellFieldSetter('bookmarksPageRequestBefore');
  const setBookmarksPanelState = useDesktopShellFieldSetter('bookmarksPanelState');
  const setCommunityNodeConfig = useDesktopShellFieldSetter('communityNodeConfig');
  const setCommunityNodeError = useDesktopShellFieldSetter('communityNodeError');
  const setCommunityNodeInput = useDesktopShellFieldSetter('communityNodeInput');
  const setCommunityNodeManifests = useDesktopShellFieldSetter('communityNodeManifests');
  const setDirectMessageError = useDesktopShellFieldSetter('directMessageError');
  const setDirectMessages = useDesktopShellFieldSetter('directMessages');
  const setDirectMessageStatusByPeer = useDesktopShellFieldSetter('directMessageStatusByPeer');
  const setDirectMessageTimelineByPeer = useDesktopShellFieldSetter(
    'directMessageTimelineByPeer'
  );
  const setDirectMessageTimelineNextCursorByPeer = useDesktopShellFieldSetter(
    'directMessageTimelineNextCursorByPeer'
  );
  const setDiscoveryConfig = useDesktopShellFieldSetter('discoveryConfig');
  const setDiscoveryError = useDesktopShellFieldSetter('discoveryError');
  const setDiscoverySeedInput = useDesktopShellFieldSetter('discoverySeedInput');
  const setGamePanelStateByScopeKey = useDesktopShellFieldSetter('gamePanelStateByScopeKey');
  const setGameRoomsByScopeKey = useDesktopShellFieldSetter('gameRoomsByScopeKey');
  const setKnownAuthorsByPubkey = useDesktopShellFieldSetter('knownAuthorsByPubkey');
  const setLivePanelStateByScopeKey = useDesktopShellFieldSetter('livePanelStateByScopeKey');
  const setLiveSessionsByScopeKey = useDesktopShellFieldSetter('liveSessionsByScopeKey');
  const setLocalPeerTicket = useDesktopShellFieldSetter('localPeerTicket');
  const setLocalProfile = useDesktopShellFieldSetter('localProfile');
  const setProfileDraft = useDesktopShellFieldSetter('profileDraft');
  const setProfileError = useDesktopShellFieldSetter('profileError');
  const setProfilePanelState = useDesktopShellFieldSetter('profilePanelState');
  const setProfileTimeline = useDesktopShellFieldSetter('profileTimeline');
  const setProfileTimelineNextCursor = useDesktopShellFieldSetter(
    'profileTimelineNextCursor'
  );
  const setProfileTimelineWindowHeadCursor = useDesktopShellFieldSetter('profileTimelineWindowHeadCursor');
  const setProfileTimelineLoadingMore = useDesktopShellFieldSetter(
    'profileTimelineLoadingMore'
  );
  const setProfileTimelineLoadMoreError = useDesktopShellFieldSetter(
    'profileTimelineLoadMoreError'
  );
  const setReactionPanelState = useDesktopShellFieldSetter('reactionPanelState');
  const setSelectedAuthor = useDesktopShellFieldSetter('selectedAuthor');
  const setSelectedAuthorTimeline = useDesktopShellFieldSetter('selectedAuthorTimeline');
  const setSelectedAuthorTimelineNextCursor = useDesktopShellFieldSetter(
    'selectedAuthorTimelineNextCursor'
  );
  const setSocialConnections = useDesktopShellFieldSetter('socialConnections');
  const setSocialConnectionsPanelState = useDesktopShellFieldSetter(
    'socialConnectionsPanelState'
  );

  const loadLiveSection = useCallback(
    async (topic: string, selectedChannelId: string | null) => {
      const scopeKey = timelineStorageKeyForChannel(topic, selectedChannelId);
      try {
        const sessions = await api.listLiveSessions(
          topic,
          privateTimelineScope(selectedChannelId)
        );
        startTransition(() => {
          setLiveSessionsByScopeKey(setRecordEntry(scopeKey, sessions));
          setLivePanelStateByScopeKey(
            setRecordEntry(scopeKey, { status: 'ready', error: null })
          );
        });
      } catch (error) {
        const panelState = {
          status: 'error' as const,
          error: messageFromError(
            error,
            translate('common:errors.failedToLoadLiveSessions')
          ),
        };
        setLivePanelStateByScopeKey(setRecordEntry(scopeKey, panelState));
      }
    },
    [
      api,
      setLivePanelStateByScopeKey,
      setLiveSessionsByScopeKey,
      translate,
    ]
  );

  const loadGameSection = useCallback(
    async (topic: string, selectedChannelId: string | null) => {
      const scopeKey = timelineStorageKeyForChannel(topic, selectedChannelId);
      try {
        const rooms = await api.listGameRooms(topic, privateTimelineScope(selectedChannelId));
        startTransition(() => {
          setGameRoomsByScopeKey(setRecordEntry(scopeKey, rooms));
          setGamePanelStateByScopeKey(
            setRecordEntry(scopeKey, { status: 'ready', error: null })
          );
        });
      } catch (error) {
        const panelState = {
          status: 'error' as const,
          error: messageFromError(error, translate('common:errors.failedToLoadGameRooms')),
        };
        setGamePanelStateByScopeKey(setRecordEntry(scopeKey, panelState));
      }
    },
    [
      api,
      setGamePanelStateByScopeKey,
      setGameRoomsByScopeKey,
      translate,
    ]
  );

  const loadProfileSection = useCallback(async (options: { resetToLatest?: boolean } = {}) => {
    const state = storeApi.getState();
    if (!state.workspaceState.columns.some((column) => column.kind === 'profile' && !column.entityId)) return;
    const requestId = ++profileRequestId.current;
    const saveRevision = state.profileSaveRevision;
    const isCurrent = () => requestId === profileRequestId.current &&
      saveRevision === storeApi.getState().profileSaveRevision;
    state.patchState({ profileRefreshing: true });
    setProfileError(null);
    setProfilePanelState({ status: state.profileHasLoaded ? 'ready' : 'loading', error: null });
    try {
      const [profile, following, followed, muted, blocking] = await Promise.all([
        api.getMyProfile(),
        api.listSocialConnections('following'),
        api.listSocialConnections('followed'),
        api.listSocialConnections('muted'),
        api.listSocialConnections('blocking'),
      ]);
      const savedHead = !options.resetToLatest && storeApi.getState().profileTimeline.length === 0
        ? storeApi.getState().profileTimelineWindowHeadCursor : null;
      const timeline = await api.listProfileTimeline(
        profile.pubkey,
        savedHead,
        VISIBLE_TIMELINE_LIMIT
      );
      if (!isCurrent()) return;
      startTransition(() => {
        storeApi.getState().patchState({ profileHasLoaded: true });
        setLocalProfile(profile);
        if (!storeApi.getState().profileDirty) {
          setProfileDraft(profileInputFromProfile(profile));
        }
        const current = storeApi.getState();
        const preserveOlderPages = !options.resetToLatest && hasReadPastHeadPage(
          current.profileTimeline,
          timeline.items,
          current.profileTimelineNextCursor,
          timeline.next_cursor,
          'desc'
        );
        setProfileTimeline(
          mergeRefreshedVisiblePosts(current.profileTimeline, timeline.items, preserveOlderPages)
        );
        const nextCursor = preserveOlderPages ? current.profileTimelineNextCursor : timeline.next_cursor ?? null;
        setProfileTimelineNextCursor((stored) => sameTimelineCursor(stored, nextCursor) ? stored : nextCursor);
        if (!preserveOlderPages) setProfileTimelineWindowHeadCursor(savedHead);
        setProfileTimelineLoadingMore(false);
        setProfileTimelineLoadMoreError(null);
        setProfileError(null);
        setProfilePanelState({ status: 'ready', error: null });
        setSocialConnections({ following, followed, muted, blocking });
        setKnownAuthorsByPubkey((current) =>
          mergeKnownAuthors(current, [...following, ...followed, ...muted, ...blocking])
        );
        setSocialConnectionsPanelState({ status: 'ready', error: null });
      });
    } catch (error) {
      if (!isCurrent()) return;
      const message = messageFromError(
        error,
        translate('common:errors.failedToLoadProfile')
      );
      setProfileError(message);
      setProfilePanelState({ status: 'error', error: message });
    } finally {
      if (requestId === profileRequestId.current) {
        storeApi.getState().patchState({
          profileRefreshing: false,
          profileTimelineLoadingMore: false,
        });
      }
    }
  }, [
    api,
    setKnownAuthorsByPubkey,
    setLocalProfile,
    setProfileDraft,
    setProfileError,
    setProfilePanelState,
    setProfileTimeline,
    setProfileTimelineLoadMoreError,
    setProfileTimelineLoadingMore,
    setProfileTimelineNextCursor,
    setProfileTimelineWindowHeadCursor,
    setSocialConnections,
    setSocialConnectionsPanelState,
    storeApi,
    translate,
  ]);

  const loadAuthorSection = useCallback(
    async (pubkey: string, options: { resetToLatest?: boolean } = {}) => {
      const requestId = (authorRequestIds.current.get(pubkey) ?? 0) + 1;
      authorRequestIds.current.set(pubkey, requestId);
      const savedHead = !options.resetToLatest && (storeApi.getState().authorTimelinesByPubkey[pubkey]?.length ?? 0) === 0
        ? storeApi.getState().authorTimelineWindowHeadCursorByPubkey[pubkey] ?? null : null;
      try {
        const [author, timeline] = await Promise.all([
          api.getAuthorSocialView(pubkey),
          api.listProfileTimeline(pubkey, savedHead, VISIBLE_TIMELINE_LIMIT),
        ]);
        if (requestId !== authorRequestIds.current.get(pubkey)) return;
        startTransition(() => {
          const current = storeApi.getState();
          const currentTimeline = current.authorTimelinesByPubkey[pubkey] ?? [];
          const preserveOlderPages = !options.resetToLatest && hasReadPastHeadPage(
            currentTimeline,
            timeline.items,
            current.authorTimelineNextCursorByPubkey[pubkey],
            timeline.next_cursor,
            'desc'
          );
          const mergedTimeline = mergeRefreshedVisiblePosts(
            currentTimeline,
            timeline.items,
            preserveOlderPages
          );
          const resolvedCursor = preserveOlderPages
            ? (current.authorTimelineNextCursorByPubkey[pubkey] ?? null)
            : (timeline.next_cursor ?? null);
          if (current.selectedAuthorPubkey === pubkey) {
            setSelectedAuthor(author);
            setSelectedAuthorTimeline(mergedTimeline);
            setSelectedAuthorTimelineNextCursor((stored) =>
              sameTimelineCursor(stored, resolvedCursor) ? stored : resolvedCursor);
            setAuthorError(null);
          }
          setAuthorTimelinesByPubkey(setRecordEntry(pubkey, mergedTimeline));
          setAuthorTimelineNextCursorByPubkey(
            setTimelineCursorEntry(pubkey, resolvedCursor)
          );
          if (!preserveOlderPages) {
            setAuthorTimelineWindowHeadCursorByPubkey(setRecordEntry(pubkey, savedHead));
          }
          setAuthorTimelineLoadingMoreByPubkey(setRecordEntry(pubkey, false));
          setAuthorTimelineLoadMoreErrorsByPubkey(setRecordEntry(pubkey, null));
          setAuthorErrorsByPubkey(setRecordEntry(pubkey, null));
          if (author) {
            setKnownAuthorsByPubkey((current) => mergeKnownAuthors(current, [author]));
          }
        });
      } catch (error) {
        if (requestId !== authorRequestIds.current.get(pubkey)) return;
        const message = messageFromError(error, translate('common:errors.failedToLoadAuthor'));
        if (storeApi.getState().selectedAuthorPubkey === pubkey) setAuthorError(message);
        setAuthorErrorsByPubkey(setRecordEntry(pubkey, message));
      } finally {
        if (requestId === authorRequestIds.current.get(pubkey)) {
          setAuthorTimelineLoadingMoreByPubkey(setRecordEntry(pubkey, false));
        }
      }
    },
    [
      api,
      setAuthorError,
      setAuthorErrorsByPubkey,
      setAuthorTimelinesByPubkey,
      setAuthorTimelineNextCursorByPubkey,
      setAuthorTimelineWindowHeadCursorByPubkey,
      setAuthorTimelineLoadingMoreByPubkey,
      setAuthorTimelineLoadMoreErrorsByPubkey,
      setKnownAuthorsByPubkey,
      setSelectedAuthor,
      setSelectedAuthorTimeline,
      setSelectedAuthorTimelineNextCursor,
      storeApi,
      translate,
    ]
  );

  const loadMoreProfileTimeline = useCallback(async () => {
    const state = storeApi.getState();
    const cursor = state.profileTimelineNextCursor;
    const pubkey = state.localProfile?.pubkey;
    if (!cursor || !pubkey || state.profileTimelineLoadingMore) return;
    const requestId = profileRequestId.current;
    const saveRevision = state.profileSaveRevision;
    setProfileTimelineLoadingMore(true);
    setProfileTimelineLoadMoreError(null);
    try {
      const timeline = await api.listProfileTimeline(pubkey, cursor, VISIBLE_TIMELINE_LIMIT);
      if (
        requestId !== profileRequestId.current ||
        saveRevision !== storeApi.getState().profileSaveRevision
      ) return;
      startTransition(() => {
        const latest = storeApi.getState();
        const merged = mergeUniquePosts(latest.profileTimeline, timeline.items);
        setProfileTimeline(merged);
        setProfileTimelineWindowHeadCursor(windowHeadCursor(latest.profileTimeline, merged,
          latest.profileTimelineWindowHeadCursor));
        setProfileTimelineNextCursor((stored) => sameTimelineCursor(stored, timeline.next_cursor)
          ? stored : timeline.next_cursor ?? null);
      });
    } catch (error) {
      if (requestId !== profileRequestId.current) return;
      setProfileTimelineLoadMoreError(
        messageFromError(error, translate('common:errors.failedToLoadProfile'))
      );
    } finally {
      if (requestId === profileRequestId.current) setProfileTimelineLoadingMore(false);
    }
  }, [
    api,
    setProfileTimeline,
    setProfileTimelineLoadMoreError,
    setProfileTimelineLoadingMore,
    setProfileTimelineNextCursor,
    setProfileTimelineWindowHeadCursor,
    storeApi,
    translate,
  ]);

  const loadMoreAuthorTimeline = useCallback(async (pubkey: string) => {
    const state = storeApi.getState();
    const cursor = state.authorTimelineNextCursorByPubkey[pubkey] ?? null;
    if (!cursor || state.authorTimelineLoadingMoreByPubkey[pubkey]) return;
    const requestId = authorRequestIds.current.get(pubkey) ?? 0;
    setAuthorTimelineLoadingMoreByPubkey(setRecordEntry(pubkey, true));
    setAuthorTimelineLoadMoreErrorsByPubkey(setRecordEntry(pubkey, null));
    try {
      const timeline = await api.listProfileTimeline(pubkey, cursor, VISIBLE_TIMELINE_LIMIT);
      if (requestId !== (authorRequestIds.current.get(pubkey) ?? 0)) return;
      startTransition(() => {
        const current = storeApi.getState();
        const merged = mergeUniquePosts(
          current.authorTimelinesByPubkey[pubkey] ?? [],
          timeline.items
        );
        setAuthorTimelinesByPubkey(setRecordEntry(pubkey, merged));
        setAuthorTimelineWindowHeadCursorByPubkey(setRecordEntry(pubkey,
          windowHeadCursor(current.authorTimelinesByPubkey[pubkey] ?? [], merged,
            current.authorTimelineWindowHeadCursorByPubkey[pubkey] ?? null)));
        setAuthorTimelineNextCursorByPubkey(
          setTimelineCursorEntry(pubkey, timeline.next_cursor ?? null)
        );
        if (current.selectedAuthorPubkey === pubkey) {
          setSelectedAuthorTimeline(merged);
          setSelectedAuthorTimelineNextCursor((stored) => sameTimelineCursor(stored, timeline.next_cursor)
            ? stored : timeline.next_cursor ?? null);
        }
      });
    } catch (error) {
      if (requestId !== (authorRequestIds.current.get(pubkey) ?? 0)) return;
      setAuthorTimelineLoadMoreErrorsByPubkey(
        setRecordEntry(
          pubkey,
          messageFromError(error, translate('common:errors.failedToLoadAuthor'))
        )
      );
    } finally {
      if (requestId === (authorRequestIds.current.get(pubkey) ?? 0)) {
        setAuthorTimelineLoadingMoreByPubkey(setRecordEntry(pubkey, false));
      }
    }
  }, [
    api,
    setAuthorTimelineLoadMoreErrorsByPubkey,
    setAuthorTimelineLoadingMoreByPubkey,
    setAuthorTimelineNextCursorByPubkey,
    setAuthorTimelineWindowHeadCursorByPubkey,
    setAuthorTimelinesByPubkey,
    setSelectedAuthorTimeline,
    setSelectedAuthorTimelineNextCursor,
    storeApi,
    translate,
  ]);

  const loadMessagesSection = useCallback(async () => {
    try {
      const directMessages = await api.listDirectMessages();
      startTransition(() => {
        setDirectMessages(directMessages);
        setKnownAuthorsByPubkey((current) =>
          mergeKnownAuthors(
            current,
            directMessages.map(authorViewFromDirectMessageConversation)
          )
        );
      });
      const selectedPeerPubkey = storeApi.getState().selectedDirectMessagePeerPubkey;
      if (!selectedPeerPubkey) {
        setDirectMessageError(null);
        return;
      }
      const [timelineResult, statusResult] = await Promise.allSettled([
        api.listDirectMessageMessages(selectedPeerPubkey, null, VISIBLE_TIMELINE_LIMIT),
        api.getDirectMessageStatus(selectedPeerPubkey),
      ]);
      startTransition(() => {
        if (timelineResult.status === 'fulfilled') {
          setDirectMessageTimelineByPeer(
            setRecordEntry(selectedPeerPubkey, timelineResult.value.items)
          );
          setDirectMessageTimelineNextCursorByPeer(
            setRecordEntry(selectedPeerPubkey, timelineResult.value.next_cursor ?? null)
          );
        }
        if (statusResult.status === 'fulfilled') {
          setDirectMessageStatusByPeer(
            setRecordEntry(selectedPeerPubkey, statusResult.value)
          );
        }
        setDirectMessageError(
          timelineResult.status === 'fulfilled' && statusResult.status === 'fulfilled'
            ? null
            : messageFromError(
                timelineResult.status === 'rejected'
                  ? timelineResult.reason
                  : statusResult.status === 'rejected'
                    ? statusResult.reason
                    : null,
                  translate('common:errors.failedToLoadDirectMessages')
              )
        );
      });
    } catch (error) {
      setDirectMessageError(
        messageFromError(error, translate('common:errors.failedToLoadDirectMessages'))
      );
    }
  }, [
    api,
    setDirectMessageError,
    setDirectMessages,
    setDirectMessageStatusByPeer,
    setDirectMessageTimelineByPeer,
    setDirectMessageTimelineNextCursorByPeer,
    setKnownAuthorsByPubkey,
    storeApi,
    translate,
  ]);

  const loadBookmarksSection = useCallback(async (
    options: { cursor?: BookmarkCursor | null; before?: boolean; preserveCurrent?: boolean } = {}
  ) => {
    // #994: 一覧をまだ出せない(初回 / 初回失敗後)ときだけ loading / error を示す。
    // 値がある再取得は一覧を保持し、成功時に差し替える(失敗は best effort のまま)。
    const current = storeApi.getState();
    const account = current.syncStatus.local_author_pubkey;
    const showsProgress = current.bookmarksPanelState.status !== 'ready';
    const cursor = options.preserveCurrent ? current.bookmarksPageRequestCursor : options.cursor ?? null;
    const before = options.preserveCurrent ? current.bookmarksPageRequestBefore : options.before ?? false;
    const requestId = ++bookmarkRequestId.current;
    if (showsProgress) setBookmarksPanelState({ status: 'loading', error: null });
    setBookmarksLoadingPage(true);
    try {
      const page = await api.listBookmarkedPostsPage(cursor, before);
      if (requestId !== bookmarkRequestId.current ||
        storeApi.getState().syncStatus.local_author_pubkey !== account) return;
      startTransition(() => {
        setBookmarkedPosts(page.items);
        setBookmarksNewerCursor(page.newer_cursor);
        setBookmarksOlderCursor(page.older_cursor);
        setBookmarksPageRequestCursor(cursor);
        setBookmarksPageRequestBefore(before);
        setBookmarksPanelState({ status: 'ready', error: null });
      });
    } catch (error) {
      if (!showsProgress || storeApi.getState().syncStatus.local_author_pubkey !== account) return;
      setBookmarksPanelState({
        status: 'error',
        error: messageFromError(error, translate('common:errors.failedToLoadBookmarks')),
      });
    } finally {
      if (requestId === bookmarkRequestId.current) setBookmarksLoadingPage(false);
    }
  }, [api, setBookmarkedPosts, setBookmarksLoadingPage, setBookmarksNewerCursor, setBookmarksOlderCursor, setBookmarksPageRequestCursor, setBookmarksPageRequestBefore, setBookmarksPanelState, storeApi, translate]);

  const navigateBookmarkPage = useCallback(async (before: boolean) => {
    const current = storeApi.getState();
    const cursor = before ? current.bookmarksNewerCursor : current.bookmarksOlderCursor;
    if (!cursor || current.bookmarksLoadingPage) return;
    await loadBookmarksSection({ cursor, before });
  }, [loadBookmarksSection, storeApi]);

  const loadCommunityIndexCapability = useCallback(async () => {
    const requestId = ++communityNodeRequestId.current;
    const previousConfig = storeApi.getState().communityNodeConfig;
    try {
      const config = await api.getCommunityNodeConfig();
      if (requestId !== communityNodeRequestId.current ||
        storeApi.getState().communityNodeConfig !== previousConfig) return;
      startTransition(() => {
        setCommunityNodeConfig(config);
        if (!storeApi.getState().communityNodeEditorDirty) {
          setCommunityNodeInput(communityNodesToDraftNodes(config));
        }
        setCommunityNodeError(null);
      });
      const baseUrls = config.nodes
        .map((node) => node.base_url)
        .filter((baseUrl) => baseUrl.trim().length > 0);
      if (baseUrls.length === 0) {
        setCommunityNodeManifests({});
        return;
      }
      setCommunityNodeManifests((current) => {
        const next = { ...current };
        for (const baseUrl of baseUrls) {
          // A refresh is background work when a usable manifest is already on screen.
          // Keep that successful value until the replacement settles so consumers do not
          // briefly project an empty/no-node state while another Column is activated.
          if (next[baseUrl]?.status !== 'ok') {
            next[baseUrl] = { status: 'loading' };
          }
        }
        return next;
      });
      const manifestResults = await Promise.all(
        baseUrls.map(async (baseUrl) => {
          try {
            const result = await api.fetchCommunityNodeManifest(baseUrl);
            return [
              baseUrl,
              result.status === 'ok' && result.manifest
                ? { status: 'ok' as const, manifest: result.manifest }
                : { status: 'absent' as const },
            ] as const;
          } catch (error) {
            return [
              baseUrl,
              {
                status: 'error' as const,
                error: messageFromError(error, translate('common:errors.failedToLoadSettings')),
              },
            ] as const;
          }
        })
      );
      if (requestId !== communityNodeRequestId.current) return;
      const currentUrls = storeApi.getState().communityNodeConfig.nodes.map((node) => node.base_url);
      if (JSON.stringify(currentUrls) !== JSON.stringify(baseUrls)) return;
      const manifests = Object.fromEntries(manifestResults);
      setCommunityNodeManifests((current) => ({ ...current, ...manifests }));
    } catch (error) {
      if (requestId !== communityNodeRequestId.current ||
        storeApi.getState().communityNodeConfig !== previousConfig) return;
      storeApi.getState().patchState({ communityNodeConfigError: 'config_unavailable' });
      setCommunityNodeError(
        messageFromError(error, translate('common:errors.failedToLoadSettings'))
      );
    }
  }, [
    api,
    setCommunityNodeConfig,
    setCommunityNodeError,
    setCommunityNodeInput,
    setCommunityNodeManifests,
    storeApi,
    translate,
  ]);

  const loadSettingsSection = useCallback(async () => {
    const { activeSettingsSection } = storeApi.getState().shellChromeState;
    if (activeSettingsSection === 'connectivity') {
      try {
        setLocalPeerTicket(await api.getLocalPeerTicket());
      } catch {
        // best effort refresh
      }
      return;
    }
    if (activeSettingsSection === 'discovery') {
      try {
        const config = await api.getDiscoveryConfig();
        setDiscoveryConfig(config);
        if (!storeApi.getState().discoveryEditorDirty) {
          setDiscoverySeedInput(seedPeersToEditorValue(config));
        }
        setDiscoveryError(null);
      } catch (error) {
        setDiscoveryError(
          messageFromError(error, translate('common:errors.failedToLoadSettings'))
        );
      }
      return;
    }
    if (activeSettingsSection === 'community-node') {
      await loadCommunityIndexCapability();
      return;
    }
    if (activeSettingsSection === 'reactions') {
      try {
        await Promise.all([
          loadBookmarksSection({ preserveCurrent: true }),
          loadReactionCatalogData(),
        ]);
      } catch (error) {
        setReactionPanelState({
          status: 'error',
          error: messageFromError(error, translate('common:errors.failedToLoadSettings')),
        });
      }
    }
  }, [
    api,
    loadReactionCatalogData,
    loadBookmarksSection,
    loadCommunityIndexCapability,
    setDiscoveryConfig,
    setDiscoveryError,
    setDiscoverySeedInput,
    setLocalPeerTicket,
    setReactionPanelState,
    storeApi,
    translate,
  ]);

  const loadShellSections = useCallback(
    async (topic: string) => {
      const state = storeApi.getState();
      const activeColumn = activeWorkspaceColumn(state.workspaceState);
      const activeScope = activeWorkspaceScope(state.workspaceState);
      const selectedChannelId = activeScope.topicId === topic ? activeScope.channelId : null;
      const selectedAuthorPubkey = state.selectedAuthorPubkey;
      const { settingsOpen } = state.shellChromeState;
      const activePrimarySection = primarySectionForColumn(activeColumn);
      const timelineView = activeColumn.kind === 'timeline' ? activeColumn.timelineView ?? 'feed' : 'feed';
      const tasks: Promise<void>[] = [];
      tasks.push(loadCommunityIndexCapability());

      if (activePrimarySection === 'live') {
        tasks.push(loadLiveSection(topic, selectedChannelId));
      }
      if (activePrimarySection === 'game') {
        tasks.push(loadGameSection(topic, selectedChannelId));
      }
      // Section navigation may bootstrap a missing profile, but never refresh a confirmed one.
      if (!state.profileHasLoaded && !state.profileRefreshing && state.profilePanelState.status === 'loading' &&
        state.workspaceState.columns.some((column) => column.kind === 'profile' && !column.entityId)) {
        tasks.push(loadProfileSection());
      }
      if (selectedAuthorPubkey) {
        tasks.push(loadAuthorSection(selectedAuthorPubkey));
      }
      if (activePrimarySection === 'messages' || state.directMessagePaneOpen) {
        tasks.push(loadMessagesSection());
      }
      if (activePrimarySection === 'notifications') {
        tasks.push(loadNotificationsSection());
      }
      if (
        (activePrimarySection === 'timeline' && timelineView === 'bookmarks') ||
        // 非 active な Timeline Column が Bookmarks を表示している場合もロードする(Issue #765)。
        state.workspaceState.columns.some((column) => column.timelineView === 'bookmarks')
      ) {
        tasks.push(loadBookmarksSection({ preserveCurrent: true }));
      }
      if (settingsOpen) {
        tasks.push(loadSettingsSection());
      }
      await Promise.allSettled(tasks);
    },
    [
      loadAuthorSection,
      loadBookmarksSection,
      loadGameSection,
      loadLiveSection,
      loadCommunityIndexCapability,
      loadMessagesSection,
      loadNotificationsSection,
      loadProfileSection,
      loadSettingsSection,
      storeApi,
    ]
  );

  // 個別 loader も公開する: section 遷移起点の effect(useDesktopShellDataEffects)が
  // 同じ実装を呼ぶための入口。通知の取得・state反映は注入したloaderへ委譲する。
  return {
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
    loadNotificationsSection,
    loadCommunityIndexCapability,
  };
}
