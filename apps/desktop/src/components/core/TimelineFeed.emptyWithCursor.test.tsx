// #1239: 非表示の著者の投稿が続く範囲では、取得が「空の items + next_cursor」を返す。画面は、行が 0 件でも
// 続きを読む手段を描く(独立監査 PR #1270 の再現 test を恒久化)。
import { fireEvent, render, screen } from '@testing-library/react';
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

test('one intersection loads one page and leaves an explicit way to continue', () => {
  class FakeObserver {
    constructor(private readonly callback: IntersectionObserverCallback) {}
    observe(target: Element) {
      this.callback([{ isIntersecting: true, target } as IntersectionObserverEntry],
        this as unknown as IntersectionObserver);
    }
    disconnect() {}
  }
  vi.stubGlobal('IntersectionObserver', FakeObserver);
  const onLoadMore = vi.fn();
  const view = render(<TimelineFeed {...baseProps} hasMore onLoadMore={onLoadMore} />);
  expect(onLoadMore).toHaveBeenCalledTimes(1);
  view.rerender(<TimelineFeed {...baseProps} hasMore loadingMore onLoadMore={onLoadMore} />);
  view.rerender(<TimelineFeed {...baseProps} hasMore onLoadMore={onLoadMore} />);
  expect(onLoadMore).toHaveBeenCalledTimes(1);
  expect(screen.getByRole('button', { name: 'Load more' })).toBeInTheDocument();
});

test('an older window offers the existing refresh action to return to latest', () => {
  const onApplyPending = vi.fn();
  render(<TimelineFeed {...baseProps} returnToLatest onApplyPending={onApplyPending} />);

  fireEvent.click(screen.getByRole('button', { name: 'Return to latest' }));
  expect(onApplyPending).toHaveBeenCalledTimes(1);
});

test('自動取得が失敗した後は observer を再接続せず、明示的な再試行を表示する', () => {
  const observe = vi.fn();
  class FakeObserver {
    observe = observe;
    disconnect() {}
    unobserve() {}
    takeRecords() { return []; }
    root = null;
    rootMargin = '';
    thresholds = [];
  }
  vi.stubGlobal('IntersectionObserver', FakeObserver);
  const onLoadMore = vi.fn();
  render(
    <TimelineFeed
      {...baseProps}
      hasMore
      loadMoreError='Older posts could not be loaded.'
      onLoadMore={onLoadMore}
    />
  );

  expect(observe).not.toHaveBeenCalled();
  expect(screen.getByText('Older posts could not be loaded.')).toBeInTheDocument();
  screen.getByRole('button', { name: 'Retry' }).click();
  expect(onLoadMore).toHaveBeenCalledTimes(1);
});
