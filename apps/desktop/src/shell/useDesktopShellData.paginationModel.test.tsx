// #1239: タイムラインの続きの読み込み・周期の refresh・新着の適用を、乱数で作った順に重ねても、最後まで読むと
// 表示できる投稿がすべて 1 回ずつ、新しい順に出る。
//
// backend は、非表示の著者の行の読み飛ばしに上限があり、上限に達すると「行の少ない(空の)ページ + 続きの位置」を
// 返す(`filtered_timeline_page`)。画面側の続きの位置の扱いは、この形の組み合わせで 3 回続けて不具合が
// 見つかったので(独立監査 PR #1270)、個別の再現 test に加えて、組み合わせを乱数で広く確かめる。
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
const TOPIC = 'kukuri:topic:general';
const KEY = `${TOPIC}::public`;
/// 1 回の取得で返す行数(画面の `limit` に当たる)。
const LIMIT = 3;
/// 1 回の取得で読む行数の上限(backend の「ページ数の上限 × ページの大きさ」に当たる)。
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
  return (
    row.createdAt < cursor.created_at ||
    (row.createdAt === cursor.created_at && row.id < cursor.object_id)
  );
}

/// `filtered_timeline_page` と同じ規則のページ。
function page(rows: Row[], cursor: TimelineCursor | null | undefined): TimelineView {
  const ordered = [...rows]
    .sort((left, right) =>
      right.createdAt !== left.createdAt
        ? right.createdAt - left.createdAt
        : right.id < left.id
          ? -1
          : right.id > left.id
            ? 1
            : 0
    )
    .filter((row) => !cursor || isOlder(row, cursor));
  const items: Row[] = [];
  let scanned = 0;
  for (const row of ordered) {
    scanned += 1;
    if (!row.hidden) {
      items.push(row);
      if (items.length >= LIMIT) {
        const exhausted = scanned === ordered.length;
        return {
          items: items.map(post),
          next_cursor: exhausted ? null : { created_at: row.createdAt, object_id: row.id },
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

/// 再現できる乱数。
function random(seed: number) {
  let state = seed >>> 0;
  return () => {
    state = (state * 1664525 + 1013904223) >>> 0;
    return state / 2 ** 32;
  };
}

beforeEach(() => {
  resetWindowHash();
  vi.useFakeTimers();
  listenMock.mockReset();
  listenMock.mockResolvedValue(() => undefined);
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
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
  // 初めの状態: 表示できる行と、非表示の著者の行のかたまりが混ざった履歴。
  for (let index = 0; index < 30; index += 1) {
    addRow(next() < 0.55);
  }
  // 半分の履歴では、新しい側に非表示の著者の行を長く続ける(先頭のページが空になり、表示できる行が 0 件のまま
  // 読み進める形)。
  if (next() < 0.5) {
    const run = SCAN_CAP * 2 + Math.floor(next() * SCAN_CAP * 2);
    for (let index = 0; index < run; index += 1) {
      addRow(true);
    }
  }

  const baseApi = createDesktopMockApi();
  const listTimeline = vi.fn(
    async (_topic: string, cursor?: TimelineCursor | null): Promise<TimelineView> =>
      page(rows, cursor ?? null)
  );
  const api: DesktopApi = { ...baseApi, listTimeline };
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
  await act(async () => {
    await vi.advanceTimersByTimeAsync(0);
  });

  const trace: string[] = [];
  for (let step = 0; step < 14; step += 1) {
    const roll = next();
    if (roll < 0.4) {
      trace.push('loadMore');
      await act(async () => {
        await view.result.current.loadMoreTimeline(TOPIC);
      });
    } else if (roll < 0.65) {
      const count = 1 + Math.floor(next() * 3);
      trace.push(`arrive${count}`);
      for (let index = 0; index < count; index += 1) {
        const hidden = next() < 0.4;
        addRow(hidden);
      }
      await act(async () => {
        await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL_MS);
      });
    } else if (roll < 0.85) {
      trace.push('apply');
      await act(async () => {
        await view.result.current.refreshTimelineFeed(TOPIC, null);
      });
    } else {
      trace.push('tick');
      await act(async () => {
        await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL_MS);
      });
    }
  }
  // 最後に新着を適用し、続きが無くなるまで読む。読むあいだにも周期の refresh が走る(refresh が読み進めた位置を
  // 巻き戻すと、ここで最後まで届かない)。
  await act(async () => {
    await view.result.current.refreshTimelineFeed(TOPIC, null);
  });
  for (let guard = 0; guard < 60; guard += 1) {
    if (!harness.store.getState().timelineNextCursorByKey[KEY]) {
      break;
    }
    await act(async () => {
      await view.result.current.loadMoreTimeline(TOPIC);
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL_MS);
    });
  }
  const shown = (harness.store.getState().timelinesByKey[KEY] ?? []).map((item) => item.object_id);
  const expected = visibleIds(rows);
  view.unmount();
  return { shown, expected, trace };
}

function visibleIds(rows: Row[]): string[] {
  return rows
    .filter((row) => !row.hidden)
    .sort((left, right) => right.createdAt - left.createdAt || (right.id < left.id ? -1 : 1))
    .map((row) => row.id);
}

test('続きの読み込み・refresh・新着の適用をどう重ねても、表示できる投稿がすべて 1 回ずつ出る', async () => {
  const failures: string[] = [];
  for (let seed = 1; seed <= 150; seed += 1) {
    const { shown, expected, trace } = await runScenario(seed);
    const missing = expected.filter((id) => !shown.includes(id));
    const duplicated = shown.filter((id, index) => shown.indexOf(id) !== index);
    const ordered = JSON.stringify(shown) === JSON.stringify(expected);
    if (missing.length > 0 || duplicated.length > 0 || !ordered) {
      failures.push(
        `seed=${seed} trace=${trace.join(',')} missing=${missing.join(',')} duplicated=${duplicated.join(',')}`
      );
    }
  }
  expect(failures).toEqual([]);
}, 60_000);
