import {
  startTransition,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  type MutableRefObject,
  type RefObject,
} from 'react';
import { useLocation, useNavigate } from 'react-router-dom';

import type { DesktopApi } from '@/lib/api';

import type {
  PrimarySection,
  ProfileConnectionsView,
  TimelineWorkspaceView,
} from '@/components/shell/types';
import {
  type OpenAuthorOptions,
  type OpenThreadOptions,
  parsePrimarySectionPath,
  resolveHashBackedRouteLocation,
} from '@/shell/routes';
import { setRecordEntry } from '@/shell/stateUpdates';
import { loadThreadForFocus } from '@/shell/data/loaders/threadFocus';
import { useRouteSynchronization } from '@/shell/routing/useRouteSynchronization';
import { useSyncRoute } from '@/shell/routing/useSyncRoute';
import { useShallow } from 'zustand/react/shallow';
import {
  authorViewFromDirectMessageConversation,
  mergeKnownAuthors,
  messageFromError,
  privateComposeTarget,
  privateTimelineScope,
} from '@/shell/presentation';
import { selectShellRoutingSlice } from '@/shell/storeSelectors';
import {
  useDesktopShellFieldSetter,
  useDesktopShellStore,
  useDesktopShellStoreApi,
} from '@/shell/store';
import {
  activeWorkspaceColumn,
  activateColumn,
  activeWorkspaceScope,
  closeColumn,
  columnIdentityId,
  openTransientColumn,
  primarySectionForColumn,
  setColumnTimelineView,
  type ColumnKind,
  type ColumnScope,
} from '@/shell/slices/workspace';

// Issue #765: window レベルの Escape cascade が Composer などの入力を巻き込まない
// ようにするための editable 判定(MetaverseScene の isEditableTarget を基準に、
// contenteditable 祖先の判定を加えたもの)。checkbox / radio / button 等の
// 非 text 入力は「編集中」ではないため cascade を止めない(Settings の toggle に
// focus したまま Escape で drawer を閉じる既存挙動を維持する)。
const NON_TEXT_INPUT_TYPES = new Set([
  'button',
  'checkbox',
  'color',
  'file',
  'radio',
  'range',
  'reset',
  'submit',
]);

function isEditableTarget(target: EventTarget | null) {
  if (!(target instanceof HTMLElement)) {
    return false;
  }
  const tagName = target.tagName.toLowerCase();
  if (tagName === 'input') {
    return !NON_TEXT_INPUT_TYPES.has((target as HTMLInputElement).type);
  }
  if (tagName === 'textarea' || tagName === 'select') {
    return true;
  }
  if (target.isContentEditable) {
    return true;
  }
  const editableAncestor = target.closest('[contenteditable]');
  return editableAncestor !== null && editableAncestor.getAttribute('contenteditable') !== 'false';
}

type UseDesktopShellRoutingArgs = {
  api: DesktopApi;
  translate: (key: string, options?: Record<string, unknown>) => string;
  loadTopics: (topics: string[], activeTopic: string, currentThread: string | null) => Promise<void>;
  settingsTriggerRef: RefObject<HTMLButtonElement | null>;
  pendingRouteUrlRef: MutableRefObject<string | null>;
};

export function useDesktopShellRouting({
  api,
  translate,
  loadTopics,
  settingsTriggerRef,
  pendingRouteUrlRef,
}: UseDesktopShellRoutingArgs) {
  const location = useLocation();
  const navigate = useNavigate();
  const storeApi = useDesktopShellStoreApi();
  const state = useDesktopShellStore(useShallow(selectShellRoutingSlice));
  const {
    activeTopic,
    developerModeEnabled,
    selectedThread,
    selectedAuthorPubkey,
    selectedDirectMessagePeerPubkey,
    lastNonNotificationsRoute,
    shellChromeState,
  } = state;

  const setSelectedThread = useDesktopShellFieldSetter('selectedThread');
  const setFocusedObjectId = useDesktopShellFieldSetter('focusedObjectId');
  const setThreadFocusRequestId = useDesktopShellFieldSetter('threadFocusRequestId');
  const threadNavigationRequestRef = useRef(0);
  useEffect(() => () => { threadNavigationRequestRef.current += 1; }, []);
  const setThreadsById = useDesktopShellFieldSetter('threadsById');
  const setThreadNextCursorById = useDesktopShellFieldSetter('threadNextCursorById');
  const setThreadUnavailableById = useDesktopShellFieldSetter('threadUnavailableById');
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
  const setDirectMessageStatusByPeer = useDesktopShellFieldSetter('directMessageStatusByPeer');
  const setDirectMessageError = useDesktopShellFieldSetter('directMessageError');
  const setKnownAuthorsByPubkey = useDesktopShellFieldSetter('knownAuthorsByPubkey');
  const setSelectedLiveSessionId = useDesktopShellFieldSetter('selectedLiveSessionId');
  const setSelectedGameRoomId = useDesktopShellFieldSetter('selectedGameRoomId');
  const setError = useDesktopShellFieldSetter('error');
  const setLastNonNotificationsRoute = useDesktopShellFieldSetter('lastNonNotificationsRoute');
  const setShellChromeState = useDesktopShellFieldSetter('shellChromeState');
  const setTimelineScopeByTopic = useDesktopShellFieldSetter('timelineScopeByTopic');
  const setComposeChannelByTopic = useDesktopShellFieldSetter('composeChannelByTopic');
  const setWorkspaceState = useDesktopShellFieldSetter('workspaceState');
  const resolvedRouteLocation = useMemo(
    () =>
      resolveHashBackedRouteLocation(
        location.pathname,
        location.search,
        typeof window === 'undefined' ? '' : window.location.hash
      ),
    // 文字列 (pathname, search) ではなく location オブジェクトで再計算する。
    // 同一 URL 文字列の history entry へ戻る遷移(push が transition で未 commit のまま
    // goBack された場合)でも新しい location として route 消費 effect を再評価させるため。
    [location]
  );

  const routeSection = useMemo(() => {
    const candidate =
      parsePrimarySectionPath(resolvedRouteLocation.pathname) ??
      primarySectionForColumn(activeWorkspaceColumn(storeApi.getState().workspaceState));
    if (!developerModeEnabled && (candidate === 'live' || candidate === 'game')) {
      return 'timeline';
    }
    return candidate;
  }, [
    developerModeEnabled,
    resolvedRouteLocation.pathname,
    storeApi,
  ]);
  const pendingAnimationFrameIdsRef = useRef<number[]>([]);
  const lastObservedRouteUrlRef = useRef(
    `${resolvedRouteLocation.pathname}${resolvedRouteLocation.search}`
  );

  const scheduleAnimationFrame = useCallback((callback: () => void) => {
    const frameId = window.requestAnimationFrame(() => {
      pendingAnimationFrameIdsRef.current = pendingAnimationFrameIdsRef.current.filter(
        (candidate) => candidate !== frameId
      );
      callback();
    });
    pendingAnimationFrameIdsRef.current.push(frameId);
  }, []);

  useEffect(() => {
    return () => {
      for (const frameId of pendingAnimationFrameIdsRef.current) {
        window.cancelAnimationFrame(frameId);
      }
      pendingAnimationFrameIdsRef.current = [];
    };
  }, []);

  const syncRoute = useSyncRoute({
    navigate,
    pendingRouteUrlRef,
    resolvedRouteLocation,
    storeApi,
  });

  const openWorkspaceColumn = useCallback(
    (kind: ColumnKind, scope?: ColumnScope, entityId?: string, parentColumnId?: string) => {
      const id = columnIdentityId(kind, scope, entityId);
      setWorkspaceState((current) =>
        current.activeColumnId === id
          ? current
          : openTransientColumn(current, {
              id,
              kind,
              scope,
              entityId,
              parentColumnId,
              pinned: false,
            })
      );
    },
    [setWorkspaceState]
  );

  const setSettingsOpen = useCallback(
    (open: boolean, restoreToTrigger = false) => {
      setShellChromeState((current) => ({
        ...current,
        settingsOpen: open,
      }));
      if (!open && restoreToTrigger) {
        scheduleAnimationFrame(() => {
          settingsTriggerRef.current?.focus();
        });
      }
      syncRoute(open ? 'push' : 'replace', {
        settingsOpen: open,
      });
    },
    [scheduleAnimationFrame, setShellChromeState, settingsTriggerRef, syncRoute]
  );

  const openDirectMessagePane = useCallback(
    async (
      peerPubkey: string,
      options?: {
        historyMode?: 'push' | 'replace';
        normalizeOnError?: boolean;
        parentColumnId?: string;
        preserveAuthorPane?: boolean;
        preservedAuthorPubkey?: string | null;
      }
    ) => {
      try {
        const [conversation, timeline, status] = await Promise.all([
          api.openDirectMessage(peerPubkey),
          api.listDirectMessageMessages(peerPubkey, null, 100),
          api.getDirectMessageStatus(peerPubkey),
        ]);
        const preserveSelectedAuthor =
          options?.preserveAuthorPane ??
          (selectedDirectMessagePeerPubkey === peerPubkey && selectedAuthorPubkey !== null);
        const nextSelectedAuthorPubkey = preserveSelectedAuthor
          ? options?.preservedAuthorPubkey ?? selectedAuthorPubkey
          : null;
        setSelectedThread(null);
        setFocusedObjectId(null);
        if (!preserveSelectedAuthor) {
          setSelectedAuthorPubkey(null);
          setSelectedAuthor(null);
          setSelectedAuthorTimeline([]);
          setAuthorError(null);
        }
        setDirectMessages((current) => {
          const remaining = current.filter((entry) => entry.peer_pubkey !== conversation.peer_pubkey);
          return [conversation, ...remaining];
        });
        setDirectMessageTimelineByPeer(setRecordEntry(peerPubkey, timeline.items));
        setDirectMessageStatusByPeer(setRecordEntry(peerPubkey, status));
        setKnownAuthorsByPubkey((current) =>
          mergeKnownAuthors(current, [authorViewFromDirectMessageConversation(conversation)])
        );
        const currentWorkspaceState = storeApi.getState().workspaceState;
        const parentColumn = options?.parentColumnId
          ? currentWorkspaceState.columns.find((column) => column.id === options.parentColumnId)
          : undefined;
        const scope = parentColumn?.scope ?? activeWorkspaceScope(currentWorkspaceState);
        openWorkspaceColumn(
          'conversation',
          scope,
          peerPubkey,
          options?.parentColumnId ?? currentWorkspaceState.activeColumnId
        );
        setDirectMessagePaneOpen(true);
        setSelectedLiveSessionId(null);
        setSelectedGameRoomId(null);
        setSelectedDirectMessagePeerPubkey(peerPubkey);
        setDirectMessageError(null);
        syncRoute(options?.historyMode ?? 'push', {
          focusedObjectId: null,
          primarySection: 'messages',
          selectedGameRoomId: null,
          selectedAuthorPubkey: nextSelectedAuthorPubkey,
          selectedDirectMessagePeerPubkey: peerPubkey,
          selectedLiveSessionId: null,
          selectedThread: null,
        });
      } catch (openError) {
        const nextError = messageFromError(
          openError,
          translate('common:errors.failedToOpenDirectMessage')
        );
        setDirectMessageError(nextError);
        if (options?.normalizeOnError) {
          setDirectMessagePaneOpen(true);
          setSelectedDirectMessagePeerPubkey(null);
          syncRoute('replace', {
            primarySection: 'messages',
            selectedDirectMessagePeerPubkey: null,
          });
        }
      }
    },
    [
      api,
      translate,
      selectedAuthorPubkey,
      selectedDirectMessagePeerPubkey,
      setAuthorError,
      setDirectMessageError,
      setDirectMessagePaneOpen,
      setDirectMessages,
      setDirectMessageStatusByPeer,
      setDirectMessageTimelineByPeer,
      setKnownAuthorsByPubkey,
      setSelectedAuthor,
      setSelectedAuthorPubkey,
      setSelectedAuthorTimeline,
      setSelectedDirectMessagePeerPubkey,
      setSelectedGameRoomId,
      setSelectedLiveSessionId,
      setSelectedThread,
      setFocusedObjectId,
      openWorkspaceColumn,
      storeApi,
      syncRoute,
    ]
  );

  const openThread = useCallback(
    async (threadId: string, options?: OpenThreadOptions) => {
      const request = ++threadNavigationRequestRef.current;
      const initialColumn = storeApi.getState().workspaceState.activeColumnId;
      const initialHash = window.location.hash;
      const isCurrent = () => request === threadNavigationRequestRef.current &&
        (options?.isCurrent?.() ?? true) &&
        (!options?.focusObjectId || (
          initialColumn === storeApi.getState().workspaceState.activeColumnId &&
          initialHash === window.location.hash
        ));
      const topic = options?.topic ?? activeTopic;
      // 非 active Column など、global の選択 channel と異なる scope から Thread を開く場合は
      // 呼び出し元が channelId を明示する。その場合は handleSelectPrivateChannel / handleSelectTopic と
      // 同じ 3 つの状態(選択 channel / timeline scope / compose target)を更新し、route にも channel を載せる。
      const hasChannelOverride = options?.channelId !== undefined;
      const channelId = options?.channelId ?? null;
      const channelRouteOverrides = hasChannelOverride
        ? {
            timelineScope: privateTimelineScope(channelId),
            composeTarget: privateComposeTarget(channelId),
          }
        : {};
      try {
        const threadView = await loadThreadForFocus(
          api, topic, threadId, options?.focusObjectId ?? null, isCurrent
        );
        if (!threadView) return;
        const nextFocusedObjectId =
          options?.focusObjectId &&
          threadView.items.some((item) => item.object_id === options.focusObjectId)
            ? options.focusObjectId
            : null;
        if (options?.normalizeOnEmpty && threadView.items.length === 0) {
          startTransition(() => {
            setSelectedThread(null);
            setFocusedObjectId(null);
            setSelectedAuthorPubkey(null);
            setSelectedAuthor(null);
            setAuthorError(null);
            setDirectMessagePaneOpen(false);
            setSelectedDirectMessagePeerPubkey(null);
            setDirectMessageError(null);
            setSelectedLiveSessionId(null);
            setSelectedGameRoomId(null);
          });
          syncRoute('replace', {
            activeTopic: topic,
            primarySection: 'timeline',
            timelineView: 'feed',
            directMessagePaneOpen: false,
            focusedObjectId: null,
            selectedGameRoomId: null,
            selectedAuthorPubkey: null,
            selectedLiveSessionId: null,
            selectedThread: null,
          });
          return;
        }
        startTransition(() => {
          if (hasChannelOverride) {
            setTimelineScopeByTopic(setRecordEntry(topic, privateTimelineScope(channelId)));
            setComposeChannelByTopic(setRecordEntry(topic, privateComposeTarget(channelId)));
          }
          setSelectedThread(threadId);
          setFocusedObjectId(nextFocusedObjectId);
          if (nextFocusedObjectId) setThreadFocusRequestId((current) => current + 1);
          setThreadsById(setRecordEntry(threadId, threadView.items));
          setThreadNextCursorById(setRecordEntry(threadId, threadView.next_cursor ?? null));
          setThreadUnavailableById(setRecordEntry(threadId, threadView.unavailable_count ?? 0));
          setSelectedAuthorPubkey(null);
          setSelectedAuthor(null);
          setAuthorError(null);
          setDirectMessagePaneOpen(false);
          setSelectedDirectMessagePeerPubkey(null);
          setDirectMessageError(null);
          setSelectedLiveSessionId(null);
          setSelectedGameRoomId(null);
          setError(null);
        });
        const scope = { topicId: topic, channelId };
        openWorkspaceColumn(
          'thread',
          scope,
          threadId,
          options?.parentColumnId ?? storeApi.getState().workspaceState.activeColumnId
        );
        syncRoute(options?.historyMode ?? 'push', {
          activeTopic: topic,
          primarySection: 'timeline',
          timelineView: 'feed',
          directMessagePaneOpen: false,
          focusedObjectId: nextFocusedObjectId,
          selectedGameRoomId: null,
          selectedAuthorPubkey: null,
          selectedLiveSessionId: null,
          selectedThread: threadId,
          ...channelRouteOverrides,
        });
      } catch (threadError) {
        if (!isCurrent()) return;
        const nextError =
          threadError instanceof Error
            ? threadError.message
            : translate('common:errors.failedToLoadThread');
        setError(nextError);
        if (options?.normalizeOnEmpty) {
          startTransition(() => {
            setSelectedThread(null);
            setFocusedObjectId(null);
            setSelectedAuthorPubkey(null);
            setSelectedAuthor(null);
            setAuthorError(null);
            setDirectMessagePaneOpen(false);
            setSelectedDirectMessagePeerPubkey(null);
            setDirectMessageError(null);
            setSelectedLiveSessionId(null);
            setSelectedGameRoomId(null);
          });
          syncRoute('replace', {
            activeTopic: topic,
            primarySection: 'timeline',
            timelineView: 'feed',
            directMessagePaneOpen: false,
            focusedObjectId: null,
            selectedGameRoomId: null,
            selectedAuthorPubkey: null,
            selectedLiveSessionId: null,
            selectedThread: null,
          });
        }
      }
    },
    [
      activeTopic,
      api,
      setAuthorError,
      setComposeChannelByTopic,
      setDirectMessageError,
      setDirectMessagePaneOpen,
      setError,
      setFocusedObjectId,
      setThreadFocusRequestId,
      setSelectedAuthor,
      setSelectedAuthorPubkey,
      setSelectedDirectMessagePeerPubkey,
      setSelectedGameRoomId,
      setSelectedLiveSessionId,
      setSelectedThread,
      openWorkspaceColumn,
      storeApi,
      setThreadsById,
      setThreadNextCursorById,
      setThreadUnavailableById,
      setTimelineScopeByTopic,
      syncRoute,
      translate,
    ]
  );

  const openAuthorDetail = useCallback(
    async (authorPubkey: string, options?: OpenAuthorOptions) => {
      try {
        const socialView = await api.getAuthorSocialView(authorPubkey);
        const nextThreadId = options?.fromThread ? (options.threadId ?? selectedThread) : null;
        const nextDirectMessagePeerPubkey = options?.preserveDirectMessageContext
          ? options.directMessagePeerPubkey ?? selectedDirectMessagePeerPubkey ?? null
          : null;
        setSelectedAuthorPubkey(authorPubkey);
        setSelectedAuthor(socialView);
        setKnownAuthorsByPubkey((current) => mergeKnownAuthors(current, [socialView]));
        setSelectedAuthorTimeline([]);
        setAuthorError(null);
        if (options?.preserveDirectMessageContext) {
          setDirectMessagePaneOpen(true);
          setSelectedDirectMessagePeerPubkey(nextDirectMessagePeerPubkey);
          setDirectMessageError(null);
        } else {
          setDirectMessagePaneOpen(false);
          setSelectedDirectMessagePeerPubkey(null);
          setDirectMessageError(null);
        }
        if (!options?.fromThread) {
          setSelectedThread(null);
          setFocusedObjectId(null);
        }
        syncRoute(options?.historyMode ?? 'push', {
          primarySection: options?.preserveDirectMessageContext ? 'messages' : 'timeline',
          timelineView: options?.preserveDirectMessageContext ? undefined : 'feed',
          focusedObjectId: options?.fromThread ? undefined : null,
          selectedThread: nextThreadId,
          selectedAuthorPubkey: authorPubkey,
          selectedDirectMessagePeerPubkey: options?.preserveDirectMessageContext
            ? nextDirectMessagePeerPubkey
            : undefined,
        });
        const currentState = storeApi.getState();
        const explicitParentColumn = options?.parentColumnId
          ? currentState.workspaceState.columns.find(
              (column) => column.id === options.parentColumnId
            )
          : undefined;
        const scope = explicitParentColumn?.scope ?? activeWorkspaceScope(currentState.workspaceState);
        const parentColumnId = options?.parentColumnId ?? (options?.fromThread && nextThreadId
          ? columnIdentityId('thread', scope, nextThreadId)
          : options?.preserveDirectMessageContext && nextDirectMessagePeerPubkey
            ? columnIdentityId('conversation', scope, nextDirectMessagePeerPubkey)
            : currentState.workspaceState.columns.find(
                (column) =>
                  column.kind === 'timeline' &&
                  column.scope?.topicId === scope.topicId &&
                  column.scope.channelId === scope.channelId
              )?.id ?? currentState.workspaceState.activeColumnId);
        openWorkspaceColumn('profile', scope, authorPubkey, parentColumnId);
      } catch (detailError) {
        const nextError =
          detailError instanceof Error
            ? detailError.message
            : translate('common:errors.failedToLoadAuthor');
        setAuthorError(nextError);
        if (options?.normalizeOnError) {
          setSelectedAuthorPubkey(null);
          setSelectedAuthor(null);
          setSelectedAuthorTimeline([]);
          if (!options?.fromThread) {
            setSelectedThread(null);
            setFocusedObjectId(null);
          }
          syncRoute('replace', {
            primarySection: options?.preserveDirectMessageContext ? 'messages' : 'timeline',
            timelineView: options?.preserveDirectMessageContext ? undefined : 'feed',
            focusedObjectId: options?.fromThread ? undefined : null,
            selectedThread: options?.fromThread ? (options.threadId ?? selectedThread) : null,
            selectedAuthorPubkey: null,
            selectedDirectMessagePeerPubkey: options?.preserveDirectMessageContext
              ? options.directMessagePeerPubkey ?? selectedDirectMessagePeerPubkey ?? null
              : undefined,
          });
        }
      }
    },
    [
      api,
      selectedDirectMessagePeerPubkey,
      selectedThread,
      setAuthorError,
      setDirectMessageError,
      setDirectMessagePaneOpen,
      setKnownAuthorsByPubkey,
      setSelectedAuthor,
      setSelectedAuthorPubkey,
      setSelectedAuthorTimeline,
      setSelectedDirectMessagePeerPubkey,
      setFocusedObjectId,
      setSelectedThread,
      openWorkspaceColumn,
      storeApi,
      syncRoute,
      translate,
    ]
  );

  const focusPrimarySection = useCallback(
    (section: PrimarySection, options?: { timelineView?: TimelineWorkspaceView }) => {
      const currentState = storeApi.getState();
      const scope = activeWorkspaceScope(currentState.workspaceState);
      const parentColumnId = currentState.workspaceState.activeColumnId;
      const kindBySection: Record<PrimarySection, ColumnKind> = {
        timeline: 'timeline',
        explore: 'explore',
        live: 'stream',
        game: 'metaverse',
        messages: 'messages',
        profile: 'profile',
        notifications: 'notifications',
      };
      const kind = kindBySection[section];
      openWorkspaceColumn(kind, scope, undefined, section === 'timeline' ? undefined : parentColumnId);
      if (section === 'timeline' && options?.timelineView) {
        const columnId = columnIdentityId('timeline', scope);
        setWorkspaceState((current) => setColumnTimelineView(current, columnId, options.timelineView!));
      }
      setShellChromeState((current) => ({
        ...current,
        profileMode: section === 'profile' ? 'overview' : current.profileMode,
        profileConnectionsView: section === 'profile' ? 'following' : current.profileConnectionsView,
      }));
      setSelectedThread(null);
      setFocusedObjectId(null);
      setSelectedAuthorPubkey(null);
      setSelectedAuthor(null);
      setAuthorError(null);
      setDirectMessagePaneOpen(section === 'messages');
      setSelectedDirectMessagePeerPubkey(null);
      setDirectMessageError(null);
      syncRoute('push', {
        primarySection: section,
        focusedObjectId: null,
        profileMode: section === 'profile' ? 'overview' : undefined,
        profileConnectionsView: section === 'profile' ? 'following' : undefined,
        selectedAuthorPubkey: null,
        selectedDirectMessagePeerPubkey: null,
        selectedThread: null,
        ...(options?.timelineView ? { timelineView: options.timelineView } : {}),
      });
    },
    [
      openWorkspaceColumn,
      setAuthorError,
      setDirectMessageError,
      setDirectMessagePaneOpen,
      setFocusedObjectId,
      setSelectedAuthor,
      setSelectedAuthorPubkey,
      setSelectedDirectMessagePeerPubkey,
      setSelectedThread,
      setShellChromeState,
      setWorkspaceState,
      storeApi,
      syncRoute,
    ]
  );

  const toggleNotificationsSection = useCallback(() => {
    const currentUrl = `${resolvedRouteLocation.pathname}${resolvedRouteLocation.search}`;
    if (routeSection === 'notifications') {
      if (lastNonNotificationsRoute) {
        pendingRouteUrlRef.current = lastNonNotificationsRoute;
        navigate(lastNonNotificationsRoute, { replace: false });
        return;
      }
      focusPrimarySection('timeline');
      return;
    }
    setLastNonNotificationsRoute(currentUrl);
    focusPrimarySection('notifications');
  }, [
    focusPrimarySection,
    lastNonNotificationsRoute,
    navigate,
    pendingRouteUrlRef,
    resolvedRouteLocation.pathname,
    resolvedRouteLocation.search,
    routeSection,
    setLastNonNotificationsRoute,
  ]);

  const focusTimelineView = useCallback(
    (view: TimelineWorkspaceView) => {
      const currentState = storeApi.getState();
      const activeColumn = activeWorkspaceColumn(currentState.workspaceState);
      if (activeColumn.kind === 'timeline') {
        setWorkspaceState((current) => setColumnTimelineView(current, activeColumn.id, view));
      }
      if (view === 'bookmarks') {
        setSelectedThread(null);
        setFocusedObjectId(null);
        setSelectedAuthorPubkey(null);
        setSelectedAuthor(null);
        setSelectedAuthorTimeline([]);
        setAuthorError(null);
        setDirectMessagePaneOpen(false);
        setSelectedDirectMessagePeerPubkey(null);
        setDirectMessageError(null);
      }
      syncRoute('push', {
        primarySection: 'timeline',
        timelineView: view,
        focusedObjectId: view === 'bookmarks' ? null : undefined,
        selectedAuthorPubkey: view === 'bookmarks' ? null : undefined,
        selectedThread: view === 'bookmarks' ? null : undefined,
        selectedDirectMessagePeerPubkey: view === 'bookmarks' ? null : undefined,
      });
    },
    [
      setAuthorError,
      setDirectMessageError,
      setDirectMessagePaneOpen,
      setFocusedObjectId,
      setSelectedAuthor,
      setSelectedAuthorPubkey,
      setSelectedAuthorTimeline,
      setSelectedDirectMessagePeerPubkey,
      setSelectedThread,
      setWorkspaceState,
      storeApi,
      syncRoute,
    ]
  );

  const closeAuthorPane = useCallback(() => {
    const currentState = storeApi.getState();
    const profileColumn = currentState.workspaceState.columns.find(
      (column) =>
        column.kind === 'profile' &&
        column.entityId === currentState.selectedAuthorPubkey
    );
    setSelectedAuthorPubkey(null);
    setSelectedAuthor(null);
    setSelectedAuthorTimeline([]);
    setAuthorError(null);
    if (profileColumn && currentState.workspaceState.columns.length > 1) {
      setWorkspaceState(closeColumn(currentState.workspaceState, profileColumn.id));
    }
    syncRoute('replace', {
      selectedAuthorPubkey: null,
    });
  }, [setAuthorError, setSelectedAuthor, setSelectedAuthorTimeline, setSelectedAuthorPubkey, setWorkspaceState, storeApi, syncRoute]);

  const closeThreadPane = useCallback(() => {
    setSelectedThread(null);
    setFocusedObjectId(null);
    setSelectedAuthorPubkey(null);
    setSelectedAuthor(null);
    setSelectedAuthorTimeline([]);
    setAuthorError(null);
    const currentState = storeApi.getState();
    const activeColumn = activeWorkspaceColumn(currentState.workspaceState);
    if (activeColumn.kind === 'thread' && currentState.workspaceState.columns.length > 1) {
      setWorkspaceState(closeColumn(currentState.workspaceState, activeColumn.id));
    }
    syncRoute('replace', {
      focusedObjectId: null,
      selectedThread: null,
      selectedAuthorPubkey: null,
    });
  }, [
    setAuthorError,
    setFocusedObjectId,
    setSelectedAuthor,
    setSelectedAuthorPubkey,
    setSelectedAuthorTimeline,
    setSelectedThread,
    setWorkspaceState,
    storeApi,
    syncRoute,
  ]);

  const openDirectMessageList = useCallback(
    (historyMode: 'push' | 'replace' = 'push') => {
      setSelectedThread(null);
      setFocusedObjectId(null);
      setSelectedAuthorPubkey(null);
      setSelectedAuthor(null);
      setSelectedAuthorTimeline([]);
      setAuthorError(null);
      const currentState = storeApi.getState();
      const scope = activeWorkspaceScope(currentState.workspaceState);
      openWorkspaceColumn('messages', scope, undefined, currentState.workspaceState.activeColumnId);
      setDirectMessagePaneOpen(true);
      setSelectedDirectMessagePeerPubkey(null);
      setDirectMessageError(null);
      syncRoute(historyMode, {
        primarySection: 'messages',
        focusedObjectId: null,
        selectedAuthorPubkey: null,
        selectedDirectMessagePeerPubkey: null,
        selectedThread: null,
      });
    },
    [
      setAuthorError,
      setDirectMessageError,
      setDirectMessagePaneOpen,
      setFocusedObjectId,
      setSelectedAuthor,
      setSelectedAuthorPubkey,
      setSelectedAuthorTimeline,
      setSelectedDirectMessagePeerPubkey,
      setSelectedThread,
      openWorkspaceColumn,
      storeApi,
      syncRoute,
    ]
  );

  const openOwnProfileColumn = useCallback(() => {
    const state = storeApi.getState();
    const own = state.workspaceState.columns.filter((column) => column.kind === 'profile' && (!column.entityId || column.entityId === state.syncStatus.local_author_pubkey));
    const existing = own.find((column) => column.id === state.workspaceState.activeColumnId) ?? own[0];
    const scope = existing?.scope ?? activeWorkspaceScope(state.workspaceState);
    const id = existing?.id ?? columnIdentityId('profile', scope);
    if (existing) setWorkspaceState((current) => activateColumn(current, id));
    else openWorkspaceColumn('profile', scope);
    setShellChromeState((current) => ({ ...current, profileMode: 'overview' }));
    syncRoute('push', { primarySection: 'profile', profileMode: 'overview', activeTopic: scope?.topicId, selectedAuthorPubkey: existing?.entityId ?? null });
    scheduleAnimationFrame(() => {
      const column = document.querySelector<HTMLElement>(`[data-column-id="${CSS.escape(id)}"]`);
      column?.scrollIntoView?.({ block: 'nearest', inline: 'nearest' });
      column?.focus({ preventScroll: true });
    });
  }, [storeApi, setWorkspaceState, openWorkspaceColumn, setShellChromeState, syncRoute, scheduleAnimationFrame]);

  const openProfileOverview = useCallback(() => {
    setShellChromeState((current) => ({
      ...current,
      profileMode: 'overview',
    }));
    syncRoute('push', {
      primarySection: 'profile',
      profileMode: 'overview',
    });
  }, [setShellChromeState, syncRoute]);

  const openProfileEditor = useCallback(() => {
    setShellChromeState((current) => ({
      ...current,
      profileMode: 'edit',
    }));
    syncRoute('push', {
      primarySection: 'profile',
      profileMode: 'edit',
    });
  }, [setShellChromeState, syncRoute]);

  const openProfileConnections = useCallback(
    (view: ProfileConnectionsView = 'following') => {
      setShellChromeState((current) => ({
        ...current,
        profileMode: 'connections',
        profileConnectionsView: view,
      }));
      syncRoute('push', {
        primarySection: 'profile',
        profileMode: 'connections',
        profileConnectionsView: view,
      });
    },
    [setShellChromeState, syncRoute]
  );

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') {
        return;
      }
      // Issue #765: Radix の dismissable layer は Escape を document capture で
      // preventDefault して Dialog を閉じる。その場合はここで cascade せず、
      // Dialog close と selection 解除を分離する。
      if (event.defaultPrevented) {
        return;
      }
      // Composer の textarea など editable 要素での Escape は selection を閉じない。
      if (isEditableTarget(event.target)) {
        return;
      }
      if (shellChromeState.settingsOpen) {
        event.preventDefault();
        setSettingsOpen(false, true);
        return;
      }
      if (selectedAuthorPubkey) {
        event.preventDefault();
        closeAuthorPane();
        return;
      }
      if (selectedThread) {
        event.preventDefault();
        closeThreadPane();
        return;
      }
    };

    window.addEventListener('keydown', handleKeyDown);
    return () => {
      window.removeEventListener('keydown', handleKeyDown);
    };
  }, [
    closeAuthorPane,
    closeThreadPane,
    selectedAuthorPubkey,
    selectedThread,
    setSettingsOpen,
    shellChromeState.settingsOpen,
  ]);

  useRouteSynchronization({
    loadTopics,
    lastObservedRouteUrlRef,
    navigate,
    openAuthorDetail,
    openDirectMessagePane,
    openThread,
    pendingRouteUrlRef,
    resolvedRouteLocation,
    routeSection,
    scheduleAnimationFrame,
    state,
    syncRoute,
    storeApi,
  });

  return {
    routeSection,
    syncRoute,
    setSettingsOpen,
    focusPrimarySection,
    toggleNotificationsSection,
    focusTimelineView,
    closeAuthorPane,
    closeThreadPane,
    openDirectMessageList,
    openDirectMessagePane,
    openThread,
    openAuthorDetail,
    openProfileOverview,
    openOwnProfileColumn,
    openProfileEditor,
    openProfileConnections,
  };
}
