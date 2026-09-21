// #1239: 表示できる行の先で非表示の著者の範囲を読み進めた後に新着を適用しても、表示していた行を消さない
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

test('読み進めた後に新着を適用しても、先頭のページから押し出された行を飛ばさない', async () => {
  const r = [post('r1', 100), post('r2', 90), post('r3', 80)];
  const n1 = post('n1', 110);
  const at = (p: PostView): TimelineCursor => ({ created_at: p.created_at, object_id: p.object_id });
  // 1 ページ 3 行。r3 の先は非表示の著者の範囲(空のページ + 続きの位置)。
  const hidden1: TimelineCursor = { created_at: 40, object_id: 'h1' };
  const hidden2: TimelineCursor = { created_at: 20, object_id: 'h2' };
  let arrived = false;
  const baseApi = createDesktopMockApi();
  const listTimeline = vi.fn(
    async (_topic: string, cursor?: TimelineCursor | null): Promise<TimelineView> => {
      if (!cursor) {
        return arrived
          ? { items: [n1, r[0], r[1]], next_cursor: at(r[1]) }
          : { items: r, next_cursor: at(r[2]) };
      }
      if (cursor.object_id === 'r3') return { items: [], next_cursor: hidden1 };
      if (cursor.object_id === 'h1') return { items: [], next_cursor: hidden2 };
      return { items: [], next_cursor: null };
    }
  );
  const api: DesktopApi = { ...baseApi, listTimeline };
  const { harness, view } = renderDataHook(api);
  await act(async () => {
    await vi.advanceTimersByTimeAsync(0);
  });
  for (let index = 0; index < 2; index += 1) {
    await act(async () => {
      await view.result.current.loadMoreTimeline('kukuri:topic:general');
    });
  }
  expect(harness.store.getState().timelineNextCursorByKey[KEY]).toEqual(hidden2);
  arrived = true;
  await act(async () => {
    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL_MS);
  });
  expect(harness.store.getState().pendingTimelineCountsByKey[KEY]).toBe(1);
  await act(async () => {
    await view.result.current.refreshTimelineFeed('kukuri:topic:general', null);
  });
  const state = harness.store.getState();
  const shown = (state.timelinesByKey[KEY] ?? []).map((p) => p.object_id);
  const cursor = state.timelineNextCursorByKey[KEY];
  // 続きの位置が r3 より先なら、r3 は表示中でなければならない。
  const covered = shown.includes('r3') || cursor?.object_id === 'r2';
  expect({ shown, cursor, covered }).toEqual(expect.objectContaining({ covered: true }));
  view.unmount();
});
