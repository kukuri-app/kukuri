// #1888 AC-2: 公開スレッドのタイトルの変更と削除。作成者だけが詳細の上部の menu から変え、削除する。表示中の詳細は同期の時刻が
// 変わると ID から読み直し、変更と削除を本文の chip・card・一覧の行へ反映する。
import { act, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, test, vi } from 'vitest';

import { SmartReferenceText } from '@/components/core/SmartReferenceText';
import type { DesktopApi, PublicChannelView } from '@/lib/api';
import { buildPublicThreadLink } from '@/lib/internalLinks';
import type { ColumnState } from '@/shell/slices/workspace';
import { createDesktopShellStore, DesktopShellStoreContext } from '@/shell/store';

import { PublicThreadLinkCard } from './PublicThreadLinkCard';
import { PublicThreadList, PublicThreadScopeActions } from './PublicThreadList';
import { PublicThreadProviders } from './PublicThreadProviders';

const TOPIC = 'kukuri:topic:general';
const CREATOR = 'c'.repeat(64);
const THREAD: PublicChannelView = {
  channel_id: 'pc-1700000000000-cccccccccccccccc',
  topic_id: TOPIC,
  title: 'Release notes',
  creator_pubkey: CREATOR,
  created_at: 1,
  last_post_at: 1,
  live_now: false,
};
const LIST: ColumnState = {
  id: 'list',
  kind: 'publicThreads',
  scope: { topicId: TOPIC, channelId: null },
  pinned: false,
  preferredDesktopSpan: 1,
};

afterEach(() => vi.restoreAllMocks());

function setup(api: Partial<DesktopApi>, localPubkey: string) {
  const store = createDesktopShellStore();
  store.getState().setField('syncStatus', (current) => ({ ...current, local_author_pubkey: localPubkey }));
  render(
    <DesktopShellStoreContext.Provider value={store}>
      <PublicThreadProviders
        api={{ listThread: vi.fn(async () => ({ items: [], next_cursor: null })), ...api } as unknown as DesktopApi}
        open={() => undefined}
        openLive={() => undefined}
        copyLink={() => undefined}
      >
        <section data-testid='detail'>
          <PublicThreadScopeActions topic={TOPIC} channelId={THREAD.channel_id} />
        </section>
        <PublicThreadList column={LIST} />
        <article data-testid='post'>
          <SmartReferenceText text={buildPublicThreadLink(TOPIC, THREAD.channel_id)} />
          <PublicThreadLinkCard topic={TOPIC} channelId={THREAD.channel_id} />
        </article>
      </PublicThreadProviders>
    </DesktopShellStoreContext.Provider>
  );
  const synced = (at: number) =>
    act(() => store.getState().setField('syncStatus', (current) => ({ ...current, last_sync_ts: at })));
  return { store, synced };
}

const titles = () => [...document.querySelectorAll('.public-thread-row-title')].map((title) => title.textContent);
const chip = () => document.querySelector('.smart-reference-chip')?.textContent;
const card = () => screen.getByTestId('post').querySelector('.public-thread-card');

test('another device’s rename and deletion reach the chip, the card and the list after a sync', async () => {
  let current = THREAD;
  const { synced } = setup(
    {
      getPublicChannel: vi.fn(async () => current),
      listPublicChannels: vi.fn(async () => ({ items: [THREAD], next_cursor: null, unavailable_count: 0 })),
    },
    'a'.repeat(64)
  );
  await waitFor(() => expect(chip()).toBe('Release notes'));
  await waitFor(() => expect(card()).toHaveTextContent('Release notes'));
  await waitFor(() => expect(titles()).toEqual(['Release notes']));
  // 作成者でなければ menu を出さない。
  expect(within(screen.getByTestId('detail')).queryByRole('button', { name: 'Manage thread' })).toBeNull();

  current = { ...THREAD, title: 'Release notes 0.5' };
  await synced(1);
  await waitFor(() => expect(chip()).toBe('Release notes 0.5'));
  await waitFor(() => expect(card()).toHaveTextContent('Release notes 0.5'));

  // 削除を知ったら、読み込んだ一覧の行・chip・card に出さず、詳細の上部の操作も出さない。
  current = { ...THREAD, title: '', deleted: true };
  await synced(2);
  await waitFor(() => expect(card()).toHaveTextContent('This thread was deleted.'));
  expect(titles()).toEqual([]);
  expect(chip()).not.toContain('Release notes');
  expect(screen.getByTestId('detail')).toBeEmptyDOMElement();
});

test('the creator renames and deletes the thread from the detail menu', async () => {
  const user = userEvent.setup();
  const updatePublicChannelTitle = vi.fn(async (_topic: string, _channelId: string, title: string) => ({
    ...THREAD,
    title,
  }));
  const deletePublicChannel = vi.fn(async () => ({ ...THREAD, title: '', deleted: true }));
  const confirm = vi.spyOn(window, 'confirm');
  setup(
    {
      getPublicChannel: vi.fn(async () => THREAD),
      listPublicChannels: vi.fn(async () => ({ items: [THREAD], next_cursor: null, unavailable_count: 0 })),
      updatePublicChannelTitle,
      deletePublicChannel,
    },
    CREATOR
  );
  const detail = screen.getByTestId('detail');
  await user.click(await within(detail).findByRole('button', { name: 'Manage thread' }));
  await user.click(await screen.findByRole('button', { name: 'Change title' }));
  const dialog = await screen.findByRole('dialog', { name: 'Change the title' });
  const input = within(dialog).getByRole('textbox', { name: 'Title' });
  // 同じタイトルと空のタイトルでは変えられない。
  expect(within(dialog).getByRole('button', { name: 'Change' })).toBeDisabled();
  await user.clear(input);
  expect(within(dialog).getByRole('button', { name: 'Change' })).toBeDisabled();
  await user.type(input, ' Release notes 0.5 ');
  await user.click(within(dialog).getByRole('button', { name: 'Change' }));
  await waitFor(() => expect(updatePublicChannelTitle).toHaveBeenCalledWith(TOPIC, THREAD.channel_id, 'Release notes 0.5'));
  await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
  await waitFor(() => expect(chip()).toBe('Release notes 0.5'));

  // 削除は確かめてから行い、断れば何もしない。
  confirm.mockReturnValueOnce(false);
  await user.click(within(detail).getByRole('button', { name: 'Manage thread' }));
  await user.click(await screen.findByRole('button', { name: 'Delete thread' }));
  expect(confirm).toHaveBeenCalledWith(expect.stringContaining('Release notes 0.5'));
  expect(deletePublicChannel).not.toHaveBeenCalled();
  confirm.mockReturnValueOnce(true);
  await user.click(within(detail).getByRole('button', { name: 'Manage thread' }));
  await user.click(await screen.findByRole('button', { name: 'Delete thread' }));
  await waitFor(() => expect(deletePublicChannel).toHaveBeenCalledWith(TOPIC, THREAD.channel_id));
  await waitFor(() => expect(detail).toBeEmptyDOMElement());
  await waitFor(() => expect(card()).toHaveTextContent('This thread was deleted.'));
  expect(titles()).toEqual([]);
});
