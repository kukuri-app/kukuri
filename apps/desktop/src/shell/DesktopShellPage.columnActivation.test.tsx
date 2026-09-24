import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, test, vi } from 'vitest';

import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import { renderAtHash, setViewportWidth } from './DesktopShellPage.testHelpers';

// Issue #1053: Column 内の操作(本文・header のボタン / プルダウン)は、押した Column を
// active にして route をその Column へ同期する。route と active Column の scope がずれて
// Timeline Column が新規に開いたり、active が奪われたりしない。

const GENERAL_HASH = '#/timeline?topic=kukuri%3Atopic%3Ageneral';
const DEV_TIMELINE_HASH = '#/timeline?topic=kukuri%3Atopic%3Adev';

beforeEach(() => {
  setViewportWidth(1024);
  window.history.replaceState(null, '', '/');
});

function columnIds() {
  return Array.from(document.querySelectorAll<HTMLElement>('[data-column-id]')).map(
    (column) => column.dataset.columnId
  );
}

function column(title: string) {
  return screen.getByRole('region', { name: new RegExp(`^${title} Column,`) });
}

function columnHeader(title: string) {
  const header = column(title).querySelector('.shell-column-header');
  if (!(header instanceof HTMLElement)) throw new Error(`${title} column header not found`);
  return header;
}

async function renderWorkspace(api = createDesktopMockApi()) {
  renderAtHash(GENERAL_HASH, api);
  const notifications = await screen.findByRole('region', { name: /^Notifications Column,/ });
  await waitFor(() => {
    expect(within(notifications).queryByText('Loading notifications...')).not.toBeInTheDocument();
  });
  return api;
}

async function switchTimelineTopicToDev(user: ReturnType<typeof userEvent.setup>) {
  await user.selectOptions(
    within(column('Timeline')).getByRole('combobox', { name: 'Timeline topic' }),
    'kukuri:topic:dev'
  );
  await waitFor(() => expect(window.location.hash).toBe(DEV_TIMELINE_HASH));
}

async function selectColumnBody(
  user: ReturnType<typeof userEvent.setup>,
  title: string,
  expectedHash: string
) {
  await user.click(column(title));
  await waitFor(() => {
    expect(column(title)).toHaveAttribute('aria-current', 'true');
    expect(window.location.hash).toBe(expectedHash);
  });
}

async function clickExploreBodyButton(user: ReturnType<typeof userEvent.setup>) {
  const discover = within(column('Explore')).getByRole('tab', { name: 'Discover' });
  const onClick = vi.fn();
  discover.addEventListener('click', onClick);
  await user.click(discover);
  return { discover, onClick };
}

test('a body button in another Column runs once and focuses that Column after the Timeline topic was switched', async () => {
  const user = userEvent.setup();
  await renderWorkspace();
  await switchTimelineTopicToDev(user);
  await selectColumnBody(user, 'Profile', '#/profile?topic=kukuri%3Atopic%3Ageneral');
  await selectColumnBody(user, 'Timeline', DEV_TIMELINE_HASH);
  const idsBefore = columnIds();

  const { discover, onClick } = await clickExploreBodyButton(user);

  await waitFor(() => {
    expect(column('Explore')).toHaveAttribute('aria-current', 'true');
    expect(window.location.hash).toBe('#/explore?topic=kukuri%3Atopic%3Ageneral');
  });
  expect(onClick).toHaveBeenCalledTimes(1);
  expect(discover).toHaveAttribute('aria-selected', 'true');
  expect(columnIds()).toEqual(idsBefore);
  expect(screen.getAllByRole('region', { name: /^Timeline Column,/ })).toHaveLength(1);
});

test('a body button in another Column moves the route to that Column', async () => {
  const user = userEvent.setup();
  await renderWorkspace();
  await selectColumnBody(user, 'Profile', '#/profile?topic=kukuri%3Atopic%3Ageneral');
  await selectColumnBody(user, 'Timeline', GENERAL_HASH);
  const idsBefore = columnIds();

  const { onClick } = await clickExploreBodyButton(user);

  await waitFor(() => {
    expect(column('Explore')).toHaveAttribute('aria-current', 'true');
    expect(window.location.hash).toBe('#/explore?topic=kukuri%3Atopic%3Ageneral');
  });
  expect(onClick).toHaveBeenCalledTimes(1);
  expect(columnIds()).toEqual(idsBefore);
});

test('a header button in another Column runs once and focuses that Column', async () => {
  const api = createDesktopMockApi();
  const listNotifications = vi.fn(api.listNotificationsPage);
  api.listNotificationsPage = listNotifications;
  const user = userEvent.setup();
  await renderWorkspace(api);
  await switchTimelineTopicToDev(user);
  const idsBefore = columnIds();
  const callsBefore = listNotifications.mock.calls.length;

  await user.click(within(columnHeader('Notifications')).getByRole('button', { name: 'Refresh' }));

  await waitFor(() => {
    expect(column('Notifications')).toHaveAttribute('aria-current', 'true');
    expect(window.location.hash).toBe('#/notifications?topic=kukuri%3Atopic%3Ageneral');
    expect(listNotifications.mock.calls.length).toBeGreaterThan(callsBefore);
  });
  expect(columnIds()).toEqual(idsBefore);
});

test('a header select in an inactive Timeline Column focuses it and routes to the selected topic', async () => {
  const user = userEvent.setup();
  await renderWorkspace();
  await selectColumnBody(user, 'Profile', '#/profile?topic=kukuri%3Atopic%3Ageneral');
  const idsBefore = columnIds();

  await user.selectOptions(
    within(column('Timeline')).getByRole('combobox', { name: 'Timeline topic' }),
    'kukuri:topic:dev'
  );

  await waitFor(() => {
    expect(column('Timeline')).toHaveAttribute('aria-current', 'true');
    expect(window.location.hash).toBe(DEV_TIMELINE_HASH);
  });
  expect(columnIds()).toEqual(idsBefore);
});
