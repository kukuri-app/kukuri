/**
 * WP-S5: timelineMerge.ts(純関数 5 本)の characterization テスト。
 *
 * 後続 WP-H6(shell の store スライス化・selector 化・prop drilling 除去)の
 * 安全網として「観測した現挙動」をそのまま固定する。このテストが落ちた場合に
 * 疑うべきは直近の変更でありテストではない。期待値は生リテラルで書き、
 * 実装側の定数・関数から期待値を生成しない。
 */
import { beforeEach, describe, expect, test } from 'vitest';

import type { PostView } from '@/lib/api';

import { buildPaginatedPost } from '../DesktopShellPage.testHelpers';
import {
  cursorIsBeyond,
  cursorIsBeyondVisiblePosts,
  hasReadPastHeadPage,
  headPageReachesVisiblePosts,
  hasLoadedOlderAuthoritativePosts,
  mergeRefreshedVisiblePosts,
  mergeUniquePosts,
  postIdentityKey,
  uniquePostsByIdentity,
} from './timelineMerge';

// 各テストを独立させる(setup.ts は hash を掃除しない)。
beforeEach(() => {
  window.history.replaceState(null, '', '/');
});

// object_id だけ差し替えた PostView fixture(検証はフィールド単位で行う)。
function post(objectId: string, overrides: Partial<PostView> = {}): PostView {
  return buildPaginatedPost(1, { object_id: objectId, ...overrides });
}

function ids(posts: PostView[]): string[] {
  return posts.map((entry) => entry.object_id);
}

describe('postIdentityKey', () => {
  const cases: Array<{
    name: string;
    input: Pick<PostView, 'object_id' | 'server_object_id'>;
    expected: string;
  }> = [
    {
      name: 'prefers server_object_id when present',
      input: { object_id: 'local-1', server_object_id: 'server-1' },
      expected: 'server-1',
    },
    {
      name: 'falls back to object_id when server_object_id is null',
      input: { object_id: 'local-1', server_object_id: null },
      expected: 'local-1',
    },
    {
      name: 'falls back to object_id when server_object_id is undefined',
      input: { object_id: 'local-1' },
      expected: 'local-1',
    },
  ];

  test.each(cases)('$name', ({ input, expected }) => {
    expect(postIdentityKey(input)).toBe(expected);
  });
});

describe('uniquePostsByIdentity', () => {
  test('returns an empty array for empty input', () => {
    expect(uniquePostsByIdentity([])).toEqual([]);
  });

  test('keeps the first occurrence when the same object_id appears twice', () => {
    const first = post('post-a', { created_at: 111 });
    const duplicate = post('post-a', { created_at: 222 });

    const result = uniquePostsByIdentity([first, post('post-b'), duplicate]);

    expect(ids(result)).toEqual(['post-a', 'post-b']);
    // 先勝ち: 最初のインスタンスが残る。
    expect(result[0]?.created_at).toBe(111);
  });

  test('dedupes a local post and its authoritative echo via server_object_id', () => {
    const localPost = post('local-1', {
      local_state: 'syncing',
      server_object_id: 'server-1',
    });
    const authoritativeEcho = post('server-1');

    const result = uniquePostsByIdentity([localPost, authoritativeEcho]);

    // identity key はどちらも 'server-1' → 先勝ちで local 側が残る。
    expect(ids(result)).toEqual(['local-1']);
    expect(result[0]?.local_state).toBe('syncing');
  });
});

describe('mergeUniquePosts', () => {
  test('appends only incoming posts whose object_id is not in current', () => {
    const currentB = post('post-b', { created_at: 111 });
    const incomingB = post('post-b', { created_at: 222 });

    const result = mergeUniquePosts(
      [post('post-a'), currentB],
      [incomingB, post('post-c')]
    );

    expect(ids(result)).toEqual(['post-a', 'post-b', 'post-c']);
    // 重複時は current 側のインスタンスが残る。
    expect(result[1]?.created_at).toBe(111);
  });

  test('dedupes by object_id only and ignores server_object_id', () => {
    // 現挙動の固定: postIdentityKey による同一視はせず object_id のみで比較する。
    const localPost = post('local-1', {
      local_state: 'syncing',
      server_object_id: 'server-1',
    });

    const result = mergeUniquePosts([localPost], [post('server-1')]);

    expect(ids(result)).toEqual(['local-1', 'server-1']);
  });

  test('keeps duplicates that appear only within incoming', () => {
    // 現挙動の固定: seen は current 側からのみ構築され incoming 内の重複は除去されない。
    const result = mergeUniquePosts(
      [post('post-a')],
      [post('post-b', { created_at: 111 }), post('post-b', { created_at: 222 })]
    );

    expect(ids(result)).toEqual(['post-a', 'post-b', 'post-b']);
  });
});

describe('hasLoadedOlderAuthoritativePosts', () => {
  function authoritativePosts(count: number): PostView[] {
    return Array.from({ length: count }, (_, index) => post(`auth-${index}`));
  }

  const cases: Array<{
    name: string;
    currentAuthoritativeCount: number;
    incomingCount: number;
    expected: boolean;
  }> = [
    {
      name: 'returns false when authoritative count equals incoming length',
      currentAuthoritativeCount: 2,
      incomingCount: 2,
      expected: false,
    },
    {
      name: 'returns true when authoritative count exceeds incoming length by one',
      currentAuthoritativeCount: 3,
      incomingCount: 2,
      expected: true,
    },
    {
      name: 'returns false when authoritative count is one below incoming length',
      currentAuthoritativeCount: 1,
      incomingCount: 2,
      expected: false,
    },
  ];

  test.each(cases)('$name', ({ currentAuthoritativeCount, incomingCount, expected }) => {
    expect(
      hasLoadedOlderAuthoritativePosts(
        authoritativePosts(currentAuthoritativeCount),
        authoritativePosts(incomingCount)
      )
    ).toBe(expected);
  });

  test('excludes local-state posts from the authoritative count', () => {
    const current = [
      post('local-1', { local_state: 'pending' }),
      post('local-2', { local_state: 'failed' }),
      post('auth-1'),
    ];

    // authoritative は auth-1 の 1 件のみ → incoming 1 件と同数なので false。
    expect(hasLoadedOlderAuthoritativePosts(current, [post('new-1')])).toBe(false);
    // incoming 0 件なら 1 > 0 で true。
    expect(hasLoadedOlderAuthoritativePosts(current, [])).toBe(true);
  });

  test('treats local_state null as authoritative', () => {
    expect(
      hasLoadedOlderAuthoritativePosts([post('auth-1', { local_state: null })], [])
    ).toBe(true);
  });

  test('compares against the raw incoming length without filtering incoming local-state posts', () => {
    const current = [post('auth-1'), post('auth-2')];
    const incomingAllLocal = [
      post('local-1', { local_state: 'pending' }),
      post('local-2', { local_state: 'syncing' }),
    ];

    // quirk の固定: local_state で filter されるのは current 側だけで、incoming は
    // 生の length と比較される。incoming 2 件が全て local_state 付きでも件数に数えられ
    // 2 > 2 → false(incoming 側も filter されるなら 2 > 0 → true になるはず)。
    expect(hasLoadedOlderAuthoritativePosts(current, incomingAllLocal)).toBe(false);
  });
});

describe('mergeRefreshedVisiblePosts', () => {
  // preserveOlderPages 真偽 × optimistic post(local_state 付き)の保持 ×
  // server_object_id による同一視(postIdentityKey)の組合せ表。
  const cases: Array<{
    name: string;
    current: PostView[];
    incoming: PostView[];
    preserveOlderPages: boolean;
    expectedIds: string[];
  }> = [
    {
      name: 'replaces authoritative posts with incoming when preserveOlderPages is false',
      current: [post('old-1'), post('old-2')],
      incoming: [post('new-1')],
      preserveOlderPages: false,
      expectedIds: ['new-1'],
    },
    {
      name: 'appends older authoritative pages after incoming when preserveOlderPages is true',
      current: [post('old-1'), post('old-2')],
      incoming: [post('new-1')],
      preserveOlderPages: true,
      expectedIds: ['new-1', 'old-1', 'old-2'],
    },
    {
      name: 'keeps a pending optimistic post ahead of incoming when preserveOlderPages is false',
      current: [post('local-1', { local_state: 'pending' }), post('old-1')],
      incoming: [post('new-1')],
      preserveOlderPages: false,
      expectedIds: ['local-1', 'new-1'],
    },
    {
      name: 'keeps a pending optimistic post ahead of incoming and older pages when preserveOlderPages is true',
      current: [post('local-1', { local_state: 'pending' }), post('old-1')],
      incoming: [post('new-1')],
      preserveOlderPages: true,
      expectedIds: ['local-1', 'new-1', 'old-1'],
    },
    {
      name: 'drops an optimistic post whose server_object_id is echoed by incoming',
      current: [
        post('local-2', { local_state: 'syncing', server_object_id: 'server-2' }),
        post('old-1'),
      ],
      incoming: [post('server-2')],
      preserveOlderPages: false,
      expectedIds: ['server-2'],
    },
    {
      name: 'keeps an optimistic post whose server_object_id is not echoed by incoming',
      current: [post('local-2', { local_state: 'syncing', server_object_id: 'server-9' })],
      incoming: [post('new-1')],
      preserveOlderPages: false,
      expectedIds: ['local-2', 'new-1'],
    },
    {
      name: 'does not duplicate an older page already present in incoming when preserveOlderPages is true',
      current: [post('server-2'), post('old-1')],
      incoming: [post('server-2', { created_at: 999 })],
      preserveOlderPages: true,
      expectedIds: ['server-2', 'old-1'],
    },
    {
      name: 'matches older pages against incoming via server_object_id when preserveOlderPages is true',
      current: [post('page-1', { server_object_id: 'server-3' })],
      incoming: [post('server-3')],
      preserveOlderPages: true,
      expectedIds: ['server-3'],
    },
  ];

  test.each(cases)('$name', ({ current, incoming, preserveOlderPages, expectedIds }) => {
    expect(ids(mergeRefreshedVisiblePosts(current, incoming, preserveOlderPages))).toEqual(
      expectedIds
    );
  });

  test('replaces the optimistic instance with the authoritative incoming instance', () => {
    const optimistic = post('local-2', {
      created_at: 111,
      local_state: 'syncing',
      server_object_id: 'server-2',
    });
    const authoritative = post('server-2', { created_at: 999 });

    const result = mergeRefreshedVisiblePosts([optimistic], [authoritative], false);

    expect(ids(result)).toEqual(['server-2']);
    // 生き残るのは incoming 側のインスタンス。
    expect(result[0]?.created_at).toBe(999);
  });

  test('prefers the incoming instance when the same id exists in current and incoming', () => {
    const result = mergeRefreshedVisiblePosts(
      [post('server-2', { created_at: 111 }), post('old-1')],
      [post('server-2', { created_at: 999 })],
      true
    );

    expect(ids(result)).toEqual(['server-2', 'old-1']);
    expect(result[0]?.created_at).toBe(999);
  });

  test('dedupes duplicate identities within incoming keeping the first', () => {
    const result = mergeRefreshedVisiblePosts(
      [],
      [post('new-1', { created_at: 111 }), post('new-1', { created_at: 222 }), post('new-2')],
      false
    );

    expect(ids(result)).toEqual(['new-1', 'new-2']);
    expect(result[0]?.created_at).toBe(111);
  });
});

describe('cursorIsBeyond', () => {
  const at = (created_at: number, object_id: string) => ({ created_at, object_id });

  test('新しい順のページでは、古い位置が先', () => {
    expect(cursorIsBeyond(at(90, 'b'), at(100, 'a'), 'desc')).toBe(true);
    expect(cursorIsBeyond(at(100, 'a'), at(100, 'b'), 'desc')).toBe(true);
    expect(cursorIsBeyond(at(110, 'a'), at(100, 'a'), 'desc')).toBe(false);
    expect(cursorIsBeyond(at(100, 'b'), at(100, 'a'), 'desc')).toBe(false);
  });

  test('古い順のページでは、新しい位置が先', () => {
    expect(cursorIsBeyond(at(110, 'a'), at(100, 'b'), 'asc')).toBe(true);
    expect(cursorIsBeyond(at(100, 'b'), at(100, 'a'), 'asc')).toBe(true);
    expect(cursorIsBeyond(at(90, 'z'), at(100, 'a'), 'asc')).toBe(false);
  });

  test('同じ位置と、どちらかが無いときは先ではない', () => {
    expect(cursorIsBeyond(at(100, 'a'), at(100, 'a'), 'desc')).toBe(false);
    expect(cursorIsBeyond(null, at(100, 'a'), 'desc')).toBe(false);
    // 先頭のページに続きが無いとき(全件が 1 ページに収まる)は、古い位置を残さない。
    expect(cursorIsBeyond(at(90, 'a'), null, 'desc')).toBe(false);
  });
});

describe('hasReadPastHeadPage', () => {
  const at = (created_at: number, object_id: string) => ({ created_at, object_id });
  const row = (object_id: string, created_at: number) =>
    post(object_id, { created_at, local_state: null });

  test('新着で先頭のページの続きの位置が動いただけなら、読み進めたとはみなさない', () => {
    const visible = [row('r1', 30), row('r2', 20), row('r3', 10)];
    const incoming = [row('n1', 40), row('r1', 30), row('r2', 20)];
    // 保存している続きの位置は、表示中の最後の行(r3)。先頭のページの続きの位置は r2 へ動いた。
    expect(hasReadPastHeadPage(visible, incoming, at(10, 'r3'), at(20, 'r2'), 'desc')).toBe(false);
  });

  test('行を増やさずに読み進めた位置は残す', () => {
    // 表示できる行が 0 件のまま、続きの位置だけが進んだ。
    expect(hasReadPastHeadPage([], [], at(5, 'hidden-4'), at(9, 'hidden-1'), 'desc')).toBe(true);
    // 表示中の行はあるが、続きの位置はその先まで進んだ。
    expect(
      hasReadPastHeadPage([row('a', 30)], [row('a', 30)], at(5, 'hidden-4'), at(9, 'hidden-1'), 'desc')
    ).toBe(true);
  });

  test('先頭のページと表示中の行のあいだに新着の隙間があれば、読み進めた位置を残さない', () => {
    // 表示中は r1 だけ。その先を読み進めた後に、1 ページ(3 行)を超える新着が届いた。
    const visible = [row('r1', 10)];
    const incoming = [row('n1', 40), row('n2', 30), row('n3', 20)];
    expect(headPageReachesVisiblePosts(visible, incoming)).toBe(false);
    expect(hasReadPastHeadPage(visible, incoming, at(1, 'h9'), at(20, 'n3'), 'desc')).toBe(false);
    // 新着が 1 ページに収まり、先頭のページが表示中の行と重なれば、位置を残す。
    const reaching = [row('n1', 40), row('n2', 30), row('r1', 10)];
    expect(headPageReachesVisiblePosts(visible, reaching)).toBe(true);
    expect(hasReadPastHeadPage(visible, reaching, at(1, 'h9'), at(10, 'r1'), 'desc')).toBe(true);
    // 先頭のページに新しい行が無ければ(非表示の範囲の途中)、あいだは無い。
    expect(headPageReachesVisiblePosts([], [])).toBe(true);
  });

  test('thread は古い順', () => {
    expect(
      hasReadPastHeadPage([row('root', 1)], [row('root', 1)], at(321, 'hidden-4'), at(81, 'hidden-1'), 'asc')
    ).toBe(true);
    expect(cursorIsBeyondVisiblePosts(at(1, 'root'), [row('root', 1)], 'asc')).toBe(false);
  });

  test('local の投稿は、表示中の位置に数えない', () => {
    expect(
      cursorIsBeyondVisiblePosts(
        at(10, 'r3'),
        [post('local-1', { created_at: 5, local_state: 'pending' })],
        'desc'
      )
    ).toBe(true);
  });
});

