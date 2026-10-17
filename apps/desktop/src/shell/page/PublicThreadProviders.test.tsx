// #1840 AC-3: 公開スレッドの ID からの読み込みは画面で共用する。同じスレッドを並行して読まず、読めたものは使い回し、
// 使い回すときも覚え直す(覚えたスレッドの上限で store から外れていても、タイトルが戻る)。読めなかったものは次にまた読む。
import { act, render, screen, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { expect, test, vi } from 'vitest';

import { SmartReferenceText } from '@/components/core/SmartReferenceText';
import type { DesktopApi, PublicChannelView } from '@/lib/api';
import { buildPublicThreadLink } from '@/lib/internalLinks';
import { KNOWN_PUBLIC_THREAD_LIMIT, rememberPublicThreads } from '@/shell/slices/publicThreads';
import { createDesktopShellStore, DesktopShellStoreContext } from '@/shell/store';

import { PublicThreadScopeActions } from './PublicThreadList';
import { PublicThreadProviders } from './PublicThreadProviders';

const TOPIC = 'kukuri:topic:general';
const CREATOR = 'c'.repeat(64);

function thread(channelId: string, title: string): PublicChannelView {
  return { channel_id: channelId, topic_id: TOPIC, title, creator_pubkey: CREATOR, created_at: 1, last_post_at: 1, live_now: false };
}

const LINKED = thread('pc-1700000000000-cccccccccccccccc', 'Linked thread');
// 一覧の列が読んで覚える別のスレッド(覚える上限と同じ数)。
const OTHERS = Array.from({ length: KNOWN_PUBLIC_THREAD_LIMIT }, (_, index) =>
  thread(`pc-${1700000001000 + index}-cccccccccccccccc`, `Other ${index}`)
);

function setup(children: ReactNode, readable = true) {
  const store = createDesktopShellStore();
  const getPublicChannel = vi.fn(async (_topic: string, channelId: string) =>
    readable && channelId === LINKED.channel_id ? LINKED : null
  );
  const api = { getPublicChannel, listThread: vi.fn(async () => ({ items: [], next_cursor: null })) } as unknown as DesktopApi;
  const view = render(
    <DesktopShellStoreContext.Provider value={store}>
      <PublicThreadProviders api={api} open={() => undefined} openLive={() => undefined} copyLink={() => undefined}>
        {children}
      </PublicThreadProviders>
    </DesktopShellStoreContext.Provider>
  );
  const pushOut = () =>
    act(() => {
      store.getState().setField('publicThreadsById', (current) => rememberPublicThreads(current, OTHERS));
    });
  return { store, getPublicChannel, pushOut, view };
}

const chips = () => [...document.querySelectorAll('.smart-reference-chip')].map((chip) => chip.textContent);

test('chips share one read of a thread and get its title back after it is pushed out of the store', async () => {
  const link = buildPublicThreadLink(TOPIC, LINKED.channel_id);
  const { store, getPublicChannel, pushOut } = setup(
    <>
      <article>
        <SmartReferenceText text={link} />
      </article>
      <article>
        <SmartReferenceText text={link} />
      </article>
    </>
  );
  await waitFor(() => expect(chips()).toEqual(['Linked thread', 'Linked thread']));
  expect(getPublicChannel).toHaveBeenCalledTimes(1);

  pushOut();
  expect(store.getState().publicThreadsById[LINKED.channel_id]).toBeUndefined();
  await waitFor(() => expect(chips()).toEqual(['Linked thread', 'Linked thread']));
  expect(getPublicChannel).toHaveBeenCalledTimes(1);
});

test('the detail header gets the title back after it is pushed out of the store', async () => {
  const { store, getPublicChannel, pushOut } = setup(
    <PublicThreadScopeActions topic={TOPIC} channelId={LINKED.channel_id} />
  );
  await waitFor(() => expect(store.getState().publicThreadsById[LINKED.channel_id]?.title).toBe('Linked thread'));

  pushOut();
  await waitFor(() => expect(store.getState().publicThreadsById[LINKED.channel_id]?.title).toBe('Linked thread'));
  expect(screen.queryByText("Couldn't load the title")).not.toBeInTheDocument();
  expect(getPublicChannel).toHaveBeenCalledTimes(1);
});

// 画面内のリンクが覚える上限より多いと、読んだスレッドが互いに store から押し出し合う。chip は読めた値を持つので、
// 押し出されても表示を保ち、読み直しを繰り返さない(読み込みは chip ごとに 1 回)。
test('more links on screen than threads remembered are each read once and keep their titles', async () => {
  const count = KNOWN_PUBLIC_THREAD_LIMIT + 44;
  const ids = Array.from({ length: count }, (_, index) => `pc-${1700000002000 + index}-cccccccccccccccc`);
  const store = createDesktopShellStore();
  const getPublicChannel = vi.fn(async (_topic: string, channelId: string) => {
    await new Promise((resolve) => setTimeout(resolve, 5 + (Number(channelId.slice(3, 16)) % 7) * 5));
    return thread(channelId, `T${channelId.slice(3, 16)}`);
  });
  const api = { getPublicChannel } as unknown as DesktopApi;
  // 投稿 1 件に 100 件まで(リンク 1 件は 83 文字なので、投稿の上限の 10,000 文字に収まる)。
  const posts = Array.from({ length: Math.ceil(count / 100) }, (_, index) =>
    ids.slice(index * 100, index * 100 + 100).map((id) => buildPublicThreadLink(TOPIC, id)).join(' ')
  );
  render(
    <DesktopShellStoreContext.Provider value={store}>
      <PublicThreadProviders api={api} open={() => undefined} openLive={() => undefined} copyLink={() => undefined}>
        {posts.map((text) => (
          <article key={text.slice(0, 120)}>
            <SmartReferenceText text={text} />
          </article>
        ))}
      </PublicThreadProviders>
    </DesktopShellStoreContext.Provider>
  );
  await waitFor(() => expect(chips().every((text) => text?.startsWith('T'))).toBe(true), { timeout: 10_000 });
  expect(getPublicChannel).toHaveBeenCalledTimes(count);
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 500));
  });
  expect(getPublicChannel).toHaveBeenCalledTimes(count);
  expect(Object.keys(store.getState().publicThreadsById)).toHaveLength(KNOWN_PUBLIC_THREAD_LIMIT);
  expect(chips().every((text) => text?.startsWith('T'))).toBe(true);
}, 20_000);

test('an unreadable thread says the title could not be loaded and is read again the next time', async () => {
  const { getPublicChannel, view } = setup(<PublicThreadScopeActions topic={TOPIC} channelId={LINKED.channel_id} />, false);
  expect(await screen.findByText("Couldn't load the title")).toBeInTheDocument();
  expect(getPublicChannel).toHaveBeenCalledTimes(1);

  view.rerender(
    <DesktopShellStoreContext.Provider value={createDesktopShellStore()}>
      <PublicThreadProviders
        api={{ getPublicChannel } as unknown as DesktopApi}
        open={() => undefined}
        openLive={() => undefined}
        copyLink={() => undefined}
      >
        <article>
          <SmartReferenceText text={buildPublicThreadLink(TOPIC, LINKED.channel_id)} />
        </article>
      </PublicThreadProviders>
    </DesktopShellStoreContext.Provider>
  );
  await waitFor(() => expect(getPublicChannel).toHaveBeenCalledTimes(2));
  expect(chips()).toEqual(['Public thread pc-1700000…']);
});
