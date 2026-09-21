import { screen, waitFor, within } from '@testing-library/react';
import { beforeEach, expect, test } from 'vitest';

import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import {
  buildNotification,
  getDetailPane,
  renderAtHash,
  setViewportWidth,
} from './DesktopShellPage.testHelpers';

// #1271: カラム本文を一段内側で丸ごと包むだけの枠(`.panel`)を置かない。
// 一覧や本文はカラム本文へ直接並べ、左右の余白はカラム本文の padding だけにする。

beforeEach(() => {
  setViewportWidth(1024);
  window.history.replaceState(null, '', '/');
});

const TOPIC_QUERY = 'topic=kukuri%3Atopic%3Ageneral';
const AUTHOR_PUBKEY = 'b'.repeat(64);

function columnBody(column: HTMLElement) {
  const body = column.querySelector<HTMLElement>('.shell-column-body');
  if (!body) throw new Error('expected column body');
  return body;
}

function expectUnframed(element: Element) {
  expect(element.closest('.panel')).toBeNull();
}

function findColumn(title: string) {
  return screen.findAllByRole('region', { name: new RegExp(`^${title} Column,`) }).then(
    (columns) => columns[0]
  );
}

test('timeline feed is placed directly in the column body', async () => {
  renderAtHash(`#/timeline?${TOPIC_QUERY}`);

  const column = await findColumn('Timeline');
  expectUnframed(await within(column).findByText('No posts yet for this topic.'));
});

test('timeline bookmarks view is placed directly in the column body', async () => {
  renderAtHash(`#/timeline?${TOPIC_QUERY}&timelineView=bookmarks`);

  const column = await findColumn('Timeline');
  expectUnframed(await within(column).findByText('No bookmarked posts yet.'));
});

test('notification list and empty state are placed directly in the column body', async () => {
  const view = renderAtHash(
    `#/notifications?${TOPIC_QUERY}`,
    createDesktopMockApi({ notifications: [buildNotification()] })
  );

  const column = await findColumn('Notifications');
  expectUnframed(await within(column).findByRole('list', { name: 'Notifications' }));
  view.unmount();

  renderAtHash(`#/notifications?${TOPIC_QUERY}`, createDesktopMockApi({ notifications: [] }));
  const emptyColumn = await findColumn('Notifications');
  expectUnframed(await within(emptyColumn).findByText('No notifications yet.'));
});

test('own profile feed is placed directly in the column body', async () => {
  renderAtHash(`#/profile?${TOPIC_QUERY}`);

  const column = await findColumn('Profile');
  expectUnframed(await within(column).findByText('No public posts published yet.'));
});

test('author detail feed is placed directly in the column body', async () => {
  renderAtHash(
    `#/timeline?${TOPIC_QUERY}&context=author&authorPubkey=${AUTHOR_PUBKEY}`,
    createDesktopMockApi({
      authorSocialViews: {
        [AUTHOR_PUBKEY]: {
          name: 'bob',
          following: false,
          followed_by: false,
          mutual: false,
        },
      },
    })
  );

  await waitFor(() => {
    expect(getDetailPane('Author')).toBeInTheDocument();
  });
  expectUnframed(await within(getDetailPane('Author')).findByText("This user hasn't posted publicly yet."));
});

test('direct message list is placed directly in the column body', async () => {
  renderAtHash(`#/messages?${TOPIC_QUERY}`);

  const column = await findColumn('Messages');
  expectUnframed(
    await waitFor(() => {
      const heading = columnBody(column).querySelector('h3');
      if (!heading) throw new Error('expected messages heading');
      return heading;
    })
  );
  expectUnframed(await within(column).findByText('No direct messages yet.'));
});

test('direct message conversation is placed directly in the column body', async () => {
  renderAtHash(`#/messages?${TOPIC_QUERY}&peerPubkey=${AUTHOR_PUBKEY}`);

  const column = await findColumn('Conversation');
  expectUnframed(await within(column).findByText('No messages yet.'));
});

test('live session header and list are placed directly in the column body', async () => {
  renderAtHash(`#/live?${TOPIC_QUERY}`);

  const column = await findColumn('Live');
  expectUnframed(await within(column).findByRole('heading', { name: 'Live' }));
  const list = await waitFor(() => {
    const found = columnBody(column).querySelector('.post-list');
    if (!found) throw new Error('expected live list');
    return found;
  });
  expectUnframed(list);
});

test('explore workspace is placed directly in the column body', async () => {
  renderAtHash(`#/explore?${TOPIC_QUERY}`);

  const column = await findColumn('Explore');
  expectUnframed(await within(column).findByTestId('community-index-explore'));
});
