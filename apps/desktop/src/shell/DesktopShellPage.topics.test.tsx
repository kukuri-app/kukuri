import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';

import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import { App } from '@/App';
import {
  expectActiveTopic,
  getActiveColumn,
  openChannelManager,
  openControlCenter,
  publishPost,
  renderAtHash,
  selectWorkspace,
  selectTimelineView,
  setViewportWidth,
} from './DesktopShellPage.testHelpers';

async function getTopicItem(user: ReturnType<typeof userEvent.setup>, label: string) {
  const controlCenter = await openControlCenter(user);
  const item = within(controlCenter).getByRole('button', { name: label }).closest('li');
  if (!(item instanceof HTMLElement)) throw new Error(`${label} topic item not found`);
  return item;
}

async function selectTopic(user: ReturnType<typeof userEvent.setup>, label: string) {
  const item = await getTopicItem(user, label);
  await user.click(within(item).getByRole('button', { name: label }));
}

async function addTopic(user: ReturnType<typeof userEvent.setup>, topic: string) {
  const controlCenter = await openControlCenter(user);
  await user.clear(within(controlCenter).getByPlaceholderText('general'));
  await user.type(within(controlCenter).getByPlaceholderText('general'), topic);
  await user.click(within(controlCenter).getByRole('button', { name: 'Add Topic' }));
}

beforeEach(() => {
  setViewportWidth(1024);
  window.history.replaceState(null, '', '/');
});

afterEach(() => {
  vi.useRealTimers();
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

test('bookmarking from the timeline syncs with the bookmark page and remove updates both views', async () => {
  const user = userEvent.setup();
  render(
    <App
      api={createDesktopMockApi({
        seedPosts: {
          'kukuri:topic:general': [
            {
              object_id: 'bookmark-me',
              envelope_id: 'envelope-bookmark-me',
              author_pubkey: 'a'.repeat(64),
              author_name: 'alice',
              author_display_name: null,
              following: false,
              followed_by: false,
              mutual: false,
              friend_of_friend: false,
              object_kind: 'post',
              content: 'save this post',
              content_status: 'Available',
              attachments: [],
              created_at: 1,
              reply_to: null,
              root_id: 'bookmark-me',
              audience_label: 'Public',
            },
          ],
        },
      })}
    />
  );

  const timelinePost = await screen.findByText('save this post');
  const timelineCard = timelinePost.closest('article');
  if (!(timelineCard instanceof HTMLElement)) {
    throw new Error('timeline card not found');
  }

  await user.click(within(timelineCard).getByRole('button', { name: 'Bookmark' }));
  await waitFor(() => {
    expect(within(timelineCard).getByRole('button', { name: 'Remove bookmark' })).toBeInTheDocument();
  });

  await selectTimelineView(user, 'Bookmarks');
  await waitFor(() => {
    expect(window.location.hash).toBe('#/timeline?topic=kukuri%3Atopic%3Ageneral&timelineView=bookmarks');
  });
  expect(await screen.findByText('save this post')).toBeInTheDocument();

  const bookmarkedCard = screen.getByText('save this post').closest('article');
  if (!(bookmarkedCard instanceof HTMLElement)) {
    throw new Error('bookmarked card not found');
  }
  await user.click(within(bookmarkedCard).getByRole('button', { name: 'Remove bookmark' }));

  await waitFor(() => {
    expect(screen.getByText('No bookmarked posts yet.')).toBeInTheDocument();
  });

  await selectTimelineView(user, 'Feed');
  await waitFor(() => {
    expect(window.location.hash).toBe('#/timeline?topic=kukuri%3Atopic%3Ageneral');
  });
  const restoredTimelinePost = await screen.findByText('save this post');
  const restoredTimelineCard = restoredTimelinePost.closest('article');
  if (!(restoredTimelineCard instanceof HTMLElement)) {
    throw new Error('restored timeline card not found');
  }
  expect(within(restoredTimelineCard).getByRole('button', { name: 'Bookmark' })).toBeInTheDocument();
});

test('topic and private channel selection sync into the hash route', async () => {
  const user = userEvent.setup();
  render(<App api={createDesktopMockApi()} />);

  await addTopic(user, 'kukuri:topic:second');

  await waitFor(() => {
    expect(window.location.hash).toBe('#/timeline?topic=kukuri%3Atopic%3Asecond');
  });

  await selectTopic(user, 'general');
  const channelDialog = await openChannelManager(user);
  await user.type(within(channelDialog).getByPlaceholderText('Channel name'), 'core');
  await user.click(within(channelDialog).getByRole('button', { name: 'Create Channel' }));

  await waitFor(() => {
    expect(window.location.hash).toBe(
      '#/timeline?topic=kukuri%3Atopic%3Ageneral&channel=channel-1'
    );
  });
});

test('Timeline header replaces the active Column topic without adding a Column and keeps scoped drafts', async () => {
  const user = userEvent.setup();
  renderAtHash('#/timeline?topic=kukuri%3Atopic%3Ageneral');

  const timeline = await screen.findByRole('region', { name: /^Timeline Column,/ });
  const columnId = timeline.dataset.columnId;
  const topicSelect = within(timeline).getByRole('combobox', { name: 'Timeline topic' });

  await user.click(within(timeline).getByRole('button', { name: /^Post to / }));
  const composer = within(timeline).getByPlaceholderText('Write a post');
  await user.type(composer, 'general draft');

  await user.selectOptions(topicSelect, 'kukuri:topic:dev');
  await waitFor(() => {
    expect(window.location.hash).toBe('#/timeline?topic=kukuri%3Atopic%3Adev');
    expect(topicSelect).toHaveValue('kukuri:topic:dev');
  });
  expect(screen.getAllByRole('region', { name: /^Timeline Column,/ })).toHaveLength(1);
  expect(timeline.dataset.columnId).toBe(columnId);
  await user.click(within(timeline).getByRole('button', { name: /^Post to Public · dev$/ }));
  expect(within(timeline).getByPlaceholderText('Write a post')).toHaveValue('');

  await user.type(within(timeline).getByPlaceholderText('Write a post'), 'dev draft');
  await user.selectOptions(topicSelect, 'kukuri:topic:general');
  await waitFor(() => {
    expect(window.location.hash).toBe('#/timeline?topic=kukuri%3Atopic%3Ageneral');
  });
  expect(within(timeline).getByPlaceholderText('Write a post')).toHaveValue('general draft');
});

// Issue #1053: 非 active Column の header 操作はその Column を active にし、route も切替後の topic へ移す。
test('Timeline header switches an inactive Column and focuses it with the new topic route', async () => {
  const user = userEvent.setup();
  renderAtHash('#/timeline?topic=kukuri%3Atopic%3Ageneral');

  await selectWorkspace(user, 'Profile');
  await waitFor(() => {
    expect(window.location.hash).toBe('#/profile?topic=kukuri%3Atopic%3Ageneral');
  });
  const timeline = screen.getByRole('region', { name: /^Timeline Column,/ });
  const topicSelect = within(timeline).getByRole('combobox', { name: 'Timeline topic' });

  await user.selectOptions(topicSelect, 'kukuri:topic:dev');
  await waitFor(() => {
    expect(topicSelect).toHaveValue('kukuri:topic:dev');
    expect(window.location.hash).toBe('#/timeline?topic=kukuri%3Atopic%3Adev');
    expect(timeline).toHaveAttribute('aria-current', 'true');
  });
  expect(screen.getByRole('region', { name: /^Profile Column,/ })).not.toHaveAttribute(
    'aria-current',
    'true'
  );
  expect(screen.getAllByRole('region', { name: /^Timeline Column,/ })).toHaveLength(1);
});

test('tracked topics show public and channel scope separately in the sidebar', async () => {
  const user = userEvent.setup();
  render(<App api={createDesktopMockApi()} />);

  const channelDialog = await openChannelManager(user);
  await user.type(within(channelDialog).getByPlaceholderText('Channel name'), 'core');
  await user.click(within(channelDialog).getByRole('button', { name: 'Create Channel' }));
  await waitFor(() => {
    expect(window.location.hash).toMatch(
      /^#\/timeline\?topic=kukuri%3Atopic%3Ageneral&channel=channel-\d+$/
    );
  });
  await user.click(within(channelDialog).getByRole('button', { name: 'Close dialog' }));
  await waitFor(() => {
    expect(screen.queryByRole('dialog', { name: 'Create / Join Private Channel' })).not.toBeInTheDocument();
  });

  const topicItem = await getTopicItem(user, 'general');
  expect(within(topicItem).getByRole('button', { name: 'Open core channel settings' })).toBeInTheDocument();

  expect(within(topicItem).getByText('Channels')).toBeInTheDocument();
  const publicButton = within(topicItem).getByText('Public').closest('button');
  const channelButton = within(topicItem).getByText('core').closest('button');
  if (!(publicButton instanceof HTMLButtonElement)) {
    throw new Error('public scope button not found');
  }
  if (!(channelButton instanceof HTMLButtonElement)) {
    throw new Error('channel scope button not found');
  }

  await waitFor(() => {
    expect(publicButton).toHaveAttribute('aria-pressed', 'false');
    expect(channelButton).toHaveAttribute('aria-pressed', 'true');
  });

  await user.click(publicButton);

  await waitFor(() => {
    expect(window.location.hash).toBe('#/timeline?topic=kukuri%3Atopic%3Ageneral');
  });
  const publicTopicItem = await getTopicItem(user, 'general');
  expect(within(publicTopicItem).getByText('Public').closest('button')).toHaveAttribute('aria-pressed', 'true');
});

test('sidebar can reselect the same private channel after switching back to public', async () => {
  const user = userEvent.setup();
  render(<App api={createDesktopMockApi()} />);

  const channelDialog = await openChannelManager(user);
  await user.type(within(channelDialog).getByPlaceholderText('Channel name'), 'core');
  await user.click(within(channelDialog).getByRole('button', { name: 'Create Channel' }));
  await waitFor(() => {
    expect(window.location.hash).toMatch(
      /^#\/timeline\?topic=kukuri%3Atopic%3Ageneral&channel=channel-\d+$/
    );
  });
  await user.click(within(channelDialog).getByRole('button', { name: 'Close dialog' }));

  const topicItem = await getTopicItem(user, 'general');

  const publicButton = within(topicItem).getByText('Public').closest('button');
  const channelButton = within(topicItem).getByText('core').closest('button');
  if (!(publicButton instanceof HTMLButtonElement) || !(channelButton instanceof HTMLButtonElement)) {
    throw new Error('scope buttons not found');
  }

  await user.click(publicButton);
  await waitFor(() => {
    expect(window.location.hash).toBe('#/timeline?topic=kukuri%3Atopic%3Ageneral');
  });

  const refreshedTopicItem = await getTopicItem(user, 'general');
  const refreshedChannelButton = within(refreshedTopicItem).getByText('core').closest('button');
  if (!(refreshedChannelButton instanceof HTMLButtonElement)) throw new Error('channel button not found');
  await user.click(refreshedChannelButton);
  await waitFor(() => {
    expect(window.location.hash).toBe('#/timeline?topic=kukuri%3Atopic%3Ageneral&channel=channel-1');
  });
});

test('sidebar can switch from one topic public scope to another topic private channel scope', async () => {
  const user = userEvent.setup();
  render(<App api={createDesktopMockApi()} />);

  await addTopic(user, 'kukuri:topic:second');
  await selectTopic(user, 'second');
  await waitFor(() => {
    expectActiveTopic('kukuri:topic:second');
  });

  const channelDialog = await openChannelManager(user);
  await user.type(within(channelDialog).getByPlaceholderText('Channel name'), 'second-core');
  await user.click(within(channelDialog).getByRole('button', { name: 'Create Channel' }));
  await waitFor(() => {
    expect(window.location.hash).toBe('#/timeline?topic=kukuri%3Atopic%3Asecond&channel=channel-1');
  });
  await user.click(within(channelDialog).getByRole('button', { name: 'Close dialog' }));

  await selectTopic(user, 'general');
  await waitFor(() => {
    expectActiveTopic('kukuri:topic:general');
    expect(window.location.hash).toBe('#/timeline?topic=kukuri%3Atopic%3Ageneral');
  });

  const secondTopicItem = await getTopicItem(user, 'second');

  const secondChannelButton = within(secondTopicItem).getByText('second-core').closest('button');
  if (!(secondChannelButton instanceof HTMLButtonElement)) {
    throw new Error('second topic channel button not found');
  }

  await user.click(secondChannelButton);
  await waitFor(() => {
    expectActiveTopic('kukuri:topic:second');
    expect(window.location.hash).toBe('#/timeline?topic=kukuri%3Atopic%3Asecond&channel=channel-1');
  });
});

test('desktop shell can track multiple topics at once', async () => {
  const user = userEvent.setup();
  render(<App api={createDesktopMockApi()} />);

  await addTopic(user, 'kukuri:topic:second');
  expect(await getTopicItem(user, 'second')).toBeInTheDocument();

  await selectTopic(user, 'general');
  await publishPost(user, 'demo post');
  await waitFor(() => {
    expect(within(getActiveColumn('Timeline')).getByText('demo post')).toBeInTheDocument();
  });

  await selectTopic(user, 'second');
  await publishPost(user, 'second post');
  await waitFor(() => {
    expect(within(getActiveColumn('Timeline')).getByText('second post')).toBeInTheDocument();
  });

  await selectTopic(user, 'general');
  const generalTopic = await getTopicItem(user, 'general');
  expect(generalTopic).not.toBeNull();
  expect(within(getActiveColumn('Timeline')).getByText('demo post')).toBeInTheDocument();
  expect(generalTopic).toHaveTextContent(/\/ peers: \d/);
  expect(generalTopic).not.toHaveTextContent('expected:');
  expect(generalTopic).not.toHaveTextContent('Connected to all configured peers for this topic');
});

test('removing the active topic falls back to the remaining tracked topic', async () => {
  const user = userEvent.setup();
  render(<App api={createDesktopMockApi()} />);

  await addTopic(user, 'kukuri:topic:second');

  await waitFor(() => {
    expectActiveTopic('kukuri:topic:second');
  });

  const secondTopic = await getTopicItem(user, 'second');
  await user.click(within(secondTopic).getByRole('button', { name: 'Remove second' }));

  await waitFor(() => {
    expectActiveTopic('kukuri:topic:general');
  });
  const controlCenter = await openControlCenter(user);
  expect(within(controlCenter).queryByRole('button', { name: 'second' })).not.toBeInTheDocument();
});

