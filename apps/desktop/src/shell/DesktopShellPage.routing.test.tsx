import { screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';

import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import {
  expectActiveTopic,
  getActiveColumn,
  getDetailPane,
  getTimelineViewTabs,
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

test.each([
  {
    path: '#/timeline',
    workspaceLabel: 'Timeline',
    expectedControl: () => screen.getByRole('button', { name: /^Post to / }),
  },
  {
    path: '#/channels',
    workspaceLabel: 'Timeline',
    expectedControl: () => screen.getByRole('button', { name: /^Post to / }),
  },
  {
    path: '#/live',
    workspaceLabel: 'Live',
    expectedControl: () => screen.getByText('Choose a live session from a post or public thread to show large'),
  },
  {
    path: '#/game',
    workspaceLabel: 'Metaverse',
    expectedControl: () => screen.getByRole('button', { name: 'Create metaverse room' }),
  },
  {
    path: '#/messages',
    workspaceLabel: 'Messages',
    expectedControl: () => screen.getByText('No direct messages yet.'),
  },
  {
    path: '#/profile',
    workspaceLabel: 'Profile',
    expectedControl: () => screen.getByRole('button', { name: 'Edit Profile' }),
  },
])(
  'primary hash route $path selects the correct section',
  async ({ path, workspaceLabel, expectedControl }) => {
    renderAtHash(path);

    await waitFor(() => expect(expectedControl()).toBeInTheDocument());

    await waitFor(() => {
      expect(getActiveColumn(workspaceLabel)).toHaveAttribute('aria-current', 'true');
      expect(window.location.hash).toBe(
        path === '#/channels'
          ? '#/timeline?topic=kukuri%3Atopic%3Ageneral'
          : `${path}?topic=kukuri%3Atopic%3Ageneral`
      );
    });
  }
);

test('invalid hash routes fall back to the active public timeline and normalize the URL', async () => {
  renderAtHash(
    '#/unknown?topic=missing-topic&timelineScope=channel:missing&composeTarget=channel:missing&context=author&authorPubkey=bad&settings=invalid'
  );

  await waitFor(() => {
    expect(window.location.hash).toBe('#/timeline?topic=kukuri%3Atopic%3Ageneral');
  });
  expectActiveTopic('kukuri:topic:general');
  expect(screen.queryByRole('dialog', { name: 'Settings' })).not.toBeInTheDocument();
});

test('invalid timelineView normalizes to the feed route', async () => {
  renderAtHash('#/timeline?topic=kukuri%3Atopic%3Ageneral&timelineView=invalid');

  await waitFor(() => {
    expect(window.location.hash).toBe('#/timeline?topic=kukuri%3Atopic%3Ageneral');
  });
  expect(within(getTimelineViewTabs()).getByRole('tab', { name: 'Feed' })).toHaveAttribute(
    'aria-selected',
    'true'
  );
});

test('thread context restores from the hash route and loads the requested thread for the active topic', async () => {
  renderAtHash(
    '#/timeline?topic=kukuri%3Atopic%3Ageneral&context=thread&threadId=post-thread-open',
    createDesktopMockApi({
      seedPosts: {
        'kukuri:topic:general': [
          {
            object_id: 'post-thread-open',
            envelope_id: 'envelope-thread-open',
            author_pubkey: 'b'.repeat(64),
            author_name: 'bob',
            author_display_name: null,
            following: false,
            followed_by: true,
            mutual: false,
            friend_of_friend: false,
            object_kind: 'post',
            content: 'open thread from timeline',
            content_status: 'Available',
            attachments: [],
            created_at: 1,
            reply_to: null,
            root_id: 'post-thread-open',
            channel_id: null,
            audience_label: 'Public',
          },
        ],
      },
    })
  );

  await waitFor(() => {
    expect(getDetailPane('Thread')).toBeInTheDocument();
  });
  expect(within(getDetailPane('Thread')).getAllByText('open thread from timeline').length).toBeGreaterThan(0);
});

test('author context restores from the hash route when a valid author pubkey is supplied', async () => {
  const authorPubkey = 'b'.repeat(64);
  renderAtHash(
    `#/timeline?topic=kukuri%3Atopic%3Ageneral&context=author&authorPubkey=${authorPubkey}`,
    createDesktopMockApi({
      authorSocialViews: {
        [authorPubkey]: {
          name: 'bob',
          display_name: null,
          about: 'author detail from route restore',
          following: false,
          followed_by: true,
          mutual: false,
          friend_of_friend: false,
          friend_of_friend_via_pubkeys: [],
        },
      },
    })
  );

  await waitFor(() => {
    expect(getDetailPane('Author')).toBeInTheDocument();
  });
  expect(within(getDetailPane('Author')).getByText('author detail from route restore')).toBeInTheDocument();
});

test('profile edit route restores the editor and keeps overview as the default profile mode', async () => {
  renderAtHash('#/profile?topic=kukuri%3Atopic%3Ageneral&profileMode=edit');

  expect(screen.getByPlaceholderText('Visible label')).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Back to profile' })).toBeInTheDocument();
});

test('profile connections route restores the requested view', async () => {
  const authorPubkey = 'b'.repeat(64);
  renderAtHash(
    '#/profile?topic=kukuri%3Atopic%3Ageneral&profileMode=connections&connectionsView=muted',
    createDesktopMockApi({
      authorSocialViews: {
        [authorPubkey]: {
          name: 'bob',
          muted: true,
        },
      },
    })
  );

  const tabs = await screen.findByRole('tablist', { name: 'Social connections' });
  await waitFor(() => {
    expect(within(tabs).getByRole('tab', { name: 'Muted' })).toHaveAttribute(
      'aria-selected',
      'true'
    );
    expect(window.location.hash).toBe(
      '#/profile?topic=kukuri%3Atopic%3Ageneral&profileMode=connections&connectionsView=muted'
    );
  });
  expect(
    screen.queryByText('Muted is local-only and is not shared with other devices.')
  ).not.toBeInTheDocument();
  expect(screen.getByTestId('profile-connection-identifier-target')).toBeInTheDocument();
  expect(screen.queryByText(authorPubkey)).not.toBeInTheDocument();
});

test('invalid profile connections view normalizes to following', async () => {
  const authorPubkey = 'b'.repeat(64);
  renderAtHash(
    '#/profile?topic=kukuri%3Atopic%3Ageneral&profileMode=connections&connectionsView=invalid',
    createDesktopMockApi({
      authorSocialViews: {
        [authorPubkey]: {
          name: 'bob',
          following: true,
        },
      },
    })
  );

  const tabs = await screen.findByRole('tablist', { name: 'Social connections' });
  await waitFor(() => {
    expect(within(tabs).getByRole('tab', { name: 'Following' })).toHaveAttribute(
      'aria-selected',
      'true'
    );
    expect(window.location.hash).toBe(
      '#/profile?topic=kukuri%3Atopic%3Ageneral&profileMode=connections&connectionsView=following'
    );
  });
  expect(screen.getByTestId('profile-connection-identifier-target')).toBeInTheDocument();
  expect(screen.queryByText(authorPubkey)).not.toBeInTheDocument();
});

test('invalid nested author route keeps the thread pane and normalizes only the author param', async () => {
  renderAtHash(
    '#/timeline?topic=kukuri%3Atopic%3Ageneral&context=thread&threadId=post-thread-open&authorPubkey=bad',
    createDesktopMockApi({
      seedPosts: {
        'kukuri:topic:general': [
          {
            object_id: 'post-thread-open',
            envelope_id: 'envelope-thread-open',
            author_pubkey: 'b'.repeat(64),
            author_name: 'bob',
            author_display_name: null,
            following: false,
            followed_by: true,
            mutual: false,
            friend_of_friend: false,
            object_kind: 'post',
            content: 'open thread from timeline',
            content_status: 'Available',
            attachments: [],
            created_at: 1,
            reply_to: null,
            root_id: 'post-thread-open',
            channel_id: null,
            audience_label: 'Public',
          },
        ],
      },
    })
  );

  await waitFor(() => {
    expect(getDetailPane('Thread')).toBeInTheDocument();
    expect(window.location.hash).toBe(
      '#/timeline?topic=kukuri%3Atopic%3Ageneral&context=thread&threadId=post-thread-open'
    );
  });
  expect(screen.queryByRole('complementary', { name: 'Author' })).not.toBeInTheDocument();
});

test('invalid thread route closes the entire detail stack and normalizes the URL', async () => {
  renderAtHash(
    `#/timeline?topic=kukuri%3Atopic%3Ageneral&context=thread&threadId=missing-thread&authorPubkey=${'b'.repeat(64)}`,
    createDesktopMockApi({
      authorSocialViews: {
        ['b'.repeat(64)]: {
          name: 'bob',
        },
      },
    })
  );

  await waitFor(() => {
    expect(window.location.hash).toBe('#/timeline?topic=kukuri%3Atopic%3Ageneral');
  });
  expect(screen.queryByRole('complementary', { name: 'Thread' })).not.toBeInTheDocument();
  expect(screen.queryByRole('complementary', { name: 'Author' })).not.toBeInTheDocument();
});

