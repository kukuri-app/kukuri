// #1521 AC-1b: 自分を指す相手の follow の edge が届いた知らせ（`author_relationship_changed`）で、その相手の開いている
// profile と会話の列だけを読み直す。知らせが溢れたとき（`pubkey: null`）は、開いている列を 1 回ずつ読み直す。
import { act, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';

import type { RuntimeEvent } from '@/lib/api';
import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import { openTransientColumn } from '@/shell/slices/workspace';
import { createShellHookHarness, resetWindowHash } from '@/shell/testSupport/renderShellHook';
import { useDesktopShellData } from '@/shell/useDesktopShellData';

const { listenMock } = vi.hoisted(() => ({ listenMock: vi.fn() }));
vi.mock('@tauri-apps/api/event', () => ({ listen: listenMock }));

const translate = (key: string) => key;
const peer = 'a'.repeat(64);
const other = 'b'.repeat(64);
let receive: ((event: { payload: RuntimeEvent }) => void) | undefined;

beforeEach(() => {
  resetWindowHash();
  vi.useFakeTimers();
  vi.setSystemTime(100_000);
  (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
  listenMock.mockReset().mockImplementation(async (_name: string, callback: typeof receive) => {
    receive = callback;
    return () => { receive = undefined; };
  });
  vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('visible');
});

afterEach(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
});

async function flush(milliseconds = 0) {
  await act(async () => { await vi.advanceTimersByTimeAsync(milliseconds); });
}

test('a relationship change re-reads only the open columns of that author', async () => {
  const api = createDesktopMockApi();
  const readView = api.getAuthorSocialView.bind(api);
  const socialView = vi.spyOn(api, 'getAuthorSocialView');
  const status = vi.spyOn(api, 'getDirectMessageStatus');
  const harness = createShellHookHarness({ hash: '/timeline?topic=kukuri%3Atopic%3Ageneral' });
  let workspace = harness.store.getState().workspaceState;
  for (const [kind, entityId] of [
    ['profile', peer], ['profile', other], ['conversation', peer], ['conversation', other],
  ] as const) {
    workspace = openTransientColumn(workspace, { id: `${kind}-${entityId}`, kind, entityId, pinned: false });
  }
  // 開いている列は画面に入っている（画面外の列の相手は表示の cache から外れる）。
  harness.store.getState().patchState({
    workspaceState: workspace,
    visibleListColumnIds: workspace.columns.filter((column) => column.entityId).map((column) => column.id),
  });
  const args = {
    api, translate,
    loadTopicsRequestRef: { current: new Map<string, number>() },
    draftPreviewUrlRef: { current: new Map<string, string>() },
    directMessageDraftPreviewUrlRef: { current: new Map<string, string>() },
    draftSequenceRef: { current: 0 },
  };
  const hook = renderHook(() => useDesktopShellData(args), { wrapper: harness.wrapper });
  await flush(1_000);
  expect(receive).toBeDefined();
  socialView.mockClear();
  status.mockClear();
  // 相手が follow し返した後の手元の関係。
  socialView.mockImplementation(async (pubkey) => ({
    ...(await readView(pubkey)), following: true, followed_by: true, mutual: true,
  }));

  await act(async () => { receive?.({ payload: { type: 'author_relationship_changed', pubkey: peer } }); });
  await vi.waitFor(() => expect(harness.store.getState().knownAuthorsByPubkey[peer]?.mutual).toBe(true));
  expect(socialView.mock.calls.map(([pubkey]) => pubkey)).toEqual([peer]);
  expect(status.mock.calls.map(([pubkey]) => pubkey)).toEqual([peer]);

  socialView.mockClear();
  status.mockClear();
  await act(async () => { receive?.({ payload: { type: 'author_relationship_changed', pubkey: null } }); });
  await flush();
  expect(socialView.mock.calls.map(([pubkey]) => pubkey).sort()).toEqual([peer, other]);
  expect(status.mock.calls.map(([pubkey]) => pubkey).sort()).toEqual([peer, other]);
  hook.unmount();
});
