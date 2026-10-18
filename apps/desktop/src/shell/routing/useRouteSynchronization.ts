import { setRecordEntry } from '@/shell/stateUpdates';
import { useEffect, useRef, type MutableRefObject } from 'react';
import type { NavigateFunction } from 'react-router-dom';

import type { PrimarySection } from '@/components/shell/types';
import { isPublicChannelId } from '@/lib/publicChannels';
import {
  PRIMARY_SECTION_PATHS,
  isProfileConnectionsView,
  isSettingsSection,
  parseHashRouteLocation,
  parseLegacyRequestedChannel,
  parseShellRouteState,
  parsePrimarySectionPath,
  type DesktopShellRouteOverrides,
  type HashRouteLocation,
  type OpenAuthorOptions,
  type OpenThreadOptions,
} from '@/shell/routes';
import type {
  DesktopShellState,
  DesktopShellStateValue,
  DesktopShellStoreApi,
} from '@/shell/store';
import { timelineStorageKeyForChannel } from '@/shell/store';
import {
  isHex64,
  channelComposeTarget,
  channelTimelineScope,
} from '@/shell/presentation';
import { selectShellRoutingSlice } from '@/shell/storeSelectors';
import {
  activeWorkspaceColumn,
  activeWorkspaceScope,
  openTransientColumn,
  timelineColumnIdForScope,
} from '@/shell/slices/workspace';
import { workspaceForRoute } from '@/shell/routing/routeWorkspaceProjection';

type OpenDirectMessagePaneOptions = {
  historyMode?: 'push' | 'replace';
  normalizeOnError?: boolean;
  preserveAuthorPane?: boolean;
  preservedAuthorPubkey?: string | null;
};

type UseRouteSynchronizationArgs = {
  loadTopics: (topics: string[], activeTopic: string, currentThread: string | null) => Promise<void>;
  lastObservedRouteUrlRef: MutableRefObject<string>;
  navigate: NavigateFunction;
  openAuthorDetail: (authorPubkey: string, options?: OpenAuthorOptions) => Promise<void>;
  openDirectMessagePane: (
    peerPubkey: string,
    options?: OpenDirectMessagePaneOptions
  ) => Promise<void>;
  openThread: (threadId: string, options?: OpenThreadOptions) => Promise<void>;
  pendingRouteUrlRef: MutableRefObject<string | null>;
  resolvedRouteLocation: HashRouteLocation;
  routeSection: PrimarySection;
  scheduleAnimationFrame: (callback: () => void) => void;
  // 読むフィールドだけを要求する(全ストア購読を強制しない。WP-H6 PR2)。
  state: ReturnType<typeof selectShellRoutingSlice>;
  syncRoute: (mode?: 'push' | 'replace', overrides?: DesktopShellRouteOverrides) => void;
  storeApi: DesktopShellStoreApi;
};

export function useRouteSynchronization({
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
}: UseRouteSynchronizationArgs) {
  const routeProjectionInitializedRef = useRef(false);
  const {
    activeTopic,
    directMessagePaneOpen,
    focusedObjectId,
    gamePanelStateByScopeKey,
    gameRoomsByScopeKey,
    joinedChannelsByTopic,
    channelPanelStateByTopic,
    livePanelStateByScopeKey,
    liveSessionsByScopeKey,
    selectedAuthor,
    selectedAuthorPubkey,
    selectedChannelIdByTopic,
    selectedDirectMessagePeerPubkey,
    selectedGameRoomId,
    selectedLiveSessionId,
    selectedThread,
    shellChromeState,
    threadsById,
    trackedTopics,
  } = state;

  useEffect(() => {
    const setField = <K extends keyof DesktopShellState>(
      key: K,
      value: DesktopShellStateValue<K>
    ) => {
      storeApi.getState().setField(key, value);
    };
    const setActiveTopic = (topicId: string) => {
      const scope = { topicId, channelId: null };
      setField('workspaceState', (current) =>
        openTransientColumn(current, {
          id: timelineColumnIdForScope(current, scope),
          kind: 'timeline',
          scope,
          pinned: false,
        })
      );
    };
    const setAuthorError = (value: DesktopShellStateValue<'authorError'>) =>
      setField('authorError', value);
    const setComposeChannelByTopic = (value: DesktopShellStateValue<'composeChannelByTopic'>) =>
      setField('composeChannelByTopic', value);
    const setDirectMessageError = (value: DesktopShellStateValue<'directMessageError'>) =>
      setField('directMessageError', value);
    const setDirectMessagePaneOpen = (
      value: DesktopShellStateValue<'directMessagePaneOpen'>
    ) => setField('directMessagePaneOpen', value);
    const setFocusedObjectId = (value: DesktopShellStateValue<'focusedObjectId'>) =>
      setField('focusedObjectId', value);
    const setLastNonNotificationsRoute = (
      value: DesktopShellStateValue<'lastNonNotificationsRoute'>
    ) => setField('lastNonNotificationsRoute', value);
    const setSelectedAuthor = (value: DesktopShellStateValue<'selectedAuthor'>) =>
      setField('selectedAuthor', value);
    const setSelectedAuthorPubkey = (
      value: DesktopShellStateValue<'selectedAuthorPubkey'>
    ) => setField('selectedAuthorPubkey', value);
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
      setField('workspaceState', (current) =>
        openTransientColumn(current, {
          id: timelineColumnIdForScope(current, scope),
          kind: 'timeline',
          scope,
          pinned: false,
        })
      );
    };
    const setSelectedDirectMessagePeerPubkey = (
      value: DesktopShellStateValue<'selectedDirectMessagePeerPubkey'>
    ) => setField('selectedDirectMessagePeerPubkey', value);
    const setSelectedGameRoomId = (value: DesktopShellStateValue<'selectedGameRoomId'>) =>
      setField('selectedGameRoomId', value);
    const setSelectedLiveSessionId = (value: DesktopShellStateValue<'selectedLiveSessionId'>) =>
      setField('selectedLiveSessionId', value);
    const setSelectedThread = (value: DesktopShellStateValue<'selectedThread'>) =>
      setField('selectedThread', value);
    const setShellChromeState = (value: DesktopShellStateValue<'shellChromeState'>) =>
      setField('shellChromeState', value);
    const setTimelineScopeByTopic = (value: DesktopShellStateValue<'timelineScopeByTopic'>) =>
      setField('timelineScopeByTopic', value);

    const currentUrl = `${resolvedRouteLocation.pathname}${resolvedRouteLocation.search}`;
    const routeChanged = lastObservedRouteUrlRef.current !== currentUrl;
    // 自分の navigate(pending)が router に commit される前に goBack 等の外部遷移へ
    // 追い越されると、location 文字列は last observed と同一のまま routeChanged が
    // 二度と立たず route 消費が止まる(React Router v7 は遷移レンダーを transition で
    // 行うため、push のレンダーは後続遷移に破棄されうる)。hash は navigate / popstate と
    // 同期して即時更新されるため、実 hash を pending と突き合わせて生死を判定する。
    let pendingRouteSuperseded = false;
    if (pendingRouteUrlRef.current && pendingRouteUrlRef.current !== currentUrl) {
      const liveLocation =
        typeof window === 'undefined' ? null : parseHashRouteLocation(window.location.hash);
      const liveUrl = liveLocation ? `${liveLocation.pathname}${liveLocation.search}` : null;
      if (liveUrl !== null && liveUrl !== pendingRouteUrlRef.current) {
        // pending の push は追い越し済み。settle 先が現在の location なら投影をやり直し、
        // 第三の URL なら破棄だけしてそのレンダーの到着に委ねる。
        pendingRouteSuperseded = liveUrl === currentUrl;
        if (!pendingRouteSuperseded && !routeChanged) {
          pendingRouteUrlRef.current = null;
          return;
        }
      } else {
        // pending はまだ生きている(実 hash が pending のまま)。先に commit された古い push を
        // 投影すると、後から操作した Column の active を奪うため、pending の到着を待つ(Issue #1053)。
        return;
      }
      pendingRouteUrlRef.current = null;
    }
    pendingRouteUrlRef.current = null;
    lastObservedRouteUrlRef.current = currentUrl;
    if (routeSection !== 'notifications') {
      setLastNonNotificationsRoute(currentUrl);
    }

    if (!parsePrimarySectionPath(resolvedRouteLocation.pathname)) {
      navigate(`${PRIMARY_SECTION_PATHS.timeline}${resolvedRouteLocation.search}`, {
        replace: true,
      });
      return;
    }

    const {
      requestedTopic,
      requestedChannel: requestedChannelParam,
      requestedTimelineView,
      requestedTimelineScope: requestedTimelineScopeValue,
      requestedComposeTarget: requestedComposeTargetValue,
      requestedSettingsSection,
      requestedContext,
      requestedProfileMode,
      requestedConnectionsView,
      requestedThreadId,
      requestedFocusObjectId,
      requestedAuthorPubkey,
      requestedPeerPubkey,
      requestedSessionId,
      requestedLivePlace,
      requestedRoomId,
    } = parseShellRouteState({
      pathname: resolvedRouteLocation.pathname,
      search: resolvedRouteLocation.search,
    });

    let nextTopic = activeTopic;
    let shouldReload = false;
    // URL の section が解決済み routeSection と食い違う場合(developer mode off の
    // live/game fallback など)は URL を routeSection 側へ正規化する。
    let shouldNormalize =
      parsePrimarySectionPath(resolvedRouteLocation.pathname) !== routeSection;
    let normalizedSelectedThread: string | null = selectedThread;
    let normalizedFocusedObjectId: string | null = focusedObjectId;
    let normalizedSelectedAuthorPubkey: string | null = selectedAuthorPubkey;
    let normalizedSelectedDirectMessagePeerPubkey: string | null =
      selectedDirectMessagePeerPubkey;
    let normalizedSelectedLiveSessionId: string | null = selectedLiveSessionId;
    let normalizedSelectedGameRoomId: string | null = selectedGameRoomId;

    let requestedTopicIsTracked = false;
    if (requestedTopic) {
      if (trackedTopics.includes(requestedTopic)) {
        requestedTopicIsTracked = true;
        if (requestedTopic !== activeTopic) {
          nextTopic = requestedTopic;
          setActiveTopic(requestedTopic);
          shouldReload = true;
        }
      } else {
        shouldNormalize = true;
      }
    } else {
      shouldNormalize = true;
    }

    const nextTimelineView =
      routeSection === 'timeline' && requestedTimelineView === 'bookmarks' ? 'bookmarks' : 'feed';
    const joinedChannelsForTopic = joinedChannelsByTopic[nextTopic] ?? [];
    const channelPanelState = channelPanelStateByTopic[nextTopic];
    const currentSelectedChannelIdForTopic = selectedChannelIdByTopic[nextTopic] ?? null;
    const allowChannelRouteParam =
      routeSection !== 'messages' && routeSection !== 'notifications';
    let nextSelectedChannelId = currentSelectedChannelIdForTopic;
    if (allowChannelRouteParam && nextTimelineView !== 'bookmarks') {
      nextSelectedChannelId =
        requestedTopic && !requestedTopicIsTracked ? null : requestedChannelParam;
      if (!nextSelectedChannelId && (!requestedTopic || requestedTopicIsTracked)) {
        nextSelectedChannelId = parseLegacyRequestedChannel(
          requestedTimelineScopeValue,
          requestedComposeTargetValue
        );
      }
    } else if (requestedChannelParam) {
      shouldNormalize = true;
    }
    if (requestedTimelineScopeValue || requestedComposeTargetValue) {
      shouldNormalize = true;
    }
    // 参加の確認は private channel だけ（公開スレッドは参加が無く、ID から読める。#1840）。
    const unjoinedPrivateChannel = Boolean(
      nextSelectedChannelId &&
        !isPublicChannelId(nextSelectedChannelId) &&
        !joinedChannelsForTopic.some((channel) => channel.channel_id === nextSelectedChannelId)
    );
    if (
      nextTimelineView !== 'bookmarks' &&
      unjoinedPrivateChannel &&
      channelPanelState?.status === 'ready'
    ) {
      shouldNormalize = true;
      nextSelectedChannelId = null;
    }
    const channelRoutePendingValidation =
      nextTimelineView !== 'bookmarks' &&
      unjoinedPrivateChannel &&
      channelPanelState?.status !== 'ready';
    const routeScopeKey = timelineStorageKeyForChannel(nextTopic, nextSelectedChannelId);
    const gameRoomsForTopic = gameRoomsByScopeKey[routeScopeKey] ?? [];
    const gamePanelState = gamePanelStateByScopeKey[routeScopeKey];

    if (
      currentSelectedChannelIdForTopic !== nextSelectedChannelId &&
      !channelRoutePendingValidation
    ) {
      // 公開スレッドの一覧と詳細の列は、下の route の反映が開く(詳細は一覧の列を親にする。#1840)。
      if (requestedContext !== 'publicThreads' && !isPublicChannelId(nextSelectedChannelId)) {
        setSelectedChannelIdByTopic(setRecordEntry(nextTopic, nextSelectedChannelId));
      }
      setTimelineScopeByTopic(setRecordEntry(nextTopic, channelTimelineScope(nextSelectedChannelId)));
      setComposeChannelByTopic(setRecordEntry(nextTopic, channelComposeTarget(nextSelectedChannelId)));
      shouldReload = true;
    }

    if (requestedContext === 'dm' && routeSection !== 'messages') {
      scheduleAnimationFrame(() => {
        syncRoute('replace', {
          activeTopic: nextTopic,
          primarySection: 'messages',
          selectedAuthorPubkey: null,
          selectedDirectMessagePeerPubkey:
            requestedPeerPubkey && isHex64(requestedPeerPubkey) ? requestedPeerPubkey : null,
          selectedThread: null,
        });
      });
      return;
    }

    const nextSettingsOpen = isSettingsSection(requestedSettingsSection);
    const nextSettingsResolvedSection = isSettingsSection(requestedSettingsSection)
      ? requestedSettingsSection
      : shellChromeState.activeSettingsSection;
    const nextProfileMode =
      routeSection === 'profile'
        ? requestedProfileMode === 'edit'
          ? 'edit'
          : requestedProfileMode === 'connections'
            ? 'connections'
            : 'overview'
        : 'overview';
    const nextProfileConnectionsView =
      routeSection === 'profile' && requestedProfileMode === 'connections'
        ? isProfileConnectionsView(requestedConnectionsView)
          ? requestedConnectionsView
          : 'following'
        : shellChromeState.profileConnectionsView;

    if (
      shellChromeState.activeSettingsSection !== nextSettingsResolvedSection ||
      shellChromeState.settingsOpen !== nextSettingsOpen ||
      shellChromeState.profileMode !== nextProfileMode ||
      shellChromeState.profileConnectionsView !== nextProfileConnectionsView
    ) {
      setShellChromeState((current) => ({
        ...current,
        activeSettingsSection: nextSettingsResolvedSection,
        settingsOpen: nextSettingsOpen,
        profileMode: nextProfileMode,
        profileConnectionsView: nextProfileConnectionsView,
      }));
    }

    if (requestedTimelineView && requestedTimelineView !== 'bookmarks') {
      shouldNormalize = true;
    }
    if (requestedTimelineView && routeSection !== 'timeline') {
      shouldNormalize = true;
    }
    if (requestedSettingsSection && !isSettingsSection(requestedSettingsSection)) {
      shouldNormalize = true;
    }
    if (
      requestedProfileMode &&
      requestedProfileMode !== 'edit' &&
      requestedProfileMode !== 'connections'
    ) {
      shouldNormalize = true;
    }
    if (requestedProfileMode && routeSection !== 'profile') {
      shouldNormalize = true;
    }
    if (
      requestedConnectionsView &&
      (requestedProfileMode !== 'connections' ||
        !isProfileConnectionsView(requestedConnectionsView))
    ) {
      shouldNormalize = true;
    }
    if (routeSection === 'messages' && requestedContext) {
      shouldNormalize = true;
    }
    if (
      routeSection === 'notifications' &&
      (requestedTimelineView ||
        requestedChannelParam ||
        requestedContext ||
        requestedProfileMode ||
        requestedConnectionsView ||
        requestedThreadId ||
        requestedFocusObjectId ||
        requestedAuthorPubkey ||
        requestedPeerPubkey ||
        requestedSessionId ||
        requestedRoomId)
    ) {
      shouldNormalize = true;
    }

    if (nextTimelineView === 'bookmarks') {
      normalizedSelectedThread = null;
      normalizedFocusedObjectId = null;
      normalizedSelectedAuthorPubkey = null;
      normalizedSelectedDirectMessagePeerPubkey = null;
      if (requestedContext) {
        shouldNormalize = true;
      }
      if (requestedFocusObjectId || requestedSessionId || requestedRoomId) {
        shouldNormalize = true;
      }
      if (selectedThread) {
        setSelectedThread(null);
        setFocusedObjectId(null);
      }
      if (focusedObjectId) {
        setFocusedObjectId(null);
      }
      if (selectedAuthorPubkey) {
        setSelectedAuthorPubkey(null);
        setSelectedAuthor(null);
        setAuthorError(null);
      }
      if (directMessagePaneOpen) {
        setDirectMessagePaneOpen(false);
      }
      if (selectedDirectMessagePeerPubkey) {
        setSelectedDirectMessagePeerPubkey(null);
      }
      setDirectMessageError(null);
    }
    if (routeSection === 'messages') {
      normalizedSelectedThread = null;
      normalizedFocusedObjectId = null;
      if (requestedThreadId) {
        shouldNormalize = true;
      }
      if (requestedFocusObjectId || requestedSessionId || requestedRoomId) {
        shouldNormalize = true;
      }
      if (selectedThread) {
        setSelectedThread(null);
        setFocusedObjectId(null);
      }
      if (focusedObjectId) {
        setFocusedObjectId(null);
      }
      if (!directMessagePaneOpen) {
        setDirectMessagePaneOpen(true);
      }
      if (!requestedPeerPubkey) {
        normalizedSelectedDirectMessagePeerPubkey = null;
        if (selectedDirectMessagePeerPubkey) {
          setSelectedDirectMessagePeerPubkey(null);
        }
        setDirectMessageError(null);
      } else if (!isHex64(requestedPeerPubkey)) {
        shouldNormalize = true;
        normalizedSelectedDirectMessagePeerPubkey = null;
        if (selectedDirectMessagePeerPubkey) {
          setSelectedDirectMessagePeerPubkey(null);
        }
      } else if (
        requestedPeerPubkey !== selectedDirectMessagePeerPubkey ||
        !directMessagePaneOpen
      ) {
        normalizedSelectedDirectMessagePeerPubkey = requestedPeerPubkey;
        void openDirectMessagePane(requestedPeerPubkey, {
          historyMode: 'replace',
          normalizeOnError: true,
          preserveAuthorPane: requestedAuthorPubkey !== null && isHex64(requestedAuthorPubkey),
          preservedAuthorPubkey:
            requestedAuthorPubkey && isHex64(requestedAuthorPubkey)
              ? requestedAuthorPubkey
              : null,
        });
      } else {
        normalizedSelectedDirectMessagePeerPubkey = requestedPeerPubkey;
      }
      if (!requestedAuthorPubkey) {
        normalizedSelectedAuthorPubkey = null;
        if (selectedAuthorPubkey) {
          setSelectedAuthorPubkey(null);
          setSelectedAuthor(null);
          setAuthorError(null);
        }
      } else if (!isHex64(requestedAuthorPubkey)) {
        shouldNormalize = true;
        normalizedSelectedAuthorPubkey = null;
        if (selectedAuthorPubkey) {
          setSelectedAuthorPubkey(null);
          setSelectedAuthor(null);
          setAuthorError(null);
        }
      } else if (
        requestedAuthorPubkey !== selectedAuthorPubkey ||
        !selectedAuthor ||
        (requestedPeerPubkey ?? null) !== (selectedDirectMessagePeerPubkey ?? null)
      ) {
        normalizedSelectedAuthorPubkey = requestedAuthorPubkey;
        void openAuthorDetail(requestedAuthorPubkey, {
          historyMode: 'replace',
          normalizeOnError: true,
          preserveDirectMessageContext: true,
          directMessagePeerPubkey:
            requestedPeerPubkey && isHex64(requestedPeerPubkey) ? requestedPeerPubkey : null,
        });
      } else {
        normalizedSelectedAuthorPubkey = requestedAuthorPubkey;
      }
    } else if (routeSection === 'notifications') {
      normalizedSelectedThread = null;
      normalizedFocusedObjectId = null;
      normalizedSelectedAuthorPubkey = null;
      normalizedSelectedDirectMessagePeerPubkey = null;
      if (selectedThread) {
        setSelectedThread(null);
        setFocusedObjectId(null);
      }
      if (focusedObjectId) {
        setFocusedObjectId(null);
      }
      if (selectedAuthorPubkey) {
        setSelectedAuthorPubkey(null);
        setSelectedAuthor(null);
        setAuthorError(null);
      }
      if (directMessagePaneOpen || selectedDirectMessagePeerPubkey) {
        setDirectMessagePaneOpen(false);
        setSelectedDirectMessagePeerPubkey(null);
        setDirectMessageError(null);
      }
    } else if (
      routeSection === 'timeline' &&
      nextTimelineView !== 'bookmarks' &&
      requestedContext === 'thread'
    ) {
      normalizedSelectedDirectMessagePeerPubkey = null;
      const threadReadyForNestedAuthor =
        requestedThreadId !== null &&
        requestedThreadId.length > 0 &&
        requestedThreadId === selectedThread &&
        (threadsById[requestedThreadId]?.length ?? 0) > 0;

      if (!requestedThreadId) {
        shouldNormalize = true;
        normalizedSelectedThread = null;
        normalizedFocusedObjectId = null;
        normalizedSelectedAuthorPubkey = null;
        if (selectedThread || selectedAuthorPubkey) {
          setSelectedThread(null);
          setFocusedObjectId(null);
          setSelectedAuthorPubkey(null);
          setSelectedAuthor(null);
          setAuthorError(null);
        }
      } else if (
        requestedThreadId !== selectedThread ||
        (threadsById[requestedThreadId]?.length ?? 0) === 0
      ) {
        normalizedSelectedThread = requestedThreadId;
        normalizedFocusedObjectId = requestedFocusObjectId;
        void openThread(requestedThreadId, {
          focusObjectId: requestedFocusObjectId,
          historyMode: 'replace',
          normalizeOnEmpty: true,
          topic: nextTopic,
        });
      } else {
        normalizedSelectedThread = requestedThreadId;
        if (!requestedFocusObjectId) {
          normalizedFocusedObjectId = null;
          if (focusedObjectId) {
            setFocusedObjectId(null);
          }
        } else if (
          (threadsById[requestedThreadId] ?? []).some(
            (item) => item.object_id === requestedFocusObjectId
          )
        ) {
          normalizedFocusedObjectId = requestedFocusObjectId;
          if (focusedObjectId !== requestedFocusObjectId) {
            setFocusedObjectId(requestedFocusObjectId);
          }
        } else {
          shouldNormalize = true;
          normalizedFocusedObjectId = null;
          if (focusedObjectId) {
            setFocusedObjectId(null);
          }
        }
      }
      if (!requestedAuthorPubkey) {
        normalizedSelectedAuthorPubkey = null;
        if (selectedAuthorPubkey) {
          setSelectedAuthorPubkey(null);
          setSelectedAuthor(null);
          setAuthorError(null);
        }
      } else if (!isHex64(requestedAuthorPubkey)) {
        shouldNormalize = true;
        normalizedSelectedAuthorPubkey = null;
        if (selectedAuthorPubkey) {
          setSelectedAuthorPubkey(null);
          setSelectedAuthor(null);
          setAuthorError(null);
        }
      } else if (!threadReadyForNestedAuthor) {
        normalizedSelectedAuthorPubkey = null;
        if (selectedAuthorPubkey) {
          setSelectedAuthorPubkey(null);
          setSelectedAuthor(null);
          setAuthorError(null);
        }
      } else if (
        requestedAuthorPubkey !== selectedAuthorPubkey ||
        !selectedAuthor ||
        requestedThreadId !== selectedThread
      ) {
        normalizedSelectedAuthorPubkey = requestedAuthorPubkey;
        void openAuthorDetail(requestedAuthorPubkey, {
          fromThread: true,
          historyMode: 'replace',
          normalizeOnError: true,
          threadId: requestedThreadId,
        });
      } else {
        normalizedSelectedAuthorPubkey = requestedAuthorPubkey;
      }
    } else if (
      routeSection === 'timeline' &&
      nextTimelineView !== 'bookmarks' &&
      requestedContext === 'author'
    ) {
      normalizedSelectedThread = null;
      normalizedFocusedObjectId = null;
      normalizedSelectedDirectMessagePeerPubkey = null;
      if (requestedThreadId) {
        shouldNormalize = true;
      }
      if (requestedFocusObjectId || requestedSessionId || requestedRoomId) {
        shouldNormalize = true;
      }
      if (selectedThread) {
        setSelectedThread(null);
        setFocusedObjectId(null);
      }
      if (focusedObjectId) {
        setFocusedObjectId(null);
      }
      if (!requestedAuthorPubkey) {
        shouldNormalize = true;
        normalizedSelectedAuthorPubkey = null;
        if (selectedAuthorPubkey) {
          setSelectedAuthorPubkey(null);
          setSelectedAuthor(null);
          setAuthorError(null);
        }
      } else if (!isHex64(requestedAuthorPubkey)) {
        shouldNormalize = true;
        normalizedSelectedAuthorPubkey = null;
        if (selectedAuthorPubkey) {
          setSelectedAuthorPubkey(null);
          setSelectedAuthor(null);
          setAuthorError(null);
        }
      } else if (requestedAuthorPubkey !== selectedAuthorPubkey || !selectedAuthor) {
        normalizedSelectedAuthorPubkey = requestedAuthorPubkey;
        void openAuthorDetail(requestedAuthorPubkey, {
          historyMode: 'replace',
          normalizeOnError: true,
        });
      } else {
        normalizedSelectedAuthorPubkey = requestedAuthorPubkey;
      }
    } else if (routeSection === 'live') {
      normalizedSelectedThread = null;
      normalizedFocusedObjectId = null;
      normalizedSelectedAuthorPubkey = null;
      normalizedSelectedDirectMessagePeerPubkey = null;
      if (
        requestedContext ||
        requestedThreadId ||
        requestedFocusObjectId ||
        requestedAuthorPubkey ||
        requestedPeerPubkey ||
        requestedRoomId
      ) {
        shouldNormalize = true;
      }
      if (selectedThread) {
        setSelectedThread(null);
        setFocusedObjectId(null);
      }
      if (focusedObjectId) {
        setFocusedObjectId(null);
      }
      if (selectedAuthorPubkey) {
        setSelectedAuthorPubkey(null);
        setSelectedAuthor(null);
        setAuthorError(null);
      }
      if (directMessagePaneOpen || selectedDirectMessagePeerPubkey) {
        setDirectMessagePaneOpen(false);
        setSelectedDirectMessagePeerPubkey(null);
        setDirectMessageError(null);
      }
      normalizedSelectedGameRoomId = null;
      if (selectedGameRoomId) {
        setSelectedGameRoomId(null);
      }
      if (!requestedSessionId) {
        normalizedSelectedLiveSessionId = null;
        if (selectedLiveSessionId) {
          setSelectedLiveSessionId(null);
        }
      } else {
        normalizedSelectedLiveSessionId = requestedSessionId;
        if (selectedLiveSessionId !== requestedSessionId) {
          setSelectedLiveSessionId(requestedSessionId);
        }
      }
    } else if (routeSection === 'game') {
      normalizedSelectedThread = null;
      normalizedFocusedObjectId = null;
      normalizedSelectedAuthorPubkey = null;
      normalizedSelectedDirectMessagePeerPubkey = null;
      if (
        requestedContext ||
        requestedThreadId ||
        requestedFocusObjectId ||
        requestedAuthorPubkey ||
        requestedPeerPubkey ||
        requestedSessionId
      ) {
        shouldNormalize = true;
      }
      if (selectedThread) {
        setSelectedThread(null);
        setFocusedObjectId(null);
      }
      if (focusedObjectId) {
        setFocusedObjectId(null);
      }
      if (selectedAuthorPubkey) {
        setSelectedAuthorPubkey(null);
        setSelectedAuthor(null);
        setAuthorError(null);
      }
      if (directMessagePaneOpen || selectedDirectMessagePeerPubkey) {
        setDirectMessagePaneOpen(false);
        setSelectedDirectMessagePeerPubkey(null);
        setDirectMessageError(null);
      }
      normalizedSelectedLiveSessionId = null;
      if (selectedLiveSessionId) {
        setSelectedLiveSessionId(null);
      }
      if (!requestedRoomId) {
        normalizedSelectedGameRoomId = null;
        if (selectedGameRoomId) {
          setSelectedGameRoomId(null);
        }
      } else {
        normalizedSelectedGameRoomId = requestedRoomId;
        if (selectedGameRoomId !== requestedRoomId) {
          setSelectedGameRoomId(requestedRoomId);
        }
      }
    } else if (
      routeSection === 'timeline' &&
      nextTimelineView !== 'bookmarks' &&
      requestedContext &&
      requestedContext !== 'publicThreads'
    ) {
      shouldNormalize = true;
      normalizedSelectedThread = null;
      normalizedFocusedObjectId = null;
      normalizedSelectedAuthorPubkey = null;
      normalizedSelectedDirectMessagePeerPubkey = null;
      if (selectedThread || selectedAuthorPubkey) {
        setSelectedThread(null);
        setFocusedObjectId(null);
        setSelectedAuthorPubkey(null);
        setSelectedAuthor(null);
        setAuthorError(null);
      }
      if (focusedObjectId) {
        setFocusedObjectId(null);
      }
      if (directMessagePaneOpen || selectedDirectMessagePeerPubkey) {
        setDirectMessagePaneOpen(false);
        setSelectedDirectMessagePeerPubkey(null);
        setDirectMessageError(null);
      }
    } else {
      if (
        requestedThreadId ||
        requestedFocusObjectId ||
        requestedAuthorPubkey ||
        requestedPeerPubkey ||
        requestedSessionId ||
        requestedRoomId
      ) {
        shouldNormalize = true;
      }
      normalizedSelectedThread = null;
      normalizedFocusedObjectId = null;
      normalizedSelectedAuthorPubkey = null;
      normalizedSelectedDirectMessagePeerPubkey = null;
      if (
        selectedThread ||
        focusedObjectId ||
        selectedAuthorPubkey ||
        directMessagePaneOpen ||
        selectedDirectMessagePeerPubkey
      ) {
        setSelectedThread(null);
        setFocusedObjectId(null);
        setSelectedAuthorPubkey(null);
        setSelectedAuthor(null);
        setAuthorError(null);
        setDirectMessagePaneOpen(false);
        setSelectedDirectMessagePeerPubkey(null);
        setDirectMessageError(null);
      }
    }

    const routeScope = {
      topicId: nextTopic,
      channelId: channelRoutePendingValidation
        ? currentSelectedChannelIdForTopic
        : nextSelectedChannelId,
    };
    const selectedRouteGameRoom = normalizedSelectedGameRoomId
      ? gameRoomsForTopic.find((room) => room.room_id === normalizedSelectedGameRoomId)
      : undefined;
    const gameColumnResolutionPending = Boolean(
      routeSection === 'game' &&
        normalizedSelectedGameRoomId &&
        !selectedRouteGameRoom &&
        gamePanelState?.status !== 'ready' &&
        gamePanelState?.status !== 'error'
    );
    // 左側に出す Live の場所は、Live を示す route だけが持つ(#1841)。
    const normalizedLivePlace = routeSection === 'live' && normalizedSelectedLiveSessionId ? requestedLivePlace : null;
    const currentRouteColumn = activeWorkspaceColumn(storeApi.getState().workspaceState);
    const resolvingCurrentGameColumn = Boolean(
      routeSection === 'game' &&
        currentRouteColumn.scope?.topicId === routeScope.topicId &&
        currentRouteColumn.scope?.channelId === routeScope.channelId &&
        (currentRouteColumn.kind === 'game' || currentRouteColumn.kind === 'metaverse') &&
        ((normalizedSelectedGameRoomId &&
          currentRouteColumn.entityId === normalizedSelectedGameRoomId) ||
          (requestedRoomId &&
            !normalizedSelectedGameRoomId &&
            currentRouteColumn.entityId === requestedRoomId))
    );
    if (
      !routeProjectionInitializedRef.current ||
      routeChanged ||
      // 追い越された push の間に store は先行遷移している(thread Column が active 等)ため、
      // location 文字列が不変でも現在 URL の投影をやり直して store を URL に揃える。
      pendingRouteSuperseded ||
      resolvingCurrentGameColumn
    ) {
      routeProjectionInitializedRef.current = true;
      setField('workspaceState', (current) =>
        workspaceForRoute(current, {
          gameColumnResolutionPending,
          isScoreGameRoom: selectedRouteGameRoom?.room_kind === 'score_game',
          nextTimelineView,
          routeScope,
          routeSection,
          selectedAuthorPubkey: normalizedSelectedAuthorPubkey,
          selectedDirectMessagePeerPubkey: normalizedSelectedDirectMessagePeerPubkey,
          selectedGameRoomId: normalizedSelectedGameRoomId,
          selectedLiveSessionId: normalizedSelectedLiveSessionId,
          selectedLivePlace: normalizedLivePlace,
          selectedThread: normalizedSelectedThread,
          publicThreadList: routeSection === 'timeline' && requestedContext === 'publicThreads',
        })
      );
    }
    if (shouldReload) {
      void loadTopics(
        trackedTopics,
        nextTopic,
        requestedContext === 'thread' ? requestedThreadId : null
      ).catch(() => undefined);
    }

    if (shouldNormalize) {
      scheduleAnimationFrame(() => {
        // router がこの URL を描画した後に hash が別の URL へ進んでいれば(戻る等)、古い URL を正規化して新しい URL を
        // 上書きしない。新しい URL は描画されたときに処理する。
        const liveLocation = window.location.hash ? parseHashRouteLocation(window.location.hash) : null;
        if (liveLocation && `${liveLocation.pathname}${liveLocation.search}` !== currentUrl) return;
        syncRoute('replace', {
          activeTopic: nextTopic,
          composeTarget: channelComposeTarget(nextSelectedChannelId),
          focusedObjectId: normalizedFocusedObjectId,
          primarySection: routeSection,
          profileConnectionsView: nextProfileConnectionsView,
          profileMode: nextProfileMode,
          selectedAuthorPubkey: normalizedSelectedAuthorPubkey,
          selectedDirectMessagePeerPubkey: normalizedSelectedDirectMessagePeerPubkey,
          selectedGameRoomId: normalizedSelectedGameRoomId,
          selectedLiveSessionId: normalizedSelectedLiveSessionId,
          ...(normalizedLivePlace ? { selectedLivePlace: normalizedLivePlace } : {}),
          selectedThread: normalizedSelectedThread,
          settingsOpen: nextSettingsOpen,
          settingsSection: nextSettingsResolvedSection,
          timelineScope: channelTimelineScope(nextSelectedChannelId),
          timelineView: nextTimelineView,
          publicThreadList: routeSection === 'timeline' && requestedContext === 'publicThreads',
        });
      });
    }
  }, [
    activeTopic,
    channelPanelStateByTopic,
    directMessagePaneOpen,
    focusedObjectId,
    gamePanelStateByScopeKey,
    gameRoomsByScopeKey,
    joinedChannelsByTopic,
    loadTopics,
    livePanelStateByScopeKey,
    liveSessionsByScopeKey,
    lastObservedRouteUrlRef,
    navigate,
    openAuthorDetail,
    openDirectMessagePane,
    openThread,
    pendingRouteUrlRef,
    // オブジェクト依存が意図: 同一 URL 文字列でも history 遷移ごとに再評価し、
    // 追い越された pending push の検出(上記)を働かせる。
    resolvedRouteLocation,
    routeSection,
    scheduleAnimationFrame,
    selectedAuthor,
    selectedAuthorPubkey,
    selectedChannelIdByTopic,
    selectedDirectMessagePeerPubkey,
    selectedGameRoomId,
    selectedLiveSessionId,
    selectedThread,
    shellChromeState.activeSettingsSection,
    shellChromeState.profileConnectionsView,
    shellChromeState.profileMode,
    shellChromeState.settingsOpen,
    storeApi,
    syncRoute,
    threadsById,
    trackedTopics,
  ]);
}
