// #1851: 反映の途中で先頭のページが続きの位置なしで表示された後、refresh が届けた表示中の先頭より古い行は新着として
// 保留せずに表示へ反映し、続きの位置を得る。表示中の先頭より新しい行だけを新着として保留する。
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
    draftPreviewUrlRef: { current: new Map<string, string>() },
    directMessageDraftPreviewUrlRef: { current: new Map<string, string>() },
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

const at = (p: PostView): TimelineCursor => ({ created_at: p.created_at, object_id: p.object_id });
const p = [post('p5', 50), post('p4', 40), post('p3', 30), post('p2', 20), post('p1', 10)];

async function refreshAfterPartialHead(refreshed: TimelineView) {
  let partial = true;
  const listTimeline = vi.fn(async (_topic: string, cursor?: TimelineCursor | null): Promise<TimelineView> => {
    if (cursor) return { items: [], next_cursor: null };
    return partial ? { items: p.slice(0, 3), next_cursor: null } : refreshed;
  });
  const { harness, view } = renderDataHook({ ...createDesktopMockApi(), listTimeline });
  await act(async () => {
    await vi.advanceTimersByTimeAsync(0);
  });
  expect(harness.store.getState().timelineNextCursorByKey[KEY] ?? null).toBeNull();
  partial = false;
  await act(async () => {
    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL_MS);
  });
  const state = harness.store.getState();
  view.unmount();
  return {
    shown: (state.timelinesByKey[KEY] ?? []).map((row) => row.object_id),
    cursor: state.timelineNextCursorByKey[KEY] ?? null,
    pending: state.pendingTimelineCountsByKey[KEY] ?? 0,
  };
}

test('新着が無ければ、残りの行と続きの位置を表示へ反映する', async () => {
  expect(await refreshAfterPartialHead({ items: p, next_cursor: at(p[4]) })).toEqual({
    shown: ['p5', 'p4', 'p3', 'p2', 'p1'],
    cursor: at(p[4]),
    pending: 0,
  });
});

test('新着があっても、表示中の先頭より古い行と続きの位置は表示へ反映し、新着だけを保留する', async () => {
  const n1 = post('n1', 60);
  expect(await refreshAfterPartialHead({ items: [n1, ...p.slice(0, 4)], next_cursor: at(p[3]) })).toEqual({
    shown: ['p5', 'p4', 'p3', 'p2'],
    cursor: at(p[3]),
    pending: 1,
  });
});
