// #994: ブックマーク view は初回取得中に loading を示し、取得成功 0 件のときだけ始め方の案内を出す。
// 失敗時は再試行を出し、案内の CTA は同じ Column をフィードへ戻すだけで bookmark API を呼ばない(INVAR-1)。
import { act, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, test, vi } from 'vitest';

import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import { App } from '@/App';
import type { BookmarkedPostView } from '@/lib/api';
import {
  createDeferred,
  getTimelineViewTabs,
  selectTimelineView,
  setViewportWidth,
} from './DesktopShellPage.testHelpers';

const DEMO_TOPIC_HASH = '#/timeline?topic=kukuri%3Atopic%3Ageneral';

beforeEach(() => {
  setViewportWidth(1024);
  window.history.replaceState(null, '', '/');
});

function timelineColumn() {
  return screen.getByRole('region', { name: /^Timeline Column,/ });
}

test('shows loading first, then the guidance with the Bookmark chip, and the CTA returns to the feed', async () => {
  const user = userEvent.setup();
  const api = createDesktopMockApi();
  // 取得を test 側で保留し、loading の観測を取得完了のタイミングに依存させない(#1165)。
  const pending = createDeferred<{ items: BookmarkedPostView[]; newer_cursor: null; older_cursor: null }>();
  const listSpy = vi.spyOn(api, 'listBookmarkedPostsPage').mockReturnValue(pending.promise);
  const bookmarkSpy = vi.spyOn(api, 'bookmarkPost');
  render(<App api={api} />);
  await waitFor(() => {
    expect(window.location.hash).toBe(DEMO_TOPIC_HASH);
  });

  await selectTimelineView(user, 'Bookmarks');
  // 初回取得中: 空文言を出さず loading を示す(false empty の禁止)。
  expect(within(timelineColumn()).getByRole('status', { name: 'Loading bookmarks…' })).toBeInTheDocument();
  expect(within(timelineColumn()).queryByText('No bookmarked posts yet.')).not.toBeInTheDocument();
  await act(async () => pending.resolve({ items: [], newer_cursor: null, older_cursor: null }));

  const guidance = await within(timelineColumn()).findByTestId('bookmarks-empty-state');
  expect(guidance).toHaveAttribute('role', 'status');
  expect(within(guidance).getByText('No bookmarked posts yet.')).toBeInTheDocument();
  expect(within(timelineColumn()).queryByRole('status', { name: 'Loading bookmarks…' })).not.toBeInTheDocument();
  const chip = guidance.querySelector('[data-testid="action-ref"]');
  expect(chip).not.toBeNull();
  expect(chip).toHaveTextContent('Bookmark');
  expect(chip?.querySelector('svg')).toHaveAttribute('aria-hidden', 'true');
  expect(within(guidance).queryByRole('button', { name: 'Bookmark' })).not.toBeInTheDocument();
  expect(within(guidance).getByText(/this device only/)).toBeInTheDocument();

  await user.click(within(guidance).getByRole('button', { name: 'Show timeline' }));
  await waitFor(() => {
    expect(within(getTimelineViewTabs()).getByRole('tab', { name: 'Feed' })).toHaveAttribute('aria-selected', 'true');
  });
  expect(window.location.hash).toBe(DEMO_TOPIC_HASH);
  expect(listSpy).toHaveBeenCalled();
  expect(bookmarkSpy).not.toHaveBeenCalled();
});

test('a failed fetch shows the error with retry instead of the empty guidance, and retry fetches once more', async () => {
  const user = userEvent.setup();
  const api = createDesktopMockApi();
  const listSpy = vi
    .spyOn(api, 'listBookmarkedPostsPage')
    .mockRejectedValueOnce(new Error('bookmarks unavailable'));
  render(<App api={api} />);
  await waitFor(() => {
    expect(window.location.hash).toBe(DEMO_TOPIC_HASH);
  });

  await selectTimelineView(user, 'Bookmarks');
  const error = await within(timelineColumn()).findByText('bookmarks unavailable');
  expect(error).toBeInTheDocument();
  expect(within(timelineColumn()).queryByTestId('bookmarks-empty-state')).not.toBeInTheDocument();
  expect(within(timelineColumn()).queryByText('No bookmarked posts yet.')).not.toBeInTheDocument();
  const callsBeforeRetry = listSpy.mock.calls.length;

  await user.click(within(timelineColumn()).getByRole('button', { name: 'Retry' }));
  expect(listSpy).toHaveBeenCalledTimes(callsBeforeRetry + 1);
  await within(timelineColumn()).findByTestId('bookmarks-empty-state');
  expect(within(timelineColumn()).queryByText('bookmarks unavailable')).not.toBeInTheDocument();
});
