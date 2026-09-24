import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';

import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import { App } from '@/App';
import {
  buildNotification,
  getDetailPane,
  openControlCenter,
  openNotificationsInbox,
  renderAtHash,
  setViewportWidth,
} from './DesktopShellPage.testHelpers';

beforeEach(() => {
  setViewportWidth(1024);
  window.history.replaceState(null, '', '/');
});

afterEach(() => {
  vi.useRealTimers();
});

test('Control Center notifications action shows unread count and opening inbox auto-marks read', async () => {
  const user = userEvent.setup();
  const api = createDesktopMockApi({
    notifications: [
      buildNotification({
        notification_id: 'notification-unread-1',
        preview_text: 'first unread notification',
      }),
      buildNotification({
        notification_id: 'notification-unread-2',
        kind: 'mention',
        object_id: 'mention-1',
        thread_root_object_id: 'mention-1',
        preview_text: 'second unread notification',
        received_at: 2,
      }),
    ],
  });

  render(<App api={api} />);

  let controlCenter = await openControlCenter(user);
  const activityButton = within(controlCenter).getByRole('button', { name: /^Notifications (?:\d+|99\+)$/ });
  await waitFor(() => {
    expect(activityButton).toHaveTextContent('2');
  });

  await openNotificationsInbox(user);

  await waitFor(() => {
    expect(window.location.hash).toBe('#/notifications?topic=kukuri%3Atopic%3Ageneral');
  });
  controlCenter = await openControlCenter(user);
  expect(within(controlCenter).getByRole('button', { name: /^Notifications/ })).toHaveTextContent('0');
  expect(screen.getByText('first unread notification')).toBeInTheDocument();
  expect(screen.getByText('second unread notification')).toBeInTheDocument();
});

test('adult-labeled object notification hides its raw preview while display is off', async () => {
  const api = createDesktopMockApi({
    notifications: [
      buildNotification({
        notification_id: 'notification-adult-preview',
        preview_text: 'adult notification raw preview',
        content_labels: ['adult'],
      }),
    ],
  });

  renderAtHash('#/notifications?topic=kukuri%3Atopic%3Ageneral', api);

  expect(await screen.findByText(/self-labeled as adult material/)).toBeInTheDocument();
  expect(screen.queryByText('adult notification raw preview')).not.toBeInTheDocument();
});

test('desktop shell loads unread notification rows outside the inbox for OS notification bridge', async () => {
  const api = createDesktopMockApi({
    notifications: [
      buildNotification({
        notification_id: 'notification-bridge-unread',
        preview_text: 'bridge unread notification',
      }),
    ],
  });
  const listNotifications = vi.fn(api.listNotificationsPage);
  const markAllNotificationsRead = vi.fn(api.markAllNotificationsRead);
  api.listNotificationsPage = listNotifications;
  api.markAllNotificationsRead = markAllNotificationsRead;

  renderAtHash('#/timeline?topic=kukuri%3Atopic%3Ageneral', api);

  await waitFor(() => {
    expect(screen.getByTestId('control-center-trigger')).toHaveTextContent('1');
    expect(listNotifications).toHaveBeenCalled();
  });
  expect(markAllNotificationsRead).not.toHaveBeenCalled();
});

test('a visible background notifications column finishes loading without marking unread items read', async () => {
  const api = createDesktopMockApi({
    notifications: [
      buildNotification({
        notification_id: 'notification-background-column',
        preview_text: 'background column notification',
      }),
    ],
  });
  const markAllNotificationsRead = vi.fn(api.markAllNotificationsRead);
  api.markAllNotificationsRead = markAllNotificationsRead;

  renderAtHash('#/timeline?topic=kukuri%3Atopic%3Ageneral', api);

  const column = await screen.findByRole('region', { name: /^Notifications Column/ });
  expect(await within(column).findByText('background column notification')).toBeInTheDocument();
  await waitFor(() => {
    expect(within(column).queryByText('Loading notifications...')).not.toBeInTheDocument();
  });
  expect(markAllNotificationsRead).not.toHaveBeenCalled();
});

// Issue #1053: header の更新操作も Column を active にするため、本文クリックと同じく既読化される。
test('refreshing a background notifications column focuses it, refetches it, and marks it read', async () => {
  const api = createDesktopMockApi({
    notifications: [
      buildNotification({
        notification_id: 'notification-background-refresh',
        preview_text: 'background refresh notification',
      }),
    ],
  });
  const user = userEvent.setup();
  const listNotifications = vi.fn(api.listNotificationsPage);
  const markAllNotificationsRead = vi.fn(api.markAllNotificationsRead);
  api.listNotificationsPage = listNotifications;
  api.markAllNotificationsRead = markAllNotificationsRead;

  renderAtHash('#/timeline?topic=kukuri%3Atopic%3Ageneral', api);

  const column = await screen.findByRole('region', { name: /^Notifications Column/ });
  expect(await within(column).findByText('background refresh notification')).toBeInTheDocument();
  await waitFor(() => {
    expect(within(column).queryByText('Loading notifications...')).not.toBeInTheDocument();
  });
  const header = column.querySelector('.shell-column-header');
  if (!(header instanceof HTMLElement)) throw new Error('notifications column header not found');
  const callsBeforeRefresh = listNotifications.mock.calls.length;

  await user.click(within(header).getByRole('button', { name: 'Refresh' }));

  await waitFor(() => {
    expect(listNotifications.mock.calls.length).toBeGreaterThan(callsBeforeRefresh);
    expect(within(column).queryByText('Loading notifications...')).not.toBeInTheDocument();
    expect(column).toHaveAttribute('aria-current', 'true');
    expect(window.location.hash).toBe('#/notifications?topic=kukuri%3Atopic%3Ageneral');
    expect(markAllNotificationsRead).toHaveBeenCalled();
  });
});

test('clicking the active notifications action focuses the existing inbox', async () => {
  const user = userEvent.setup();

  renderAtHash('#/profile?topic=kukuri%3Atopic%3Ageneral');

  expect(await screen.findByRole('button', { name: 'Edit Profile' })).toBeInTheDocument();

  await openNotificationsInbox(user);

  await waitFor(() => {
    expect(window.location.hash).toBe('#/notifications?topic=kukuri%3Atopic%3Ageneral');
  });
  expect(screen.getAllByRole('heading', { name: 'Notifications' }).length).toBeGreaterThan(0);

  const controlCenter = await openControlCenter(user);
  await user.click(within(controlCenter).getByRole('button', { name: /^Notifications/ }));

  await waitFor(() => {
    expect(window.location.hash).toBe('#/notifications?topic=kukuri%3Atopic%3Ageneral');
  });
  expect(screen.getAllByRole('region', { name: /^Notifications Column/ })).toHaveLength(1);
});

test('notifications route renders inbox and marks unread notifications as read on load', async () => {
  const api = createDesktopMockApi({
    notifications: [
      buildNotification({
        notification_id: 'notification-read-on-load',
        preview_text: 'open from route',
      }),
    ],
  });
  const markAllNotificationsRead = vi.fn(api.markAllNotificationsRead);
  api.markAllNotificationsRead = markAllNotificationsRead;

  renderAtHash('#/notifications?topic=kukuri%3Atopic%3Ageneral', api);

  expect((await screen.findAllByRole('heading', { name: 'Notifications' })).length).toBeGreaterThan(0);
  await waitFor(() => {
    expect(markAllNotificationsRead).toHaveBeenCalledTimes(1);
    expect(screen.getByTestId('control-center-trigger')).not.toHaveTextContent('1');
  });
  expect(screen.getByText('open from route')).toBeInTheDocument();
});

test('notifications exposes summary and refresh in the column header', async () => {
  const api = createDesktopMockApi({
    notifications: [
      buildNotification({
        notification_id: 'notification-column-header',
        preview_text: 'header action notification',
      }),
    ],
  });
  const user = userEvent.setup();
  const listNotifications = vi.fn(api.listNotificationsPage);
  api.listNotificationsPage = listNotifications;

  renderAtHash('#/notifications?topic=kukuri%3Atopic%3Ageneral', api);

  const column = await screen.findByRole('region', { name: /^Notifications Column/ });
  await screen.findByText('header action notification');
  const header = column.querySelector('.shell-column-header');
  if (!(header instanceof HTMLElement)) throw new Error('notifications column header not found');

  expect(within(header).getByText(/1 notification/)).toBeInTheDocument();
  const callsBeforeRefresh = listNotifications.mock.calls.length;
  await user.click(within(header).getByRole('button', { name: 'Refresh' }));

  await waitFor(() => {
    expect(listNotifications.mock.calls.length).toBeGreaterThan(callsBeforeRefresh);
  });
  expect(column.querySelector('.shell-workspace-header')).not.toBeInTheDocument();
});

test('notifications route renders an empty state when the inbox has no items', async () => {
  renderAtHash('#/notifications?topic=kukuri%3Atopic%3Ageneral', createDesktopMockApi());

  expect((await screen.findAllByRole('heading', { name: 'Notifications' })).length).toBeGreaterThan(0);
  expect(await screen.findByText('No notifications yet.')).toBeInTheDocument();
});

test('notifications route surfaces a load error when the inbox request fails', async () => {
  const api = createDesktopMockApi();
  api.listNotificationsPage = vi.fn().mockRejectedValue(new Error('load notifications exploded'));

  renderAtHash('#/notifications?topic=kukuri%3Atopic%3Ageneral', api);

  expect((await screen.findAllByRole('heading', { name: 'Notifications' })).length).toBeGreaterThan(0);
  expect(await screen.findByText('load notifications exploded')).toBeInTheDocument();
});

test('notifications route surfaces auto-read errors and keeps unread state visible', async () => {
  const api = createDesktopMockApi({
    notifications: [
      buildNotification({
        notification_id: 'notification-auto-read-failure',
        preview_text: 'still unread notification',
      }),
    ],
  });
  api.markAllNotificationsRead = vi.fn().mockRejectedValue(new Error('mark read failed'));

  renderAtHash('#/notifications?topic=kukuri%3Atopic%3Ageneral', api);

  expect((await screen.findAllByRole('heading', { name: 'Notifications' })).length).toBeGreaterThan(0);
  expect(await screen.findByText('mark read failed')).toBeInTheDocument();
  expect(screen.getByText('still unread notification')).toBeInTheDocument();
  expect(screen.getByText('Unread')).toBeInTheDocument();
  await waitFor(() => {
    expect(screen.getByTestId('control-center-trigger')).toHaveTextContent('1');
  });
});

test('reply notification click-through opens the source thread in timeline', async () => {
  const user = userEvent.setup();
  renderAtHash(
    '#/notifications?topic=kukuri%3Atopic%3Ageneral',
    createDesktopMockApi({
      notifications: [
        buildNotification({
          notification_id: 'notification-thread-open',
          preview_text: 'open thread from notification',
          object_id: 'reply-1',
          thread_root_object_id: 'post-thread-open',
        }),
      ],
      seedPosts: {
        'kukuri:topic:general': [
          {
            object_id: 'post-thread-open',
            envelope_id: 'envelope-thread-open',
            author_pubkey: 'b'.repeat(64),
            author_name: 'bob',
            author_display_name: null,
            following: false,
            followed_by: false,
            mutual: false,
            friend_of_friend: false,
            object_kind: 'post',
            content: 'thread root post',
            content_status: 'Available',
            attachments: [],
            created_at: 1,
            reply_to: null,
            root_id: 'post-thread-open',
            channel_id: null,
            audience_label: 'Public',
          },
          {
            object_id: 'reply-1',
            envelope_id: 'envelope-reply-1',
            author_pubkey: 'c'.repeat(64),
            author_name: 'carol',
            author_display_name: null,
            following: false,
            followed_by: false,
            mutual: false,
            friend_of_friend: false,
            object_kind: 'post',
            content: 'thread reply post',
            content_status: 'Available',
            attachments: [],
            created_at: 2,
            reply_to: 'post-thread-open',
            root_id: 'post-thread-open',
            channel_id: null,
            audience_label: 'Public',
          },
        ],
      },
    })
  );

  await screen.findAllByRole('heading', { name: 'Notifications' });
  await user.click(screen.getByText('open thread from notification'));

  await waitFor(() => {
    expect(window.location.hash).toBe(
      '#/timeline?topic=kukuri%3Atopic%3Ageneral&context=thread&threadId=post-thread-open&focusObjectId=reply-1'
    );
  });
  expect(getDetailPane('Thread')).toBeInTheDocument();
  await waitFor(() => {
    expect(getDetailPane('Thread').querySelector('[data-post-object-id="reply-1"]')).toHaveFocus();
  });
});

test('direct message notification click-through opens the messages pane', async () => {
  const user = userEvent.setup();
  const actorPubkey = 'd'.repeat(64);
  renderAtHash(
    '#/notifications?topic=kukuri%3Atopic%3Ageneral',
    createDesktopMockApi({
      notifications: [
        buildNotification({
          notification_id: 'notification-dm-open',
          kind: 'direct_message',
          actor_pubkey: actorPubkey,
          actor_name: 'dan',
          topic_id: null,
          object_id: null,
          thread_root_object_id: null,
          dm_id: 'dm-1',
          message_id: 'message-1',
          preview_text: 'hello from dm notification',
        }),
      ],
      authorSocialViews: {
        [actorPubkey]: {
          name: 'dan',
          mutual: true,
          following: true,
          followed_by: true,
        },
      },
    })
  );

  await screen.findAllByRole('heading', { name: 'Notifications' });
  await user.click(screen.getByText('hello from dm notification'));

  await waitFor(() => {
    expect(window.location.hash).toContain('#/messages?topic=kukuri%3Atopic%3Ageneral&peerPubkey=');
  });
  const conversationColumn = await screen.findByRole('region', { name: /Conversation Column/ });
  await user.click(within(conversationColumn).getByRole('button', { name: /Message to / }));
  expect(within(conversationColumn).getByPlaceholderText('Write a message')).toBeInTheDocument();
});

test('follow notification click-through opens the author pane from timeline', async () => {
  const user = userEvent.setup();
  const actorPubkey = 'e'.repeat(64);
  renderAtHash(
    '#/notifications?topic=kukuri%3Atopic%3Ageneral',
    createDesktopMockApi({
      notifications: [
        buildNotification({
          notification_id: 'notification-follow-open',
          kind: 'followed',
          actor_pubkey: actorPubkey,
          actor_name: 'erin',
          topic_id: null,
          object_id: null,
          thread_root_object_id: null,
          preview_text: null,
        }),
      ],
      authorSocialViews: {
        [actorPubkey]: {
          name: 'erin',
          about: 'opened from follow notification',
        },
      },
    })
  );

  await screen.findAllByRole('heading', { name: 'Notifications' });
  await user.click(screen.getByText('Started following you.'));

  await waitFor(() => {
    expect(window.location.hash).toBe(
      `#/timeline?topic=kukuri%3Atopic%3Ageneral&context=author&authorPubkey=${actorPubkey}`
    );
  });
  expect(getDetailPane('Author')).toBeInTheDocument();
  expect(screen.getByText('opened from follow notification')).toBeInTheDocument();
  const columns = Array.from(document.querySelectorAll<HTMLElement>('.shell-column-surface'));
  const notificationsIndex = columns.findIndex((column) =>
    column.getAttribute('aria-label')?.startsWith('Notifications Column,')
  );
  const openedProfileIndex = columns.findIndex(
    (column) =>
      column.getAttribute('aria-label')?.startsWith('Profile Column,') &&
      column.getAttribute('aria-current') === 'true'
  );
  expect(openedProfileIndex).toBe(notificationsIndex + 1);
});

