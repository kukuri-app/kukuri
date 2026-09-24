import { beforeEach, expect, test } from 'vitest';

import type { PostView } from '@/lib/api';
import { createDesktopShellStore, createInitialShellState, timelineStorageKeyForChannel } from '@/shell/store';
import { closeColumn, columnIdentityId, type ColumnState } from '@/shell/slices/workspace';

beforeEach(() => {
  window.history.replaceState(null, '', '/');
});

test('shell store keeps settings drawer closed by default', () => {
  const state = createInitialShellState();

  expect(state.shellChromeState.activeSettingsSection).toBe('connectivity');
  expect(state.shellChromeState.settingsOpen).toBe(false);
});

test('shell store seeds settings drawer state from the initial hash route', () => {
  window.history.replaceState(
    null,
    '',
    '/#/timeline?topic=kukuri%3Atopic%3Ageneral&settings=appearance'
  );

  const state = createInitialShellState();

  expect(state.shellChromeState.activeSettingsSection).toBe('appearance');
  expect(state.shellChromeState.settingsOpen).toBe(true);
});

test('retains at most eight 200-row list windows when many columns were visited', () => {
  const store = createDesktopShellStore();
  const columns: ColumnState[] = Array.from({ length: 10 }, (_, index) => {
    const scope = { topicId: `topic-${index}`, channelId: null };
    return { id: columnIdentityId('timeline', scope), kind: 'timeline', scope,
      pinned: true, preferredDesktopSpan: 1 };
  });
  store.getState().patchState({ workspaceState: {
    ...store.getState().workspaceState,
    columns,
    activeColumnId: columns[0].id,
  } });
  store.getState().setField('timelinesByKey', Object.fromEntries(columns.map((column) => [
    `${column.scope!.topicId}::public`,
    Array.from({ length: 220 }, (_, index) => ({ object_id: `post-${index}` } as PostView)),
  ])));
  const windows = Object.entries(store.getState().timelinesByKey)
    .filter(([, posts]) => posts.length > 0);
  expect(windows).toHaveLength(8);
  expect(windows.every(([, posts]) => posts.length <= 200)).toBe(true);
  expect(store.getState().timelinesByKey['topic-0::public']).toHaveLength(200);
});

test('keeps an open private column within the eight-list budget while it is offscreen', () => {
  const store = createDesktopShellStore();
  const workspace = store.getState().workspaceState;
  const privateScope = { topicId: 'kukuri:topic:general', channelId: 'channel-1' };
  const privateKey = timelineStorageKeyForChannel(privateScope.topicId, privateScope.channelId);
  const privateColumn: ColumnState = { id: columnIdentityId('timeline', privateScope),
    kind: 'timeline', scope: privateScope, pinned: false, preferredDesktopSpan: 1 };
  store.getState().patchState({
    workspaceState: { ...workspace, columns: [...workspace.columns, privateColumn] },
    visibleListColumnIds: [workspace.activeColumnId],
    timelinesByKey: {
      'kukuri:topic:general::public': [{ object_id: 'public' } as PostView],
      [privateKey]: [{ object_id: 'private' } as PostView],
    },
  });
  expect(store.getState().timelinesByKey[privateKey]).toHaveLength(1);
});

test('closing a timeline column frees its rows while keeping the continuation cursor', () => {
  const store = createDesktopShellStore();
  const key = 'kukuri:topic:general::public';
  const cursor = { created_at: 1, object_id: 'last-post' };
  store.getState().patchState({
    timelinesByKey: { [key]: [{ object_id: 'post' } as PostView] },
    timelineNextCursorByKey: { [key]: cursor },
  });
  const initial = store.getState().workspaceState;
  store.getState().patchState({ workspaceState: closeColumn(initial, initial.activeColumnId) });

  expect(store.getState().timelinesByKey[key]).toBeUndefined();
  expect(store.getState().timelineNextCursorByKey[key]).toEqual(cursor);
});
