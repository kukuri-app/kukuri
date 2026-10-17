import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';

import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import { App } from '@/App';
import {
  openChannelManager,
  openControlCenter,
  publishPost,
  renderAtHash,
  selectTimelineView,
  setViewportWidth,
} from './DesktopShellPage.testHelpers';

beforeEach(() => {
  setViewportWidth(1024);
  window.history.replaceState(null, '', '/');
});

afterEach(() => {
  vi.useRealTimers();
});

const DEMO_TOPIC_HASH = '#/timeline?topic=kukuri%3Atopic%3Ageneral';
const CHANNEL_HASH = `${DEMO_TOPIC_HASH}&channel=channel-1`;

function findTimelineColumnByScope(scopeLabel: string) {
  const column = screen
    .getAllByRole('region', { name: /^Timeline Column,/ })
    .find((candidate) =>
      candidate.querySelector('.shell-column-header')?.textContent?.includes(scopeLabel)
    );
  if (!column) {
    throw new Error(`Timeline Column (${scopeLabel}) が見つかりません`);
  }
  return column;
}

function columnViewTabs(column: HTMLElement) {
  const header = column.querySelector('.shell-column-header');
  if (!(header instanceof HTMLElement)) {
    throw new Error('column header が見つかりません');
  }
  return within(header).getByRole('tablist', { name: 'Timeline views' });
}

async function setUpPublicAndPrivateColumns(user: ReturnType<typeof userEvent.setup>) {
  // private channel を作成すると選択 channel が channel-1 になり、Timeline Column が Public / core の 2 本になる。
  const channelDialog = await openChannelManager(user);
  await user.type(within(channelDialog).getByPlaceholderText('Channel name'), 'core');
  await user.click(within(channelDialog).getByRole('button', { name: 'Create Channel' }));
  await waitFor(() => {
    expect(window.location.hash).toBe(CHANNEL_HASH);
  });
  await user.click(within(channelDialog).getByRole('button', { name: 'Close dialog' }));

  // private channel 側に投稿を 1 件入れる(feed 表示の識別用)。
  await publishPost(user, 'channel post');
  await waitFor(() => {
    expect(
      within(findTimelineColumnByScope('core · general')).getByText('channel post')
    ).toBeInTheDocument();
  });

  // Control Center の topic 行で「Public」を押して global 選択を public に戻す。
  const controlCenter = await openControlCenter(user);
  const generalTopicRow = within(controlCenter).getByRole('button', { name: 'general' }).closest('li');
  if (!(generalTopicRow instanceof HTMLElement)) {
    throw new Error('general topic row not found');
  }
  await user.click(within(generalTopicRow).getByRole('button', { name: /^Public(?! thread)/ }));
  await waitFor(() => {
    expect(window.location.hash).toBe(DEMO_TOPIC_HASH);
  });
  await waitFor(() => {
    expect(findTimelineColumnByScope('Public · general')).toHaveAttribute('aria-current', 'true');
  });
  return findTimelineColumnByScope('core · general');
}

// Issue #1053: header の view 切替は操作した Column を active にし、URL もその Column の view を指す。
test('非 active な Timeline Column の Bookmarks 切替はその Column を active にし、他の Column の view に波及しない', async () => {
  const user = userEvent.setup();
  render(<App api={createDesktopMockApi()} />);

  const privateColumn = await setUpPublicAndPrivateColumns(user);

  await user.click(within(columnViewTabs(privateColumn)).getByRole('tab', { name: 'Bookmarks' }));
  await waitFor(() => {
    expect(
      within(columnViewTabs(findTimelineColumnByScope('core · general'))).getByRole('tab', {
        name: 'Bookmarks',
      })
    ).toHaveAttribute('aria-selected', 'true');
  });

  // 他の Timeline Column は Feed のまま。
  const publicColumn = findTimelineColumnByScope('Public · general');
  expect(
    within(columnViewTabs(publicColumn)).getByRole('tab', { name: 'Feed' })
  ).toHaveAttribute('aria-selected', 'true');
  expect(
    within(columnViewTabs(publicColumn)).getByRole('tab', { name: 'Bookmarks' })
  ).toHaveAttribute('aria-selected', 'false');

  // 切り替えた Column の body は bookmarks 一覧(空)になり、feed の投稿は出ない。
  const switchedColumn = findTimelineColumnByScope('core · general');
  expect(await within(switchedColumn).findByText('No bookmarked posts yet.')).toBeInTheDocument();
  expect(within(switchedColumn).queryByText('channel post')).not.toBeInTheDocument();

  // 操作した Column が focus 対象になり、route はその view を指す(bookmarks は topic 単位の URL)。
  await waitFor(() => {
    expect(switchedColumn).toHaveAttribute('aria-current', 'true');
    expect(window.location.hash).toBe(`${DEMO_TOPIC_HASH}&timelineView=bookmarks`);
  });
  expect(publicColumn).not.toHaveAttribute('aria-current', 'true');
});

test('active な Timeline Column の view 切替は従来どおり URL の timelineView query を同期する', async () => {
  const user = userEvent.setup();
  render(<App api={createDesktopMockApi()} />);
  await waitFor(() => {
    expect(window.location.hash).toBe(DEMO_TOPIC_HASH);
  });

  await selectTimelineView(user, 'Bookmarks');
  await waitFor(() => {
    expect(window.location.hash).toBe(`${DEMO_TOPIC_HASH}&timelineView=bookmarks`);
  });

  await selectTimelineView(user, 'Feed');
  await waitFor(() => {
    expect(window.location.hash).toBe(DEMO_TOPIC_HASH);
  });
});

test('timelineView=bookmarks の deep link は対象 Timeline Column の view に反映される', async () => {
  renderAtHash(`${DEMO_TOPIC_HASH}&timelineView=bookmarks`);

  const timelineColumn = await screen.findByRole('region', { name: /^Timeline Column,/ });
  await waitFor(() => {
    expect(
      within(columnViewTabs(timelineColumn)).getByRole('tab', { name: 'Bookmarks' })
    ).toHaveAttribute('aria-selected', 'true');
  });
  expect(await within(timelineColumn).findByText('No bookmarked posts yet.')).toBeInTheDocument();
});

test('reload 後に各 Column の view が復元され、非 active の Bookmarks Column に bookmark 済み投稿が表示される', async () => {
  const user = userEvent.setup();
  const api = createDesktopMockApi();
  const view = render(<App api={api} />);

  const privateColumn = await setUpPublicAndPrivateColumns(user);

  // active(Public)Column に投稿して bookmark する。
  await publishPost(user, 'public post');
  const publicColumn = findTimelineColumnByScope('Public · general');
  await waitFor(() => {
    expect(within(publicColumn).getByText('public post')).toBeInTheDocument();
  });
  await user.click(within(publicColumn).getAllByRole('button', { name: 'Bookmark' })[0]);
  await waitFor(() => {
    expect(
      within(publicColumn).getAllByRole('button', { name: 'Remove bookmark' }).length
    ).toBeGreaterThan(0);
  });

  // 非 active(core)Column を Bookmarks に切り替える(操作した Column が active になる)。
  await user.click(within(columnViewTabs(privateColumn)).getByRole('tab', { name: 'Bookmarks' }));
  await waitFor(() => {
    expect(
      within(columnViewTabs(findTimelineColumnByScope('core · general'))).getByRole('tab', {
        name: 'Bookmarks',
      })
    ).toHaveAttribute('aria-selected', 'true');
    expect(window.location.hash).toBe(`${DEMO_TOPIC_HASH}&timelineView=bookmarks`);
  });

  // Public Column を選び直し、Bookmarks Column を非 active に戻してから reload する。
  await user.click(findTimelineColumnByScope('Public · general'));
  await waitFor(() => {
    expect(findTimelineColumnByScope('Public · general')).toHaveAttribute('aria-current', 'true');
    expect(window.location.hash).toBe(DEMO_TOPIC_HASH);
  });

  // reload(App re-mount、localStorage は保持)。
  view.unmount();
  render(<App api={api} />);

  // 各 Column の view が復元される。
  await waitFor(() => {
    expect(
      within(columnViewTabs(findTimelineColumnByScope('core · general'))).getByRole('tab', {
        name: 'Bookmarks',
      })
    ).toHaveAttribute('aria-selected', 'true');
  });
  expect(
    within(columnViewTabs(findTimelineColumnByScope('Public · general'))).getByRole('tab', {
      name: 'Feed',
    })
  ).toHaveAttribute('aria-selected', 'true');

  // 非 active の Bookmarks Column でも bookmarks データがロードされ、bookmark 済み投稿が表示される。
  await waitFor(() => {
    expect(
      within(findTimelineColumnByScope('core · general')).getByText('public post')
    ).toBeInTheDocument();
  });
});
