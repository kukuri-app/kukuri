// #1239: 非表示の著者の投稿が続く範囲では、取得が「空の items + next_cursor」を返す。表示中の行が 0 件の
// あいだに周期の refresh(buffer)が走っても、読み進めた cursor を先頭のページの cursor に戻さない
// (独立監査 PR #1270 の再現 test を恒久化)。
import { act, renderHook } from '@testing-library/react';
import { beforeEach, expect, test, vi } from 'vitest';

const { listenMock } = vi.hoisted(() => ({ listenMock: vi.fn() }));

vi.mock('@tauri-apps/api/event', () => ({
  listen: (...args: unknown[]) => listenMock(...args),
}));

import type { DesktopApi, PostView, TimelineCursor, TimelineView } from '@/lib/api';
import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import { REFRESH_INTERVAL_MS } from '@/shell/store';
import { useDesktopShellData } from '@/shell/useDesktopShellData';
import { createShellHookHarness, resetWindowHash } from '@/shell/testSupport/renderShellHook';

const stubTranslate = (key: string) => key;
const KEY = 'kukuri:topic:general::public';

// タイムラインは新しい順なので、読み進めるほど時刻が古くなる。
function cursorAt(step: number): TimelineCursor {
  return { created_at: 10_000 - step * 80, object_id: `hidden-${step}` };
}

// thread は古い順なので、読み進めるほど時刻が新しくなる。
function threadCursorAt(step: number): TimelineCursor {
  return { created_at: 1 + step * 80, object_id: `hidden-${step}` };
}

function renderDataHook(api: DesktopApi) {
  const harness = createShellHookHarness();
  const refs = {
    loadTopicsRequestRef: { current: new Map<string, number>() },
    remoteObjectUrlRef: { current: new Map<string, string>() },
    draftPreviewUrlRef: { current: new Map<string, string>() },
    directMessageDraftPreviewUrlRef: { current: new Map<string, string>() },
    mediaFetchAttemptRef: { current: new Map<string, number>() },
    draftSequenceRef: { current: 0 },
  };
  const view = renderHook(
    () => useDesktopShellData({ api, translate: stubTranslate, ...refs }),
    { wrapper: harness.wrapper }
  );
  return { harness, view };
}

beforeEach(() => {
  resetWindowHash();
  vi.useFakeTimers();
  listenMock.mockReset();
  listenMock.mockResolvedValue(() => undefined);
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
});

test('行が 0 件のまま読み進めた cursor を、refresh が先頭の cursor に戻さない', async () => {
  const baseApi = createDesktopMockApi();
  // 先頭のページも続きのページも、非表示の著者だけの範囲(空の items + next_cursor)。
  const listTimeline = vi.fn(
    async (_topic: string, cursor?: TimelineCursor | null): Promise<TimelineView> => {
      const step = cursor ? Number(cursor.object_id.replace('hidden-', '')) : 0;
      return { items: [], next_cursor: cursorAt(step + 1) };
    }
  );
  const api: DesktopApi = { ...baseApi, listTimeline };
  const { harness, view } = renderDataHook(api);
  await act(async () => {
    await vi.advanceTimersByTimeAsync(0);
  });
  expect(harness.store.getState().timelineNextCursorByKey[KEY]).toEqual(cursorAt(1));

  // 自動の読み込みで 3 ページ読み進める。
  for (let index = 0; index < 3; index += 1) {
    await act(async () => {
      await view.result.current.loadMoreTimeline('kukuri:topic:general');
    });
  }
  expect(harness.store.getState().timelineNextCursorByKey[KEY]).toEqual(cursorAt(4));

  // 3 秒ごとの refresh(buffer)。
  await act(async () => {
    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL_MS);
  });
  const after = harness.store.getState().timelineNextCursorByKey[KEY];
  // eslint-disable-next-line no-console
  console.log('AUDIT cursor after refresh', JSON.stringify(after));
  expect(after).toEqual(cursorAt(4));
  view.unmount();
});

test('thread: root だけが見えているあいだに読み進めた cursor を、refresh が戻さない', async () => {
  const baseApi = createDesktopMockApi();
  const root = {
    object_id: 'thread-root',
    envelope_id: 'envelope-thread-root',
    author_pubkey: 'a'.repeat(64),
    author_name: 'alice',
    author_display_name: null,
    following: false,
    followed_by: false,
    mutual: false,
    friend_of_friend: false,
    object_kind: 'post',
    is_threadable: true,
    content: 'root',
    content_status: 'Available',
    attachments: [],
    created_at: 1,
    reply_to: null,
    root_id: 'thread-root',
    channel_id: null,
    audience_label: 'Public',
  } as unknown as PostView;
  const listThread = vi.fn(
    async (
      _topic: string,
      _thread: string,
      cursor?: TimelineCursor | null
    ): Promise<TimelineView> => {
      if (!cursor) return { items: [root], next_cursor: threadCursorAt(1) };
      const step = Number(cursor.object_id.replace('hidden-', ''));
      return { items: [], next_cursor: threadCursorAt(step + 1) };
    }
  );
  const api: DesktopApi = { ...baseApi, listThread };
  const { harness, view } = renderDataHook(api);
  await act(async () => {
    await vi.advanceTimersByTimeAsync(0);
  });
  await act(async () => {
    await view.result.current.refreshVisibleShellData('kukuri:topic:general', 'thread-root', 'apply');
  });
  expect(harness.store.getState().threadNextCursorById['thread-root']).toEqual(threadCursorAt(1));
  for (let index = 0; index < 3; index += 1) {
    await act(async () => {
      await view.result.current.loadMoreThread('kukuri:topic:general', 'thread-root');
    });
  }
  expect(harness.store.getState().threadNextCursorById['thread-root']).toEqual(threadCursorAt(4));
  await act(async () => {
    await view.result.current.refreshVisibleShellData('kukuri:topic:general', 'thread-root', 'buffer');
  });
  expect(harness.store.getState().threadNextCursorById['thread-root']).toEqual(threadCursorAt(4));
  view.unmount();
});
