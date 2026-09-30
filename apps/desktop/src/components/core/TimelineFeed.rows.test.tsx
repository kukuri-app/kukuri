// #1425: 続きの読み込みや親の再描画のたびに、窓の既存の行を描き直さない。
import { render } from '@testing-library/react';
import { expect, test, vi } from 'vitest';

import type { PostCardView } from './types';

const postCard = vi.hoisted(() => vi.fn<(props: { onReply: () => void }) => null>(() => null));
vi.mock('./PostCard', () => ({ PostCard: postCard }));

import { TimelineFeed } from './TimelineFeed';

const view = (id: string) => ({ post: { object_id: id } }) as unknown as PostCardView;

test('新しい行だけを描き、既存の行は最新の callback を呼ぶ', () => {
  const first = [view('a'), view('b')];
  const firstReply = vi.fn();
  const feed = render(
    <TimelineFeed posts={first} emptyCopy='' onOpenAuthor={() => {}} onOpenThread={() => {}}
      onReply={firstReply} />
  );
  expect(postCard).toHaveBeenCalledTimes(2);

  const latestReply = vi.fn();
  feed.rerender(
    <TimelineFeed posts={[...first, view('c')]} emptyCopy='' onOpenAuthor={() => {}}
      onOpenThread={() => {}} onReply={latestReply} />
  );
  expect(postCard).toHaveBeenCalledTimes(3);

  postCard.mock.calls[0][0].onReply();
  expect(latestReply).toHaveBeenCalledTimes(1);
  expect(firstReply).not.toHaveBeenCalled();
});
