import { act, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, test, vi } from 'vitest';

import type { DesktopApi, LiveSessionView, PostView } from '@/lib/api';
import { createDesktopShellStore, DesktopShellStoreContext } from '@/shell/store';

import { LiveActionsContext, type LiveActions } from './liveActions';
import { PostLive } from './PostLive';

const observers: Array<(entries: IntersectionObserverEntry[]) => void> = [];
afterEach(() => {
  vi.unstubAllGlobals();
  observers.length = 0;
});

function session(overrides: Partial<LiveSessionView> = {}): LiveSessionView {
  return {
    session_id: 'live-1',
    host_pubkey: 'host',
    title: 'Launch Party',
    description: 'watch along',
    status: 'Live',
    started_at: 1,
    ended_at: null,
    viewer_count: 0,
    joined_by_me: false,
    channel_id: null,
    audience_label: 'Public',
    start_post_id: 'start-post',
    ...overrides,
  };
}

function post(objectId: string, channelId: string | null = null): PostView {
  return {
    object_id: objectId,
    live: { session_id: 'live-1', topic_id: 'kukuri:topic:origin', channel_id: channelId },
  } as PostView;
}

function renderLive(target: PostView, found: LiveSessionView | null, overrides: Partial<DesktopApi> = {}) {
  vi.stubGlobal('IntersectionObserver', class {
    constructor(callback: (entries: IntersectionObserverEntry[]) => void) {
      observers.push(callback);
    }
    observe() {}
    disconnect() {}
  });
  const getLiveSession = vi.fn().mockResolvedValue(found);
  const setSessionDisplay = vi.fn().mockResolvedValue(undefined);
  const actions: LiveActions = {
    api: { getLiveSession, setSessionDisplay, ...overrides } as unknown as DesktopApi,
    locale: 'en',
    localPubkey: 'me',
    pendingBySessionId: {},
    join: vi.fn().mockResolvedValue(undefined),
    leave: vi.fn().mockResolvedValue(undefined),
    end: vi.fn().mockResolvedValue(undefined),
    introduce: vi.fn(),
    broadcastingSessionId: null,
    startBroadcast: vi.fn(),
    openPost: vi.fn(),
    copyLink: vi.fn(),
    activateReference: vi.fn(),
    stage: vi.fn(),
  };
  const store = createDesktopShellStore();
  render(
    <DesktopShellStoreContext.Provider value={store}>
      <LiveActionsContext.Provider value={actions}>
        <PostLive post={target} />
      </LiveActionsContext.Provider>
    </DesktopShellStoreContext.Provider>
  );
  const display = async () => {
    await waitFor(() => expect(observers.length).toBeGreaterThan(0));
    await act(async () =>
      observers[observers.length - 1]([{ isIntersecting: true, intersectionRatio: 1 } as IntersectionObserverEntry])
    );
  };
  return { actions, getLiveSession, setSessionDisplay, display, store };
}

// #1839: 紹介 post の中の Live は、Live を作った topic と scope で表示を要求し、元の投稿(開始 post)へ移れる。
test('an introduction shows the live session from its origin and opens the original post', async () => {
  const user = userEvent.setup();
  const { actions, setSessionDisplay, display } = renderLive(post('intro-post'), session());
  await display();
  expect(setSessionDisplay).toHaveBeenLastCalledWith(
    expect.objectContaining({ topic: 'kukuri:topic:origin', scope: { kind: 'public' }, session_id: 'live-1', kind: 'live', visible: true })
  );
  const tile = await screen.findByRole('article');
  await userEvent.click(within(tile).getByRole('button'));
  const card = await screen.findByRole('dialog', { name: 'Launch Party' });
  expect(actions.join).not.toHaveBeenCalled();
  await user.click(within(card).getByRole('button', { name: 'Join' }));
  expect(actions.join).toHaveBeenCalledWith('kukuri:topic:origin', 'live-1');
  expect(within(card).getByRole('heading', { name: 'Launch Party' })).toBeInTheDocument();
  await user.click(within(card).getByRole('button', { name: 'Open original post' }));
  expect(actions.openPost).toHaveBeenCalledWith('kukuri:topic:origin', 'start-post');
  await user.click(within(tile).getByRole('button'));
  await user.click(within(await screen.findByRole('dialog', { name: 'Launch Party' })).getByRole('button', { name: 'Share' }));
  expect(actions.introduce).toHaveBeenCalledWith(
    { session_id: 'live-1', topic_id: 'kukuri:topic:origin', channel_id: null },
    'Launch Party'
  );
});

// 表示要求が終わるまでは読込み中。読み終えても手元に無ければ、読めない Live として出す。
test('a live session that cannot be read is shown as unavailable after the display request', async () => {
  const { display } = renderLive(post('intro-post'), null);
  expect(await screen.findByText('Loading live sessions…')).toBeInTheDocument();
  await display();
  expect(await screen.findByText("This live session can't be loaded")).toBeInTheDocument();
});

// 参照の channel と Live の channel が違う(限定 Live を別の場所から指す)ときは、Live を出さない。
test('a reference whose channel differs from the live session is not expanded', async () => {
  const { display } = renderLive(post('crafted'), session({ channel_id: 'channel-1', audience_label: 'Private channel' }));
  await display();
  expect(await screen.findByText("This live session can't be loaded")).toBeInTheDocument();
  expect(screen.queryByText('Launch Party')).toBeNull();
});

// 開始 post の中では元の投稿へのボタンを出さず、終了した Live は終了として出す。
test('the start post shows an ended live session without a link to itself', async () => {
  const { display } = renderLive(post('start-post'), session({ status: 'Ended', ended_at: 2 }));
  await display();
  const tile = await screen.findByRole('article');
  await userEvent.click(within(tile).getByRole('button'));
  const card = await screen.findByRole('dialog', { name: 'Launch Party' });
  expect(within(card).getByText('Ended')).toBeInTheDocument();
  expect(within(card).queryByRole('button', { name: 'Open original post' })).toBeNull();
  expect(within(card).getByRole('button', { name: 'Join' })).toBeDisabled();
});


test.each(['post', 'thread'] as const)('the detail modal keeps adult text gated from the %s source', async (source) => {
  const channel = 'pc-1700000000000-cccccccccccccccc';
  const secret = { ...post('adult-post', channel), content: 'private adult text', content_labels: ['adult'] };
  const target = source === 'post' ? secret : post('empty-start', channel);
  const { store, display } = renderLive(target, session({ channel_id: channel }), {
    getPublicChannel: vi.fn(async () => ({ channel_id: channel, topic_id: 'kukuri:topic:origin', title: 'Thread',
      creator_pubkey: 'host', created_at: 1, last_post_at: 1, live_now: true, first_post: secret })),
  });
  await display();
  const tile = await screen.findByRole('article');
  await userEvent.click(within(tile).getByRole('button'));
  const dialog = await screen.findByRole('dialog', { name: 'Launch Party' });
  if (source === 'thread') await waitFor(() => expect(within(dialog).queryByText('Loading live sessions…')).toBeNull());
  expect(within(dialog).queryByText('private adult text')).toBeNull();
  act(() => store.getState().setField('adultContentEnabled', true));
  expect(await within(dialog).findByText('private adult text')).toBeInTheDocument();
});
