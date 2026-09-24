import { fireEvent, render } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';

import { useWindowScrollAnchor } from './useWindowScrollAnchor';

const posts = (ids: string[]) => ids.map((id) => ({ post: { object_id: id } }));

afterEach(() => vi.restoreAllMocks());

test('replacing the head of a bounded window keeps the visible post at the same position', () => {
  let afterReplacement = false;
  const rect = (top: number, bottom: number) => ({ top, bottom } as DOMRect);
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
    if (this.classList.contains('shell-column-body')) return rect(0, 100);
    const id = this.dataset.postId;
    if (id === 'p0') return rect(-40, -10);
    if (id === 'p1') return afterReplacement ? rect(40, 70) : rect(-10, 20);
    if (id === 'p2') return rect(20, 50);
    return rect(70, 100);
  });
  const load = vi.fn();
  function Harness({ ids }: { ids: string[] }) {
    const { listRef, loadMore } = useWindowScrollAnchor(posts(ids), load);
    return <div className='shell-column-body'>
      <ul ref={listRef}>{ids.map((id) => <li key={id} data-post-id={id}>{id}</li>)}</ul>
      <button onClick={loadMore}>next</button>
    </div>;
  }
  const view = render(<Harness ids={['p0', 'p1', 'p2']} />);
  const scroll = view.container.querySelector('.shell-column-body') as HTMLElement;
  scroll.scrollTop = 100;
  fireEvent.click(view.getByRole('button', { name: 'next' }));
  afterReplacement = true;
  view.rerender(<Harness ids={['p1', 'p2', 'p3']} />);

  expect(load).toHaveBeenCalledTimes(1);
  expect(scroll.scrollTop).toBe(150);
});
