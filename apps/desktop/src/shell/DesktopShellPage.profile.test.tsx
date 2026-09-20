import { act, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';

import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import { App } from '@/App';
import type { TimelineView } from '@/lib/api';
import {
  expectActiveTopic,
  createDeferred,
  getActiveColumn,
  getDetailPane,
  getSocialConnectionsTabs,
  openChannelManager,
  openControlCenter,
  openSettingsDrawer,
  openSettingsSection,
  publishPost,
  selectWorkspace,
  setViewportWidth,
} from './DesktopShellPage.testHelpers';

beforeEach(() => {
  setViewportWidth(1024);
  window.history.replaceState(null, '', '/');
});

afterEach(() => {
  vi.useRealTimers();
});

test('publishing refreshes the inactive profile column without changing the active timeline', async () => {
  const user = userEvent.setup();
  const api = createDesktopMockApi();
  const createPost = vi.spyOn(api, 'createPost');
  render(<App api={api} />);
  const profileColumn = await screen.findByRole('region', { name: /^Profile Column,/ });
  const timelineColumn = getActiveColumn('Timeline');

  await publishPost(user, 'inactive profile regression');
  await waitFor(() => expect(createPost).toHaveResolved());
  const profile = await api.getMyProfile();
  const saved = await api.listProfileTimeline(profile.pubkey);
  expect(saved.items.some((post) => post.content === 'inactive profile regression')).toBe(true);
  await waitFor(() => {
    expect(within(profileColumn).getAllByText('inactive profile regression')).toHaveLength(1);
  });
  expect(getActiveColumn('Timeline')).toBe(timelineColumn);
  expect(profileColumn).not.toHaveAttribute('aria-current', 'true');
});

test('an unrequested or loading profile does not report an empty public feed', async () => {
  const api = createDesktopMockApi();
  const pending = createDeferred<TimelineView>();
  vi.spyOn(api, 'listProfileTimeline').mockReturnValue(pending.promise);
  render(<App api={api} />);
  const column = await screen.findByRole('region', { name: /^Profile Column,/ });
  expect(within(column).queryByText('No public posts published yet.')).not.toBeInTheDocument();
  expect(within(column).getByText('Loading profile…')).toBeInTheDocument();
  await act(async () => pending.resolve({ items: [], next_cursor: null }));
  await waitFor(() => expect(within(column).getByText('No public posts published yet.')).toBeInTheDocument());
});

test('a failed profile read offers retry instead of an empty public feed', async () => {
  const user = userEvent.setup();
  const api = createDesktopMockApi();
  const read = vi.spyOn(api, 'listProfileTimeline').mockRejectedValue(new Error('profile unavailable'));
  render(<App api={api} />);
  const column = await screen.findByRole('region', { name: /^Profile Column,/ });
  await waitFor(() => expect(within(column).getByText('profile unavailable')).toBeInTheDocument());
  expect(within(column).queryByText('No public posts published yet.')).not.toBeInTheDocument();
  read.mockResolvedValue({ items: [], next_cursor: null });
  await user.click(within(column).getByRole('button', { name: 'Retry' }));
  await waitFor(() => expect(within(column).getByText('No public posts published yet.')).toBeInTheDocument());
  expect(within(column).queryByText('profile unavailable')).not.toBeInTheDocument();
});

// #1165: 1 本の長い操作列だと、負荷時に full App の再描画が積み重なって timeout するため、
// private channel 投稿の除外と複数 topic の集約を別 test に分けている。
// 入力は paste で渡す(打鍵ごとに App 全体が再描画され、入力操作はこれらの test の検証対象ではない)。
test('profile overview shows public posts and excludes private channel posts', async () => {
  const user = userEvent.setup();
  render(<App api={createDesktopMockApi()} />);

  await publishPost(user, 'demo public post', { input: 'paste' });
  await waitFor(() => {
    expect(within(getActiveColumn('Timeline')).getByText('demo public post')).toBeInTheDocument();
  });

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
    expect(
      screen.queryByRole('dialog', { name: 'Create / Join Private Channel' })
    ).not.toBeInTheDocument();
  });
  await publishPost(user, 'demo private post', { input: 'paste' });
  await waitFor(() => {
    expect(screen.getByText('demo private post')).toBeInTheDocument();
  });

  await selectWorkspace(user, 'Profile');
  const profileColumn = getActiveColumn('Profile');
  expect(within(profileColumn).getByText('demo public post')).toBeInTheDocument();
  expect(within(profileColumn).queryByText('demo private post')).not.toBeInTheDocument();
  expect(screen.getAllByText('general').length).toBeGreaterThan(0);
  expect(within(profileColumn).getAllByRole('button', { name: 'Open original topic' }).length).toBe(1);
});

test('profile overview aggregates public posts across topics', async () => {
  const user = userEvent.setup();
  render(<App api={createDesktopMockApi()} />);

  await publishPost(user, 'demo public post', { input: 'paste' });
  await waitFor(() => {
    expect(within(getActiveColumn('Timeline')).getByText('demo public post')).toBeInTheDocument();
  });

  const controlCenter = await openControlCenter(user);
  await user.click(within(controlCenter).getByPlaceholderText('general'));
  await user.paste('kukuri:topic:second');
  await user.click(within(controlCenter).getByRole('button', { name: 'Add Topic' }));
  await waitFor(() => {
    expectActiveTopic('kukuri:topic:second');
  });

  await publishPost(user, 'second public post', { input: 'paste' });
  await waitFor(() => {
    expect(within(getActiveColumn('Timeline')).getByText('second public post')).toBeInTheDocument();
  });

  await selectWorkspace(user, 'Profile');
  const profileColumn = getActiveColumn('Profile');
  expect(within(profileColumn).getByText('demo public post')).toBeInTheDocument();
  expect(within(profileColumn).getByText('second public post')).toBeInTheDocument();
  expect(within(profileColumn).queryByRole('button', { name: 'Reply' })).not.toBeInTheDocument();
  expect(within(profileColumn).getAllByRole('button', { name: 'Open original topic' }).length).toBe(2);
});

test('profile overview connection count buttons open the requested connections tab', async () => {
  const followedPubkey = 'b'.repeat(64);
  const mutedPubkey = 'c'.repeat(64);
  const blockedPubkey = 'd'.repeat(64);
  const user = userEvent.setup();

  render(
    <App
      api={createDesktopMockApi({
        authorSocialViews: {
          [followedPubkey]: {
            name: 'bob',
            followed_by: true,
          },
          [mutedPubkey]: {
            name: 'carol',
            muted: true,
          },
          [blockedPubkey]: {
            name: 'dave',
            blocking: true,
          },
        },
      })}
    />
  );

  await selectWorkspace(user, 'Profile');
  await user.click(screen.getByRole('button', { name: '1 follower' }));

  await waitFor(() => {
    expect(within(getSocialConnectionsTabs()).getByRole('tab', { name: 'Followers' })).toHaveAttribute(
      'aria-selected',
      'true'
    );
  });
  expect(
    screen.queryByText('Followed shows only followers already observed on this device.')
  ).not.toBeInTheDocument();

  await user.click(screen.getByRole('button', { name: 'Back to profile' }));
  await user.click(screen.getByRole('button', { name: '1 muted user' }));

  await waitFor(() => {
    expect(within(getSocialConnectionsTabs()).getByRole('tab', { name: 'Muted' })).toHaveAttribute(
      'aria-selected',
      'true'
    );
  });

  // #961: ブロック中の件数導線と、一覧行からの解除。
  await user.click(screen.getByRole('button', { name: 'Back to profile' }));
  await user.click(screen.getByRole('button', { name: '1 blocked user' }));
  await waitFor(() => {
    expect(within(getSocialConnectionsTabs()).getByRole('tab', { name: 'Blocked' })).toHaveAttribute(
      'aria-selected',
      'true'
    );
  });
  expect(window.location.hash).toContain('connectionsView=blocking');
  const blockedColumn = getActiveColumn('Profile');
  expect(within(blockedColumn).getByText('dave', { selector: '.post-title' })).toBeInTheDocument();
  await user.click(within(blockedColumn).getByRole('button', { name: 'Unblock' }));
  await waitFor(() => {
    expect(within(blockedColumn).getByText('No blocked users yet.')).toBeInTheDocument();
  });
  await user.click(screen.getByRole('button', { name: 'Back to profile' }));
  expect(screen.getByRole('button', { name: '0 blocked users' })).toBeInTheDocument();
});

test('blocking from the muted list moves the user into the blocked list', async () => {
  const mutedPubkey = 'c'.repeat(64);
  const user = userEvent.setup();
  const api = createDesktopMockApi({
    authorSocialViews: {
      [mutedPubkey]: { name: 'carol', muted: true },
    },
  });
  const blockAuthor = vi.spyOn(api, 'blockAuthor');
  render(<App api={api} />);

  await selectWorkspace(user, 'Profile');
  await user.click(screen.getByRole('button', { name: '1 muted user' }));
  const column = getActiveColumn('Profile');
  await user.click(within(column).getByRole('button', { name: 'Actions for carol' }));
  await user.click(screen.getByRole('menuitem', { name: 'Block' }));
  await waitFor(() => expect(blockAuthor).toHaveBeenCalledWith(mutedPubkey));
  await waitFor(() => {
    expect(within(column).getByText('Blocked', { selector: '.relationship-badge' })).toBeInTheDocument();
  });
  await user.click(within(column).getByRole('tab', { name: 'Blocked' }));
  await waitFor(() => {
    expect(within(column).getByText('carol', { selector: '.post-title' })).toBeInTheDocument();
  });
  expect(within(column).getByText('Muted', { selector: '.relationship-badge' })).toBeInTheDocument();
});

test('safety settings open the blocked users list and close the drawer', async () => {
  const blockedPubkey = 'd'.repeat(64);
  const user = userEvent.setup();
  render(
    <App
      api={createDesktopMockApi({
        authorSocialViews: {
          [blockedPubkey]: { name: 'dave', blocking: true },
        },
      })}
    />
  );

  const drawer = await openSettingsSection(user, 'safety');
  await user.click(within(drawer).getByRole('button', { name: 'Open blocked users' }));
  await waitFor(() => {
    expect(screen.queryByRole('dialog', { name: 'Settings' })).not.toBeInTheDocument();
  });
  await waitFor(() => {
    expect(within(getSocialConnectionsTabs()).getByRole('tab', { name: 'Blocked' })).toHaveAttribute(
      'aria-selected',
      'true'
    );
  });
  expect(
    within(getActiveColumn('Profile')).getByText('dave', { selector: '.post-title' })
  ).toBeInTheDocument();
});

test('author detail shows profile topic posts and can open an untracked origin topic', async () => {
  const authorPubkey = 'b'.repeat(64);
  const user = userEvent.setup();

  render(
    <App
      api={createDesktopMockApi({
        seedPosts: {
          'kukuri:topic:general': [
            {
              object_id: 'post-author-demo',
              envelope_id: 'envelope-author-demo',
              author_pubkey: authorPubkey,
              author_name: 'bob',
              author_display_name: null,
              following: false,
              followed_by: false,
              mutual: false,
              friend_of_friend: false,
              object_kind: 'post',
              content: 'post from demo topic',
              content_status: 'Available',
              attachments: [],
              created_at: 1,
              reply_to: null,
              root_id: 'post-author-demo',
              audience_label: 'Public',
            },
          ],
          'kukuri:topic:relay': [
            {
              object_id: 'post-author-relay',
              envelope_id: 'envelope-author-relay',
              author_pubkey: authorPubkey,
              author_name: 'bob',
              author_display_name: null,
              following: false,
              followed_by: false,
              mutual: false,
              friend_of_friend: false,
              object_kind: 'post',
              content: 'post from relay topic',
              content_status: 'Available',
              attachments: [],
              created_at: 2,
              reply_to: null,
              root_id: 'post-author-relay',
              audience_label: 'Public',
            },
          ],
        },
        authorSocialViews: {
          [authorPubkey]: {
            name: 'bob',
            about: 'author detail profile feed',
          },
        },
      })}
    />
  );

  await user.click(await screen.findByRole('button', { name: 'bob' }));

  await waitFor(() => expect(getDetailPane('Author')).toBeInTheDocument());
  const authorPane = getDetailPane('Author');
  expect(within(authorPane).getByText('post from demo topic')).toBeInTheDocument();
  expect(within(authorPane).getByText('post from relay topic')).toBeInTheDocument();
  expect(within(authorPane).getByText('relay')).toBeInTheDocument();
  expect(within(authorPane).queryByRole('button', { name: 'Reply' })).not.toBeInTheDocument();

  await user.click(within(authorPane).getAllByRole('button', { name: 'Open original topic' })[0]);

  await waitFor(() => {
    expectActiveTopic('kukuri:topic:relay');
    expect(getActiveColumn('Timeline')).toBeInTheDocument();
  });
  expect(within(getActiveColumn('Timeline')).getByText('post from relay topic')).toBeInTheDocument();
  const controlCenter = await openControlCenter(user);
  expect(within(controlCenter).getByRole('button', { name: 'relay' })).toBeInTheDocument();
});

test('local profile editor saves profile draft from primary navigation and settings stays diagnostics-only', async () => {
  const api = createDesktopMockApi();
  const user = userEvent.setup();

  render(<App api={api} />);

  await selectWorkspace(user, 'Profile');
  await user.click(screen.getByRole('button', { name: 'Edit Profile' }));
  const profileSection = screen.getByPlaceholderText('Visible label').closest('.shell-section');
  if (!(profileSection instanceof HTMLElement)) {
    throw new Error('profile section not found');
  }

  const displayNameInput = within(profileSection).getByPlaceholderText('Visible label');
  await user.type(displayNameInput, 'Local Author');
  await user.click(within(profileSection).getByRole('button', { name: 'Save Profile' }));

  // 保存後は loadTopics / refreshProfile の完了を待って hash の profileMode=edit が消え、
  // それまで route 同期が edit に戻すので、overview は再取得の連鎖が終わってから現れる。
  // 負荷下ではこの連鎖が既定の 1 秒を超えるため、待ちの上限を明示する（#1167）。
  await waitFor(
    () => {
      expect(screen.getByText('Local Author')).toBeInTheDocument();
      expect(screen.getByRole('button', { name: 'Edit Profile' })).toBeInTheDocument();
      expect(window.location.hash).toBe('#/profile?topic=kukuri%3Atopic%3Ageneral');
    },
    { timeout: 10_000 }
  );

  const drawer = await openSettingsDrawer(user);
  expect(within(drawer).queryByTestId('settings-section-profile')).not.toBeInTheDocument();
});

test('keeps local peer ticket visible when profile loading fails', async () => {
  const user = userEvent.setup();
  render(<App api={createDesktopMockApi({ myProfileError: 'profile load failed' })} />);

  const drawer = await openSettingsSection(user, 'connectivity');
  await waitFor(() => {
    expect(within(drawer).getByDisplayValue('peer1@127.0.0.1:7777')).toBeInTheDocument();
  });
});
