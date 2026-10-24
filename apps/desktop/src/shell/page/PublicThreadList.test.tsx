// #1840 AC-3: 非表示の作者(mute・block)の公開スレッドは、一覧の行・本文の chip・card に出さない(一覧と ID からの取得と同じ)。
import { act, render, screen, waitFor } from '@testing-library/react';
import { expect, test, vi } from 'vitest';

import { SmartReferenceText } from '@/components/core/SmartReferenceText';
import { usePublicThreadColumnTitles } from '@/shell/page/columnTitles';
import type { AttachmentView, AuthorSocialView, DesktopApi, PostView, PublicChannelView } from '@/lib/api';
import { buildPublicThreadLink } from '@/lib/internalLinks';
import type { ColumnState } from '@/shell/slices/workspace';
import { createDesktopShellStore, DesktopShellStoreContext } from '@/shell/store';

import { buildImagePost, buildVideoPost } from '@/shell/DesktopShellPage.testHelpers';
import { isAdultLabeledPost } from '@/shell/media';
import { buildPostMediaView } from '@/shell/viewModels/postMediaView';

import { PublicThreadLinkCard } from './PublicThreadLinkCard';
import { PublicThreadList } from './PublicThreadList';
import { PublicThreadProviders } from './PublicThreadProviders';

const TOPIC = 'kukuri:topic:general';
const MUTED = 'b'.repeat(64);
const OTHER = 'c'.repeat(64);

function thread(channelId: string, title: string, creator: string): PublicChannelView {
  return { channel_id: channelId, topic_id: TOPIC, title, creator_pubkey: creator, created_at: 1, last_post_at: 1, live_now: false };
}

const HIDDEN = thread('pc-1700000000000-bbbbbbbbbbbbbbbb', 'Muted thread', MUTED);
const SHOWN = thread('pc-1700000000001-cccccccccccccccc', 'Shown thread', OTHER);

function author(pubkey: string, muted: boolean): AuthorSocialView {
  return { author_pubkey: pubkey, muted, blocking: false, blocked_by: false } as AuthorSocialView;
}

test('a hidden creator’s thread is left out of the list rows, the chips and the card', async () => {
  const store = createDesktopShellStore();
  store.getState().setField('knownAuthorsByPubkey', { [MUTED]: author(MUTED, false), [OTHER]: author(OTHER, false) });
  const api = {
    listPublicChannels: vi.fn(async () => ({ items: [HIDDEN, SHOWN], next_cursor: null, unavailable_count: 0 })),
    getPublicChannel: vi.fn(async (_topic: string, channelId: string) =>
      [HIDDEN, SHOWN].find((item) => item.channel_id === channelId) ?? null
    ),
    listThread: vi.fn(async () => ({ items: [], next_cursor: null })),
  } as unknown as DesktopApi;
  const column: ColumnState = {
    id: 'list',
    kind: 'publicThreads',
    scope: { topicId: TOPIC, channelId: null },
    pinned: false,
    preferredDesktopSpan: 1,
  };
  render(
    <DesktopShellStoreContext.Provider value={store}>
      <PublicThreadProviders api={api} open={() => undefined} openLive={() => undefined} copyLink={() => undefined}>
        <PublicThreadList column={column} />
        <article data-testid='post'>
          <SmartReferenceText
            text={`${buildPublicThreadLink(TOPIC, HIDDEN.channel_id)} ${buildPublicThreadLink(TOPIC, SHOWN.channel_id)}`}
          />
          <PublicThreadLinkCard topic={TOPIC} channelId={HIDDEN.channel_id} />
        </article>
      </PublicThreadProviders>
    </DesktopShellStoreContext.Provider>
  );
  const titles = () => [...document.querySelectorAll('.public-thread-row-title')].map((title) => title.textContent);
  const chips = () => [...document.querySelectorAll('.smart-reference-chip')].map((chip) => chip.textContent);
  await waitFor(() => expect(titles()).toEqual(['Muted thread', 'Shown thread']));
  await waitFor(() => expect(chips()).toEqual(['Muted thread', 'Shown thread']));
  await waitFor(() => expect(screen.getByTestId('post')).toHaveTextContent('Public thread Muted thread'));

  // mute した直後から、読み込んだ行・chip・card に出さない(次に読んだページが除く前でも)。
  act(() => {
    store.getState().setField('knownAuthorsByPubkey', (current) => ({ ...current, [MUTED]: author(MUTED, true) }));
  });
  expect(titles()).toEqual(['Shown thread']);
  expect(chips()).toEqual(['Public thread pc-1700000…', 'Shown thread']);
  expect(screen.getByTestId('post').querySelector('.public-thread-card')).toHaveTextContent(
    "Couldn't load this public thread"
  );
});

function ColumnTitles() {
  return <output>{JSON.stringify(usePublicThreadColumnTitles())}</output>;
}

// 列の見出しと Control Center の列の一覧は、列の scope の公開スレッドのタイトルを使う。作成者を非表示にしたら出さない。
test('column titles leave out a hidden creator’s thread', () => {
  const store = createDesktopShellStore();
  store.getState().setField('publicThreadsById', { [HIDDEN.channel_id]: HIDDEN, [SHOWN.channel_id]: SHOWN });
  store.getState().setField('knownAuthorsByPubkey', { [MUTED]: author(MUTED, false), [OTHER]: author(OTHER, false) });
  store.getState().setField('workspaceState', (current) => ({
    ...current,
    columns: [
      ...current.columns,
      ...[HIDDEN, SHOWN].map((item) => ({
        id: `detail-${item.channel_id}`,
        kind: 'timeline' as const,
        scope: { topicId: TOPIC, channelId: item.channel_id },
        pinned: false,
        preferredDesktopSpan: 1 as const,
      })),
    ],
  }));
  render(
    <DesktopShellStoreContext.Provider value={store}>
      <ColumnTitles />
    </DesktopShellStoreContext.Provider>
  );
  expect(JSON.parse(screen.getByRole('status').textContent ?? '{}')).toEqual({
    [HIDDEN.channel_id]: 'Muted thread',
    [SHOWN.channel_id]: 'Shown thread',
  });
  act(() => {
    store.getState().setField('knownAuthorsByPubkey', (current) => ({ ...current, [MUTED]: author(MUTED, true) }));
  });
  expect(JSON.parse(screen.getByRole('status').textContent ?? '{}')).toEqual({ [SHOWN.channel_id]: 'Shown thread' });
});

const IMAGE = 'a'.repeat(64);
const POSTER = 'p'.repeat(64);

// #1900: 行の右に最初の投稿の 1 枚目の画像(無ければ動画の静止画)を出す。添付の無い行には出さない。表示設定 OFF の
// 成人向けは伏せた印だけを出す。最初の投稿が手元に無い行は、画面に出たら ID から読み、読めたら出す。
test('rows show the first post’s first image, a video poster or a hidden mark', async () => {
  const store = createDesktopShellStore();
  const withPost = (channelId: string, title: string, post: PostView): PublicChannelView => ({
    ...thread(channelId, title, OTHER),
    first_post_id: post.object_id,
    first_post: post,
  });
  const second: AttachmentView = { hash: 'b'.repeat(64), mime: 'image/png', bytes: 1, role: 'image_original', status: 'Available' };
  const image = buildImagePost({ object_id: 'image' });
  const rows = [
    withPost('pc-1700000000005-cccccccccccccccc', 'Image', { ...image, attachments: [...image.attachments, second] }),
    withPost('pc-1700000000004-cccccccccccccccc', 'Video', buildVideoPost({ object_id: 'video' })),
    withPost('pc-1700000000003-cccccccccccccccc', 'Text', buildImagePost({ object_id: 'text', attachments: [] })),
    withPost('pc-1700000000002-cccccccccccccccc', 'Adult', buildImagePost({ object_id: 'adult', content_labels: ['adult'] })),
    { ...thread('pc-1700000000001-cccccccccccccccc', 'Not local', OTHER), first_post_id: 'remote' },
  ];
  const api = {
    listPublicChannels: vi.fn(async () => ({ items: rows, next_cursor: null, unavailable_count: 0 })),
    getPublicChannel: vi.fn(async (_topic: string, channelId: string) => ({
      ...rows.find((row) => row.channel_id === channelId)!,
      first_post: buildImagePost({ object_id: 'remote' }),
    })),
  } as unknown as DesktopApi;
  const postMedia = (post: PostView) =>
    buildPostMediaView(post, {
      mediaObjectUrls: { [IMAGE]: 'blob:image', [POSTER]: 'blob:poster' },
      adultContentGated: isAdultLabeledPost(post),
      unsupportedVideoManifests: {},
    });
  const column: ColumnState = {
    id: 'list',
    kind: 'publicThreads',
    scope: { topicId: TOPIC, channelId: null },
    pinned: false,
    preferredDesktopSpan: 1,
  };
  render(
    <DesktopShellStoreContext.Provider value={store}>
      <PublicThreadProviders api={api} open={() => undefined} openLive={() => undefined} copyLink={() => undefined}>
        <PublicThreadList column={column} postMedia={postMedia} />
      </PublicThreadProviders>
    </DesktopShellStoreContext.Provider>
  );
  const thumbnail = (title: string) =>
    screen.getByText(title).closest('li')?.querySelector('.public-thread-row-thumbnail') ?? null;
  await waitFor(() => expect(thumbnail('Not local')?.querySelector('img')).toHaveAttribute('src', 'blob:image'));
  expect(api.getPublicChannel).toHaveBeenCalledTimes(1);
  expect(api.getPublicChannel).toHaveBeenCalledWith(TOPIC, 'pc-1700000000001-cccccccccccccccc');
  expect(thumbnail('Image')?.querySelectorAll('img')).toHaveLength(1);
  expect(thumbnail('Image')?.querySelector('img')).toHaveAttribute('src', 'blob:image');
  expect(thumbnail('Video')?.querySelector('img')).toHaveAttribute('src', 'blob:poster');
  expect(thumbnail('Text')).toBeNull();
  expect(thumbnail('Adult')).toHaveAccessibleName('Image hidden as adult material');
  expect(thumbnail('Adult')?.querySelector('img')).toBeNull();
});
