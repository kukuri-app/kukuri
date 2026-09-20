import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, test } from 'vitest';

import { App } from '@/App';
import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import { resolveHashBackedRouteLocation } from '@/shell/routes';
import { topicDisplayName } from '@/lib/topicId';

beforeEach(() => {
  Object.defineProperty(window, 'innerWidth', {
    configurable: true,
    writable: true,
    value: 1024,
  });
  window.dispatchEvent(new Event('resize'));
  window.history.replaceState(null, '', '/');
});

function renderAtHash(hash: string, api = createDesktopMockApi()) {
  window.history.replaceState(null, '', `/${hash}`);
  return render(<App api={api} />);
}

function expectActiveTopic(topic: string) {
  expect(window.location.hash).toContain(`topic=${encodeURIComponent(topic)}`);
  expect(getActiveColumn('Timeline')).toHaveTextContent(topicDisplayName(topic));
}

function getTimelineViewTabs() {
  return screen.getByRole('tablist', { name: 'Timeline views' });
}

function getDetailPane(name: 'Thread' | 'Author') {
  return screen.queryByRole('complementary', { name }) ?? getActiveColumn(
    name === 'Author' ? 'Profile' : 'Thread'
  );
}

function getActiveColumn(title: string) {
  return screen.getByRole('region', {
    name: new RegExp(`^${title} Column,.*Active,`),
  });
}

async function openChannelManager(user: ReturnType<typeof userEvent.setup>) {
  const controlCenter = await openControlCenter(user);
  await user.click(within(controlCenter).getByRole('button', { name: 'Create or join a private channel' }));
  return await screen.findByRole('dialog', { name: 'Create / Join Private Channel' });
}

async function openControlCenter(user: ReturnType<typeof userEvent.setup>) {
  const existing = screen.queryByRole('complementary', { name: 'Control Center' });
  if (existing) return existing;
  await user.click(await screen.findByTestId('control-center-trigger'));
  return screen.getByRole('complementary', { name: 'Control Center' });
}

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

test('hash-backed route resolution preserves hash query during router hydration gaps', () => {
  window.history.replaceState(
    null,
    '',
    '/#/timeline?topic=kukuri%3Atopic%3Ageneral&settings=appearance'
  );

  expect(resolveHashBackedRouteLocation('/', '', window.location.hash)).toEqual({
    pathname: '/timeline',
    search: '?topic=kukuri%3Atopic%3Ageneral&settings=appearance',
  });
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

test('bookmark page route closes detail context and normalizes timeline-specific params', async () => {
  renderAtHash(
    '#/timeline?topic=kukuri%3Atopic%3Ageneral&timelineView=bookmarks&channel=channel-1&context=thread&threadId=post-thread-open',
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
            followed_by: false,
            mutual: false,
            friend_of_friend: false,
            object_kind: 'post',
            content: 'thread should close',
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
    expect(window.location.hash).toBe('#/timeline?topic=kukuri%3Atopic%3Ageneral&timelineView=bookmarks');
  });
  expect(screen.queryByRole('complementary', { name: 'Thread' })).not.toBeInTheDocument();
  expect(await screen.findByText('No bookmarked posts yet.')).toBeInTheDocument();
});

test('notifications route keeps topic context and strips unrelated nested params', async () => {
  renderAtHash(
    '#/notifications?topic=kukuri%3Atopic%3Ageneral&channel=channel-1&timelineView=bookmarks&context=thread&threadId=post-thread-open&authorPubkey=bad&peerPubkey=bad'
  );

  await waitFor(() => {
    expect(window.location.hash).toBe('#/notifications?topic=kukuri%3Atopic%3Ageneral');
  });
  expect(within(getActiveColumn('Notifications')).getAllByRole('heading', { name: 'Notifications' })[0]).toBeInTheDocument();
  expect(screen.queryByRole('complementary', { name: 'Thread' })).not.toBeInTheDocument();
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

test('thread focusObjectId restores and highlights the requested post', async () => {
  renderAtHash(
    '#/timeline?topic=kukuri%3Atopic%3Ageneral&context=thread&threadId=post-thread-open&focusObjectId=post-thread-reply',
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
          {
            object_id: 'post-thread-reply',
            envelope_id: 'envelope-thread-reply',
            author_pubkey: 'c'.repeat(64),
            author_name: 'carol',
            author_display_name: null,
            following: false,
            followed_by: false,
            mutual: false,
            friend_of_friend: false,
            object_kind: 'post',
            content: 'reply target',
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

  await waitFor(() => {
    expect(window.location.hash).toContain('focusObjectId=post-thread-reply');
    expect(within(getDetailPane('Thread')).getByText('reply target').closest('article')).toHaveClass(
      'post-card-targeted'
    );
  });
});

test('invalid thread focusObjectId normalizes only the target param', async () => {
  renderAtHash(
    '#/timeline?topic=kukuri%3Atopic%3Ageneral&context=thread&threadId=post-thread-open&focusObjectId=missing-post',
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
    expect(window.location.hash).toBe(
      '#/timeline?topic=kukuri%3Atopic%3Ageneral&context=thread&threadId=post-thread-open'
    );
  });
  expect(getDetailPane('Thread')).toBeInTheDocument();
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

test('live session route restores and normalizes invalid session targets without leaving live', async () => {
  const firstRender = renderAtHash(
    '#/live?topic=kukuri%3Atopic%3Ageneral&sessionId=session-demo',
    createDesktopMockApi({
      seedLiveSessions: {
        'kukuri:topic:general': [
          {
            session_id: 'session-demo',
            host_pubkey: 'b'.repeat(64),
            title: 'Live Demo',
            description: 'watch here',
            status: 'Live',
            started_at: 1,
            viewer_count: 4,
            joined_by_me: false,
            channel_id: null,
            audience_label: 'Public',
          },
        ],
      },
    })
  );

  await waitFor(() => {
    expect(window.location.hash).toBe('#/live?topic=kukuri%3Atopic%3Ageneral&sessionId=session-demo');
  });
  expect(within(getActiveColumn('Live')).getByText('Live Demo').closest('article')).toHaveClass('post-card-targeted');

  firstRender.unmount();

  renderAtHash(
    '#/live?topic=kukuri%3Atopic%3Ageneral&sessionId=missing-session',
    createDesktopMockApi({
      seedLiveSessions: {
        'kukuri:topic:general': [
          {
            session_id: 'session-demo',
            host_pubkey: 'b'.repeat(64),
            title: 'Live Demo',
            description: 'watch here',
            status: 'Live',
            started_at: 1,
            viewer_count: 4,
            joined_by_me: false,
            channel_id: null,
            audience_label: 'Public',
          },
        ],
      },
    })
  );

  await waitFor(() => {
    expect(window.location.hash).toBe('#/live?topic=kukuri%3Atopic%3Ageneral');
  });
  expect(within(getActiveColumn('Live')).getAllByRole('heading', { name: 'Live Sessions' })[0]).toBeInTheDocument();
});

test('game route restores score rooms in Game Columns and normalizes invalid targets', async () => {
  const firstRender = renderAtHash(
    '#/game?topic=kukuri%3Atopic%3Ageneral&roomId=room-demo',
    createDesktopMockApi({
      seedGameRooms: {
        'kukuri:topic:general': [
          {
            room_id: 'room-demo',
            host_pubkey: 'b'.repeat(64),
            title: 'Room Demo',
            description: 'play here',
            status: 'Waiting',
            phase_label: 'Round 1',
            scores: [],
            updated_at: 1,
            channel_id: null,
            audience_label: 'Public',
          },
        ],
      },
    })
  );

  await waitFor(() => {
    expect(window.location.hash).toBe('#/game?topic=kukuri%3Atopic%3Ageneral&roomId=room-demo');
  });
  expect(within(getActiveColumn('Game')).getAllByRole('heading', { name: 'Game Rooms' })[0]).toBeInTheDocument();
  expect(within(getActiveColumn('Game')).getByText('Room Demo')).toBeInTheDocument();

  firstRender.unmount();

  renderAtHash(
    '#/game?topic=kukuri%3Atopic%3Ageneral&roomId=missing-room',
    createDesktopMockApi({
      seedGameRooms: {
        'kukuri:topic:general': [
          {
            room_id: 'room-demo',
            host_pubkey: 'b'.repeat(64),
            title: 'Room Demo',
            description: 'play here',
            status: 'Waiting',
            phase_label: 'Round 1',
            scores: [],
            updated_at: 1,
            channel_id: null,
            audience_label: 'Public',
          },
        ],
      },
    })
  );

  await waitFor(() => {
    expect(window.location.hash).toBe('#/game?topic=kukuri%3Atopic%3Ageneral');
  });
  expect(within(getActiveColumn('Metaverse')).getByRole('heading', { name: 'Metaverse Rooms' })).toBeInTheDocument();
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
  expect(screen.getByTestId('profile-connection-identifier-target')).toBeInTheDocument();
  expect(screen.queryByText(authorPubkey)).not.toBeInTheDocument();
});

test('settings hash route opens the drawer and keeps the selected section in sync', async () => {
  const user = userEvent.setup();
  renderAtHash('#/timeline?topic=kukuri%3Atopic%3Ageneral&settings=discovery');

  const drawer = await screen.findByRole('dialog', { name: 'Settings' });
  await waitFor(() => {
    expect(within(drawer).getByTestId('settings-section-discovery')).toHaveAttribute(
      'aria-current',
      'location'
    );
  });

  await user.click(within(drawer).getByTestId('settings-section-connectivity'));

  await waitFor(() => {
    expect(window.location.hash).toContain('settings=connectivity');
  });
});

test('topic and private channel selection sync into the hash route', async () => {
  const user = userEvent.setup();
  render(<App api={createDesktopMockApi()} />);

  let controlCenter = await openControlCenter(user);
  await user.type(within(controlCenter).getByPlaceholderText('general'), 'kukuri:topic:second');
  await user.click(within(controlCenter).getByRole('button', { name: 'Add Topic' }));

  await waitFor(() => {
    expect(window.location.hash).toBe('#/timeline?topic=kukuri%3Atopic%3Asecond');
  });

  controlCenter = await openControlCenter(user);
  await user.click(within(controlCenter).getByRole('button', { name: 'general' }));
  const channelDialog = await openChannelManager(user);
  await user.type(within(channelDialog).getByPlaceholderText('Channel name'), 'core');
  await user.click(within(channelDialog).getByRole('button', { name: 'Create Channel' }));

  await waitFor(() => {
    expect(window.location.hash).toBe('#/timeline?topic=kukuri%3Atopic%3Ageneral&channel=channel-1');
  });
}, 10_000);

test('messages hash route restores the direct message and author pane together', async () => {
  const user = userEvent.setup();
  const authorPubkey = 'b'.repeat(64);
  const api = createDesktopMockApi({
    authorSocialViews: {
      [authorPubkey]: {
        name: 'bob',
        following: true,
        followed_by: true,
        mutual: true,
      },
    },
  });

  renderAtHash(
    `#/messages?topic=kukuri%3Atopic%3Ageneral&peerPubkey=${authorPubkey}&authorPubkey=${authorPubkey}`,
    api
  );

  await waitFor(() => {
    expect(screen.getByRole('region', { name: /^Messages Column/ })).toBeInTheDocument();
    expect(getDetailPane('Author')).toBeInTheDocument();
  });
  const conversationColumn = await screen.findByRole('region', { name: /Conversation Column/ });
  await user.click(within(conversationColumn).getByRole('button', { name: /Message to / }));
  expect(within(conversationColumn).getByPlaceholderText('Write a message')).toBeInTheDocument();
  expect(window.location.hash).toBe(
    `#/messages?topic=kukuri%3Atopic%3Ageneral&peerPubkey=${authorPubkey}&authorPubkey=${authorPubkey}`
  );
});
