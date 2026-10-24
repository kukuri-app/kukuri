import { describe, expect, it } from 'vitest';

import {
  activateColumn,
  closeColumn,
  columnSpanPolicy,
  columnIdentityId,
  createInitialWorkspaceState as createDefaultWorkspaceState,
  defaultColumnSpan,
  INITIAL_TIMELINE_COLUMN_ID,
  moveColumn,
  openTransientColumn,
  openPinnedColumn,
  setColumnSpan,
  setColumnPinned,
  setColumnTimelineView,
  setColumnTopic,
  columnIdForScope,
  type ColumnState,
} from '@/shell/slices/workspace';

// 個別transitionの単体試験は、製品のfresh defaultとは独立した最小fixtureで行う。
function createInitialWorkspaceState() {
  const state = createDefaultWorkspaceState();
  return { ...state, columns: [state.columns[0]] };
}

function transientColumn(overrides: Partial<ColumnState> = {}): ColumnState {
  return {
    id: 'thread-1',
    kind: 'thread',
    entityId: 'thread-1',
    parentColumnId: INITIAL_TIMELINE_COLUMN_ID,
    pinned: false,
    preferredDesktopSpan: 1,
    ...overrides,
  };
}

describe('workspace state transitions', () => {
  it.each([
    ['timeline', 1, 1, 1],
    ['notifications', 1, 1, 1],
    ['thread', 1, 1, 1],
    ['profile', 1, 1, 1],
    ['explore', 1, 1, 1],
    ['messages', 1, 1, 2],
    ['conversation', 1, 1, 2],
    ['stream', 2, 1, 2],
    ['game', 1, 1, 1],
    ['metaverse', 3, 1, 4],
  ] as const)('defines %s span policy', (kind, defaultSpan, min, max) => {
    expect(columnSpanPolicy(kind)).toEqual({ default: defaultSpan, min, max });
    expect(defaultColumnSpan(kind)).toBe(defaultSpan);
  });

  it('starts with the pinned product overview Columns and keeps Timeline active', () => {
    const scope = {
      topicId: 'kukuri:topic:general',
      channelId: null,
    };
    const state = createDefaultWorkspaceState({
      ...scope,
    });

    expect(state.columns).toEqual([
      {
        id: INITIAL_TIMELINE_COLUMN_ID,
        kind: 'timeline',
        scope,
        pinned: true,
        preferredDesktopSpan: 1,
      },
      {
        id: columnIdentityId('profile', scope),
        kind: 'profile',
        scope,
        pinned: true,
        preferredDesktopSpan: 1,
      },
      {
        id: columnIdentityId('explore', scope),
        kind: 'explore',
        scope,
        pinned: true,
        preferredDesktopSpan: 1,
      },
      {
        id: columnIdentityId('notifications', scope),
        kind: 'notifications',
        scope,
        pinned: true,
        preferredDesktopSpan: 1,
      },
      {
        id: columnIdentityId('messages', scope),
        kind: 'messages',
        scope,
        pinned: true,
        preferredDesktopSpan: 1,
      },
    ]);
    expect(state.activeColumnId).toBe(INITIAL_TIMELINE_COLUMN_ID);
  });

  it('builds stable identity from kind, scope, and entity', () => {
    expect(
      columnIdentityId('timeline', {
        topicId: 'kukuri:topic:general',
        channelId: null,
      })
    ).toBe('column:timeline:kukuri%3Atopic%3Ageneral:-:-');
    expect(
      columnIdentityId(
        'conversation',
        { topicId: 'kukuri:topic:general', channelId: 'friends/plus' },
        'peer:alice'
      )
    ).toBe(
      'column:conversation:kukuri%3Atopic%3Ageneral:friends%2Fplus:peer%3Aalice'
    );
    expect(
      columnIdentityId('timeline', {
        topicId: 'kukuri:topic:general',
        channelId: 'friends',
      })
    ).not.toBe(
      columnIdentityId('timeline', {
        topicId: 'kukuri:topic:general',
        channelId: null,
      })
    );
  });

  it('activates an existing Column without changing the layout', () => {
    const initial = createInitialWorkspaceState();
    const withThread = openTransientColumn(initial, transientColumn());
    const next = activateColumn(withThread, INITIAL_TIMELINE_COLUMN_ID);

    expect(next.activeColumnId).toBe(INITIAL_TIMELINE_COLUMN_ID);
    expect(next.columns).toEqual(withThread.columns);
  });

  it('replaces only an unpinned transient Column opened from the same parent', () => {
    const initial = createInitialWorkspaceState();
    const withThread = openTransientColumn(initial, transientColumn());
    const withProfile = openTransientColumn(
      withThread,
      transientColumn({ id: 'profile-1', kind: 'profile', entityId: 'author-1' })
    );

    expect(withProfile.columns.map((column) => column.id)).toEqual([
      INITIAL_TIMELINE_COLUMN_ID,
      'profile-1',
    ]);
    expect(withProfile.activeColumnId).toBe('profile-1');
  });

  it('keeps pinned and unrelated Columns when another transient Column opens', () => {
    const initial = createInitialWorkspaceState();
    const withThread = openTransientColumn(initial, transientColumn());
    const pinnedThread = setColumnPinned(withThread, 'thread-1', true);
    const withProfile = openTransientColumn(
      pinnedThread,
      transientColumn({ id: 'profile-1', kind: 'profile', entityId: 'author-1' })
    );
    const withConversation = openTransientColumn(
      withProfile,
      transientColumn({
        id: 'conversation-1',
        kind: 'conversation',
        entityId: 'peer-1',
        parentColumnId: 'thread-1',
      })
    );

    expect(withConversation.columns.map((column) => column.id)).toEqual([
      INITIAL_TIMELINE_COLUMN_ID,
      'profile-1',
      'thread-1',
      'conversation-1',
    ]);
  });

  it('adds an explicit pinned Column without replacing transient siblings', () => {
    const initial = openTransientColumn(createInitialWorkspaceState(), transientColumn());
    const explore = transientColumn({
      id: 'explore-demo',
      kind: 'explore',
      entityId: undefined,
      parentColumnId: undefined,
    });
    const next = openPinnedColumn(initial, explore);

    expect(next.columns.map((column) => column.id)).toEqual([
      INITIAL_TIMELINE_COLUMN_ID,
      'thread-1',
      'explore-demo',
    ]);
    expect(next.columns.at(-1)?.pinned).toBe(true);
    expect(next.activeColumnId).toBe('explore-demo');
  });

  it('applies the kind default when a new Column omits its preferred span', () => {
    const initial = createInitialWorkspaceState();
    const next = openPinnedColumn(initial, {
      id: 'stream-demo',
      kind: 'stream',
      pinned: true,
    });

    expect(next.columns.at(-1)?.preferredDesktopSpan).toBe(2);
  });

  it('clamps span changes without touching Column identity or relationships', () => {
    const initial = createInitialWorkspaceState();
    const withMetaverse = openPinnedColumn(initial, {
      id: 'metaverse-demo',
      kind: 'metaverse',
      parentColumnId: INITIAL_TIMELINE_COLUMN_ID,
      pinned: true,
    });
    const expanded = setColumnSpan(withMetaverse, 'metaverse-demo', 4);
    const clampedTimeline = setColumnSpan(expanded, INITIAL_TIMELINE_COLUMN_ID, 4);

    expect(expanded.columns.at(-1)).toMatchObject({
      id: 'metaverse-demo',
      parentColumnId: INITIAL_TIMELINE_COLUMN_ID,
      preferredDesktopSpan: 4,
    });
    expect(clampedTimeline.columns[0].preferredDesktopSpan).toBe(1);
    expect(setColumnSpan(clampedTimeline, 'missing', 2)).toBe(clampedTimeline);
  });

  it('moves a multi-span Column atomically and preserves product state', () => {
    let state = createInitialWorkspaceState();
    state = openPinnedColumn(state, {
      id: 'stream-demo',
      kind: 'stream',
      scope: { topicId: 'kukuri:topic:stream', channelId: null },
      entityId: 'session-1',
      parentColumnId: INITIAL_TIMELINE_COLUMN_ID,
      pinned: true,
    });
    state = openPinnedColumn(state, {
      id: 'profile-demo',
      kind: 'profile',
      entityId: 'alice',
      pinned: true,
    });
    const streamBefore = state.columns[1];
    const moved = moveColumn(state, 'stream-demo', 0);

    expect(moved.columns.map((column) => column.id)).toEqual([
      'stream-demo',
      INITIAL_TIMELINE_COLUMN_ID,
      'profile-demo',
    ]);
    expect(moved.columns[0]).toEqual(streamBefore);
    expect(moved.activeColumnId).toBe('profile-demo');
    expect(moveColumn(moved, 'stream-demo', 0)).toBe(moved);
    expect(moveColumn(moved, 'missing', 1)).toBe(moved);
  });

  it('inserts a new child immediately to the right of its parent', () => {
    const initial = createInitialWorkspaceState();
    const withProfile = openTransientColumn(
      initial,
      transientColumn({ id: 'profile-1', kind: 'profile', parentColumnId: undefined })
    );
    const withThread = openTransientColumn(
      withProfile,
      transientColumn({ parentColumnId: INITIAL_TIMELINE_COLUMN_ID })
    );

    expect(withThread.columns.map((column) => column.id)).toEqual([
      INITIAL_TIMELINE_COLUMN_ID,
      'thread-1',
      'profile-1',
    ]);
  });

  it('reparents and reorders an existing transient Column when its causal chain changes', () => {
    const profile = transientColumn({
      id: 'profile-bob',
      kind: 'profile',
      entityId: 'bob',
    });
    let state = openTransientColumn(createInitialWorkspaceState(), profile);
    state = openTransientColumn(
      state,
      transientColumn({ id: 'messages', kind: 'messages', parentColumnId: undefined })
    );
    state = openTransientColumn(
      state,
      transientColumn({
        id: 'conversation-bob',
        kind: 'conversation',
        entityId: 'bob',
        parentColumnId: 'messages',
      })
    );
    state = openTransientColumn(state, {
      ...profile,
      parentColumnId: 'conversation-bob',
    });

    expect(state.columns.map((column) => column.id)).toEqual([
      INITIAL_TIMELINE_COLUMN_ID,
      'messages',
      'conversation-bob',
      'profile-bob',
    ]);
    expect(state.columns.at(-1)?.parentColumnId).toBe('conversation-bob');
  });

  it('does not let a re-opened transient Column adopt itself as its parent', () => {
    const profile = transientColumn({
      id: 'profile-alice',
      kind: 'profile',
      entityId: 'alice',
    });
    const withProfile = openTransientColumn(createInitialWorkspaceState(), profile);
    // 自分自身を parent に指定して再 open しても、自己参照にはならず既存の parent を維持する。
    const reopened = openTransientColumn(withProfile, {
      ...profile,
      parentColumnId: 'profile-alice',
    });

    expect(reopened.columns.map((column) => column.id)).toEqual([
      INITIAL_TIMELINE_COLUMN_ID,
      'profile-alice',
    ]);
    expect(reopened.columns.at(-1)?.parentColumnId).toBe(INITIAL_TIMELINE_COLUMN_ID);

    // 以後、同じ parent から別 author を開くと置き換わる(蓄積しない)。
    const replaced = openTransientColumn(
      reopened,
      transientColumn({ id: 'profile-bob', kind: 'profile', entityId: 'bob' })
    );
    expect(replaced.columns.map((column) => column.id)).toEqual([
      INITIAL_TIMELINE_COLUMN_ID,
      'profile-bob',
    ]);
  });

  it('drops a self-referencing parent when a new transient Column is opened', () => {
    const next = openTransientColumn(
      createInitialWorkspaceState(),
      transientColumn({ id: 'profile-alice', kind: 'profile', parentColumnId: 'profile-alice' })
    );

    expect(next.columns.at(-1)?.parentColumnId).toBeUndefined();
  });

  it('replaces a parentless immersive transient Column with another immersive one', () => {
    // stream ↔ game / metaverse の deep link 切替は親なし transient 同士で置換され、増殖しない。
    const withStream = openTransientColumn(
      createInitialWorkspaceState(),
      transientColumn({
        id: 'stream-1',
        kind: 'stream',
        entityId: 'session-1',
        parentColumnId: undefined,
      })
    );
    const withGame = openTransientColumn(
      withStream,
      transientColumn({
        id: 'game-1',
        kind: 'game',
        entityId: 'room-1',
        parentColumnId: undefined,
      })
    );

    expect(withGame.columns.map((column) => column.id)).toEqual([
      INITIAL_TIMELINE_COLUMN_ID,
      'game-1',
    ]);
    expect(withGame.activeColumnId).toBe('game-1');
  });

  it('does not let a parentless immersive transient Column replace an unpinned Timeline Column', () => {
    const withSecondTimeline = openTransientColumn(
      createInitialWorkspaceState(),
      transientColumn({
        id: 'timeline-2',
        kind: 'timeline',
        entityId: undefined,
        parentColumnId: undefined,
      })
    );
    const withStream = openTransientColumn(
      withSecondTimeline,
      transientColumn({
        id: 'stream-1',
        kind: 'stream',
        entityId: 'session-1',
        parentColumnId: undefined,
      })
    );

    expect(withStream.columns.map((column) => column.id)).toEqual([
      INITIAL_TIMELINE_COLUMN_ID,
      'timeline-2',
      'stream-1',
    ]);
  });

  it('keeps orphaned conversation-family Columns out of the parentless replacement pool', () => {
    // close 時の孫付け替え等で parent を失った profile などは immersive の置換に巻き込まない。
    const withOrphanProfile = openTransientColumn(
      createInitialWorkspaceState(),
      transientColumn({ id: 'profile-1', kind: 'profile', parentColumnId: undefined })
    );
    const withStream = openTransientColumn(
      withOrphanProfile,
      transientColumn({
        id: 'stream-1',
        kind: 'stream',
        entityId: 'session-1',
        parentColumnId: undefined,
      })
    );

    expect(withStream.columns.map((column) => column.id)).toEqual([
      INITIAL_TIMELINE_COLUMN_ID,
      'profile-1',
      'stream-1',
    ]);

    // section 系(notifications / messages / explore)は置換プールに含まれ、
    // 親なし transient 同士として immersive とも相互置換される(従来挙動の維持)。
    const withMessages = openTransientColumn(
      withStream,
      transientColumn({
        id: 'messages-1',
        kind: 'messages',
        entityId: undefined,
        parentColumnId: undefined,
      })
    );
    expect(withMessages.columns.map((column) => column.id)).toEqual([
      INITIAL_TIMELINE_COLUMN_ID,
      'profile-1',
      'messages-1',
    ]);

    // section 同士も置換され、Column が増殖しない。
    const withExplore = openTransientColumn(
      withMessages,
      transientColumn({
        id: 'explore-1',
        kind: 'explore',
        entityId: undefined,
        parentColumnId: undefined,
      })
    );
    expect(withExplore.columns.map((column) => column.id)).toEqual([
      INITIAL_TIMELINE_COLUMN_ID,
      'profile-1',
      'explore-1',
    ]);
  });

  it('moves active state to the parent, then a neighbor, when closing a Column', () => {
    const initial = createInitialWorkspaceState();
    const withThread = openTransientColumn(initial, transientColumn());
    const afterThreadClose = closeColumn(withThread, 'thread-1');

    expect(afterThreadClose.activeColumnId).toBe(INITIAL_TIMELINE_COLUMN_ID);
    expect(afterThreadClose.columns).toHaveLength(1);

    const withOrphan = openTransientColumn(
      initial,
      transientColumn({ id: 'profile-1', kind: 'profile', parentColumnId: undefined })
    );
    const afterTimelineClose = closeColumn(withOrphan, INITIAL_TIMELINE_COLUMN_ID);

    expect(afterTimelineClose.activeColumnId).toBe('profile-1');
    expect(afterTimelineClose.columns).toHaveLength(1);
  });

  it('sets timelineView only on the target Timeline Column', () => {
    const initial = createInitialWorkspaceState();
    const withThread = openTransientColumn(initial, transientColumn());

    const next = setColumnTimelineView(withThread, INITIAL_TIMELINE_COLUMN_ID, 'bookmarks');

    expect(
      next.columns.find((column) => column.id === INITIAL_TIMELINE_COLUMN_ID)?.timelineView
    ).toBe('bookmarks');
    // 他 Column と active 状態は不変。
    expect(next.columns.find((column) => column.id === 'thread-1')?.timelineView).toBeUndefined();
    expect(next.activeColumnId).toBe(withThread.activeColumnId);

    const backToFeed = setColumnTimelineView(next, INITIAL_TIMELINE_COLUMN_ID, 'feed');
    expect(
      backToFeed.columns.find((column) => column.id === INITIAL_TIMELINE_COLUMN_ID)?.timelineView
    ).toBe('feed');
  });

  it('ignores timelineView updates for unknown ids, non-timeline Columns, and same values', () => {
    const initial = createInitialWorkspaceState();
    const withThread = openTransientColumn(initial, transientColumn());

    // 未知の id / timeline 以外の Column には何もしない(同一参照)。
    expect(setColumnTimelineView(withThread, 'missing', 'bookmarks')).toBe(withThread);
    expect(setColumnTimelineView(withThread, 'thread-1', 'bookmarks')).toBe(withThread);

    // 未設定(既定 feed)へ feed を設定しても no-op。
    expect(setColumnTimelineView(withThread, INITIAL_TIMELINE_COLUMN_ID, 'feed')).toBe(withThread);

    const bookmarks = setColumnTimelineView(withThread, INITIAL_TIMELINE_COLUMN_ID, 'bookmarks');
    expect(setColumnTimelineView(bookmarks, INITIAL_TIMELINE_COLUMN_ID, 'bookmarks')).toBe(
      bookmarks
    );
  });

  it.each(['timeline', 'publicThreads', 'stream', 'game', 'metaverse'] as const)(
    'replaces only the target %s topic while preserving Column identity and state',
    (kind) => {
      const initial = createInitialWorkspaceState();
      const timeline = {
        ...initial.columns[0],
        kind,
        parentColumnId: 'source-column',
        pinned: true,
        timelineView: 'bookmarks' as const,
      };
      const thread = transientColumn();
      const state = {
        ...initial,
        columns: [timeline, thread],
        activeColumnId: thread.id,
      };

      const next = setColumnTopic(state, timeline.id, 'kukuri:topic:next');

      expect(next).not.toBe(state);
      expect(next.columns).toHaveLength(2);
      expect(next.activeColumnId).toBe(thread.id);
      expect(next.columns[0]).toEqual({
        ...timeline,
        scope: { topicId: 'kukuri:topic:next', channelId: null },
      });
      expect(next.columns[1]).toBe(thread);
    }
  );

  it('ignores Timeline topic replacement for invalid targets and unchanged topics', () => {
    const initial = createInitialWorkspaceState();
    const withThread = openTransientColumn(initial, transientColumn());
    const currentTopic = initial.columns[0].scope?.topicId ?? '';

    expect(setColumnTopic(withThread, 'missing', 'kukuri:topic:next')).toBe(withThread);
    expect(setColumnTopic(withThread, 'thread-1', 'kukuri:topic:next')).toBe(withThread);
    expect(setColumnTopic(withThread, INITIAL_TIMELINE_COLUMN_ID, currentTopic)).toBe(
      withThread
    );
    expect(setColumnTopic(withThread, INITIAL_TIMELINE_COLUMN_ID, '')).toBe(withThread);
  });

  it.each([
    { kind: 'profile' }, { kind: 'notifications' }, { kind: 'messages' },
    { kind: 'conversation', entityId: 'peer' }, { kind: 'explore' },
    { kind: 'thread', entityId: 'post' },
    { kind: 'timeline', scope: { topicId: 'kukuri:topic:general', channelId: 'channel-1' } },
    { kind: 'stream', entityId: 'live' }, { kind: 'game', entityId: 'room' },
    { kind: 'metaverse', entityId: 'dome' },
  ] satisfies Partial<ColumnState>[])(
    'keeps the topic fixed for $kind outside topic lists', (overrides) => {
      const initial = createInitialWorkspaceState();
      initial.columns[0] = { ...initial.columns[0], ...overrides };
      expect(setColumnTopic(initial, initial.activeColumnId, 'kukuri:topic:dev')).toBe(initial);
    }
  );

  // Issue #1053: topic 切替後の Timeline は id が切替前 topic のままなので、scope で探して再利用する。
  it('resolves a Timeline Column by scope even when its id still carries the previous topic', () => {
    const initial = createDefaultWorkspaceState();
    const devScope = { topicId: 'kukuri:topic:dev', channelId: null };
    const switched = setColumnTopic(initial, INITIAL_TIMELINE_COLUMN_ID, devScope.topicId);
    const exploreActive = activateColumn(
      switched,
      columnIdentityId('explore', initial.columns[0].scope)
    );

    expect(columnIdForScope(exploreActive, 'timeline', devScope)).toBe(INITIAL_TIMELINE_COLUMN_ID);
    const reopened = openTransientColumn(exploreActive, {
      id: columnIdForScope(exploreActive, 'timeline', devScope),
      kind: 'timeline',
      scope: devScope,
      pinned: false,
    });
    expect(reopened.columns.map((column) => column.id)).toEqual(
      exploreActive.columns.map((column) => column.id)
    );
    expect(reopened.activeColumnId).toBe(INITIAL_TIMELINE_COLUMN_ID);
  });

  it('prefers the active, then the identity, then the first scope-matching Timeline Column', () => {
    const scope = { topicId: 'kukuri:topic:dev', channelId: null };
    const identityId = columnIdentityId('timeline', scope);
    const timeline = (id: string): ColumnState => ({
      id,
      kind: 'timeline',
      scope,
      pinned: true,
      preferredDesktopSpan: 1,
    });
    const state = {
      ...createInitialWorkspaceState(),
      columns: [timeline('first'), timeline(identityId), timeline('active')],
      activeColumnId: 'active',
    };

    expect(columnIdForScope(state, 'timeline', scope)).toBe('active');
    expect(columnIdForScope({ ...state, activeColumnId: 'none' }, 'timeline', scope)).toBe(identityId);
    expect(
      columnIdForScope(
        { ...state, columns: [timeline('first'), timeline('second')], activeColumnId: 'none' },
        'timeline',
        scope
      )
    ).toBe('first');
    expect(columnIdForScope({ ...state, columns: [] }, 'timeline', scope)).toBe(identityId);
    expect(
      columnIdForScope(state, 'timeline', { topicId: 'kukuri:topic:dev', channelId: 'channel-1' })
    ).toBe(columnIdentityId('timeline', { topicId: 'kukuri:topic:dev', channelId: 'channel-1' }));
  });
});
