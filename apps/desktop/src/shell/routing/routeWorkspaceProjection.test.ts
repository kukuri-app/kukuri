import { expect, test } from 'vitest';

import {
  createInitialWorkspaceState,
  setColumnTopic,
} from '@/shell/slices/workspace';
import { workspaceForRoute } from './routeWorkspaceProjection';

test.each(['timeline', 'publicThreads', 'stream', 'game', 'metaverse'] as const)(
  'route projection reuses an active %s list whose topic was replaced in place',
  (kind) => {
    const initial = createInitialWorkspaceState();
    const timelineId = initial.activeColumnId;
    initial.columns[0].kind = kind;
    const switched = setColumnTopic(initial, timelineId, 'kukuri:topic:dev');

    const projected = workspaceForRoute(switched, {
      gameColumnResolutionPending: false,
      isScoreGameRoom: false,
      nextTimelineView: 'feed',
      routeScope: { topicId: 'kukuri:topic:dev', channelId: null },
      routeSection: kind === 'stream' ? 'live' : kind === 'game' || kind === 'metaverse' ? 'game' : 'timeline',
      publicThreadList: kind === 'publicThreads',
      selectedAuthorPubkey: null,
      selectedDirectMessagePeerPubkey: null,
      selectedGameRoomId: null,
      selectedLiveSessionId: null,
      selectedThread: null,
    });

    expect(projected.columns).toHaveLength(initial.columns.length);
    expect(projected.activeColumnId).toBe(timelineId);
    expect(projected.columns.find((column) => column.id === timelineId)?.scope).toEqual({
      topicId: 'kukuri:topic:dev',
      channelId: null,
    });
  }
);
