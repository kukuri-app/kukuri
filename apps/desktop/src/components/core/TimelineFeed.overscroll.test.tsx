// #1851: 列の途中で上へ scroll しても、新着の適用(保留が無ければ先頭のページからの読み直し)を呼ばない。
// 縦に scroll するのは一覧ではなく列の body(`.shell-column-body`)。
import { fireEvent, render } from '@testing-library/react';
import { expect, test, vi } from 'vitest';

import type { PostCardView } from './types';

vi.mock('./PostCard', () => ({ PostCard: () => null }));

import { TimelineFeed } from './TimelineFeed';

const view = (id: string) => ({ post: { object_id: id } }) as unknown as PostCardView;

function renderInColumn(scrollTop: number) {
  const onApplyPending = vi.fn();
  const { container } = render(
    <div className='shell-column-body'>
      <TimelineFeed posts={[view('a'), view('b')]} emptyCopy='' onOpenAuthor={() => {}}
        onOpenThread={() => {}} onReply={() => {}} onApplyPending={onApplyPending} />
    </div>
  );
  (container.querySelector('.shell-column-body') as HTMLElement).scrollTop = scrollTop;
  return { list: container.querySelector('ul') as HTMLElement, onApplyPending };
}

function pull(list: HTMLElement) {
  fireEvent.wheel(list, { deltaY: -200 });
  fireEvent.touchStart(list, { touches: [{ clientY: 0 }] });
  fireEvent.touchMove(list, { touches: [{ clientY: 100 }] });
}

test('列の途中で上へ scroll しても、新着を適用しない', () => {
  const { list, onApplyPending } = renderInColumn(800);
  pull(list);
  expect(onApplyPending).not.toHaveBeenCalled();
});

test('列の上端で引っ張ると、新着を適用する', () => {
  const { list, onApplyPending } = renderInColumn(0);
  pull(list);
  expect(onApplyPending).toHaveBeenCalledTimes(2);
});
