import { describe, expect, test } from 'vitest';

import type { PublicChannelListView, PublicChannelView } from '@/lib/api';
import { MAX_VISIBLE_POSTS } from '@/shell/pagination';
import { KNOWN_PUBLIC_THREAD_LIMIT, rememberPublicThreads } from '@/shell/slices/publicThreads';

import { INITIAL_PUBLIC_THREAD_LIST, withHeadPage, withNextPage } from './publicThreadListState';

function thread(index: number, lastPostAt = index): PublicChannelView {
  return {
    channel_id: `pc-${1000 + index}-${'a'.repeat(16)}`,
    topic_id: 'kukuri:topic:general',
    title: `Thread ${index}`,
    creator_pubkey: 'b'.repeat(64),
    created_at: index,
    last_post_at: lastPostAt,
    live_now: false,
  };
}

function page(items: PublicChannelView[], more: boolean): PublicChannelListView {
  const last = items.at(-1);
  return {
    items,
    next_cursor: more && last ? { at: last.last_post_at, channel_id: last.channel_id } : null,
    unavailable_count: 0,
  };
}

// 新しい順の index の列(大きいほど新しい)。
const range = (from: number, count: number) => Array.from({ length: count }, (_, offset) => thread(from - offset));

describe('public thread list rows', () => {
  test('the head page goes in front of the loaded pages and keeps their continuation', () => {
    const loaded = withNextPage(withHeadPage(INITIAL_PUBLIC_THREAD_LIST, page(range(100, 20), true), 'last_post'),
      page(range(80, 20), true));
    expect(loaded.rows).toHaveLength(40);
    const continuation = loaded.next;

    // 新しいスレッドが 1 件と、続きのページの行の 1 件が先頭へ動いた。
    const moved = thread(70, 200);
    const head = withHeadPage(loaded, page([moved, thread(101), ...range(100, 18)], true), 'last_post');
    expect(head.rows.map((row) => row.title).slice(0, 3)).toEqual(['Thread 70', 'Thread 101', 'Thread 100']);
    expect(head.rows.filter((row) => row.title === 'Thread 70')).toHaveLength(1);
    expect(head.rows).toHaveLength(41);
    expect(head.next).toEqual(continuation);
    expect(head.status).toBe('ready');
  });

  test('the first page alone continues from where the page ends', () => {
    const first = withHeadPage(INITIAL_PUBLIC_THREAD_LIST, page(range(100, 20), true), 'created');
    expect(first.next).toEqual({ at: 81, channel_id: thread(81).channel_id });
    const exhausted = withHeadPage(INITIAL_PUBLIC_THREAD_LIST, page(range(5, 5), false), 'created');
    expect(exhausted.next).toBeNull();
  });

  test('rows stay within the shown limit: the head drops the oldest rows and loading more drops the newest', () => {
    let state = withHeadPage(INITIAL_PUBLIC_THREAD_LIST, page(range(1000, 20), true), 'last_post');
    for (let from = 980; state.rows.length < MAX_VISIBLE_POSTS; from -= 20) {
      state = withNextPage(state, page(range(from, 20), true));
    }
    expect(state.slid).toBe(false);

    // 上限で先頭のページを重ねると、古い側の行を外し、続きは残した最後の行から読む。
    const head = withHeadPage(state, page([thread(2000), ...range(1000, 19)], true), 'last_post');
    expect(head.rows).toHaveLength(MAX_VISIBLE_POSTS);
    expect(head.next).toEqual({ at: head.rows.at(-1)!.last_post_at, channel_id: head.rows.at(-1)!.channel_id });

    // 上限で続きを読むと、先頭の行を外して読んだ側へ進む。
    const next = withNextPage(state, page(range(800, 20), true));
    expect(next.rows).toHaveLength(MAX_VISIBLE_POSTS);
    expect(next.rows[0]?.title).toBe('Thread 980');
    expect(next.rows.at(-1)?.title).toBe('Thread 781');
    expect(next.slid).toBe(true);
  });
});

describe('remembered public threads', () => {
  test('unchanged threads keep the same value and changed threads move to the newest side', () => {
    const first = rememberPublicThreads({}, [thread(1), thread(2)]);
    expect(rememberPublicThreads(first, [thread(1), thread(2)])).toBe(first);

    const updated = rememberPublicThreads(first, [thread(1, 50)]);
    expect(Object.keys(updated)).toEqual([thread(2).channel_id, thread(1).channel_id]);
    expect(updated[thread(1).channel_id]?.last_post_at).toBe(50);
  });

  test('at most the limit is kept, dropping the oldest remembered first', () => {
    const threads = Array.from({ length: KNOWN_PUBLIC_THREAD_LIMIT + 2 }, (_, index) => thread(index));
    const known = rememberPublicThreads({}, threads);
    expect(Object.keys(known)).toHaveLength(KNOWN_PUBLIC_THREAD_LIMIT);
    expect(known[thread(0).channel_id]).toBeUndefined();
    expect(known[thread(1).channel_id]).toBeUndefined();
    expect(known[thread(2).channel_id]).toBeDefined();
  });
});
