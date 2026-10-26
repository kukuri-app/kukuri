import { act, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import type { DesktopApi, LiveOverlayPushView, PostView } from '@/lib/api';
import { announceLiveOverlayPush } from '@/shell/data/useRuntimeEventBridge';
import { createDesktopShellStore, DesktopShellStoreContext } from '@/shell/store';
import { useLiveOverlayPushes } from './liveOverlayPush';

beforeEach(() => { vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'requestAnimationFrame', 'cancelAnimationFrame', 'Date', 'performance'] }); vi.setSystemTime(100_000); });
afterEach(() => vi.useRealTimers());
const topic = 'topic';
const channelId = 'pc-1700000000000-aaaaaaaaaaaaaaaa';
const source = { live: { topic_id: topic, session_id: 'live' }, conversation: { topic, channelId } };
const post = { object_id: 'old-post', author_pubkey: 'author', content: '', attachments: [], withdrawal: null } as unknown as PostView;
const event = (id: number, overrides: Partial<LiveOverlayPushView> = {}): LiveOverlayPushView => ({
  event_id: `event-${id}`, sender_pubkey: id % 2 ? 'a' : 'b', post_created_at: 1,
  sent_at: Date.now(), received_at: Date.now(),
  target: { topic, channel_id: channelId, object_id: 'old-post', attachment_index: 0, attachment_hash: 'gif' }, ...overrides,
});
function setup(resolve = vi.fn(async () => post)) {
  const store = createDesktopShellStore();
  const api = { resolveLiveOverlayPush: resolve } as unknown as DesktopApi;
  const hook = renderHook(({ active, hidden }: { active: boolean; hidden?: (post: PostView) => boolean }) => useLiveOverlayPushes(api, source, active, hidden), {
    initialProps: { active: true },
    wrapper: ({ children }) => <DesktopShellStoreContext.Provider value={store}>{children}</DesktopShellStoreContext.Provider>,
  });
  return { ...hook, store, resolve };
}
const flush = async () => { await act(async () => { await vi.advanceTimersByTimeAsync(32); }); };

test('combines every sender into one count and resolves the attachment only once', async () => {
  const { result, resolve } = setup();
  act(() => { for (let id = 0; id < 20; id++) announceLiveOverlayPush(event(id)); });
  await flush();
  expect(result.current.items).toHaveLength(1);
  expect(result.current.items[0].count).toBe(20);
  expect(resolve).toHaveBeenCalledTimes(1);
  act(() => { vi.advanceTimersByTime(4000); announceLiveOverlayPush(event(21)); });
  await flush();
  expect(result.current.items[0].count).toBe(21);
  act(() => vi.advanceTimersByTime(5000)); await flush();
  expect(result.current.items).toHaveLength(0);
  act(() => announceLiveOverlayPush(event(22))); await flush();
  expect(result.current.items[0].count).toBe(1);
});

test('keeps six distinct attachments and replaces the least recently pushed slot without a queue', async () => {
  const { result } = setup();
  act(() => {
    for (let id = 0; id < 8; id++) announceLiveOverlayPush(event(id, { target: { ...event(id).target, object_id: `post-${id}` } }));
  });
  await flush();
  expect(result.current.items.map((item) => item.key)).toEqual(['post-2:0', 'post-3:0', 'post-4:0', 'post-5:0', 'post-6:0', 'post-7:0']);
  expect(result.current.notice).toBe('replaced');
});

test('clears on inactivity and does not replay a previous view or accept another scope or hidden sender', async () => {
  const { result, resolve, rerender, store } = setup();
  act(() => announceLiveOverlayPush(event(1))); await flush();
  rerender({ active: false });
  act(() => { vi.advanceTimersByTime(100); announceLiveOverlayPush(event(2)); }); await flush();
  expect(result.current.items).toHaveLength(0);
  rerender({ active: true });
  act(() => {
    announceLiveOverlayPush(event(3, { received_at: 100_000 }));
    announceLiveOverlayPush(event(4, { target: { ...event(4).target, channel_id: 'another-thread' } }));
    store.getState().setField('authorTrustGates', { a: { hidden: true } as never });
    announceLiveOverlayPush(event(5));
  }); await flush();
  expect(result.current.items).toHaveLength(0);
  expect(resolve).toHaveBeenCalledTimes(1);
});

test('discarding a pending slot prevents its late post from restoring demand', async () => {
  let complete!: (post: PostView) => void;
  const resolve = vi.fn(() => new Promise<PostView>((done) => { complete = done; }));
  const { result, rerender } = setup(resolve);
  act(() => announceLiveOverlayPush(event(1))); await flush();
  rerender({ active: false });
  await act(async () => complete(post)); await flush();
  expect(result.current.items).toHaveLength(0);
});

test('cumulative press history does not add slots, pending resolutions, or timers', async () => {
  const { result, resolve, unmount } = setup();
  act(() => { for (let id = 0; id < 12000; id++) announceLiveOverlayPush(event(id)); });
  await flush();
  expect(result.current.items).toHaveLength(1);
  expect(result.current.items[0].count).toBe(12000);
  expect(resolve).toHaveBeenCalledTimes(1);
  expect(vi.getTimerCount()).toBe(1);
  unmount();
  expect(vi.getTimerCount()).toBe(0);
});

test('a different Live or conversation never renders the previous context while effects are being cleared', async () => {
  const store = createDesktopShellStore();
  const api = { resolveLiveOverlayPush: vi.fn(async () => post) } as unknown as DesktopApi;
  const rendered: { session: string; count: number }[] = [];
  const hook = renderHook(({ session, channel }) => {
    const result = useLiveOverlayPushes(api, { live: { ...source.live, session_id: session }, conversation: { topic, channelId: channel } }, true);
    rendered.push({ session, count: result.items.length });
    return result;
  }, { initialProps: { session: 'first', channel: channelId }, wrapper: ({ children }) => <DesktopShellStoreContext.Provider value={store}>{children}</DesktopShellStoreContext.Provider> });
  act(() => announceLiveOverlayPush(event(1))); await flush();
  expect(hook.result.current.items).toHaveLength(1);
  hook.rerender({ session: 'second', channel: 'pc-1700000000000-bbbbbbbbbbbbbbbb' });
  expect(rendered.filter((item) => item.session === 'second').every((item) => item.count === 0)).toBe(true);
});
