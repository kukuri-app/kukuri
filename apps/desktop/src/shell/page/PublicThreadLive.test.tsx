// #1840 AC-4: 公開スレッドの詳細の列の上部に、そのスレッドで配信中の Live を新しく始まった順に 3 件まで出し、押すとスレッドの
// 「ライブ」の列を開く。終わった Live と、非表示の主催者(mute・CN の評価)の Live は出さない。一覧の行の配信中の印と、詳細の
// 上部の「ライブ」は、Live の画面と同じく開発者モードでだけ出す。
import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, test, vi } from 'vitest';

import type { AuthorSocialView, AuthorTrustGate, DesktopApi, LiveSessionView, PublicChannelView } from '@/lib/api';
import type { ColumnState } from '@/shell/slices/workspace';
import { createDesktopShellStore, DesktopShellStoreContext, timelineStorageKeyForChannel } from '@/shell/store';

import { PublicThreadList, PublicThreadScopeActions } from './PublicThreadList';
import { PublicThreadLiveRows } from './PublicThreadLive';
import { LiveActionsContext, type LiveActions } from './liveActions';
import { PublicThreadProviders } from './PublicThreadProviders';

const TOPIC = 'kukuri:topic:general';
const THREAD = 'pc-1700000000000-cccccccccccccccc';
const HOST = 'a'.repeat(64);
const GATED = 'b'.repeat(64);
const MUTED = 'd'.repeat(64);

function live(id: string, host: string, status: LiveSessionView['status'] = 'Live'): LiveSessionView {
  return {
    session_id: id,
    host_pubkey: host,
    title: `Live ${id}`,
    description: '',
    status,
    started_at: 1,
    viewer_count: 0,
    joined_by_me: false,
    channel_id: THREAD,
    audience_label: 'Public thread',
  };
}

function setup(children: React.ReactNode, sessions: LiveSessionView[], developerMode = true) {
  const store = createDesktopShellStore();
  store.getState().setField('developerModeEnabled', developerMode);
  store.getState().setField('liveSessionsByScopeKey', { [timelineStorageKeyForChannel(TOPIC, THREAD)]: sessions });
  store.getState().setField('knownAuthorsByPubkey', {
    [HOST]: { author_pubkey: HOST, display_name: 'Alice', muted: false } as AuthorSocialView,
    [MUTED]: { author_pubkey: MUTED, muted: true } as AuthorSocialView,
  });
  store.getState().setField('authorTrustGates', {
    [GATED]: { author_pubkey: GATED, hidden: true, reasons: [] } as unknown as AuthorTrustGate,
  });
  const openLive = vi.fn();
  const thread: PublicChannelView = {
    channel_id: THREAD,
    topic_id: TOPIC,
    title: 'Watch party',
    creator_pubkey: HOST,
    created_at: 1,
    last_post_at: 1,
    live_now: true,
  };
  const api = {
    getPublicChannel: vi.fn(async () => thread),
    listPublicChannels: vi.fn(async () => ({ items: [thread], next_cursor: null, unavailable_count: 0 })),
  } as unknown as DesktopApi;
  render(
    <DesktopShellStoreContext.Provider value={store}>
      <PublicThreadProviders api={api} open={() => undefined} openLive={openLive} copyLink={() => undefined}>
        <LiveActionsContext.Provider value={{ api, locale: 'en', localPubkey: 'me', pendingBySessionId: {}, broadcastingSessionId: null } as unknown as LiveActions}>{children}</LiveActionsContext.Provider>
      </PublicThreadProviders>
    </DesktopShellStoreContext.Provider>
  );
  return { openLive };
}

test('the carousel shows current live thumbnails and opens details before the live column', async () => {
  const user = userEvent.setup();
  // 一覧の行は新しく始まった順(store の行の順)。
  const { openLive } = setup(<PublicThreadLiveRows topic={TOPIC} channelId={THREAD} />, [
    live('gated', GATED),
    live('4', HOST),
    live('ended', HOST, 'Ended'),
    live('muted', MUTED),
    live('3', HOST),
    live('2', HOST),
    live('1', HOST),
  ]);
  const rows = within(screen.getByRole('list', { name: 'Live now' }));
  expect(rows.getAllByRole('button').map((button) => button.getAttribute('aria-label'))).toEqual([
    'Live Live 4 Alice', 'Live Live 3 Alice', 'Live Live 2 Alice', 'Live Live 1 Alice',
  ]);
  await user.click(rows.getByRole('button', { name: 'Live Live 3 Alice' }));
  expect(openLive).not.toHaveBeenCalled();
  const dialog = screen.getByRole('dialog', { name: 'Live 3' });
  expect(within(dialog).getByText('Alice')).toBeInTheDocument();
  await user.click(within(dialog).getByRole('button', { name: 'Show large' }));
  expect(openLive).toHaveBeenLastCalledWith(TOPIC, THREAD, '3');

});

test('nothing is shown without a live session', () => {
  setup(<PublicThreadLiveRows topic={TOPIC} channelId={THREAD} />, [live('ended', HOST, 'Ended')]);
  expect(screen.queryByRole('list', { name: 'Live now' })).toBeNull();
});

const LIST: ColumnState = {
  id: 'list',
  kind: 'publicThreads',
  scope: { topicId: TOPIC, channelId: null },
  pinned: false,
  preferredDesktopSpan: 1,
};

test('the list row is marked live without an unselected live column entry', async () => {
  const { openLive } = setup(
    <>
      <PublicThreadList column={LIST} />
      <PublicThreadScopeActions topic={TOPIC} channelId={THREAD} />
    </>,
    []
  );
  expect(await screen.findByRole('button', { name: /^Watch party .* Live$/ })).toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Live' })).toBeNull();
  expect(openLive).not.toHaveBeenCalled();
});

test('without developer mode the list row has no live mark and the header has no live button', async () => {
  setup(
    <>
      <PublicThreadList column={LIST} />
      <PublicThreadScopeActions topic={TOPIC} channelId={THREAD} />
    </>,
    [],
    false
  );
  const row = await screen.findByRole('button', { name: /^Watch party/ });
  expect(row).not.toHaveTextContent(/Live$/);
  expect(screen.queryByRole('button', { name: 'Live' })).toBeNull();
});
