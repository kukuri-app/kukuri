// #1689: タイムラインのフィルターは、フィードを相互フォロー・フォロー中の著者と自分の投稿に絞り、Column ごとに保存される。
import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, test } from 'vitest';

import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import { App } from '@/App';
import type { DesktopApi, PostView } from '@/lib/api';
import { columnIdentityId } from '@/shell/slices/workspace';
import { WORKSPACE_LAYOUT_STORAGE_KEY } from '@/shell/workspacePersistence';
import { selectTimelineView, setViewportWidth } from './DesktopShellPage.testHelpers';

const TOPIC = 'kukuri:topic:general';

beforeEach(() => {
  setViewportWidth(1024);
  window.history.replaceState(null, '', '/');
});

function post(
  id: string,
  author: string,
  relation: Partial<Pick<PostView, 'following' | 'followed_by' | 'mutual'>>,
  createdAt: number
): PostView {
  return {
    object_id: id,
    envelope_id: `envelope-${id}`,
    author_pubkey: author.repeat(64),
    author_name: id,
    author_display_name: null,
    following: false,
    followed_by: false,
    mutual: false,
    friend_of_friend: false,
    ...relation,
    object_kind: 'post',
    is_threadable: true,
    content: `${id} body`,
    content_status: 'Available',
    attachments: [],
    created_at: createdAt,
    reply_to: null,
    root_id: id,
    channel_id: null,
    audience_label: 'Public',
  };
}

// mock の自分は 'f'。相互フォロー・フォローのみ・フォロワーのみ・関係なし・自分の投稿。
const POSTS = [
  post('mutual', 'b', { following: true, followed_by: true, mutual: true }, 5),
  post('following', 'c', { following: true }, 4),
  post('follower', 'e', { followed_by: true }, 3),
  post('stranger', 'd', {}, 2),
  post('own', 'f', {}, 1),
];

function timelineColumn() {
  return screen.getByRole('region', { name: /^Timeline Column,/ });
}

function header() {
  return timelineColumn().querySelector<HTMLElement>('.shell-column-header')!;
}

function shownPosts() {
  return POSTS.map((item) => item.object_id).filter((id) =>
    within(timelineColumn()).queryByText(`${id} body`)
  );
}

function storedFilter() {
  const layout = JSON.parse(window.localStorage.getItem(WORKSPACE_LAYOUT_STORAGE_KEY) ?? '{}');
  return layout.columns?.find((column: { kind: string }) => column.kind === 'timeline')?.timelineFilter;
}

async function selectFilter(user: ReturnType<typeof userEvent.setup>, trigger: string, option: string) {
  await user.click(within(header()).getByRole('button', { name: trigger }));
  const menu = await screen.findByRole('menu', { name: 'Filter' });
  await user.click(within(menu).getByRole('menuitemradio', { name: option }));
}

test('the filter menu narrows the feed to mutuals or follows, keeps own posts and is stored per column', async () => {
  const user = userEvent.setup();
  render(<App api={createDesktopMockApi({ seedPosts: { [TOPIC]: POSTS } })} />);
  expect(await within(timelineColumn()).findByText('stranger body')).toBeInTheDocument();

  const trigger = within(header()).getByRole('button', { name: 'Filter' });
  expect(trigger).toHaveAttribute('aria-haspopup', 'menu');
  expect(trigger).not.toHaveAttribute('data-active');
  expect(trigger.querySelector('svg')).toHaveClass('lucide-funnel');

  // keyboard で開いて選ぶ。開くと先頭の項目へ focus が移り、矢印 key で次の項目へ移る。
  trigger.focus();
  await user.keyboard('{Enter}');
  const menu = await screen.findByRole('menu', { name: 'Filter' });
  const options = within(menu).getAllByRole('menuitemradio');
  expect(options.map((option) => [option.textContent, option.getAttribute('aria-checked')])).toEqual([
    ['No filter', 'true'],
    ['Mutuals only', 'false'],
    ['Following only', 'false'],
  ]);
  await waitFor(() => expect(options[0]).toHaveFocus());
  await user.keyboard('{ArrowDown}');
  expect(options[1]).toHaveFocus();
  await user.keyboard('{Enter}');

  await waitFor(() => expect(shownPosts()).toEqual(['mutual', 'own']));
  const mutual = within(header()).getByRole('button', { name: 'Filter (Mutuals only)' });
  expect(mutual).toHaveAttribute('data-active', 'true');
  expect(mutual.querySelector('svg')).toHaveClass('lucide-users');
  await waitFor(() => expect(storedFilter()).toBe('mutual'));

  await selectFilter(user, 'Filter (Mutuals only)', 'Following only');
  await waitFor(() => expect(shownPosts()).toEqual(['mutual', 'following', 'own']));
  expect(
    within(header()).getByRole('button', { name: 'Filter (Following only)' }).querySelector('svg')
  ).toHaveClass('lucide-user-round-arrow-left');
  await waitFor(() => expect(storedFilter()).toBe('following'));

  await selectFilter(user, 'Filter (Following only)', 'No filter');
  await waitFor(() => expect(shownPosts()).toEqual(['mutual', 'following', 'follower', 'stranger', 'own']));
  expect(within(header()).getByRole('button', { name: 'Filter' })).not.toHaveAttribute('data-active');
  await waitFor(() => expect(storedFilter()).toBeUndefined());
});

test('a stored filter is restored, shows its own empty copy and counts only matching new posts', async () => {
  const user = userEvent.setup();
  const scope = { topicId: TOPIC, channelId: null };
  window.localStorage.setItem(WORKSPACE_LAYOUT_STORAGE_KEY, JSON.stringify({
    version: 1,
    activeColumnId: columnIdentityId('timeline', scope),
    columns: [
      { id: columnIdentityId('timeline', scope), kind: 'timeline', scope, pinned: true, preferredDesktopSpan: 1, timelineFilter: 'following' },
    ],
  }));
  let items = [POSTS[3]];
  const baseApi = createDesktopMockApi();
  const api: DesktopApi = {
    ...baseApi,
    async listTimeline(topic, cursor, limit, timelineScope) {
      if (topic !== TOPIC) return baseApi.listTimeline(topic, cursor, limit, timelineScope);
      return { items: items.map((item) => ({ ...item })), next_cursor: null };
    },
  };

  render(<App api={api} />);
  expect(await within(timelineColumn()).findByText('No posts match this filter.')).toBeInTheDocument();
  expect(within(header()).getByRole('button', { name: 'Filter (Following only)' })).toBeInTheDocument();

  // 新着 2 件のうち、フィルターに一致する 1 件だけを数える。
  items = [post('following-new', 'c', { following: true }, 7), post('stranger-new', 'd', {}, 6), ...items];
  window.dispatchEvent(new Event('focus'));
  await user.click(await within(timelineColumn()).findByRole('button', { name: 'Show 1 new post' }));
  expect(await within(timelineColumn()).findByText('following-new body')).toBeInTheDocument();
  expect(within(timelineColumn()).queryByText('stranger-new body')).not.toBeInTheDocument();
});

test('the filter applies to the feed only and leaves bookmarks unfiltered', async () => {
  const user = userEvent.setup();
  const api = createDesktopMockApi({ seedPosts: { [TOPIC]: POSTS } });
  await api.bookmarkPost(TOPIC, 'stranger');
  render(<App api={api} />);
  expect(await within(timelineColumn()).findByText('stranger body')).toBeInTheDocument();

  await selectTimelineView(user, 'Bookmarks');
  await selectFilter(user, 'Filter', 'Mutuals only');
  expect(within(timelineColumn()).getByText('stranger body')).toBeInTheDocument();

  await selectTimelineView(user, 'Feed');
  await waitFor(() => expect(shownPosts()).toEqual(['mutual', 'own']));
});
