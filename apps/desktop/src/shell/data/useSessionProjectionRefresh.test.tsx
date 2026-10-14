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
