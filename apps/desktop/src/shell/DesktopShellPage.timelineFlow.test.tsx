// #1647: Flow モードの Timeline Column は新着を保留せずに反映し、window が非表示の間と画面外でも取得を続ける。
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';

import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import { App } from '@/App';
import { setViewportWidth } from './DesktopShellPage.testHelpers';
import type { DesktopApi, PostView } from '@/lib/api';
import { columnIdentityId } from '@/shell/slices/workspace';
import { REFRESH_INTERVAL_MS } from '@/shell/store';
import { WORKSPACE_LAYOUT_STORAGE_KEY } from '@/shell/workspacePersistence';

beforeEach(() => {
  setViewportWidth(1024);
  window.history.replaceState(null, '', '/');
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  Reflect.deleteProperty(document, 'visibilityState');
});

function storedTimelineFlow() {
  const layout = JSON.parse(window.localStorage.getItem(WORKSPACE_LAYOUT_STORAGE_KEY) ?? '{}');
  return layout.columns?.find((column: { kind: string }) => column.kind === 'timeline')?.timelineFlow;
}

function post(id: string, createdAt: number): PostView {
  return {
    object_id: id,
    envelope_id: `envelope-${id}`,
    author_pubkey: 'a'.repeat(64),
    author_name: 'alice',
    author_display_name: null,
    following: false,
    followed_by: false,
    mutual: false,
    friend_of_friend: false,
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

test('the feed button toggles Flow mode only by pointer while the feed is shown', async () => {
  const user = userEvent.setup();
  render(<App api={createDesktopMockApi()} />);

  const timelineColumn = screen.getByRole('region', { name: /Timeline Column/ });
  const views = within(timelineColumn).getByRole('tablist', { name: 'Timeline views' });
  await user.click(within(views).getByRole('tab', { name: 'Feed' }));

  const flowTab = within(views).getByRole('tab', { name: 'Feed (Flow mode)' });
  expect(flowTab).toHaveAttribute('aria-selected', 'true');
  expect(flowTab.querySelector('svg')).toHaveClass('lucide-loader-pinwheel', 'icon-spinning');
  await waitFor(() => expect(storedTimelineFlow()).toBe(true));

  // tab の移動の key では切り替わらない。
  fireEvent.keyDown(flowTab, { key: 'Home' });
  expect(within(views).getByRole('tab', { name: 'Feed (Flow mode)' })).toBeInTheDocument();

  // ブックマークからフィードへ戻すだけでは Flow の ON/OFF は変わらない。
  await user.click(within(views).getByRole('tab', { name: 'Bookmarks' }));
  await user.click(within(views).getByRole('tab', { name: 'Feed (Flow mode)' }));
  expect(within(views).getByRole('tab', { name: 'Feed (Flow mode)' })).toHaveAttribute('aria-selected', 'true');

  await user.click(within(views).getByRole('tab', { name: 'Feed (Flow mode)' }));
  const feedTab = within(views).getByRole('tab', { name: 'Feed' });
  expect(feedTab.querySelector('svg')).toHaveClass('lucide-list');
  await waitFor(() => expect(storedTimelineFlow()).toBeUndefined());
});

test('Flow mode shows remote posts without the pending banner', async () => {
  const user = userEvent.setup();
  const olderPost = post('post-old', 1);
  let timelineItems = [olderPost];
  const baseApi = createDesktopMockApi({ seedPosts: { 'kukuri:topic:general': timelineItems } });
  const api: DesktopApi = {
    ...baseApi,
    async listTimeline(topic, cursor, limit, scope) {
      if (topic !== 'kukuri:topic:general') return baseApi.listTimeline(topic, cursor, limit, scope);
      return { items: timelineItems.map((item) => ({ ...item })), next_cursor: null };
    },
  };

  render(<App api={api} />);
  expect(await screen.findByText('post-old body')).toBeInTheDocument();
  await user.click(screen.getByRole('tab', { name: 'Feed' }));

  timelineItems = [post('post-new', 2), olderPost];
  window.dispatchEvent(new Event('focus'));

  expect(await screen.findByText('post-new body')).toBeInTheDocument();
  expect(screen.queryByRole('button', { name: /new post/ })).not.toBeInTheDocument();
});

test('Flow mode columns keep refreshing while off screen and while the window is hidden', async () => {
  vi.useFakeTimers();
  // 画面外: Canvas の交差の通知が来ない(どの背景 Column も表示中にならない)。
  vi.stubGlobal('IntersectionObserver', class {
    observe() {}
    unobserve() {}
    disconnect() {}
  });
  const generalScope = { topicId: 'kukuri:topic:general', channelId: null };
  const devScope = { topicId: 'kukuri:topic:dev', channelId: null };
  const opsScope = { topicId: 'kukuri:topic:ops', channelId: null };
  window.localStorage.setItem(WORKSPACE_LAYOUT_STORAGE_KEY, JSON.stringify({
    version: 1,
    activeColumnId: columnIdentityId('timeline', generalScope),
    columns: [
      { id: columnIdentityId('timeline', generalScope), kind: 'timeline', scope: generalScope, pinned: true, preferredDesktopSpan: 1 },
      { id: columnIdentityId('timeline', devScope), kind: 'timeline', scope: devScope, pinned: true, preferredDesktopSpan: 1, timelineFlow: true },
      { id: columnIdentityId('timeline', opsScope), kind: 'timeline', scope: opsScope, pinned: true, preferredDesktopSpan: 1 },
    ],
  }));
  const api = createDesktopMockApi();
  const listTimelineSpy = vi.spyOn(api, 'listTimeline');
  const refreshedTopics = async () => {
    listTimelineSpy.mockClear();
    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL_MS + 50);
    return new Set(listTimelineSpy.mock.calls.map(([topic]) => topic));
  };

  render(<App api={api} />);
  await vi.advanceTimersByTimeAsync(0);

  const offScreen = await refreshedTopics();
  expect(offScreen).toEqual(new Set(['kukuri:topic:general', 'kukuri:topic:dev']));

  Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => 'hidden' });
  const hidden = await refreshedTopics();
  expect(hidden).toEqual(new Set(['kukuri:topic:dev']));
});
