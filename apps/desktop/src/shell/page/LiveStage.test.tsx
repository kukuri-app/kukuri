// #1841 AC-2: 選んだ Live に会話の場所の投稿と開始 post のリアクションを重ねる。投稿が重なり、custom reaction の画像が描かれ、
// 層を切ると映像だけになり、リアクションと返信は開始 post に付く。同じ新着を重複して浮かべず、層の切替・設定では何も送らない。
import { act, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';

import { LiveSessionMedia } from '@/components/extended/LiveSessionMedia';
import { createLivePlayer } from '@/components/extended/liveMedia';
import { liveBroadcastFor } from '@/components/extended/liveBroadcast';
import type { DesktopApi, LiveSessionView, PostView, ReactionReceivedView, ReactionSummaryView } from '@/lib/api';
import { announceLiveOverlayPush, announceReactionReceived } from '@/shell/data/useRuntimeEventBridge';
import { createDesktopShellStore, DesktopShellStoreContext, timelineStorageKeyForChannel } from '@/shell/store';
import { buildPostMediaView } from '@/shell/viewModels/postMediaView';

import { LiveActionsContext, type LiveActions } from './liveActions';
import { resetLiveOverlaySettingsForTest } from './liveOverlaySettings';
import { LiveStage } from './LiveStage';

vi.mock('@/components/extended/liveMedia', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@/components/extended/liveMedia')>()),
  createLivePlayer: vi.fn(),
}));

const TOPIC = 'kukuri:topic:origin';
const THREAD = 'pc-1700000000000-cccccccccccccccc';
const ME = 'm'.repeat(64);
const CAT = 'f'.repeat(64);

beforeEach(() => {
  vi.spyOn(HTMLMediaElement.prototype, 'play').mockResolvedValue();
  vi.mocked(createLivePlayer).mockReturnValue({ push: vi.fn(), reset: vi.fn(), close: vi.fn() });
  vi.stubGlobal('IntersectionObserver', class {
    observe() {}
    disconnect() {}
  });
  localStorage.clear();
  resetLiveOverlaySettingsForTest();
});

afterEach(() => {
  vi.unstubAllGlobals();
});

function session(overrides: Partial<LiveSessionView> = {}): LiveSessionView {
  return {
    session_id: 'live-1',
    host_pubkey: 'h'.repeat(64),
    title: 'Windows 11 の確認',
    description: '',
    status: 'Live',
    started_at: 1,
    ended_at: null,
    viewer_count: 0,
    joined_by_me: false,
    channel_id: THREAD,
    audience_label: 'Public thread',
    start_post_id: 'start',
    ...overrides,
  };
}

function summary(key: string, count: number, overrides: Partial<ReactionSummaryView> = {}): ReactionSummaryView {
  return { reaction_key_kind: 'emoji', normalized_reaction_key: key, emoji: key, custom_asset: null, count, ...overrides };
}

function post(id: string, overrides: Partial<PostView> = {}): PostView {
  return {
    object_id: id,
    author_pubkey: 'a'.repeat(64),
    author_display_name: `author-${id}`,
    content: `body of ${id}`,
    content_status: 'Available',
    attachments: [],
    created_at: 1,
    withdrawal: null,
    reply_to: null,
    root_id: null,
    reaction_summary: [],
    ...overrides,
  } as PostView;
}

const catAsset = {
  asset_id: 'cat',
  owner_pubkey: CAT,
  blob_hash: 'cat-hash',
  search_key: 'neko',
  mime: 'image/png',
  bytes: 1,
  width: 128,
  height: 128,
};

function setup({
  found = session(),
  posts = [post('reply', { reply_to: 'start', root_id: 'start' }), post('chat')],
  startPost = post('start', {
    reaction_summary: [summary('🎉', 2), summary('custom_asset:cat', 3, { reaction_key_kind: 'custom_asset', emoji: null, custom_asset: catAsset })],
  }),
  mediaObjectUrls = { 'cat-hash': 'data:image/png;base64,AAAA' } as Record<string, string | null>,
  conversationChannel = THREAD,
  previewStream = null as MediaStream | null,
} = {}) {
  const store = createDesktopShellStore();
  const key = timelineStorageKeyForChannel(TOPIC, conversationChannel);
  store.getState().setField('timelinesByKey', { [key]: posts });
  store.getState().setField('joinedChannelsByTopic', { [TOPIC]: [] });
  store.getState().setField('mediaObjectUrls', mediaObjectUrls);
  const api = {
    getLiveSession: vi.fn(async () => found),
    setSessionDisplay: vi.fn(async () => undefined),
    listThread: vi.fn(async () => ({ items: [startPost], next_cursor: null })),
    watchLiveMedia: vi.fn(async () => undefined),
    nextLiveVideo: vi.fn(() => new Promise<ArrayBuffer>(() => undefined)),
    stopWatchingLiveMedia: vi.fn(async () => undefined),
    setLiveVolume: vi.fn(async () => undefined),
    toggleReaction: vi.fn(),
    createPost: vi.fn(),
    setLiveBroadcast: vi.fn(),
    pushLiveVideo: vi.fn(),
    resolveLiveOverlayPush: vi.fn(async () => null as PostView | null),
  };
  const actions = {
    api: api as unknown as DesktopApi,
    locale: 'ja',
    localPubkey: ME,
    pendingBySessionId: {},
  } as unknown as LiveActions;
  const controller = liveBroadcastFor(actions.api);
  controller.preview.srcObject = previewStream;
  if (previewStream) vi.spyOn(controller, 'getState').mockReturnValue({
    topic: TOPIC, sessionId: found.session_id, title: found.title, source: 'camera',
    microphone: false, phase: 'live', notice: null, cameras: [], cameraId: '',
  });
  const onReply = vi.fn();
  const onToggleReaction = vi.fn();
  const view = () => (
    <DesktopShellStoreContext.Provider value={store}>
      <LiveActionsContext.Provider value={actions}>
        <LiveStage
          source={{ live: { session_id: 'live-1', topic_id: TOPIC, channel_id: THREAD }, conversation: { topic: TOPIC, channelId: conversationChannel } }}
          conversationLabel='v0.5 の確認'
          visible
          mediaForPost={(post) => buildPostMediaView(post, { mediaObjectUrls: store.getState().mediaObjectUrls, adultContentGated: false, unsupportedVideoManifests: {} })}
          recentReactions={[]}
          reactionAssets={[]}
          onReactionPickerOpen={() => undefined}
          onToggleReaction={onToggleReaction}
          onReply={onReply}
        />
      </LiveActionsContext.Provider>
    </DesktopShellStoreContext.Provider>
  );
  const rendered = render(view());
  return {
    api,
    store,
    key,
    onReply,
    onToggleReaction,
    rerender: () => rendered.rerender(view()),
    unmount: rendered.unmount,
    container: rendered.container,
  };
}

const layers = (container: HTMLElement) => container.querySelector<HTMLElement>('.live-stage-layers')!;

test('shows the local capture in the large stage without watching the broadcast over the network', async () => {
  const stream = {} as MediaStream;
  const { container, api } = setup({ found: session({ host_pubkey: ME }), previewStream: stream });
  await screen.findByText('Windows 11 の確認');
  const video = container.querySelector('video');
  expect(video).not.toBeNull();
  expect(video?.srcObject).toBe(stream);
  expect(video?.muted).toBe(true);
  expect(api.watchLiveMedia).not.toHaveBeenCalled();
});

test('overlays the conversation posts and the start post reactions, including a custom reaction image', async () => {
  const { container, store, key } = setup();
  await screen.findByText('body of chat');
  const overlay = layers(container);
  expect(overlay).toHaveAttribute('aria-hidden', 'true');
  // 重ねた層は読み上げないので、role ではなく要素で確かめる。
  expect([...overlay.querySelectorAll('li')].map((item) => item.dataset.postObjectId)).toEqual(['chat', 'reply']);
  expect(within(overlay).getByText('Live reply').closest('li')).toHaveAttribute('data-post-object-id', 'reply');
  await waitFor(() => expect(overlay.querySelectorAll('.live-stage-count')).toHaveLength(2));
  const counts = [...overlay.querySelectorAll('.live-stage-count')];
  // 多い順に出す。custom reaction は手元の画像で描く。
  expect(counts[0].querySelector('img')).toHaveAttribute('src', 'data:image/png;base64,AAAA');
  expect(counts[0]).toHaveTextContent('3');
  expect(counts[1]).toHaveTextContent('🎉2');
  // 後から届いた新しい投稿は、新着の印を付けて下に足す。
  act(() =>
    store.getState().setField('timelinesByKey', {
      [key]: [post('new', { created_at: Math.ceil(Date.now() / 1000) + 1 }), post('reply', { reply_to: 'start', root_id: 'start' }), post('chat')],
    })
  );
  const added = within(layers(container)).getByText('body of new').closest('li')!;
  expect(added).toHaveClass('live-stage-post-fresh');
  expect(within(layers(container)).getByText('body of chat').closest('li')).not.toHaveClass('live-stage-post-fresh');
});

test('falls back to the name of a custom reaction whose image is not available', async () => {
  const { container } = setup({ mediaObjectUrls: {} });
  await waitFor(() => expect(layers(container).querySelectorAll('.live-stage-count')).toHaveLength(2));
  const custom = layers(container).querySelector('.live-stage-count')!;
  expect(custom.querySelector('img')).toBeNull();
  expect(custom).toHaveTextContent('ne3');
});

test('shows every sender\'s Push count as x20 at the requested attachment, including an old post outside the window', async () => {
  const { container, api } = setup({ mediaObjectUrls: { gif: 'blob:original-gif', png: 'blob:first-png' } });
  const old = post('old', { channel_id: THREAD, attachments: [
    { hash: 'png', mime: 'image/png', bytes: 100, role: 'image_original', status: 'Available' },
    { hash: 'gif', mime: 'image/gif', bytes: 100, role: 'image_original', status: 'Available' },
  ] });
  api.resolveLiveOverlayPush.mockResolvedValue(old);
  await screen.findByText('body of chat');
  act(() => {
    for (let id = 0; id < 20; id++) announceLiveOverlayPush({
      event_id: `push-${id}`, sender_pubkey: (id % 2 ? 'a' : 'b').repeat(64),
      target: { topic: TOPIC, channel_id: THREAD, object_id: 'old', attachment_index: 1, attachment_hash: 'gif' },
      post_created_at: 1, sent_at: Date.now(), received_at: Date.now(),
    });
  });
  await waitFor(() => expect(layers(container).querySelector('.live-overlay-push-count')).toHaveTextContent('x20'));
  expect(layers(container).querySelector('.live-stage-pushes img')?.getAttribute('src')).toMatch(/^blob:original-gif#kukuri-overlay-/);
  expect(api.resolveLiveOverlayPush).toHaveBeenCalledTimes(1);
  await userEvent.setup().click(screen.getByRole('button', { name: 'Posts' }));
  expect(layers(container).querySelector('.live-overlay-push-count')).toBeNull();
});

test('overlays only the first image of each post, including animated GIFs and attachment-only posts', async () => {
  const attachment = (hash: string, mime: string) => ({ hash, mime, bytes: 100, role: 'image_original', status: 'Available' as const });
  const { container } = setup({
    posts: [
      post('png', { attachments: [attachment('transparent', 'image/png'), attachment('second', 'image/png')] }),
      post('gif', { content: '', attachments: [attachment('animated', 'image/gif')] }),
    ],
    mediaObjectUrls: { transparent: 'blob:transparent-png', second: 'blob:second', animated: 'blob:animated-gif' },
  });
  await screen.findByText('body of png');
  const images = layers(container).querySelectorAll('.live-overlay-image img');
  expect([...images].map((image) => image.getAttribute('src'))).toEqual(['blob:animated-gif', 'blob:transparent-png']);
  expect(within(layers(container)).getByText('author-gif')).toBeInTheDocument();
  expect(layers(container).querySelector('[data-post-object-id="png"] .live-stage-post-copy')).toHaveTextContent('body of png');
  await userEvent.setup().click(screen.getByRole('button', { name: 'Posts' }));
  expect(layers(container).querySelector('.live-overlay-image')).toBeNull();
});

test('turning the layers off leaves only the video, and the settings are kept on the device', async () => {
  const user = userEvent.setup();
  const { container, api } = setup();
  await screen.findByText('body of chat');
  await user.click(screen.getByRole('button', { name: 'Posts' }));
  expect(layers(container).querySelector('.live-stage-posts')).toBeNull();
  await user.click(screen.getByRole('button', { name: 'Reactions' }));
  expect(layers(container)).toBeEmptyDOMElement();
  expect(container.querySelector('.live-viewer-screen')).not.toBeNull();
  expect(screen.getByRole('button', { name: 'Posts' })).toHaveAttribute('aria-pressed', 'false');

  await user.click(screen.getByRole('button', { name: 'Display settings' }));
  const menu = await screen.findByRole('menu', { name: 'Display settings' });
  await user.click(within(within(menu).getByRole('group', { name: 'Post position' })).getByRole('menuitemradio', { name: 'Bottom right' }));
  await user.click(within(within(menu).getByRole('group', { name: 'Text size' })).getByRole('menuitemradio', { name: 'Large' }));
  expect(layers(container)).toHaveAttribute('data-position', 'right');
  expect(layers(container)).toHaveAttribute('data-text', 'large');
  expect(JSON.parse(localStorage.getItem('kukuri.desktop.live-overlay')!)).toMatchObject({
    posts: false,
    reactions: false,
    position: 'right',
    text: 'large',
  });
  // 表示の操作だけでは、投稿・リアクション・配信の操作を呼ばない。
  for (const mutation of [api.toggleReaction, api.createPost, api.setLiveBroadcast, api.pushLiveVideo]) {
    expect(mutation).not.toHaveBeenCalled();
  }
});

test('reactions and replies go to the start post of the live', async () => {
  const user = userEvent.setup();
  const { onReply, onToggleReaction } = setup();
  await user.click(await screen.findByRole('button', { name: 'Reply to the live' }));
  expect(onReply).toHaveBeenCalledWith(expect.objectContaining({ object_id: 'start' }));
  await user.click(screen.getByRole('button', { name: 'React to the live' }));
  await user.click(await screen.findByRole('button', { name: 'party-popper' }));
  expect(onToggleReaction).toHaveBeenCalledWith(expect.objectContaining({ object_id: 'start' }), {
    kind: 'emoji',
    emoji: '🎉',
  });
});

function received(id: string, overrides: Partial<ReactionReceivedView> = {}): ReactionReceivedView {
  return {
    target_object_id: 'start',
    reaction_id: id,
    reaction: { reaction_key_kind: 'emoji', normalized_reaction_key: '🎉', emoji: '🎉', custom_asset: null },
    updated_at: Date.now(),
    ...overrides,
  };
}

// #1841 AC-3: 開いている間だけ、開始 post を数の表示の custom reaction の画像の取得の対象に載せる。
test('lists the start post for image fetching while open and drops it when closed', async () => {
  const { store, unmount } = setup();
  await waitFor(() => expect(Object.values(store.getState().liveStagePosts).map((post) => post.object_id)).toEqual(['start']));
  unmount();
  expect(store.getState().liveStagePosts).toEqual({});
});

test('floats a new reaction once, even across re-renders and a repeated announcement', async () => {
  const { container, rerender } = setup();
  await waitFor(() => expect(layers(container).querySelectorAll('.live-stage-count')).toHaveLength(2));
  const event = received('r1');
  act(() => announceReactionReceived(event));
  expect(layers(container).querySelectorAll('.live-stage-float')).toHaveLength(1);
  rerender();
  act(() => announceReactionReceived(event));
  expect(layers(container).querySelectorAll('.live-stage-float')).toHaveLength(1);
});

test('shows the state of an ended live, of the broadcaster, and of a conversation the viewer cannot read', async () => {
  const ended = setup({ found: session({ status: 'Ended' }) });
  expect(await screen.findByText('This live session has ended')).toBeVisible();
  expect(ended.container.querySelector('.live-stage-layers')).toBeNull();
  ended.unmount();

  const broadcaster = setup({ found: session({ host_pubkey: ME }) });
  expect(await screen.findByText(/Start broadcasting your camera or screen/)).toBeVisible();
  broadcaster.unmount();

  const unreadable = setup({ conversationChannel: 'channel-1700000000000-abcdefgh', posts: [post('secret')] });
  expect(await screen.findByText("You can't read posts in this place")).toBeVisible();
  expect(within(layers(unreadable.container)).queryByText('body of secret')).toBeNull();
});

/// 視聴中の映像の応答(状態 0、keyframe の chunk 1 件で bytes は 1・2・3)。
function video() {
  const buffer = new ArrayBuffer(11 + 13 + 3);
  const view = new DataView(buffer);
  view.setUint16(9, 1, true);
  view.setUint8(19, 1);
  view.setUint32(20, 3, true);
  new Uint8Array(buffer, 24).set([1, 2, 3]);
  return buffer;
}

test('draws the same video under the overlay as a view without it, sharing one watch', async () => {
  const players = [0, 1].map(() => ({ push: vi.fn(), reset: vi.fn(), close: vi.fn() }));
  vi.mocked(createLivePlayer).mockReturnValueOnce(players[0]).mockReturnValueOnce(players[1]);
  const { container, api } = setup({ found: session({ joined_by_me: true }) });
  api.nextLiveVideo.mockImplementation(async () => {
    await new Promise((resolve) => setTimeout(resolve, 10));
    return video();
  });
  await waitFor(() => expect(players[0].push).toHaveBeenCalled());
  const screenBox = container.querySelector('.live-viewer-screen')!;
  expect(screenBox.querySelector('.live-stage-layers')).not.toBeNull();
  expect(createLivePlayer).toHaveBeenNthCalledWith(1, screenBox.querySelector('canvas'));
  // 同じ Live の重ねない表示(投稿の中の Live)と視聴を 1 つ共有し、同じ映像の bytes を受ける。
  render(<LiveSessionMedia api={api} topic={TOPIC} session={session({ joined_by_me: true })} isOwner={false} />);
  await waitFor(() => expect(players[1].push).toHaveBeenCalled());
  expect(api.watchLiveMedia).toHaveBeenCalledTimes(1);
  for (const player of players) {
    expect(player.push.mock.lastCall![0].chunks.map((chunk: { data: Uint8Array }) => [...chunk.data])).toEqual([[1, 2, 3]]);
  }
});
