import type { PrimarySection } from '@/components/shell/types';
import { isPublicChannelId } from '@/lib/publicChannels';
import {
  activateColumn,
  columnIdentityId,
  openTransientColumn,
  setColumnTimelineView,
  columnIdForScope,
  type ColumnKind,
  type ColumnScope,
  type ColumnTimelineView,
  type WorkspaceState,
} from '@/shell/slices/workspace';

type RouteWorkspaceProjection = {
  gameColumnResolutionPending: boolean;
  isScoreGameRoom: boolean;
  nextTimelineView: ColumnTimelineView;
  routeScope: ColumnScope;
  routeSection: PrimarySection;
  selectedAuthorPubkey: string | null;
  selectedDirectMessagePeerPubkey: string | null;
  selectedGameRoomId: string | null;
  selectedLiveSessionId: string | null;
  // 左側に出す Live の場所(列の scope と違うときだけ。#1841)。
  selectedLivePlace?: ColumnScope | null;
  selectedThread: string | null;
  // topic の公開スレッドの一覧の列(#1840)。
  publicThreadList?: boolean;
};

/// topic の公開スレッドの一覧の列を開く(既にあればそのまま)。
function ensurePublicThreadList(
  state: WorkspaceState,
  topicId: string
): { state: WorkspaceState; id: string } {
  const scope = { topicId, channelId: null };
  const id = columnIdForScope(state, 'publicThreads', scope);
  if (state.columns.some((column) => column.id === id)) return { state, id };
  return { state: openTransientColumn(state, { id, kind: 'publicThreads', scope, pinned: false }), id };
}

export function workspaceForRoute(
  incoming: WorkspaceState,
  projection: RouteWorkspaceProjection
): WorkspaceState {
  const {
    gameColumnResolutionPending,
    isScoreGameRoom,
    nextTimelineView,
    routeScope,
    routeSection,
    selectedAuthorPubkey,
    selectedDirectMessagePeerPubkey,
    selectedGameRoomId,
    selectedLiveSessionId,
    selectedLivePlace,
    selectedThread,
    publicThreadList,
  } = projection;
  if (publicThreadList) {
    const listed = ensurePublicThreadList(incoming, routeScope.topicId);
    return activateColumn(listed.state, listed.id);
  }
  const timelineId = columnIdForScope(incoming, 'timeline', routeScope);
  const ensureTimelineColumn = (current: WorkspaceState) => {
    const withView = setColumnTimelineView(current, timelineId, nextTimelineView);
    if (withView.columns.some((column) => column.id === timelineId)) return withView;
    // 公開スレッドの詳細は、その topic の一覧の列を親にする(直接リンクで開いても一覧へ戻れる。#1840)。
    const listed = isPublicChannelId(routeScope.channelId)
      ? ensurePublicThreadList(withView, routeScope.topicId)
      : null;
    return openTransientColumn(listed?.state ?? withView, {
      id: timelineId,
      kind: 'timeline',
      scope: routeScope,
      pinned: false,
      timelineView: nextTimelineView,
      ...(listed ? { parentColumnId: listed.id } : {}),
    });
  };

  if (selectedAuthorPubkey && selectedDirectMessagePeerPubkey) {
    const profileId = columnIdentityId('profile', routeScope, selectedAuthorPubkey);
    if (incoming.activeColumnId === profileId) return incoming;
    const messagesId = columnIdentityId('messages', routeScope);
    let next = openTransientColumn(incoming, {
      id: messagesId,
      kind: 'messages',
      scope: routeScope,
      pinned: false,
    });
    const conversationId = columnIdentityId(
      'conversation',
      routeScope,
      selectedDirectMessagePeerPubkey
    );
    next = openTransientColumn(next, {
      id: conversationId,
      kind: 'conversation',
      scope: routeScope,
      entityId: selectedDirectMessagePeerPubkey,
      parentColumnId: messagesId,
      pinned: false,
    });
    return openTransientColumn(next, {
      id: profileId,
      kind: 'profile',
      scope: routeScope,
      entityId: selectedAuthorPubkey,
      parentColumnId: conversationId,
      pinned: false,
    });
  }

  if (selectedThread) {
    const threadId = columnIdentityId('thread', routeScope, selectedThread);
    const activeRouteColumnId = selectedAuthorPubkey
      ? columnIdentityId('profile', routeScope, selectedAuthorPubkey)
      : threadId;
    if (incoming.activeColumnId === activeRouteColumnId) return incoming;
    let next = ensureTimelineColumn(incoming);
    next = openTransientColumn(next, {
      id: threadId,
      kind: 'thread',
      scope: routeScope,
      entityId: selectedThread,
      parentColumnId: timelineId,
      pinned: false,
    });
    if (!selectedAuthorPubkey) return next;
    return openTransientColumn(next, {
      id: columnIdentityId('profile', routeScope, selectedAuthorPubkey),
      kind: 'profile',
      scope: routeScope,
      entityId: selectedAuthorPubkey,
      parentColumnId: threadId,
      pinned: false,
    });
  }

  if (selectedDirectMessagePeerPubkey) {
    const conversationId = columnIdentityId(
      'conversation',
      routeScope,
      selectedDirectMessagePeerPubkey
    );
    if (incoming.activeColumnId === conversationId) return incoming;
    const messagesId = columnIdentityId('messages', routeScope);
    const next = openTransientColumn(incoming, {
      id: messagesId,
      kind: 'messages',
      scope: routeScope,
      pinned: false,
    });
    return openTransientColumn(next, {
      id: conversationId,
      kind: 'conversation',
      scope: routeScope,
      entityId: selectedDirectMessagePeerPubkey,
      parentColumnId: messagesId,
      pinned: false,
    });
  }

  if (selectedAuthorPubkey) {
    const id = columnIdentityId('profile', routeScope, selectedAuthorPubkey);
    if (incoming.activeColumnId === id) return incoming;
    return openTransientColumn(incoming, {
      id,
      kind: 'profile',
      scope: routeScope,
      entityId: selectedAuthorPubkey,
      parentColumnId: incoming.activeColumnId,
      pinned: false,
    });
  }

  let routeColumnKind: ColumnKind;
  let routeEntityId: string | undefined;
  if (routeSection === 'notifications') {
    routeColumnKind = 'notifications';
  } else if (routeSection === 'messages') {
    routeColumnKind = 'messages';
  } else if (routeSection === 'profile') {
    routeColumnKind = 'profile';
  } else if (routeSection === 'explore') {
    routeColumnKind = 'explore';
  } else if (routeSection === 'live') {
    routeColumnKind = 'stream';
    routeEntityId = selectedLiveSessionId ?? undefined;
  } else if (routeSection === 'game') {
    const current = incoming.columns.find((column) => column.id === incoming.activeColumnId);
    routeColumnKind = isScoreGameRoom || gameColumnResolutionPending ||
      (!selectedGameRoomId && current?.kind === 'game') ? 'game' : 'metaverse';
    routeEntityId = selectedGameRoomId ?? undefined;
  } else {
    return activateColumn(ensureTimelineColumn(incoming), timelineId);
  }
  const routeColumnId = columnIdForScope(incoming, routeColumnKind, routeScope, routeEntityId);
  if (incoming.activeColumnId === routeColumnId) return incoming;
  return openTransientColumn(incoming, {
    id: routeColumnId,
    kind: routeColumnKind,
    scope: routeScope,
    entityId: routeEntityId,
    ...(routeColumnKind === 'stream' && routeEntityId && selectedLivePlace ? { live: selectedLivePlace } : {}),
    pinned: false,
  });
}
