import type { ReactNode } from 'react';
import { act, renderHook } from '@testing-library/react';
import { expect, test, vi } from 'vitest';
import type { CommunityNodeNodeStatus } from '@/lib/api';
import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import { createDesktopShellStore, DesktopShellStoreContext } from '@/shell/store';
import { applySyncStatusDelta } from '@/shell/slices/connectivity';
import { createDeferred } from '@/shell/DesktopShellPage.testHelpers';
import { useConnectivityStatusRefresh } from './useConnectivityStatusRefresh';

test('sync read failure retains its snapshot and retry exposes pending then clears the error', async () => {
  const api = createDesktopMockApi();
  const store = createDesktopShellStore();
  const response = createDeferred<Awaited<ReturnType<typeof api.getSyncStatus>>>();
  const initial = await api.getSyncStatus();
  const syncRead = vi.spyOn(api, 'getSyncStatus').mockRejectedValueOnce(new Error('offline'))
    .mockResolvedValueOnce(initial).mockRejectedValueOnce(new Error('later failure')).mockReturnValue(response.promise);
  const { result } = renderHook(() => useConnectivityStatusRefresh(api,
    next => store.getState().setField('syncStatus', next), vi.fn()), {
    wrapper: ({ children }: { children: ReactNode }) => <DesktopShellStoreContext.Provider value={store}>{children}</DesktopShellStoreContext.Provider>,
  });
  await act(async () => result.current());
  expect(store.getState().syncStatusRead).toEqual({ loaded: false, refreshing: false, error: true });
  await act(async () => result.current());
  expect(store.getState().syncStatusRead).toEqual({ loaded: true, refreshing: false, error: false });
  await act(async () => result.current());
  expect(store.getState().syncStatus).toBe(initial);
  expect(store.getState().syncStatusRead.error).toBe(true);
  let refresh!: ReturnType<typeof result.current>;
  act(() => { refresh = result.current(); void result.current(); });
  expect(store.getState().syncStatusRead.refreshing).toBe(true);
  expect(syncRead).toHaveBeenCalledTimes(4);
  await act(async () => { response.resolve(initial); await refresh; });
  expect(store.getState().syncStatusRead).toEqual({ loaded: true, refreshing: false, error: false });
});

test('a failed old sync read cannot mark a newer event snapshot as failed', async () => {
  const api = createDesktopMockApi();
  const store = createDesktopShellStore();
  const response = createDeferred<Awaited<ReturnType<typeof api.getSyncStatus>>>();
  const initial = await api.getSyncStatus();
  vi.spyOn(api, 'getSyncStatus').mockReturnValue(response.promise);
  const { result } = renderHook(() => useConnectivityStatusRefresh(api,
    next => store.getState().setField('syncStatus', next), vi.fn()), {
    wrapper: ({ children }: { children: ReactNode }) => <DesktopShellStoreContext.Provider value={store}>{children}</DesktopShellStoreContext.Provider>,
  });
  let refresh!: ReturnType<typeof result.current>;
  act(() => { refresh = result.current(); store.getState().setField('syncStatus', initial); });
  await act(async () => { response.reject(new Error('old read')); await refresh; });
  expect(store.getState().syncStatus).toBe(initial);
  expect(store.getState().syncStatusRead).toEqual({ loaded: true, refreshing: false, error: false });
});

// #1221 R2-D: 読む間に届いた差分(件数と変わった topic)は、読み直した状態へ重ねて残す。
test('changes pushed during a pending read survive the read', async () => {
  const api = createDesktopMockApi();
  const store = createDesktopShellStore();
  const before = await api.getSyncStatus();
  const response = createDeferred<typeof before>();
  vi.spyOn(api, 'getSyncStatus').mockReturnValue(response.promise);
  const { result } = renderHook(() => useConnectivityStatusRefresh(api,
    next => store.getState().setField('syncStatus', next), vi.fn()), {
    wrapper: ({ children }: { children: ReactNode }) => <DesktopShellStoreContext.Provider value={store}>{children}</DesktopShellStoreContext.Provider>,
  });
  let refresh!: ReturnType<typeof result.current>;
  act(() => { refresh = result.current(); });
  const [changed, ...untouched] = before.topic_diagnostics;
  const pushed = { ...changed, peer_count: 99 };
  act(() => store.getState().setField('syncStatus', applySyncStatusDelta(
    store.getState().syncStatus, { ...before, peer_count: 99, topic_diagnostics: [pushed] }, []
  )));
  await act(async () => { response.resolve(before); await refresh; });
  const merged = store.getState().syncStatus;
  expect(merged.peer_count).toBe(99);
  expect(merged.topic_diagnostics.find((topic) => topic.topic === changed.topic)).toBe(pushed);
  for (const topic of untouched) {
    expect(merged.topic_diagnostics.find((entry) => entry.topic === topic.topic)).toBe(topic);
  }
  const removed = applySyncStatusDelta(merged, { ...merged, topic_diagnostics: [] }, [changed.topic]);
  expect(removed.topic_diagnostics.map((topic) => topic.topic)).toEqual(untouched.map((topic) => topic.topic).sort());
});

test('an in-flight poll preserves a newer per-node event and refreshes untouched nodes', async () => {
  const api = createDesktopMockApi();
  await api.setCommunityNodeConfig([{ base_url: 'https://a.example' }, { base_url: 'https://b.example' }]);
  const pending = await api.getCommunityNodeStatuses();
  const store = createDesktopShellStore();
  store.getState().setField('communityNodeStatuses', pending);
  const response = createDeferred<CommunityNodeNodeStatus[]>();
  vi.spyOn(api, 'getCommunityNodeStatuses').mockReturnValue(response.promise);
  const { result } = renderHook(() => useConnectivityStatusRefresh(api, vi.fn(), (next) => store.getState().setField('communityNodeStatuses', next)), {
    wrapper: ({ children }: { children: ReactNode }) => <DesktopShellStoreContext.Provider value={store}>{children}</DesktopShellStoreContext.Provider>,
  });
  let refresh!: ReturnType<typeof result.current>;
  act(() => { refresh = result.current(); });
  const accepted = await api.acceptCommunityNodeConsents(pending[0].base_url, (await api.fetchCommunityNodePolicies(pending[0].base_url)).policies, 'en');
  act(() => store.getState().setField('communityNodeStatuses', [accepted, pending[1]]));
  await act(async () => { response.resolve([pending[0], { ...pending[1], last_error: 'new connection failure' }]); await refresh; });
  expect(store.getState().communityNodeStatuses[0].local_consent?.records.length).toBeGreaterThan(0);
  expect(store.getState().communityNodeStatuses[1].last_error).toBe('new connection failure');
});

test('failed status reads are distinguishable from a successfully empty list', async () => {
  const api = createDesktopMockApi();
  const store = createDesktopShellStore();
  vi.spyOn(api, 'getCommunityNodeStatuses').mockRejectedValueOnce(new Error('local status unavailable')).mockResolvedValue([]);
  const { result } = renderHook(() => useConnectivityStatusRefresh(api, vi.fn(), (next) => store.getState().setField('communityNodeStatuses', next)), {
    wrapper: ({ children }: { children: ReactNode }) => <DesktopShellStoreContext.Provider value={store}>{children}</DesktopShellStoreContext.Provider>,
  });
  await act(async () => result.current());
  expect(store.getState().communityNodeStatusesLoaded).toBe(false);
  expect(store.getState().communityNodeStatusError).not.toBeNull();
  await act(async () => result.current());
  expect(store.getState().communityNodeStatusesLoaded).toBe(true);
  expect(store.getState().communityNodeStatusError).toBeNull();
});
