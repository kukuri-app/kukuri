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
  return { channel_id: channelId, topic_id: TOPIC, title, creator_pubkey: CREATOR, created_at: 1, last_post_at: 1 };
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
      <PublicThreadProviders api={api} open={() => undefined} copyLink={() => undefined}>
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

test('an unreadable thread says the title could not be loaded and is read again the next time', async () => {
  const { getPublicChannel, view } = setup(<PublicThreadScopeActions topic={TOPIC} channelId={LINKED.channel_id} />, false);
  expect(await screen.findByText("Couldn't load the title")).toBeInTheDocument();
  expect(getPublicChannel).toHaveBeenCalledTimes(1);

  view.rerender(
    <DesktopShellStoreContext.Provider value={createDesktopShellStore()}>
      <PublicThreadProviders
        api={{ getPublicChannel } as unknown as DesktopApi}
        open={() => undefined}
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
