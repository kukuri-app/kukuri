// #1239: 非表示の著者の投稿が続く範囲では、取得が「空の items + next_cursor」を返す。画面は、行が 0 件でも
// 続きを読む手段を描く(独立監査 PR #1270 の再現 test を恒久化)。
import { render, screen } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';

import { TimelineFeed } from './TimelineFeed';

const baseProps = {
  posts: [],
  emptyCopy: 'No posts in this topic yet.',
  onOpenAuthor: vi.fn(),
  onOpenThread: vi.fn(),
  onReply: vi.fn(),
};

afterEach(() => {
  vi.unstubAllGlobals();
});

test('空のページでも next_cursor があれば、続きを読む手段を描く(IntersectionObserver 無し)', () => {
  vi.stubGlobal('IntersectionObserver', undefined);
  // jsdom では 'IntersectionObserver' in window を false にするため、property ごと消す。
  delete (window as unknown as Record<string, unknown>).IntersectionObserver;
  const onLoadMore = vi.fn();
  render(<TimelineFeed {...baseProps} hasMore onLoadMore={onLoadMore} />);
  expect(screen.getByRole('button')).toBeInTheDocument();
});

test('空のページでも next_cursor があれば、sentinel を観測して続きを読む(IntersectionObserver あり)', () => {
  const observe = vi.fn();
  class FakeObserver {
    constructor(private readonly callback: IntersectionObserverCallback) {}
    observe(target: Element) {
      observe(target);
      this.callback(
        [{ isIntersecting: true, target } as IntersectionObserverEntry],
        this as unknown as IntersectionObserver
      );
    }
    disconnect() {}
    unobserve() {}
    takeRecords() {
      return [];
    }
  }
  vi.stubGlobal('IntersectionObserver', FakeObserver);
  const onLoadMore = vi.fn();
  render(<TimelineFeed {...baseProps} hasMore onLoadMore={onLoadMore} />);
  expect(onLoadMore).toHaveBeenCalled();
});
