import { act, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { DesktopApi } from '@/lib/api';
import { PendingSessionCards, SessionVisibility } from './SessionVisibility';

const observers: Array<(entries: IntersectionObserverEntry[]) => void> = [];
function installObserver() {
  observers.length = 0;
  vi.stubGlobal('IntersectionObserver', class {
    constructor(callback: (entries: IntersectionObserverEntry[]) => void) { observers.push(callback); }
    observe() {}
    disconnect() {}
  });
}
const intersect = async (value: boolean, index = observers.length - 1) => {
  await act(async () => observers[index]([{ isIntersecting: value, intersectionRatio: value ? 1 : 0 } as IntersectionObserverEntry]));
};
afterEach(() => { vi.unstubAllGlobals(); vi.restoreAllMocks(); });

describe('session manifest visibility', () => {
  it('keeps a missing candidate separate and replaces it with the verified card after acquisition', async () => {
    installObserver();
    const setSessionDisplay = vi.fn().mockResolvedValue(undefined);
    const listSessionCandidates = vi.fn().mockResolvedValue([{ replica_id: 'replica', session_id: 'live-one', kind: 'live' }]);
    const api = { setSessionDisplay, listSessionCandidates } as unknown as DesktopApi;
    const context = { api, topic: 'topic', scope: { kind: 'public' as const } };
    const initial = {};
    const view = render(<PendingSessionCards context={context} kind='live' refreshToken={initial} knownIds={[]} />);
    // 候補の card は背景の render で、observer はその後の effect で作られる。card の表示ではなく observer の登録を待つ。
    await waitFor(() => expect(observers).toHaveLength(1));
    expect(screen.getByRole('button')).toBeVisible();
    expect(setSessionDisplay).not.toHaveBeenCalled();
    expect(screen.queryByText('verified session')).not.toBeInTheDocument();
    await intersect(true);
    expect(setSessionDisplay).toHaveBeenLastCalledWith(expect.objectContaining({ replica_id: 'replica', visible: true }));
    listSessionCandidates.mockResolvedValue([]);
    view.rerender(<><ul><SessionVisibility context={context} kind='live' sessionId='live-one'>verified session</SessionVisibility></ul>
      <PendingSessionCards context={context} kind='live' refreshToken={{}} knownIds={['live-one']} /></>);
    await waitFor(() => expect(screen.queryByRole('button')).not.toBeInTheDocument());
    expect(screen.getByText('verified session')).toBeVisible();
    expect(setSessionDisplay).toHaveBeenLastCalledWith(expect.objectContaining({ visible: false }));
  });
  it('requests only intersecting cards, ignores rerenders, and releases hidden/unmounted cards', async () => {
    installObserver();
    const setSessionDisplay = vi.fn().mockResolvedValue(undefined);
    const api = { setSessionDisplay } as unknown as DesktopApi;
    const card = <ul><SessionVisibility context={{ api, topic: 'topic', scope: { kind: 'public' } }}
      sessionId='live-one' kind='live'><article>session</article></SessionVisibility></ul>;
    const view = render(card);
    expect(setSessionDisplay).not.toHaveBeenCalled();
    await intersect(true);
    expect(setSessionDisplay).toHaveBeenLastCalledWith(expect.objectContaining({ session_id: 'live-one', visible: true, retry: false }));
    view.rerender(card);
    await intersect(true);
    expect(setSessionDisplay).toHaveBeenCalledTimes(1);
    await intersect(false);
    expect(setSessionDisplay).toHaveBeenLastCalledWith(expect.objectContaining({ visible: false }));
    await intersect(true);
    view.unmount();
    await waitFor(() => expect(setSessionDisplay).toHaveBeenLastCalledWith(expect.objectContaining({ visible: false })));
  });

  it('releases a visible card when the document becomes hidden', async () => {
    installObserver();
    const setSessionDisplay = vi.fn().mockResolvedValue(undefined);
    const api = { setSessionDisplay } as unknown as DesktopApi;
    render(<ul><SessionVisibility context={{ api, topic: 'topic', scope: { kind: 'public' } }} sessionId='room' kind='game'>room</SessionVisibility></ul>);
    await intersect(true);
    vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('hidden');
    await act(async () => document.dispatchEvent(new Event('visibilitychange')));
    expect(setSessionDisplay).toHaveBeenLastCalledWith(expect.objectContaining({ visible: false }));
  });

  it('uses different observer generations when the scope changes during an outstanding request', async () => {
    installObserver();
    let release!: () => void;
    const setSessionDisplay = vi.fn().mockImplementationOnce(() => new Promise<void>((resolve) => { release = resolve; })).mockResolvedValue(undefined);
    const api = { setSessionDisplay } as unknown as DesktopApi;
    const card = (topic: string) => <ul><SessionVisibility context={{ api, topic, scope: { kind: 'public' } }} sessionId='live-one' kind='live'>session</SessionVisibility></ul>;
    const view = render(card('first'));
    await intersect(true);
    const oldObserver = setSessionDisplay.mock.calls[0][0].observer;
    view.rerender(card('second'));
    await intersect(true);
    const newObserver = setSessionDisplay.mock.calls[1][0].observer;
    expect(newObserver).not.toBe(oldObserver);
    await act(async () => release());
    expect(setSessionDisplay).toHaveBeenLastCalledWith(expect.objectContaining({ observer: oldObserver, visible: false }));
    expect(screen.getByText('session')).toBeVisible();
  });
});
