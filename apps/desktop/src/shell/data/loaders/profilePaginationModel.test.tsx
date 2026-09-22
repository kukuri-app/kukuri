// #1278: profile pagination must keep the same no-gap cursor contract as the main timeline.
// Exercise refresh, new arrivals, empty filtered pages, and load-more in reproducible random orders.
import type { ReactNode } from 'react';
import { act, renderHook } from '@testing-library/react';
import { beforeEach, expect, test, vi } from 'vitest';

import type { DesktopApi, PostView, TimelineCursor, TimelineView } from '@/lib/api';
import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import { useDesktopShellSectionLoaders } from '@/shell/data/loaders/useDesktopShellSectionLoaders';
import { useNotificationLoaders } from '@/shell/data/loaders/useNotificationLoaders';
import {
  createDesktopShellStore,
  DesktopShellStoreContext,
} from '@/shell/store';

const LIMIT = 3;
const SCAN_CAP = 7;

type Row = { id: string; createdAt: number; hidden: boolean };

function post(row: Row): PostView {
  return {
    object_id: row.id,
    envelope_id: `envelope-${row.id}`,
    author_pubkey: 'a'.repeat(64),
    author_name: 'alice',
    author_display_name: null,
    following: false,
    followed_by: false,
    mutual: false,
    friend_of_friend: false,
    object_kind: 'post',
    is_threadable: true,
    content: row.id,
    content_status: 'Available',
    attachments: [],
    created_at: row.createdAt,
    reply_to: null,
    root_id: row.id,
    channel_id: null,
    audience_label: 'Public',
  } as unknown as PostView;
}

function isOlder(row: Row, cursor: TimelineCursor): boolean {
  return row.createdAt < cursor.created_at ||
    (row.createdAt === cursor.created_at && row.id < cursor.object_id);
}

function page(rows: Row[], cursor: TimelineCursor | null | undefined): TimelineView {
  const ordered = [...rows]
    .sort((left, right) =>
      right.createdAt !== left.createdAt
        ? right.createdAt - left.createdAt
        : right.id < left.id ? -1 : right.id > left.id ? 1 : 0
    )
    .filter((row) => !cursor || isOlder(row, cursor));
  const items: Row[] = [];
  let scanned = 0;
  for (const row of ordered) {
    scanned += 1;
    if (!row.hidden) {
      items.push(row);
      if (items.length >= LIMIT) {
        return {
          items: items.map(post),
          next_cursor: scanned === ordered.length
            ? null
            : { created_at: row.createdAt, object_id: row.id },
        };
      }
    }
    if (scanned >= SCAN_CAP && scanned < ordered.length) {
      return {
        items: items.map(post),
        next_cursor: { created_at: row.createdAt, object_id: row.id },
      };
    }
  }
  return { items: items.map(post), next_cursor: null };
}

function random(seed: number) {
  let state = seed >>> 0;
  return () => {
    state = (state * 1664525 + 1013904223) >>> 0;
    return state / 2 ** 32;
  };
}

function visibleIds(rows: Row[]): string[] {
  return rows
    .filter((row) => !row.hidden)
    .sort((left, right) => right.createdAt - left.createdAt || (right.id < left.id ? -1 : 1))
    .map((row) => row.id);
}

beforeEach(() => {
  window.history.replaceState(null, '', '/');
});

async function runScenario(seed: number) {
  const next = random(seed);
  const rows: Row[] = [];
  let clock = 1_000;
  let sequence = 0;
  const addRow = (hidden: boolean) => {
    sequence += 1;
    clock += 1 + Math.floor(next() * 2);
    rows.push({ id: `p${String(sequence).padStart(3, '0')}`, createdAt: clock, hidden });
  };
  for (let index = 0; index < 30; index += 1) addRow(next() < 0.55);
  if (next() < 0.5) {
    const run = SCAN_CAP * 2 + Math.floor(next() * SCAN_CAP * 2);
    for (let index = 0; index < run; index += 1) addRow(true);
  }

  const baseApi = createDesktopMockApi();
  const profile = await baseApi.getMyProfile();
  const api: DesktopApi = {
    ...baseApi,
    listProfileTimeline: vi.fn(async (_pubkey, cursor) => page(rows, cursor)),
  };
  const store = createDesktopShellStore();
  const wrapper = ({ children }: { children: ReactNode }) => (
    <DesktopShellStoreContext.Provider value={store}>{children}</DesktopShellStoreContext.Provider>
  );
  const view = renderHook(() => {
    const { loadNotificationsSection } = useNotificationLoaders({
      api,
      translate: (key) => key,
      activePrimarySection: 'notifications',
    });
    return useDesktopShellSectionLoaders({
      api,
      loadReactionCatalogData: async () => undefined,
      loadNotificationsSection,
      storeApi: store,
      translate: (key) => key,
    });
  }, { wrapper });

  const trace: string[] = [];
  await act(async () => view.result.current.loadProfileSection());
  expect(store.getState().localProfile?.pubkey).toBe(profile.pubkey);
  for (let step = 0; step < 14; step += 1) {
    if (next() < 0.55) {
      trace.push('loadMore');
      await act(async () => view.result.current.loadMoreProfileTimeline());
    } else {
      const count = 1 + Math.floor(next() * 4);
      trace.push(`arrive${count}`);
      for (let index = 0; index < count; index += 1) addRow(next() < 0.4);
      await act(async () => view.result.current.loadProfileSection());
    }
  }
  await act(async () => view.result.current.loadProfileSection());
  for (let guard = 0; guard < 80 && store.getState().profileTimelineNextCursor; guard += 1) {
    await act(async () => view.result.current.loadMoreProfileTimeline());
    // Refresh between pages catches cursor rollback after a row-less page.
    await act(async () => view.result.current.loadProfileSection());
  }
  const shown = store.getState().profileTimeline.map((item) => item.object_id);
  const expected = visibleIds(rows);
  view.unmount();
  return { shown, expected, trace };
}

test('refreshと追加取得の順序を変えても、プロフィール投稿を欠落・重複なく新しい順に表示する', async () => {
  const failures: string[] = [];
  for (let seed = 1; seed <= 100; seed += 1) {
    const { shown, expected, trace } = await runScenario(seed);
    if (JSON.stringify(shown) !== JSON.stringify(expected)) {
      failures.push(`seed=${seed} trace=${trace.join(',')} shown=${shown.join(',')}`);
    }
  }
  expect(failures).toEqual([]);
}, 60_000);
