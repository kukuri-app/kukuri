import { describe, expect, it, vi } from 'vitest';

import { createDesktopShellStore } from '@/shell/store';
import { createInitialWorkspaceState } from '@/shell/slices/workspace';
import {
  readWorkspaceLayout,
  startWorkspaceLayoutPersistence,
  WORKSPACE_LAYOUT_STORAGE_KEY,
  writeWorkspaceLayout,
  type WorkspaceStorage,
} from '@/shell/workspacePersistence';

function memoryStorage(initial: string | null = null): WorkspaceStorage & {
  getItem: ReturnType<typeof vi.fn>;
  setItem: ReturnType<typeof vi.fn>;
} {
  let value = initial;
  return {
    getItem: vi.fn(() => value),
    setItem: vi.fn((_key: string, next: string) => {
      value = next;
    }),
  };
}

describe('workspace layout persistence', () => {
  it('round-trips product layout without transient UI state', () => {
    const fallback = createInitialWorkspaceState();
    const storage = memoryStorage();
    const state = {
      columns: [
        fallback.columns[0],
        {
          id: 'metaverse-room',
          kind: 'metaverse' as const,
          scope: { topicId: 'kukuri:topic:room', channelId: null },
          entityId: 'room-1',
          parentColumnId: fallback.columns[0].id,
          pinned: true,
          preferredDesktopSpan: 4 as const,
        },
      ],
      activeColumnId: 'metaverse-room',
      controlCenterOpen: true,
      activeLayoutId: 'future-layout',
    };

    expect(writeWorkspaceLayout(storage, state)).toBe(true);
    const restored = readWorkspaceLayout(storage, fallback);

    expect(storage.setItem).toHaveBeenCalledWith(
      WORKSPACE_LAYOUT_STORAGE_KEY,
      expect.any(String)
    );
    expect(restored).toEqual({
      ...state,
      controlCenterOpen: false,
    });
  });

  it('normalizes spans, duplicate ids, dangling parents, and a missing active id', () => {
    const fallback = createInitialWorkspaceState();
    const storage = memoryStorage(
      JSON.stringify({
        version: 1,
        activeColumnId: 'missing',
        columns: [
          {
            id: 'stream',
            kind: 'stream',
            pinned: false,
            preferredDesktopSpan: 99,
          },
          {
            id: 'stream',
            kind: 'stream',
            pinned: true,
            preferredDesktopSpan: 1,
          },
          {
            id: 'profile',
            kind: 'profile',
            parentColumnId: 'missing-parent',
            pinned: true,
            preferredDesktopSpan: 4,
          },
          { id: '', kind: 'timeline', pinned: true, preferredDesktopSpan: 1 },
          { id: 'unknown', kind: 'unknown', pinned: true, preferredDesktopSpan: 1 },
        ],
      })
    );

    expect(readWorkspaceLayout(storage, fallback)).toEqual({
      columns: [
        {
          id: 'stream',
          kind: 'stream',
          pinned: false,
          preferredDesktopSpan: 2,
        },
        {
          id: 'profile',
          kind: 'profile',
          pinned: true,
          preferredDesktopSpan: 1,
        },
      ],
      activeColumnId: 'stream',
      controlCenterOpen: false,
      activeLayoutId: null,
    });
  });

  it('clears a persisted parentColumnId that points at the Column itself', () => {
    const fallback = createInitialWorkspaceState();
    const storage = memoryStorage(
      JSON.stringify({
        version: 1,
        activeColumnId: 'profile-alice',
        columns: [
          { id: 'timeline', kind: 'timeline', pinned: true, preferredDesktopSpan: 1 },
          {
            id: 'profile-alice',
            kind: 'profile',
            entityId: 'alice',
            parentColumnId: 'profile-alice',
            pinned: false,
            preferredDesktopSpan: 1,
          },
          {
            id: 'thread-1',
            kind: 'thread',
            entityId: 'thread-1',
            parentColumnId: 'timeline',
            pinned: false,
            preferredDesktopSpan: 1,
          },
        ],
      })
    );

    const restored = readWorkspaceLayout(storage, fallback);

    // 自己参照は「存在しない id」と同様に読み込み時へ解除し、正常な親子関係は維持する。
    expect(restored.columns.find((column) => column.id === 'profile-alice')).toEqual({
      id: 'profile-alice',
      kind: 'profile',
      entityId: 'alice',
      pinned: false,
      preferredDesktopSpan: 1,
    });
    expect(
      restored.columns.find((column) => column.id === 'thread-1')?.parentColumnId
    ).toBe('timeline');
    expect(restored.activeColumnId).toBe('profile-alice');
  });

  it('round-trips per-Column timelineView and defaults legacy layouts to feed', () => {
    const fallback = createInitialWorkspaceState();
    const storage = memoryStorage();
    const state = {
      ...fallback,
      columns: [
        { ...fallback.columns[0], timelineView: 'bookmarks' as const, timelineFlow: true },
        {
          id: 'timeline-core',
          kind: 'timeline' as const,
          scope: { topicId: 'kukuri:topic:general', channelId: 'channel-1' },
          pinned: false,
          preferredDesktopSpan: 1 as const,
        },
      ],
    };

    expect(writeWorkspaceLayout(storage, state)).toBe(true);
    const restored = readWorkspaceLayout(storage, fallback);

    expect(restored.columns[0].timelineView).toBe('bookmarks');
    // timelineView 未設定の Column は undefined(既定 feed)のまま。
    expect(restored.columns[1].timelineView).toBeUndefined();
    // Flow モード(#1647)も Column ごとに往復し、未設定は通常モードのまま。
    expect(restored.columns[0].timelineFlow).toBe(true);
    expect(restored.columns[1].timelineFlow).toBeUndefined();
  });

  it('drops invalid persisted timelineView values', () => {
    const fallback = createInitialWorkspaceState();
    const storage = memoryStorage(
      JSON.stringify({
        version: 1,
        activeColumnId: 'timeline',
        columns: [
          {
            id: 'timeline',
            kind: 'timeline',
            pinned: true,
            preferredDesktopSpan: 1,
            timelineView: 'invalid',
            timelineFlow: 'yes',
          },
        ],
      })
    );

    const restored = readWorkspaceLayout(storage, fallback);
    // 旧 layout / 破損値は既定(feed)へ倒す。
    expect(restored.columns[0].timelineView).toBeUndefined();
    expect(restored.columns[0].timelineFlow).toBeUndefined();
  });

  it('falls back for malformed, unknown-version, empty, and failing storage', () => {
    const fallback = createInitialWorkspaceState();
    expect(readWorkspaceLayout(memoryStorage('{'), fallback)).toBe(fallback);
    expect(
      readWorkspaceLayout(memoryStorage(JSON.stringify({ version: 2, columns: [] })), fallback)
    ).toBe(fallback);
    expect(
      readWorkspaceLayout(
        memoryStorage(JSON.stringify({ version: 1, activeColumnId: '', columns: [] })),
        fallback
      )
    ).toBe(fallback);
    expect(
      readWorkspaceLayout(
        { getItem: () => { throw new Error('denied'); }, setItem: vi.fn() },
        fallback
      )
    ).toBe(fallback);
    expect(
      writeWorkspaceLayout(
        { getItem: vi.fn(), setItem: () => { throw new Error('denied'); } },
        fallback
      )
    ).toBe(false);
  });

  it('restores synchronously and persists only product layout changes', () => {
    const fallback = createInitialWorkspaceState();
    const storage = memoryStorage();
    const saved = {
      ...fallback,
      columns: [
        {
          id: 'stream',
          kind: 'stream' as const,
          pinned: true,
          preferredDesktopSpan: 2 as const,
        },
      ],
      activeColumnId: 'stream',
    };
    writeWorkspaceLayout(storage, saved);
    storage.setItem.mockClear();

    const store = createDesktopShellStore({ workspaceStorage: storage });
    expect(store.getState().workspaceState.activeColumnId).toBe('stream');
    const unsubscribe = startWorkspaceLayoutPersistence(store, storage);

    store.getState().setField('topicInput', 'unrelated');
    expect(storage.setItem).not.toHaveBeenCalled();
    store.getState().setField('workspaceState', (current) => ({
      ...current,
      columns: current.columns.map((column) => ({
        ...column,
        preferredDesktopSpan: 1 as const,
      })),
    }));
    expect(storage.setItem).toHaveBeenCalledTimes(1);

    unsubscribe();
    store.getState().setField('workspaceState', fallback);
    expect(storage.setItem).toHaveBeenCalledTimes(1);
  });

  it('persists the current workspace when startup changes it before subscription begins', () => {
    const storage = memoryStorage();
    const store = createDesktopShellStore({ workspaceStorage: storage });
    store.getState().setField('workspaceState', (current) => ({
      ...current,
      columns: [
        ...current.columns,
        {
          id: 'stream',
          kind: 'stream',
          pinned: false,
          preferredDesktopSpan: 2,
        },
      ],
      activeColumnId: 'stream',
    }));

    const unsubscribe = startWorkspaceLayoutPersistence(store, storage);

    expect(readWorkspaceLayout(storage, createInitialWorkspaceState()).activeColumnId).toBe(
      'stream'
    );
    unsubscribe();
  });
});
