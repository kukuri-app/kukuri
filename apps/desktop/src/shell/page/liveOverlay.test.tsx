// #1841 AC-1: 重ねる Live・会話の場所・Live の開始 post の結び付け。会話の場所ごとの閲覧の資格、紹介 post と開始 post の区別、
// 取り下げ・非表示の反映、開いた時点の投稿と後から届いた投稿の区別、浮かべるリアクションの重複・時刻・上限を確かめる。
import { act, renderHook, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, expect, test, vi } from 'vitest';

import type {
  DesktopApi,
  JoinedPrivateChannelView,
  LiveSessionView,
  PostView,
  ReactionReceivedView,
} from '@/lib/api';
import { announceReactionReceived } from '@/shell/data/useRuntimeEventBridge';
import { createDesktopShellStore, DesktopShellStoreContext, timelineStorageKeyForChannel } from '@/shell/store';

import {
  LIVE_FLOAT_CONCURRENT,
  LIVE_FLOAT_LIFETIME_MS,
  LIVE_FLOAT_QUEUE,
  type LiveOverlaySource,
  useLiveOverlay,
  useLiveReactionFloats,
} from './liveOverlay';

const ORIGIN = 'kukuri:topic:origin';
const ELSEWHERE = 'kukuri:topic:elsewhere';
const THREAD = 'pc-1700000000000-cccccccccccccccc';
const PRIVATE = 'channel-1700000000000-abcdefgh';

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

function session(overrides: Partial<LiveSessionView> = {}): LiveSessionView {
  return {
    session_id: 'live-1',
    host_pubkey: 'h'.repeat(64),
    title: 'Watch party',
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

function post(id: string, overrides: Partial<PostView> = {}): PostView {
  return {
    object_id: id,
    author_pubkey: 'a'.repeat(64),
    content: id,
    content_status: 'Available',
    created_at: 1,
    withdrawal: null,
    reply_to: null,
    root_id: null,
    ...overrides,
  } as PostView;
}

function source(conversation: LiveOverlaySource['conversation']): LiveOverlaySource {
  return { live: { session_id: 'live-1', topic_id: ORIGIN, channel_id: THREAD }, conversation };
}

function setup(
  target: LiveOverlaySource,
  {
    posts = [],
    joined,
    found = session(),
    startPost = post('start'),
    animate = false,
  }: {
    posts?: PostView[];
    joined?: JoinedPrivateChannelView[];
    found?: LiveSessionView | null;
    startPost?: PostView | null;
    animate?: boolean;
  } = {}
) {
  vi.stubGlobal('IntersectionObserver', class {
    observe() {}
    disconnect() {}
  });
  const store = createDesktopShellStore();
  const key = timelineStorageKeyForChannel(target.conversation.topic, target.conversation.channelId);
  store.getState().setField('timelinesByKey', { [key]: posts });
  if (joined) store.getState().setField('joinedChannelsByTopic', { [target.conversation.topic]: joined });
  const api = {
    getLiveSession: vi.fn(async () => found),
    setSessionDisplay: vi.fn(async () => undefined),
    listThread: vi.fn(async () => ({ items: startPost ? [startPost] : [], next_cursor: null })),
  } as unknown as DesktopApi;
  const wrapper = ({ children }: { children: ReactNode }) => (
    <DesktopShellStoreContext.Provider value={store}>{children}</DesktopShellStoreContext.Provider>
  );
  const hook = renderHook(
    ({ hidden }: { hidden?: (post: PostView) => boolean }) => useLiveOverlay(api, target, { hidden, animate }),
    { wrapper, initialProps: {} }
  );
  return { api, store, key, hook };
}

const ids = (posts: { post: PostView }[]) => posts.map((entry) => entry.post.object_id);

test('binds a public live to its thread, reads the start post, and marks replies to the start post', async () => {
  const { api, hook } = setup(source({ topic: ORIGIN, channelId: THREAD }), {
    posts: [
      post('to-intro', { reply_to: 'intro', root_id: 'intro' }),
      post('nested', { reply_to: 'direct', root_id: 'start' }),
      post('direct', { reply_to: 'start', root_id: 'start' }),
      post('chat'),
      post('start'),
      post('older'),
    ],
  });
  await waitFor(() => expect(hook.result.current.startPost?.object_id).toBe('start'));
  expect(api.getLiveSession).toHaveBeenCalledWith(ORIGIN, 'live-1');
  expect(api.listThread).toHaveBeenCalledWith(ORIGIN, 'start', null, 1);
  const { conversation, posts } = hook.result.current;
  expect(conversation).toBe('readable');
  // 最新の 5 件を古い順に出す。開始 post の返信ツリーの投稿だけが「ライブへの返信」で、紹介 post への返信と開始 post 自身は違う。
  expect(ids(posts)).toEqual(['start', 'chat', 'direct', 'nested', 'to-intro']);
  expect(posts.filter((entry) => entry.liveReply).map((entry) => entry.post.object_id)).toEqual(['direct', 'nested']);
});

test('prefers the newer posts held back for the timeline column', async () => {
  const { store, key, hook } = setup(source({ topic: ORIGIN, channelId: THREAD }), { posts: [post('shown')] });
  act(() => store.getState().setField('pendingTimelineSnapshotsByKey', { [key]: [post('held'), post('shown')] }));
  await waitFor(() => expect(ids(hook.result.current.posts)).toEqual(['shown', 'held']));
});

test('does not read posts of a private channel the viewer has not joined', async () => {
  const conversation = { topic: ELSEWHERE, channelId: PRIVATE };
  const notJoined = setup(source(conversation), { posts: [post('secret')], joined: [] });
  await waitFor(() => expect(notJoined.hook.result.current.session).toBeTruthy());
  expect(notJoined.hook.result.current.conversation).toBe('unreadable');
  expect(notJoined.hook.result.current.posts).toEqual([]);
  notJoined.hook.unmount();

  const unknown = setup(source(conversation), { posts: [post('secret')] });
  expect(unknown.hook.result.current.conversation).toBe('loading');
  expect(unknown.hook.result.current.posts).toEqual([]);
  unknown.hook.unmount();

  const joined = setup(source(conversation), {
    posts: [post('member-only')],
    joined: [{ channel_id: PRIVATE } as JoinedPrivateChannelView],
  });
  expect(joined.hook.result.current.conversation).toBe('readable');
  expect(ids(joined.hook.result.current.posts)).toEqual(['member-only']);
});

test('reads the live and its start post from their own topic when introduced in another topic', async () => {
  const { api, hook } = setup(source({ topic: ELSEWHERE, channelId: null }), { posts: [post('intro-chat')] });
  await waitFor(() => expect(hook.result.current.startPost?.object_id).toBe('start'));
  expect(api.getLiveSession).toHaveBeenCalledWith(ORIGIN, 'live-1');
  expect(api.listThread).toHaveBeenCalledWith(ORIGIN, 'start', null, 1);
  expect(ids(hook.result.current.posts)).toEqual(['intro-chat']);
});

test('leaves out withdrawn, unfetched, and hidden posts and follows a reload without a muted author', async () => {
  const { store, key, hook } = setup(source({ topic: ORIGIN, channelId: THREAD }), {
    posts: [
      post('muted-author', { author_pubkey: 'm'.repeat(64) }),
      post('withdrawn', { withdrawal: { withdrawn_at: 2, reason_visibility: 'none' } }),
      post('missing', { content_status: 'Missing' }),
      post('gated', { author_pubkey: 'g'.repeat(64) }),
      post('kept'),
    ],
  });
  hook.rerender({ hidden: (target: PostView) => target.author_pubkey === 'g'.repeat(64) });
  expect(ids(hook.result.current.posts)).toEqual(['kept', 'muted-author']);
  // mute の後は、一覧の読み直しが作者の投稿を除く(backend)。重ねた表示もそれに従う。
  act(() => store.getState().setField('timelinesByKey', { [key]: [post('kept')] }));
  expect(ids(hook.result.current.posts)).toEqual(['kept']);
});

test('does not read a start post or float reactions for a live the viewer cannot see', async () => {
  const { api, hook } = setup(source({ topic: ORIGIN, channelId: THREAD }), { found: null, animate: true });
  await waitFor(() => expect(api.getLiveSession).toHaveBeenCalled());
  act(() => announceReactionReceived(received('on-hidden-live')));
  expect(hook.result.current.session).toBeFalsy();
  expect(hook.result.current.startPost).toBeUndefined();
  expect(hook.result.current.floats).toEqual([]);
  expect(api.listThread).not.toHaveBeenCalled();
});

test('treats a legacy live without a start post and a withdrawn start post as having none', async () => {
  const legacy = setup(source({ topic: ORIGIN, channelId: THREAD }), { found: session({ start_post_id: null }) });
  await waitFor(() => expect(legacy.hook.result.current.session).toBeTruthy());
  expect(legacy.hook.result.current.startPost).toBeNull();
  expect(legacy.api.listThread).not.toHaveBeenCalled();
  legacy.hook.unmount();

  const withdrawn = setup(source({ topic: ORIGIN, channelId: THREAD }), {
    startPost: post('start', { withdrawal: { withdrawn_at: 2, reason_visibility: 'none' } }),
  });
  await waitFor(() => expect(withdrawn.api.listThread).toHaveBeenCalled());
  await waitFor(() => expect(withdrawn.hook.result.current.startPost).toBeNull());
});

test('marks only posts that arrive after opening and are newer than the opening as fresh', async () => {
  const opened = Math.ceil(Date.now() / 1000);
  const { store, key, hook } = setup(source({ topic: ORIGIN, channelId: THREAD }), { posts: [post('before')] });
  await waitFor(() => expect(hook.result.current.posts).toHaveLength(1));
  expect(hook.result.current.posts[0].fresh).toBe(false);
  act(() =>
    store.getState().setField('timelinesByKey', {
      [key]: [post('new', { created_at: opened + 1 }), post('late-but-old', { created_at: 2 }), post('before')],
    })
  );
  const fresh = Object.fromEntries(hook.result.current.posts.map((entry) => [entry.post.object_id, entry.fresh]));
  expect(fresh).toEqual({ before: false, 'late-but-old': false, new: true });
});

function received(id: string, overrides: Partial<ReactionReceivedView> = {}): ReactionReceivedView {
  return {
    target_object_id: 'start',
    reaction_id: id,
    reaction: { reaction_key_kind: 'emoji', normalized_reaction_key: 'emoji:🎉', emoji: '🎉', custom_asset: null },
    updated_at: Date.now(),
    ...overrides,
  };
}

test('floats a new reaction to the start post once and removes it after its lifetime', () => {
  vi.useFakeTimers();
  const openedAt = Date.now();
  const { result } = renderHook(() => useLiveReactionFloats('start', true));
  act(() => {
    announceReactionReceived(received('r1'));
    // 同じ版の重複、紹介 post など別の投稿へのリアクション、開いた時点より前に付いたものは浮かべない。
    announceReactionReceived(received('r1'));
    announceReactionReceived(received('on-intro', { target_object_id: 'intro' }));
    announceReactionReceived(received('before-open', { updated_at: openedAt - 5_000 }));
  });
  expect(result.current.map((float) => float.id)).toEqual([`r1:${openedAt}`]);
  act(() => vi.advanceTimersByTime(LIVE_FLOAT_LIFETIME_MS));
  expect(result.current).toEqual([]);
});

test('caps the floats shown at once and the waiting queue, dropping the oldest waiting ones', () => {
  vi.useFakeTimers();
  const { result } = renderHook(() => useLiveReactionFloats('start', true));
  const total = LIVE_FLOAT_CONCURRENT + LIVE_FLOAT_QUEUE + 2;
  act(() => {
    for (let index = 0; index < total; index += 1) announceReactionReceived(received(`r${index}`));
  });
  const shown = () => result.current.map((float) => Number(float.id.split(':')[0].slice(1)));
  const range = (from: number, count: number) => Array.from({ length: count }, (_, index) => from + index);
  expect(shown()).toEqual(range(0, LIVE_FLOAT_CONCURRENT));
  act(() => vi.advanceTimersByTime(LIVE_FLOAT_LIFETIME_MS));
  expect(shown()).toEqual(range(LIVE_FLOAT_CONCURRENT + 2, LIVE_FLOAT_CONCURRENT));
  act(() => vi.advanceTimersByTime(LIVE_FLOAT_LIFETIME_MS));
  expect(shown()).toEqual(range(LIVE_FLOAT_CONCURRENT * 2 + 2, LIVE_FLOAT_CONCURRENT));
  act(() => vi.advanceTimersByTime(LIVE_FLOAT_LIFETIME_MS));
  expect(result.current).toEqual([]);
});

test('does not float while inactive and leaves no timer behind when closed', () => {
  vi.useFakeTimers();
  const { result, rerender, unmount } = renderHook(({ active }) => useLiveReactionFloats('start', active), {
    initialProps: { active: false },
  });
  act(() => announceReactionReceived(received('hidden-column')));
  expect(result.current).toEqual([]);
  rerender({ active: true });
  act(() => announceReactionReceived(received('shown')));
  expect(result.current).toHaveLength(1);
  unmount();
  expect(vi.getTimerCount()).toBe(0);
});
