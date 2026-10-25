import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';

import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import { App } from '@/App';
import {
  getSocialConnectionsTabs,
  getDetailPane,
  selectTimelineView,
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

test('post card shows friend of friend and audience icons and author name fallback', async () => {
  render(
    <App
      api={createDesktopMockApi({
        seedPosts: {
          'kukuri:topic:general': [
            {
              object_id: 'post-fof',
              envelope_id: 'envelope-fof',
              author_pubkey: 'a'.repeat(64),
              author_name: 'alice',
              author_display_name: null,
              following: false,
              followed_by: false,
              mutual: false,
              friend_of_friend: true,
              object_kind: 'post',
              content: 'hello network',
              content_status: 'Available',
              attachments: [],
              created_at: 1,
              reply_to: null,
              root_id: 'post-fof',
              audience_label: 'Public',
            },
          ],
        },
      })}
    />
  );

  expect(await screen.findByRole('button', { name: 'alice' })).toBeInTheDocument();
  expect(screen.getByRole('img', { name: 'connected via someone you follow' })).toBeInTheDocument();
  expect(screen.getByRole('img', { name: 'Public' })).toBeInTheDocument();
});

test('profile social management updates follow and mute lists and muted authors disappear from content surfaces', async () => {
  const mutedAuthorPubkey = 'b'.repeat(64);
  const visibleAuthorPubkey = 'c'.repeat(64);
  const user = userEvent.setup();

  render(
    <App
      api={createDesktopMockApi({
        seedPosts: {
          'kukuri:topic:general': [
            {
              object_id: 'post-muted-author',
              envelope_id: 'envelope-muted-author',
              author_pubkey: mutedAuthorPubkey,
              author_name: 'bob',
              author_display_name: null,
              following: false,
              followed_by: true,
              mutual: false,
              friend_of_friend: false,
              object_kind: 'post',
              content: 'mute this post',
              live: { session_id: 'live-muted', topic_id: 'kukuri:topic:general', channel_id: null },
              content_status: 'Available',
              attachments: [],
              created_at: 2,
              reply_to: null,
              root_id: 'post-muted-author',
              audience_label: 'Public',
            },
            {
              object_id: 'post-visible-author',
              envelope_id: 'envelope-visible-author',
              author_pubkey: visibleAuthorPubkey,
              author_name: 'carol',
              author_display_name: null,
              following: false,
              followed_by: false,
              mutual: false,
              friend_of_friend: false,
              object_kind: 'post',
              content: 'keep this post',
              live: { session_id: 'live-visible', topic_id: 'kukuri:topic:general', channel_id: null },
              content_status: 'Available',
              attachments: [],
              created_at: 1,
              reply_to: null,
              root_id: 'post-visible-author',
              audience_label: 'Public',
            },
          ],
        },
        seedLiveSessions: {
          'kukuri:topic:general': [
            {
              session_id: 'live-muted',
              host_pubkey: mutedAuthorPubkey,
              title: 'Muted Live',
              description: 'muted host session',
              status: 'Live',
              started_at: 2,
              ended_at: null,
              viewer_count: 0,
              joined_by_me: false,
              channel_id: null,
              audience_label: 'Public',
            },
            {
              session_id: 'live-visible',
              host_pubkey: visibleAuthorPubkey,
              title: 'Visible Live',
              description: 'visible host session',
              status: 'Live',
              started_at: 1,
              ended_at: null,
              viewer_count: 0,
              joined_by_me: false,
              channel_id: null,
              audience_label: 'Public',
            },
          ],
        },
        seedGameRooms: {
          'kukuri:topic:general': [
            {
              room_id: 'room-muted',
              host_pubkey: mutedAuthorPubkey,
              title: 'Muted Room',
              description: 'muted host room',
              status: 'Waiting',
              phase_label: null,
              scores: [
                {
                  participant_id: 'participant-bob',
                  label: 'Bob',
                  score: 0,
                },
                {
                  participant_id: 'participant-carol',
                  label: 'Carol',
                  score: 0,
                },
              ],
              updated_at: 2,
              channel_id: null,
              audience_label: 'Public',
            },
            {
              room_id: 'room-visible',
              host_pubkey: visibleAuthorPubkey,
              title: 'Visible Room',
              description: 'visible host room',
              status: 'Waiting',
              phase_label: null,
              scores: [
                {
                  participant_id: 'participant-dave',
                  label: 'Dave',
                  score: 0,
                },
                {
                  participant_id: 'participant-erin',
                  label: 'Erin',
                  score: 0,
                },
              ],
              updated_at: 1,
              channel_id: null,
              audience_label: 'Public',
            },
          ],
        },
        authorSocialViews: {
          [mutedAuthorPubkey]: {
            name: 'bob',
            followed_by: true,
          },
          [visibleAuthorPubkey]: {
            name: 'carol',
          },
        },
      })}
    />
  );

  const mutedPostCard = (await screen.findByText('mute this post')).closest('article');
  if (!(mutedPostCard instanceof HTMLElement)) {
    throw new Error('muted author post card not found');
  }
  await user.click(within(mutedPostCard).getByRole('button', { name: 'Bookmark' }));
  await waitFor(() => {
    expect(within(mutedPostCard).getByRole('button', { name: 'Remove bookmark' })).toBeInTheDocument();
  });

  await selectWorkspace(user, 'Profile');
  await user.click(screen.getByRole('button', { name: 'Following 0 users' }));

  const tabs = getSocialConnectionsTabs();
  await waitFor(() => {
    expect(within(tabs).getByRole('tab', { name: 'Following' })).toHaveAttribute(
      'aria-selected',
      'true'
    );
  });
  expect(screen.getByText('You are not following anyone yet.')).toBeInTheDocument();

  await user.click(within(tabs).getByRole('tab', { name: 'Followers' }));
  await waitFor(() => {
    expect(within(tabs).getByRole('tab', { name: 'Followers' })).toHaveAttribute(
      'aria-selected',
      'true'
    );
  });
  expect(
    screen.queryByText('Followed shows only followers already observed on this device.')
  ).not.toBeInTheDocument();

  let bobConnectionCard = screen.getByTestId('profile-connection-identifier-target');
  expect(screen.queryByText(mutedAuthorPubkey)).not.toBeInTheDocument();
  await user.click(within(bobConnectionCard).getByRole('button', { name: 'Follow' }));
  await waitFor(() => {
    const refreshedCard = screen.getByTestId('profile-connection-identifier-target');
    expect(
      within(refreshedCard).getByRole('button', { name: 'Unfollow' })
    ).toBeInTheDocument();
  });

  bobConnectionCard = screen.getByTestId('profile-connection-identifier-target');
  await user.click(within(bobConnectionCard).getByRole('button', { name: 'Actions for bob' }));
  await user.click(screen.getByRole('menuitem', { name: 'Mute' }));
  await waitFor(() => {
    const refreshedCard = screen.getByTestId('profile-connection-identifier-target');
    expect(within(refreshedCard).getByText('Muted')).toBeInTheDocument();
  });
  await user.click(within(bobConnectionCard).getByRole('button', { name: 'Actions for bob' }));
  expect(screen.getByRole('menuitem', { name: 'Unmute' })).toBeInTheDocument();
  await user.keyboard('{Escape}');

  await user.click(within(tabs).getByRole('tab', { name: 'Following' }));
  await waitFor(() => {
    expect(within(tabs).getByRole('tab', { name: 'Following' })).toHaveAttribute(
      'aria-selected',
      'true'
    );
  });
  bobConnectionCard = screen.getByTestId('profile-connection-identifier-target');
  expect(within(bobConnectionCard).getByRole('button', { name: 'Unfollow' })).toBeInTheDocument();

  await user.click(within(tabs).getByRole('tab', { name: 'Muted' }));
  await waitFor(() => {
    expect(within(tabs).getByRole('tab', { name: 'Muted' })).toHaveAttribute(
      'aria-selected',
      'true'
    );
  });
  bobConnectionCard = screen.getByTestId('profile-connection-identifier-target');
  expect(within(bobConnectionCard).getByRole('button', { name: 'Unmute' })).toBeInTheDocument();
  expect(within(bobConnectionCard).getByText('Muted')).toBeInTheDocument();

  await selectWorkspace(user, 'Timeline');
  await waitFor(() => {
    expect(screen.queryByText('mute this post')).not.toBeInTheDocument();
  });
  expect(screen.getByText('keep this post')).toBeInTheDocument();

  await selectTimelineView(user, 'Bookmarks');
  await waitFor(() => {
    expect(screen.getByText('No bookmarked posts yet.')).toBeInTheDocument();
  });

  await selectTimelineView(user, 'Feed');
  await waitFor(() => {
    expect(screen.queryByText('Muted Live')).not.toBeInTheDocument();
  });
  expect(screen.getByText('Visible Live')).toBeInTheDocument();

  await selectWorkspace(user, 'Metaverse');
  await waitFor(() => {
    expect(screen.queryByText('Muted Room')).not.toBeInTheDocument();
  });
  expect(screen.queryByText('Visible Room')).not.toBeInTheDocument();
  expect(screen.getByText('Metaverse Rooms')).toBeInTheDocument();
}, 15_000); // 複数画面を操作する実行枠。個々のwaitForと検証条件は維持する。

test('author detail shows via authors and follow action updates relationship', async () => {
  const authorPubkey = 'b'.repeat(64);
  const viaA = 'c'.repeat(64);
  const viaB = 'd'.repeat(64);
  const api = createDesktopMockApi({
    seedPosts: {
      'kukuri:topic:general': [
        {
          object_id: 'post-author',
          envelope_id: 'envelope-author',
          author_pubkey: authorPubkey,
          author_name: 'bob',
          author_display_name: null,
          following: false,
          followed_by: false,
          mutual: false,
          friend_of_friend: true,
          object_kind: 'post',
          content: 'author detail',
          content_status: 'Available',
          attachments: [],
          created_at: 1,
          reply_to: null,
          root_id: 'post-author',
          audience_label: 'Public',
        },
      ],
    },
    authorSocialViews: {
      [authorPubkey]: {
        name: 'bob',
        friend_of_friend: true,
        friend_of_friend_via_pubkeys: [viaA, viaB],
      },
    },
  });
  const user = userEvent.setup();

  render(<App api={api} />);

  await user.click(await screen.findByRole('button', { name: 'bob' }));

  expect(await screen.findByTestId('author-detail-avatar')).toBeInTheDocument();
  expect(screen.queryByText(viaA.slice(0, 12), { exact: false })).not.toBeInTheDocument();
  expect(screen.queryByText(viaB.slice(0, 12), { exact: false })).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Follow' })).toBeInTheDocument();

  await user.click(screen.getByRole('button', { name: 'Follow' }));

  await waitFor(() => {
    expect(screen.getByRole('button', { name: 'Unfollow' })).toBeInTheDocument();
  });
  expect(screen.getAllByText('following').length).toBeGreaterThan(0);
});

test('author detail mute toggle updates the selected author state', async () => {
  const authorPubkey = 'b'.repeat(64);
  const user = userEvent.setup();
  render(
    <App
      api={createDesktopMockApi({
        seedPosts: {
          'kukuri:topic:general': [
            {
              object_id: 'post-author-mute',
              envelope_id: 'envelope-author-mute',
              author_pubkey: authorPubkey,
              author_name: 'bob',
              author_display_name: null,
              following: false,
              followed_by: false,
              mutual: false,
              friend_of_friend: false,
              object_kind: 'post',
              content: 'author mute target',
              content_status: 'Available',
              attachments: [],
              created_at: 1,
              reply_to: null,
              root_id: 'post-author-mute',
              audience_label: 'Public',
            },
          ],
        },
        authorSocialViews: {
          [authorPubkey]: {
            name: 'bob',
            about: 'author detail stays visible while muted',
          },
        },
      })}
    />
  );

  await user.click(await screen.findByRole('button', { name: 'bob' }));

  await waitFor(() => expect(getDetailPane('Author')).toBeInTheDocument());
  const authorPane = getDetailPane('Author');
  expect(within(authorPane).getByRole('button', { name: 'Mute' })).toBeInTheDocument();

  await user.click(within(authorPane).getByRole('button', { name: 'Mute' }));

  await waitFor(() => {
    expect(within(authorPane).getByRole('button', { name: 'Unmute' })).toBeInTheDocument();
  });
  expect(within(authorPane).getByText('author detail stays visible while muted')).toBeInTheDocument();
});


// #992: ブロック中の作者詳細はフォロー／メッセージを無効にし、解除直後に同じ画面で復帰する。
function blockedAuthorApi(authorPubkey: string, social: Record<string, unknown>) {
  return createDesktopMockApi({
    seedPosts: {
      'kukuri:topic:general': [
        {
          object_id: 'post-author-blocked',
          envelope_id: 'envelope-author-blocked',
          author_pubkey: authorPubkey,
          author_name: 'bob',
          author_display_name: null,
          following: false,
          followed_by: false,
          mutual: false,
          friend_of_friend: false,
          object_kind: 'post',
          content: 'blocked author detail',
          content_status: 'Available',
          attachments: [],
          created_at: 1,
          reply_to: null,
          root_id: 'post-author-blocked',
          audience_label: 'Public',
        },
      ],
    },
    authorSocialViews: { [authorPubkey]: { name: 'bob', blocking: true, ...social } },
  });
}

test('author detail keeps follow disabled while blocked and restores it right after unblock', async () => {
  const authorPubkey = 'b'.repeat(64);
  const api = blockedAuthorApi(authorPubkey, { followed_by: true });
  const followAuthor = vi.spyOn(api, 'followAuthor');
  const user = userEvent.setup();
  render(<App api={api} />);

  await user.click(await screen.findByRole('button', { name: 'bob' }));
  await waitFor(() => expect(getDetailPane('Author')).toBeInTheDocument());
  const authorPane = getDetailPane('Author');

  const follow = within(authorPane).getByRole('button', { name: 'Follow' });
  expect(follow).toHaveAttribute('aria-disabled', 'true');
  expect(within(authorPane).getByText('Blocked', { selector: '.relationship-badge' })).toBeInTheDocument();
  await user.click(follow);
  expect(followAuthor).not.toHaveBeenCalled();

  await user.click(within(authorPane).getByRole('button', { name: 'Unblock' }));
  await waitFor(() => {
    expect(within(authorPane).getByRole('button', { name: 'Follow' })).not.toHaveAttribute('aria-disabled');
  });
  expect(within(authorPane).queryByText('Blocked', { selector: '.relationship-badge' })).not.toBeInTheDocument();
  await user.click(within(authorPane).getByRole('button', { name: 'Follow' }));
  await waitFor(() => expect(followAuthor).toHaveBeenCalledWith(authorPubkey));
  await waitFor(() => {
    expect(within(authorPane).getByRole('button', { name: 'Unfollow' })).toBeInTheDocument();
  });
});

test('author detail keeps message disabled while a mutual follow is blocked', async () => {
  const authorPubkey = 'b'.repeat(64);
  const api = blockedAuthorApi(authorPubkey, { following: true, followed_by: true, mutual: true });
  const user = userEvent.setup();
  render(<App api={api} />);

  await user.click(await screen.findByRole('button', { name: 'bob' }));
  await waitFor(() => expect(getDetailPane('Author')).toBeInTheDocument());
  const authorPane = getDetailPane('Author');

  const message = within(authorPane).getByRole('button', { name: 'Message' });
  expect(message).toHaveAttribute('aria-disabled', 'true');
  expect(within(authorPane).getByRole('button', { name: 'Unfollow' })).not.toHaveAttribute('aria-disabled');
  await user.click(message);
  expect(window.location.hash).not.toContain('#/messages');

  await user.click(within(authorPane).getByRole('button', { name: 'Unblock' }));
  await waitFor(() => {
    expect(within(authorPane).getByRole('button', { name: 'Message' })).not.toHaveAttribute('aria-disabled');
  });
  await user.click(within(authorPane).getByRole('button', { name: 'Message' }));
  await waitFor(() => expect(window.location.hash).toContain('#/messages'));
});
