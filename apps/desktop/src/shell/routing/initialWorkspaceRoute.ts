import { buildShellUrl, type RouteState } from '@/shell/routes';
import type { ColumnState, WorkspaceState } from '@/shell/slices/workspace';

// Issue #765 T4: hash を持たない cold start で、復元済み layout の active Column の
// canonical target を初期 route として採用するための helper。
// ここで生成した hash は通常の deep link として既存の route 同期・normalize に処理される
// (存在しない entity は既存の安全側 fallback で Timeline に戻る)。

const DEFAULT_STARTUP_HASHES = new Set(['', '#', '#/']);

export function isDefaultStartupHash(hash: string): boolean {
  return DEFAULT_STARTUP_HASHES.has(hash);
}

function baseRouteState(activeTopic: string): RouteState {
  return {
    activeTopic,
    primarySection: 'timeline',
    timelineView: 'feed',
    profileMode: 'overview',
    profileConnectionsView: 'following',
    selectedThread: null,
    focusedObjectId: null,
    selectedAuthorPubkey: null,
    selectedDirectMessagePeerPubkey: null,
    settingsOpen: false,
    settingsSection: 'appearance',
    selectedChannelId: null,
    selectedLiveSessionId: null,
    selectedGameRoomId: null,
  };
}

export function routeStateForColumn(column: ColumnState): RouteState | null {
  const topicId = column.scope?.topicId;
  if (!topicId) return null;
  const route = baseRouteState(topicId);
  route.selectedChannelId = column.scope?.channelId ?? null;
  switch (column.kind) {
    case 'timeline':
      route.timelineView = column.timelineView ?? 'feed';
      return route;
    case 'thread':
      if (!column.entityId) return null;
      route.selectedThread = column.entityId;
      return route;
    case 'profile':
      if (!column.entityId) {
        route.primarySection = 'profile';
        return route;
      }
      route.selectedAuthorPubkey = column.entityId;
      return route;
    case 'notifications':
      route.primarySection = 'notifications';
      return route;
    case 'messages':
      route.primarySection = 'messages';
      return route;
    case 'conversation':
      if (!column.entityId) return null;
      route.primarySection = 'messages';
      route.selectedDirectMessagePeerPubkey = column.entityId;
      return route;
    case 'explore':
      route.primarySection = 'explore';
      return route;
    case 'stream':
      route.primarySection = 'live';
      route.selectedLiveSessionId = column.entityId ?? null;
      route.selectedLivePlace = column.live ?? null;
      return route;
    case 'game':
    case 'metaverse':
      route.primarySection = 'game';
      route.selectedGameRoomId = column.entityId ?? null;
      return route;
    case 'publicThreads':
      route.publicThreadList = true;
      return route;
    default:
      return null;
  }
}

export function initialHashForRestoredWorkspace(
  workspaceState: WorkspaceState
): string | null {
  const column = workspaceState.columns.find(
    (candidate) => candidate.id === workspaceState.activeColumnId
  );
  if (!column) return null;
  const route = routeStateForColumn(column);
  if (!route) return null;
  return `#${buildShellUrl(route)}`;
}
