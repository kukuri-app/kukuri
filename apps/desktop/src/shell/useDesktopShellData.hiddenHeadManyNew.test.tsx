// #1239: 表示できる行が少なく、その先の非表示の著者の範囲を読み進めた利用者に、1 ページを超える新着が届いてから
// 適用しても、新着が欠けない(独立監査 PR #1270 の再現 test を恒久化)。
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

test('表示できる 1 行の先の非表示の範囲を読み進めた後に 1 ページを超える新着を適用しても、新着が欠けない', async () => {
  const at = (p: PostView): TimelineCursor => ({ created_at: p.created_at, object_id: p.object_id });
  const news = [post('n1', 140), post('n2', 130), post('n3', 120), post('n4', 110)];
  const r1 = post('r1', 100);
  const hidden1: TimelineCursor = { created_at: 90, object_id: 'h1' };
  const hidden2: TimelineCursor = { created_at: 50, object_id: 'h2' };
  let arrived = false;
  const baseApi = createDesktopMockApi();
  // 1 ページ 3 行。新着の前は、先頭から非表示の著者の範囲(空のページ + 続きの位置)。
  const listTimeline = vi.fn(
    async (_topic: string, cursor?: TimelineCursor | null): Promise<TimelineView> => {
      if (!cursor) {
        return arrived
          ? { items: news.slice(0, 3), next_cursor: at(news[2]) }
          : { items: [r1], next_cursor: hidden1 };
      }
      if (cursor.object_id === 'n3') return { items: [news[3], r1], next_cursor: hidden1 };
      if (cursor.object_id === 'h1') return { items: [], next_cursor: hidden2 };
      return { items: [], next_cursor: null };
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
  expect(harness.store.getState().timelineNextCursorByKey[KEY]).toEqual(hidden2);
  arrived = true;
  await act(async () => {
    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL_MS);
  });
  await act(async () => {
    await view.result.current.refreshTimelineFeed('kukuri:topic:general', null);
  });
  // 利用者は続きへ scroll する(周期の refresh を挟む)。
  for (let guard = 0; guard < 10; guard += 1) {
    if (!harness.store.getState().timelineNextCursorByKey[KEY]) break;
    await act(async () => {
      await view.result.current.loadMoreTimeline('kukuri:topic:general');
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL_MS);
    });
  }
  const shown = (harness.store.getState().timelinesByKey[KEY] ?? []).map((p) => p.object_id);
  expect(shown).toEqual(['n1', 'n2', 'n3', 'n4', 'r1']);
  view.unmount();
});
