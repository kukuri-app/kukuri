import { act, render, renderHook, screen, waitFor } from '@testing-library/react';
import { expect, test, vi } from 'vitest';

import type { DesktopApi, ScopeDisplayRequest } from '@/lib/api';
import { InvokeError } from '@/lib/api/invoke/error';
import {
  SCOPE_LIMIT_REACHED,
  explainScopeLimit,
  useColumnScopeLeases,
} from '@/shell/columnScopeLeases';
import { ColumnScopeLeases } from '@/shell/page/ColumnScopeLeases';
import type { ColumnState } from '@/shell/slices/workspace';
import { DesktopShellStoreContext, createDesktopShellStore } from '@/shell/store';

function column(id: string, kind: ColumnState['kind'], extra: Partial<ColumnState> = {}): ColumnState {
  return { id, kind, pinned: false, preferredDesktopSpan: 1, ...extra };
}

function scopeApi(reject?: (request: ScopeDisplayRequest) => boolean) {
  const requests: ScopeDisplayRequest[] = [];
  const setScopeDisplay = vi.fn(async (request: ScopeDisplayRequest) => {
    requests.push(request);
    if (request.visible && reject?.(request)) {
      throw new InvokeError(SCOPE_LIMIT_REACHED, 'SCOPE_LIMIT_REACHED: all 64 active scopes are in use');
    }
  });
  return { api: { setScopeDisplay } as unknown as DesktopApi, requests };
}

const timeline = column('timeline-a', 'timeline', {
  scope: { topicId: 'kukuri:topic:a', channelId: 'channel-1' },
});
const profile = column('profile-b', 'profile', { entityId: 'b'.repeat(64) });
const conversation = column('conversation-c', 'conversation', { entityId: 'c'.repeat(64) });

test('open columns register their scope once and a closed column releases only its own holder', async () => {
  const { api, requests } = scopeApi();
  const onLimit = vi.fn();
  const { rerender, unmount } = renderHook(
    ({ columns }) => useColumnScopeLeases(api, columns, onLimit),
    { initialProps: { columns: [timeline, profile, conversation, column('notifications', 'notifications')] } }
  );
  await waitFor(() => expect(requests).toHaveLength(3));
  expect(requests).toEqual([
    {
      observer: 'timeline-a',
      target: { kind: 'timeline', topic: 'kukuri:topic:a', scope: { kind: 'channel', channel_id: 'channel-1' } },
      visible: true,
    },
    { observer: 'profile-b', target: { kind: 'author', pubkey: 'b'.repeat(64) }, visible: true },
    { observer: 'conversation-c', target: { kind: 'author', pubkey: 'c'.repeat(64) }, visible: true },
  ]);

  // 同じ列の再描画は登録し直さない。閉じた列だけを外す。
  rerender({ columns: [timeline, conversation] });
  await waitFor(() => expect(requests).toHaveLength(4));
  expect(requests[3]).toMatchObject({ observer: 'profile-b', visible: false });

  unmount();
  await waitFor(() => expect(requests).toHaveLength(6));
  expect(requests.slice(4).map((request) => [request.observer, request.visible])).toEqual([
    ['timeline-a', false],
    ['conversation-c', false],
  ]);
  expect(onLimit).not.toHaveBeenCalled();
});

test('a column over the limit is closed and explained, and a refused participation uses the same dialog', async () => {
  const { api } = scopeApi((request) => request.observer === 'profile-b');
  const store = createDesktopShellStore();
  store.getState().setField('workspaceState', {
    ...store.getState().workspaceState,
    columns: [timeline, profile],
    activeColumnId: profile.id,
  });
  const onActivateColumn = vi.fn();
  render(
    <DesktopShellStoreContext.Provider value={store}>
      <ColumnScopeLeases api={api} onActivateColumn={onActivateColumn} />
    </DesktopShellStoreContext.Provider>
  );

  expect(
    await screen.findByText(/cannot add a new column because the column limit has been reached/)
  ).toBeTruthy();
  expect(store.getState().workspaceState.columns.map((current) => current.id)).toEqual([timeline.id]);
  expect(onActivateColumn).toHaveBeenCalledWith(timeline);

  act(() => screen.getByRole('button', { name: 'OK' }).click());
  await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());

  await act(async () => {
    await explainScopeLimit(
      Promise.reject(new InvokeError(SCOPE_LIMIT_REACHED, 'SCOPE_LIMIT_REACHED'))
    ).catch(() => undefined);
  });
  expect(await screen.findByText(/joined channels and open columns have reached the limit/)).toBeTruthy();
});
