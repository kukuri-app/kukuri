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

function renderLive(target: PostView, found: LiveSessionView | null) {
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
    api: { getLiveSession, setSessionDisplay } as unknown as DesktopApi,
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
  render(
    <DesktopShellStoreContext.Provider value={createDesktopShellStore()}>
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
  return { actions, getLiveSession, setSessionDisplay, display };
}

// #1839: 紹介 post の中の Live は、Live を作った topic と scope で表示を要求し、元の投稿(開始 post)へ移れる。
test('an introduction shows the live session from its origin and opens the original post', async () => {
  const user = userEvent.setup();
  const { actions, setSessionDisplay, display } = renderLive(post('intro-post'), session());
  await display();
  expect(setSessionDisplay).toHaveBeenLastCalledWith(
    expect.objectContaining({ topic: 'kukuri:topic:origin', scope: { kind: 'public' }, session_id: 'live-1', kind: 'live', visible: true })
  );
  const card = await screen.findByRole('article');
  expect(within(card).getByText('Launch Party')).toBeInTheDocument();
  await user.click(within(card).getByRole('button', { name: 'Open original post' }));
  expect(actions.openPost).toHaveBeenCalledWith('kukuri:topic:origin', 'start-post');
  await user.click(within(card).getByRole('button', { name: 'Share' }));
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
  const card = await screen.findByRole('article');
  expect(within(card).getByText('Ended')).toBeInTheDocument();
  expect(within(card).queryByRole('button', { name: 'Open original post' })).toBeNull();
  expect(within(card).getByRole('button', { name: 'Join' })).toBeDisabled();
});
