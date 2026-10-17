// #1840 AC-3: 非表示の作者(mute・block)の公開スレッドは、一覧の行・本文の chip・card に出さない(一覧と ID からの取得と同じ)。
import { act, render, screen, waitFor } from '@testing-library/react';
import { expect, test, vi } from 'vitest';

import { SmartReferenceText } from '@/components/core/SmartReferenceText';
import { usePublicThreadColumnTitles } from '@/shell/page/columnTitles';
import type { AuthorSocialView, DesktopApi, PublicChannelView } from '@/lib/api';
import { buildPublicThreadLink } from '@/lib/internalLinks';
import type { ColumnState } from '@/shell/slices/workspace';
import { createDesktopShellStore, DesktopShellStoreContext } from '@/shell/store';

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
