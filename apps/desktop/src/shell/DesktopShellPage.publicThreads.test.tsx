// #1840 AC-3: 公開スレッドの一覧の列・作成・詳細の列・リンクの表示。
import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, test, vi } from 'vitest';

import { App } from '@/App';
import type { PostView, PublicChannelView } from '@/lib/api';
import { buildPublicThreadLink } from '@/lib/internalLinks';
import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import type { ColumnState } from '@/shell/slices/workspace';
import { WORKSPACE_LAYOUT_STORAGE_KEY } from '@/shell/workspacePersistence';

import { getActiveColumn, openControlCenter, renderAtHash, setViewportWidth } from './DesktopShellPage.testHelpers';

const TOPIC = 'kukuri:topic:general';
const OTHER = 'b'.repeat(64);
const CHANNEL = 'pc-1700000000000-0123456789abcdef';
const LIST_ROUTE = '#/timeline?topic=kukuri%3Atopic%3Ageneral&context=publicThreads';

beforeEach(() => {
  setViewportWidth(1024);
  window.history.replaceState(null, '', '/');
});

function thread(channelId: string, title: string, createdAt: number, lastPostAt: number): PublicChannelView {
  return { channel_id: channelId, topic_id: TOPIC, title, creator_pubkey: OTHER, created_at: createdAt, last_post_at: lastPostAt, live_now: false };
}

function post(objectId: string, content: string, channelId: string | null, createdAt: number): PostView {
  return {
    object_id: objectId,
    envelope_id: `envelope-${objectId}`,
    author_pubkey: OTHER,
    author_name: 'bob',
    author_display_name: null,
    following: false,
    followed_by: false,
    mutual: false,
    friend_of_friend: false,
    object_kind: 'post',
    is_threadable: true,
    content,
    content_status: 'Available',
    attachments: [],
    created_at: createdAt,
    reply_to: null,
    root_id: objectId,
    channel_id: channelId,
    audience_label: channelId ? 'Public thread' : 'Public',
  };
}

function rowTitles(column: HTMLElement) {
  return [...column.querySelectorAll('.public-thread-row-title')].map((title) => title.textContent);
}

function persistedColumns(): ColumnState[] {
  return (JSON.parse(window.localStorage.getItem(WORKSPACE_LAYOUT_STORAGE_KEY) ?? '{}') as { columns?: ColumnState[] })
    .columns ?? [];
}

test('a public thread is created from the topic list, opens its detail and keeps its posts out of the topic timeline', async () => {
  const user = userEvent.setup();
  const api = createDesktopMockApi();
  const createPublicChannel = vi.spyOn(api, 'createPublicChannel');
  const createPost = vi.spyOn(api, 'createPost');
  render(<App api={api} />);

  // Control Center の topic の「公開」の次の行から、その topic の一覧の列を開く(判断 9)。
  const controlCenter = await openControlCenter(user);
  const general = [...controlCenter.querySelectorAll<HTMLElement>('li.topic-item')].find(
    (item) => item.querySelector('.topic-link')?.textContent === 'general'
  );
  if (!general) throw new Error('general topic row not found');
  await user.click(within(general).getByRole('button', { name: /^Public thread/ }));
  const list = await waitFor(() => getActiveColumn('Public threads'));
  expect(window.location.hash).toBe(LIST_ROUTE);
  expect(await within(list).findByText('No public threads in this topic yet.')).toBeInTheDocument();

  // 列の下部の「スレッドを作る」。タイトルが空の間は作れない。
  await user.click(
    within(list.querySelector<HTMLElement>('.shell-column-footer') ?? list).getByRole('button', {
      name: 'Create public thread',
    })
  );
  const dialog = await screen.findByRole('dialog', { name: 'Create a public thread' });
  expect(within(dialog).getByRole('button', { name: 'Create' })).toBeDisabled();
  await user.type(within(dialog).getByRole('textbox', { name: 'Title' }), 'Release notes');
  await user.type(within(dialog).getByRole('textbox', { name: 'First post (optional)' }), 'What changed in 0.5');
  await user.click(within(dialog).getByRole('button', { name: 'Create' }));
  await waitFor(() => expect(createPublicChannel).toHaveBeenCalledWith(TOPIC, 'Release notes', 'What changed in 0.5'));
  const created = await createPublicChannel.mock.results[0]!.value;

  // 作った後は、一覧の列を親にした詳細の列を開く。
  const detail = await waitFor(() => getActiveColumn('Public thread'));
  expect(window.location.hash).toBe(`#/timeline?topic=kukuri%3Atopic%3Ageneral&channel=${created.channel_id}`);
  expect(within(detail).getAllByText('Release notes · general').length).toBeGreaterThan(0);
  expect(await within(detail).findByText('What changed in 0.5')).toBeInTheDocument();
  expect(within(detail).getByRole('img', { name: 'Public thread: Release notes' })).toBeInTheDocument();
  const detailColumn = persistedColumns().find((column) => column.scope?.channelId === created.channel_id);
  const listColumn = persistedColumns().find((column) => column.kind === 'publicThreads');
  expect(detailColumn?.parentColumnId).toBe(listColumn?.id);

  // 詳細の列から投稿する。投稿先は公開スレッド。
  await user.click(within(detail).getByRole('button', { name: /^Post to / }));
  await user.type(within(detail).getByPlaceholderText('Write a post'), 'second post');
  await user.click(within(detail).getByRole('button', { name: 'Post' }));
  await waitFor(() =>
    expect(createPost.mock.lastCall?.slice(0, 5)).toEqual([
      TOPIC,
      'second post',
      null,
      [],
      { kind: 'public_channel', channel_id: created.channel_id },
    ])
  );
  expect(await within(detail).findByText('second post')).toBeInTheDocument();

  // 公開スレッドの中で返信する。返信ツリーの列は公開スレッドの scope のままで、返信も同じスレッドへ送る。
  await user.click(within(detail).getAllByRole('button', { name: 'Reply' })[0]!);
  const replyTree = await screen.findByRole('region', { name: /^Thread Column,/ });
  await waitFor(() =>
    expect(replyTree.querySelector('.shell-column-header')).toHaveTextContent('Thread · Release notes · general')
  );
  const replyInput = await within(replyTree).findByPlaceholderText('Write a reply');
  await user.type(replyInput, 'a reply in the thread');
  await user.click(within(replyInput.closest('form')!).getByRole('button', { name: 'Reply' }));
  await waitFor(() =>
    expect(createPost.mock.lastCall?.slice(0, 5)).toEqual([
      TOPIC,
      'a reply in the thread',
      expect.any(String),
      expect.any(Array),
      { kind: 'public_channel', channel_id: created.channel_id },
    ])
  );
  await user.click(within(replyTree).getByRole('button', { name: 'Close Thread' }));
  await waitFor(() => expect(getActiveColumn('Public thread')).toBe(detail));

  // 公開スレッドの投稿は topic のタイムラインに出さない(判断 4)。
  const timeline = screen.getByRole('region', { name: /^Timeline Column,/ });
  expect(within(timeline).queryByText('What changed in 0.5')).not.toBeInTheDocument();
  expect(within(timeline).queryByText('second post')).not.toBeInTheDocument();
  expect(within(timeline).queryByText('a reply in the thread')).not.toBeInTheDocument();

  // 詳細の列を閉じると、一覧の列へ戻り、作ったスレッドが並ぶ。
  await user.click(within(detail).getByRole('button', { name: 'Close Public thread' }));
  const back = await waitFor(() => getActiveColumn('Public threads'));
  await waitFor(() => expect(rowTitles(back)).toEqual(['Release notes']));
}, 30_000);

test('the list order is chosen with the keyboard, stored with the pinned column and restored after a restart', async () => {
  const user = userEvent.setup();
  const seedPublicChannels = {
    [TOPIC]: [
      thread('pc-1700000000000-aaaaaaaaaaaaaaaa', 'Older thread', 1_700_000_000_000, 1_700_000_900_000),
      thread('pc-1700000500000-bbbbbbbbbbbbbbbb', 'Newer thread', 1_700_000_500_000, 1_700_000_600_000),
    ],
  };
  const first = renderAtHash(LIST_ROUTE, createDesktopMockApi({ seedPublicChannels }));
  const list = await waitFor(() => getActiveColumn('Public threads'));
  await waitFor(() => expect(rowTitles(list)).toEqual(['Older thread', 'Newer thread']));

  const trigger = within(list).getByRole('button', { name: 'Sort order: Latest post first' });
  expect(trigger).toHaveAttribute('aria-haspopup', 'menu');
  trigger.focus();
  await user.keyboard('{Enter}');
  const menu = await screen.findByRole('menu', { name: 'Sort order' });
  const options = within(menu).getAllByRole('menuitemradio');
  expect(options.map((option) => [option.textContent, option.getAttribute('aria-checked')])).toEqual([
    ['Latest post first', 'true'],
    ['Newest first', 'false'],
  ]);
  await waitFor(() => expect(options[0]).toHaveFocus());
  await user.keyboard('{ArrowDown}');
  expect(options[1]).toHaveFocus();
  await user.keyboard('{Enter}');
  await waitFor(() => expect(rowTitles(getActiveColumn('Public threads'))).toEqual(['Newer thread', 'Older thread']));
  await user.click(within(getActiveColumn('Public threads')).getByRole('button', { name: 'Pin Public threads' }));
  await waitFor(() =>
    expect(persistedColumns().find((column) => column.kind === 'publicThreads')).toMatchObject({
      publicThreadOrder: 'created',
      pinned: true,
    })
  );

  first.unmount();
  window.history.replaceState(null, '', '/');
  render(<App api={createDesktopMockApi({ seedPublicChannels })} />);
  const restored = await waitFor(() => getActiveColumn('Public threads'));
  await waitFor(() => expect(rowTitles(restored)).toEqual(['Newer thread', 'Older thread']));
  expect(within(restored).getByRole('button', { name: 'Sort order: Newest first' })).toBeInTheDocument();
  expect(within(restored).getByRole('button', { name: 'Unpin Public threads' })).toBeInTheDocument();
  expect(window.location.hash).toBe(LIST_ROUTE);
}, 30_000);

test('public thread links in a post show their titles and the first post, and open the thread under its list', async () => {
  const user = userEvent.setup();
  const link = buildPublicThreadLink(TOPIC, CHANNEL);
  const other = buildPublicThreadLink(TOPIC, 'pc-1700000500000-fedcba9876543210');
  const api = createDesktopMockApi({
    seedPublicChannels: {
      [TOPIC]: [
        { ...thread(CHANNEL, 'Weekly sync', 1_700_000_000_000, 1_700_000_000_000), first_post_id: 'first-post' },
        thread('pc-1700000500000-fedcba9876543210', 'Design review', 1_700_000_500_000, 1_700_000_500_000),
      ],
    },
    seedPosts: {
      [TOPIC]: [
        post('share', `Join us ${link} and ${other}`, null, 1_700_000_100_000),
        post('first-post', 'Agenda for this week', CHANNEL, 1_700_000_000_000),
      ],
    },
  });
  render(<App api={api} />);

  const timeline = await waitFor(() => getActiveColumn('Timeline'));
  const share = (await within(timeline).findByText('Join us', { exact: false })).closest('article');
  if (!(share instanceof HTMLElement)) throw new Error('shared post not found');
  // 本文の下の card(判断 10)。タイトルと最初の投稿を出し、本文の chip もタイトルになる。
  const card = await waitFor(() => {
    const found = share.querySelector<HTMLElement>('.public-thread-card');
    if (!found) throw new Error('public thread card not shown');
    return found;
  });
  await waitFor(() => expect(card).toHaveAccessibleName('Public thread Weekly sync Agenda for this week'));
  // どのリンクもタイトルの chip になる。card は最初の 1 件だけ。
  await waitFor(() =>
    expect([...share.querySelectorAll('.smart-reference-chip')].map((chip) => chip.textContent)).toEqual([
      'Weekly sync',
      'Design review',
    ])
  );
  expect(share.querySelectorAll('.public-thread-card')).toHaveLength(1);
  expect(within(timeline).queryByText('Agenda for this week', { selector: '.post-title *' })).not.toBeInTheDocument();

  await user.click(card);
  const detail = await waitFor(() => getActiveColumn('Public thread'));
  expect(within(detail).getAllByText('Weekly sync · general').length).toBeGreaterThan(0);
  expect(await within(detail).findByText('Agenda for this week')).toBeInTheDocument();
  await user.click(within(detail).getByRole('button', { name: 'Close Public thread' }));
  await waitFor(() => expect(getActiveColumn('Public threads')).toBeInTheDocument());
}, 30_000);

test('a direct link opens the thread under its list, copies its link and is restored after a restart', async () => {
  const user = userEvent.setup();
  const writeText = vi.fn().mockResolvedValue(undefined);
  Object.defineProperty(window.navigator, 'clipboard', { configurable: true, value: { writeText } });
  const seedPublicChannels = { [TOPIC]: [thread(CHANNEL, 'Weekly sync', 1_700_000_000_000, 1_700_000_000_000)] };
  const link = buildPublicThreadLink(TOPIC, CHANNEL);
  const first = renderAtHash(link, createDesktopMockApi({ seedPublicChannels }));

  // タイトルは詳細の列が表示中に読む。一覧の列は親として開く。
  const detail = await waitFor(() => getActiveColumn('Public thread'));
  await waitFor(() => expect(within(detail).getAllByText('Weekly sync · general').length).toBeGreaterThan(0));
  expect(screen.getByRole('region', { name: /^Public threads Column,/ })).toBeInTheDocument();
  await user.click(within(detail).getByRole('button', { name: 'Copy link' }));
  expect(writeText).toHaveBeenLastCalledWith(link);

  first.unmount();
  window.history.replaceState(null, '', '/');
  render(<App api={createDesktopMockApi({ seedPublicChannels })} />);
  const restored = await waitFor(() => getActiveColumn('Public thread'));
  await waitFor(() => expect(within(restored).getAllByText('Weekly sync · general').length).toBeGreaterThan(0));
  expect(window.location.hash).toBe(link);

  // Control Center の列の一覧でも「公開スレッド」と呼ぶ。共有のボタンは private channel のものなので押せない。
  const controlCenter = await openControlCenter(user);
  expect(within(controlCenter).getByRole('button', { name: 'Focus Public thread' })).toBeInTheDocument();
  expect(within(controlCenter).getByRole('button', { name: 'Share active channel' })).toBeDisabled();
  await user.keyboard('{Escape}');

  await user.click(within(restored).getByRole('button', { name: 'Close Public thread' }));
  await waitFor(() => expect(getActiveColumn('Public threads')).toBeInTheDocument());
}, 30_000);
