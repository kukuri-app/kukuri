import { act, renderHook } from '@testing-library/react';
import { expect, test, vi } from 'vitest';
import { columnIdentityId, openTransientColumn } from '@/shell/slices/workspace';
import { createDesktopShellStore } from '@/shell/store';
import { useSessionProjectionRefresh } from './useSessionProjectionRefresh';

// #1815: 一覧の取り直しは、projection の変更の知らせ 1 回だけに頼らない。表示中の live の列が見えていなかった間に
// 届いた変更も、次の store の更新で取り直す。取り直した後は、同じ変更で読み直さない(取り直しが一覧を store へ
// 書く更新でも)。
test('a projection change that arrives while the live column is not visible is loaded on the next store update', async () => {
  const store = createDesktopShellStore();
  const scope = { topicId: 'kukuri:topic:general', channelId: null };
  const live = columnIdentityId('stream', scope);
  store.setState((state) => ({
    workspaceState: openTransientColumn(state.workspaceState, { id: live, kind: 'stream', scope, pinned: false }),
  }));
  const visible = { current: [] as string[] };
  const loadLiveSection = vi.fn(async () => {
    await Promise.resolve();
    store.setState((state) => ({ liveSessionsByScopeKey: { ...state.liveSessionsByScopeKey } }));
  });
  renderHook(() => useSessionProjectionRefresh(store, loadLiveSection, vi.fn(async () => {}), visible));
  const update = (patch: Partial<ReturnType<typeof store.getState>>) => act(async () => store.setState(patch));
  const projected = (at: number) => update({ syncStatus: { ...store.getState().syncStatus, last_sync_ts: at } });

  await projected(1);
  expect(loadLiveSection).not.toHaveBeenCalled();

  visible.current = [live];
  await update({ profileSaving: true });
  expect(loadLiveSection).toHaveBeenCalledTimes(1);
  expect(loadLiveSection).toHaveBeenCalledWith('kukuri:topic:general', null);

  await update({ profileSaving: false });
  expect(loadLiveSection).toHaveBeenCalledTimes(1);

  await projected(2);
  expect(loadLiveSection).toHaveBeenCalledTimes(2);
});

// #1840 AC-4: 表示中の公開スレッドの詳細の列は、上部の配信中の Live のためにそのスレッドの Live を読み直す(別の端末で
// 始まった Live や終わった Live を反映する)。Live の画面を出さない開発者モードの無効の間は読まない。
test('a visible public thread detail column reloads the lives of its thread only in developer mode', async () => {
  const store = createDesktopShellStore();
  const scope = { topicId: 'kukuri:topic:general', channelId: 'pc-1700000000000-0123456789abcdef' };
  const detail = columnIdentityId('timeline', scope);
  store.setState((state) => ({
    developerModeEnabled: false,
    workspaceState: openTransientColumn(state.workspaceState, { id: detail, kind: 'timeline', scope, pinned: false }),
  }));
  const loadLiveSection = vi.fn(async () => {});
  renderHook(() => useSessionProjectionRefresh(store, loadLiveSection, vi.fn(async () => {}), { current: [detail] }));
  const projected = (at: number) =>
    act(async () => store.setState({ syncStatus: { ...store.getState().syncStatus, last_sync_ts: at } }));

  await projected(1);
  expect(loadLiveSection).not.toHaveBeenCalled();

  await act(async () => store.setState({ developerModeEnabled: true }));
  await projected(2);
  expect(loadLiveSection).toHaveBeenLastCalledWith(scope.topicId, scope.channelId);
});
