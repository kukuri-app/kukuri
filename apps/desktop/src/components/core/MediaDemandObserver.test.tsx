import { act, render } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import { MediaDemandObserver } from './MediaDemandObserver';
import { MediaDemandContext } from './mediaRetryContext';

afterEach(() => vi.unstubAllGlobals());

test('video demand follows visibility of the whole containing card', () => {
  let notify: IntersectionObserverCallback | null = null;
  const disconnect = vi.fn();
  const observe = vi.fn();
  vi.stubGlobal('IntersectionObserver', class {
    constructor(callback: IntersectionObserverCallback) { notify = callback; }
    observe(target: Element) { observe(target); }
    disconnect() { disconnect(); }
  });
  const demand = vi.fn();
  const view = render(
    <MediaDemandContext.Provider value={demand}>
      <div data-testid='video-card'><MediaDemandObserver hash='video-hash' /></div>
    </MediaDemandContext.Provider>
  );
  expect(observe).toHaveBeenCalledWith(view.getByTestId('video-card'));
  act(() => notify?.([{ isIntersecting: true } as IntersectionObserverEntry], {} as IntersectionObserver));
  expect(demand).toHaveBeenCalledWith('video-hash', true);
  act(() => notify?.([{ isIntersecting: false } as IntersectionObserverEntry], {} as IntersectionObserver));
  expect(demand).toHaveBeenCalledWith('video-hash', false);
  // 1 回の callback に複数の entry が届いたときは、最後の entry が現在の状態を表す。
  act(() => notify?.(
    [{ isIntersecting: false }, { isIntersecting: true }] as IntersectionObserverEntry[],
    {} as IntersectionObserver
  ));
  expect(demand).toHaveBeenLastCalledWith('video-hash', true);
  view.unmount();
  expect(disconnect).toHaveBeenCalledOnce();
});
