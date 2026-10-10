import { fireEvent, render } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';

import { useWindowScrollAnchor } from './useWindowScrollAnchor';

const posts = (ids: string[]) => ids.map((id) => ({ post: { object_id: id } }));

afterEach(() => vi.restoreAllMocks());

const rect = (top: number, bottom: number) => ({ top, bottom } as DOMRect);

function Harness({ ids, loading = false, load }: { ids: string[]; loading?: boolean; load: () => void }) {
  const { listRef, loadMore } = useWindowScrollAnchor(posts(ids), load, loading);
  return <div className='shell-column-body'>
    <ul ref={listRef}>{ids.map((id) => <li key={id} data-post-id={id}>{id}</li>)}</ul>
    <button onClick={loadMore}>next</button>
  </div>;
}

test('replacing the head of a bounded window keeps the visible post at the same position', () => {
  let afterReplacement = false;
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
    if (this.classList.contains('shell-column-body')) return rect(0, 100);
    const id = this.dataset.postId;
    if (id === 'p0') return rect(-40, -10);
    if (id === 'p1') return afterReplacement ? rect(40, 70) : rect(-10, 20);
    if (id === 'p2') return rect(20, 50);
    return rect(70, 100);
  });
  const load = vi.fn();
  const view = render(<Harness ids={['p0', 'p1', 'p2']} load={load} />);
  const scroll = view.container.querySelector('.shell-column-body') as HTMLElement;
  scroll.scrollTop = 100;
  fireEvent.click(view.getByRole('button', { name: 'next' }));
  afterReplacement = true;
  view.rerender(<Harness ids={['p1', 'p2', 'p3']} load={load} />);

  expect(load).toHaveBeenCalledTimes(1);
  expect(scroll.scrollTop).toBe(150);
});

// #1689: フィルター中は、続きを読んでも表示する行が増えないことがある。その後に上へ戻って先頭に行が増えても、
// 読み込み前の位置へ戻さない。
test('a load that leaves the shown rows unchanged does not move the view on a later change', () => {
  let afterInsertion = false;
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
    if (this.classList.contains('shell-column-body')) return rect(0, 100);
    if (this.dataset.postId === 'p1') return afterInsertion ? rect(30, 60) : rect(-10, 20);
    return rect(-40, -10);
  });
  const load = vi.fn();
  const view = render(<Harness ids={['p0', 'p1']} load={load} />);
  const scroll = view.container.querySelector('.shell-column-body') as HTMLElement;
  scroll.scrollTop = 100;
  fireEvent.click(view.getByRole('button', { name: 'next' }));
  view.rerender(<Harness ids={['p0', 'p1']} loading load={load} />);
  view.rerender(<Harness ids={['p0', 'p1']} load={load} />);

  scroll.scrollTop = 0;
  afterInsertion = true;
  view.rerender(<Harness ids={['new', 'p0', 'p1']} load={load} />);

  expect(load).toHaveBeenCalledTimes(1);
  expect(scroll.scrollTop).toBe(0);
});
