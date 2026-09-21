// #1239: thread も、行が 0 件でも続きがあれば、続きを読む手段を描く(`TimelineFeed` と同じ)。
import { render, screen } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';

import { ThreadTree } from './ThreadTree';

afterEach(() => {
  vi.unstubAllGlobals();
});

test('空のページでも next_cursor があれば、続きを読む button を描く', () => {
  vi.stubGlobal('IntersectionObserver', undefined);
  delete (window as unknown as Record<string, unknown>).IntersectionObserver;
  const onLoadMore = vi.fn();
  render(
    <ThreadTree
      posts={[]}
      emptyCopy='No replies yet.'
      onOpenAuthor={vi.fn()}
      onOpenThread={vi.fn()}
      onReply={vi.fn()}
      hasMore
      onLoadMore={onLoadMore}
    />
  );
  expect(screen.getByRole('button')).toBeInTheDocument();
});

test('続きが無ければ、空の文言を描く', () => {
  render(
    <ThreadTree
      posts={[]}
      emptyCopy='No replies yet.'
      onOpenAuthor={vi.fn()}
      onOpenThread={vi.fn()}
      onReply={vi.fn()}
    />
  );
  expect(screen.getByText('No replies yet.')).toBeInTheDocument();
});
