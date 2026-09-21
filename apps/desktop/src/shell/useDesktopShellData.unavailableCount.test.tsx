// #1239 AC-4: 取得が返した「まだ取得できていない投稿の数」を、読んだ範囲ごとに store へ残す。周期の refresh が
// 読んだ範囲を残すあいだは、その範囲の数も残す(先頭のページの 0 で消さない)。
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

function cursorAt(step: number): TimelineCursor {
  return { created_at: 10_000 - step * 80, object_id: `range-${step}` };
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

test('続きのページが返した数を残し、読んだ範囲を残す refresh では消さない', async () => {
  const baseApi = createDesktopMockApi();
  // 先頭のページは 0 件の欠け。続きのページは、まだ取得できていない投稿 5 件を含む範囲。
  const listTimeline = vi.fn(
    async (_topic: string, cursor?: TimelineCursor | null): Promise<TimelineView> => {
      if (!cursor) return { items: [], next_cursor: cursorAt(1), unavailable_count: 0 };
      const step = Number(cursor.object_id.replace('range-', ''));
      return { items: [], next_cursor: cursorAt(step + 1), unavailable_count: 5 };
    }
  );
  const api: DesktopApi = { ...baseApi, listTimeline };
  const { harness, view } = renderDataHook(api);
  await act(async () => {
    await vi.advanceTimersByTimeAsync(0);
  });
  expect(harness.store.getState().timelineUnavailableByKey[KEY] ?? 0).toBe(0);

  await act(async () => {
    await view.result.current.loadMoreTimeline('kukuri:topic:general');
  });
  expect(harness.store.getState().timelineUnavailableByKey[KEY]).toBe(5);

  // 周期の refresh(buffer)は、読み進めた範囲を残す。その範囲の数も残す。
  await act(async () => {
    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL_MS);
  });
  expect(harness.store.getState().timelineNextCursorByKey[KEY]).toEqual(cursorAt(2));
  expect(harness.store.getState().timelineUnavailableByKey[KEY]).toBe(5);
  view.unmount();
});

test('thread の続きのページが返した数を残す', async () => {
  const baseApi = createDesktopMockApi();
  const listThread = vi.fn(
    async (
      _topic: string,
      _thread: string,
      cursor?: TimelineCursor | null
    ): Promise<TimelineView> =>
      cursor
        ? { items: [], next_cursor: null, unavailable_count: 2 }
        : { items: [], next_cursor: cursorAt(1), unavailable_count: 0 }
  );
  const api: DesktopApi = { ...baseApi, listThread };
  const { harness, view } = renderDataHook(api);
  await act(async () => {
    await vi.advanceTimersByTimeAsync(0);
  });
  await act(async () => {
    await view.result.current.refreshVisibleShellData('kukuri:topic:general', 'thread-root', 'apply');
  });
  await act(async () => {
    await view.result.current.loadMoreThread('kukuri:topic:general', 'thread-root');
  });
  expect(harness.store.getState().threadUnavailableById['thread-root']).toBe(2);
  view.unmount();
});

function post(id: string, createdAt: number): PostView {
  return {
    object_id: id,
    envelope_id: `envelope-${id}`,
    author_pubkey: 'a'.repeat(64),
    author_name: 'alice',
    author_display_name: null,
    following: false,
    followed_by: false,
    mutual: false,
    friend_of_friend: false,
    object_kind: 'post',
    is_threadable: true,
    content: id,
    content_status: 'Available',
    attachments: [],
    created_at: createdAt,
    reply_to: null,
    root_id: id,
    channel_id: null,
    audience_label: 'Public',
  } as unknown as PostView;
}

// 独立監査 B2: 1 ページを超える新着を保留して適用し、読んだ範囲を捨てるときは、数も保留した先頭のページの数に置き換える。
test('保留した新着の適用で読んだ範囲を捨てるときは、先頭のページの数に置き換える', async () => {
  const baseApi = createDesktopMockApi();
  let phase: 'initial' | 'newer' = 'initial';
  const listTimeline = vi.fn(
    async (_topic: string, cursor?: TimelineCursor | null): Promise<TimelineView> => {
      if (cursor) return { items: [], next_cursor: cursorAt(2), unavailable_count: 5 };
      if (phase === 'initial') {
        return { items: [post('old-2', 1_000), post('old-1', 990)], next_cursor: cursorAt(1), unavailable_count: 0 };
      }
      // 表示中の行と重ならない、新しい側だけのページ(あいだができた)。
      return { items: [post('new-2', 5_000), post('new-1', 4_990)], next_cursor: cursorAt(9), unavailable_count: 1 };
    }
  );
  const api: DesktopApi = { ...baseApi, listTimeline };
  const { harness, view } = renderDataHook(api);
  await act(async () => {
    await vi.advanceTimersByTimeAsync(0);
  });
  await act(async () => {
    await view.result.current.loadMoreTimeline('kukuri:topic:general');
  });
  expect(harness.store.getState().timelineUnavailableByKey[KEY]).toBe(5);

  phase = 'newer';
  await act(async () => {
    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL_MS);
  });
  expect(harness.store.getState().pendingTimelineCountsByKey[KEY]).toBe(2);
  expect(harness.store.getState().timelineUnavailableByKey[KEY]).toBe(5);

  await act(async () => {
    view.result.current.applyPendingTimeline('kukuri:topic:general');
  });
  expect(harness.store.getState().timelinesByKey[KEY]?.map((item) => item.object_id)).toEqual([
    'new-2',
    'new-1',
  ]);
  expect(harness.store.getState().timelineUnavailableByKey[KEY]).toBe(1);
  view.unmount();
});
