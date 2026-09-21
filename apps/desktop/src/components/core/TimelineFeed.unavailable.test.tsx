// #1239 AC-4: 遡った範囲にまだ取得できていない投稿があるとき、その旨を示し、続きを読む操作は止めない。
import { render, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';

import { ThreadTree } from './ThreadTree';
import { TimelineFeed } from './TimelineFeed';

const baseProps = {
  posts: [],
  emptyCopy: 'No posts in this topic yet.',
  onOpenAuthor: vi.fn(),
  onOpenThread: vi.fn(),
  onReply: vi.fn(),
};

beforeEach(() => {
  // 続きを読む手段を button で描かせる。
  vi.stubGlobal('IntersectionObserver', undefined);
  delete (window as unknown as Record<string, unknown>).IntersectionObserver;
});

afterEach(() => {
  vi.unstubAllGlobals();
});

test('まだ取得できていない投稿の数を示し、続きを読む button を残す', () => {
  render(<TimelineFeed {...baseProps} unavailableCount={3} hasMore onLoadMore={vi.fn()} />);
  expect(screen.getByRole('status')).toHaveTextContent('3');
  expect(screen.getByRole('button')).toBeInTheDocument();
  expect(screen.queryByText(baseProps.emptyCopy)).not.toBeInTheDocument();
});

test('続きが無くても、まだ取得できていない投稿があれば空の文言にしない', () => {
  render(<TimelineFeed {...baseProps} unavailableCount={2} />);
  expect(screen.getByRole('status')).toHaveTextContent('2');
  expect(screen.queryByText(baseProps.emptyCopy)).not.toBeInTheDocument();
});

test('まだ取得できていない投稿が無ければ、何も示さない', () => {
  render(<TimelineFeed {...baseProps} unavailableCount={0} />);
  expect(screen.queryByRole('status')).not.toBeInTheDocument();
  expect(screen.getByText(baseProps.emptyCopy)).toBeInTheDocument();
});

test('thread でも、まだ取得できていない返信の数を示し、続きを読む button を残す', () => {
  render(<ThreadTree {...baseProps} unavailableCount={4} hasMore onLoadMore={vi.fn()} />);
  expect(screen.getByRole('status')).toHaveTextContent('4');
  expect(screen.getByRole('button')).toBeInTheDocument();
});
