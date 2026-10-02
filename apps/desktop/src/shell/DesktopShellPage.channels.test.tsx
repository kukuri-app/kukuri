import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';

import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import { buildChannelAccessPreviewDeepLink } from '@/lib/internalLinks';
import { App } from '@/App';
import {
  createDeferred,
  expectActiveTopic,
  getChannelShareButton,
  openChannelManager,
  openChannelSettings,
  openControlCenter,
  openLiveCreateDialog,
  renderAtHash,
  selectWorkspace,
  setViewportWidth,
} from './DesktopShellPage.testHelpers';
import type { JoinedPrivateChannelPage } from '@/lib/api';

beforeEach(() => {
  setViewportWidth(1024);
  window.history.replaceState(null, '', '/');
});

afterEach(() => {
  vi.useRealTimers();
});

test('desktop shell can create, join, leave, and end a live session', async () => {
  const user = userEvent.setup();
  render(<App api={createDesktopMockApi()} />);

  const liveDialog = await openLiveCreateDialog(user);
  await user.type(within(liveDialog).getByPlaceholderText('Friday stream'), 'Launch Party');
  await user.type(within(liveDialog).getByPlaceholderText('short session summary'), 'watch along');
  await user.click(within(liveDialog).getByRole('button', { name: 'Start Live' }));

  await waitFor(() => {
    expect(screen.getByText('Launch Party')).toBeInTheDocument();
  });
  expect(screen.getByText('watch along')).toBeInTheDocument();

  const liveCard = screen.getByText('Launch Party').closest('article');
  if (!(liveCard instanceof HTMLElement)) {
    throw new Error('live session card not found');
  }

  await user.click(within(liveCard).getByRole('button', { name: 'Join' }));
  await waitFor(() => {
    expect(screen.getByText('viewers: 1')).toBeInTheDocument();
  });
  expect(within(liveCard).getByRole('button', { name: 'Leave' })).toBeInTheDocument();

  await user.click(within(liveCard).getByRole('button', { name: 'Leave' }));
  await waitFor(() => {
    expect(screen.getByText('viewers: 0')).toBeInTheDocument();
  });

  await user.click(within(liveCard).getByRole('button', { name: 'End' }));
  await waitFor(() => {
    expect(screen.getByText('Ended')).toBeInTheDocument();
  });
});

test('desktop shell can create a private channel and export an invite', async () => {
  const user = userEvent.setup();
  const writeText = vi.fn().mockResolvedValue(undefined);
  Object.defineProperty(window.navigator, 'clipboard', {
    configurable: true,
    value: {
      writeText,
    },
  });
  render(<App api={createDesktopMockApi()} />);
  const channelDialog = await openChannelManager(user);
  await user.type(within(channelDialog).getByPlaceholderText('Channel name'), 'core');
  await user.click(within(channelDialog).getByRole('button', { name: 'Create Channel' }));

  await waitFor(() => {
    expect(window.location.hash).toBe('#/timeline?topic=kukuri%3Atopic%3Ageneral&channel=channel-1');
    expect(within(channelDialog).getByText('Copy share link')).toBeInTheDocument();
  });
  await user.click(within(channelDialog).getByRole('button', { name: 'Copy link' }));
  expect(writeText).toHaveBeenLastCalledWith(
    buildChannelAccessPreviewDeepLink('invite:kukuri:topic:general:channel-1')
  );
  await user.click(within(channelDialog).getByRole('button', { name: 'Close dialog' }));

  const settingsDialog = await openChannelSettings(user, 'core');
  await user.click(getChannelShareButton(settingsDialog, 'core', 'Invite only'));

  await waitFor(() => {
    expect(within(settingsDialog).getByText('Copy share link')).toBeInTheDocument();
    expect(within(settingsDialog).queryByText(/invite:kukuri:topic:general:channel-1/)).not.toBeInTheDocument();
  });
});

test('desktop shell confirms and leaves a private channel', async () => {
  const user = userEvent.setup();
  const api = createDesktopMockApi();
  const leavePrivateChannel = vi.spyOn(api, 'leavePrivateChannel');
  render(<App api={api} />);

  const channelDialog = await openChannelManager(user);
  await user.type(within(channelDialog).getByPlaceholderText('Channel name'), 'core');
  await user.click(within(channelDialog).getByRole('button', { name: 'Create Channel' }));

  await waitFor(() => {
    expect(window.location.hash).toBe('#/timeline?topic=kukuri%3Atopic%3Ageneral&channel=channel-1');
  });
  await user.click(within(channelDialog).getByRole('button', { name: 'Close dialog' }));

  let controlCenter = await openControlCenter(user);
  await user.click(within(controlCenter).getByRole('button', { name: 'Leave core channel' }));
  let leaveDialog = await screen.findByRole('dialog', { name: 'Leave channel' });
  expect(within(leaveDialog).getByText('Leave this channel?')).toBeInTheDocument();
  await user.click(within(leaveDialog).getByRole('button', { name: 'Close dialog' }));
  expect(leavePrivateChannel).not.toHaveBeenCalled();

  controlCenter = await openControlCenter(user);
  await user.click(within(controlCenter).getByRole('button', { name: 'Leave core channel' }));
  leaveDialog = await screen.findByRole('dialog', { name: 'Leave channel' });
  await user.click(within(leaveDialog).getByRole('button', { name: 'No' }));
  expect(leavePrivateChannel).not.toHaveBeenCalled();

  controlCenter = await openControlCenter(user);
  await user.click(within(controlCenter).getByRole('button', { name: 'Leave core channel' }));
  leaveDialog = await screen.findByRole('dialog', { name: 'Leave channel' });
  await user.click(within(leaveDialog).getByRole('button', { name: 'Yes' }));

  await waitFor(() => {
    expect(leavePrivateChannel).toHaveBeenCalledWith('kukuri:topic:general', 'channel-1');
    expect(window.location.hash).toBe('#/timeline?topic=kukuri%3Atopic%3Ageneral');
    expect(screen.queryByRole('button', { name: /core.*Invite only/ })).not.toBeInTheDocument();
  });
});

test('desktop shell joins an imported private channel and selects its topic scope', async () => {
  const user = userEvent.setup();
  render(
    <App
      api={createDesktopMockApi({
        invitePreview: {
          channel_id: 'channel-imported',
          topic_id: 'kukuri:topic:private-imported',
          channel_label: 'Imported',
          inviter_pubkey: 'f'.repeat(64),
          owner_pubkey: 'f'.repeat(64),
          epoch_id: 'epoch-imported-1',
          expires_at: null,
          namespace_secret_hex: 'a'.repeat(64),
        },
      })}
    />
  );
  const channelDialog = await openChannelManager(user);
  await user.type(within(channelDialog).getByPlaceholderText(/paste a private channel invite/i), 'invite-token');
  await user.click(within(channelDialog).getByRole('button', { name: 'Join' }));
  await user.click(within(channelDialog).getByRole('button', { name: 'Close dialog' }));

  await waitFor(() => {
    expectActiveTopic('kukuri:topic:private-imported');
    expect(window.location.hash).toBe(
      '#/timeline?topic=kukuri%3Atopic%3Aprivate-imported&channel=channel-imported'
    );
  });
});

test('channel route restore waits for joined channel list before normalizing', async () => {
  const user = userEvent.setup();
  const joinedChannels = createDeferred<JoinedPrivateChannelPage>();
  const api = createDesktopMockApi();
  const listJoinedPrivateChannels = vi
    .spyOn(api, 'listJoinedPrivateChannels')
    .mockImplementation(async (topic) => {
      if (topic !== 'kukuri:topic:general') {
        return { items: [], next_cursor: null };
      }
      return joinedChannels.promise;
    });

  renderAtHash('#/timeline?topic=kukuri%3Atopic%3Ageneral&channel=channel-restored', api);

  await waitFor(() => {
    expect(listJoinedPrivateChannels).toHaveBeenCalledWith('kukuri:topic:general');
  });
  expect(window.location.hash).toBe(
    '#/timeline?topic=kukuri%3Atopic%3Ageneral&channel=channel-restored'
  );

  joinedChannels.resolve({ next_cursor: null, items: [
    {
      topic_id: 'kukuri:topic:general',
      channel_id: 'channel-restored',
      label: 'restored',
      creator_pubkey: 'f'.repeat(64),
      owner_pubkey: 'f'.repeat(64),
      joined_via_pubkey: null,
      audience_kind: 'friend_plus',
      is_owner: false,
      current_epoch_id: 'epoch-restored',
      archived_epoch_ids: [],
      sharing_state: 'open',
      rotation_required: false,
      participant_count: 1,
      stale_participant_count: 0,
    },
  ] });

  await waitFor(() => {
    expect(window.location.hash).toBe(
      '#/timeline?topic=kukuri%3Atopic%3Ageneral&channel=channel-restored'
    );
  });
  const controlCenter = await openControlCenter(user);
  expect(within(controlCenter).getByRole('button', { name: /restored.*Mutuals\+/ })).toHaveClass(
    'topic-subitem-active'
  );
});

test('desktop shell shows friend-only controls and can create a grant', async () => {
  const user = userEvent.setup();
  render(<App api={createDesktopMockApi()} />);
  const channelDialog = await openChannelManager(user);
  await user.type(within(channelDialog).getByPlaceholderText('Channel name'), 'friends');
  await user.selectOptions(within(channelDialog).getByLabelText('Audience'), 'friend_only');
  await user.click(within(channelDialog).getByRole('button', { name: 'Create Channel' }));

  await waitFor(() => {
    expect(window.location.hash).toBe('#/timeline?topic=kukuri%3Atopic%3Ageneral&channel=channel-1');
    expect(screen.queryByRole('button', { name: 'Rotate' })).not.toBeInTheDocument();
  });
  await user.click(within(channelDialog).getByRole('button', { name: 'Close dialog' }));

  const settingsDialog = await openChannelSettings(user, 'friends');
  await user.click(getChannelShareButton(settingsDialog, 'friends', 'Mutuals'));

  await waitFor(() => {
    expect(within(settingsDialog).getByText('Copy share link')).toBeInTheDocument();
    expect(within(settingsDialog).queryByText(/grant:kukuri:topic:general:channel-1/)).not.toBeInTheDocument();
  });
});

test('desktop shell shows friend-plus controls and can create a share', async () => {
  const user = userEvent.setup();
  const writeText = vi.fn().mockResolvedValue(undefined);
  Object.defineProperty(window.navigator, 'clipboard', {
    configurable: true,
    value: {
      writeText,
    },
  });
  render(<App api={createDesktopMockApi()} />);
  const channelDialog = await openChannelManager(user);
  await user.type(within(channelDialog).getByPlaceholderText('Channel name'), 'friends+');
  await user.selectOptions(within(channelDialog).getByLabelText('Audience'), 'friend_plus');
  await user.click(within(channelDialog).getByRole('button', { name: 'Create Channel' }));

  await waitFor(() => {
    expect(window.location.hash).toBe('#/timeline?topic=kukuri%3Atopic%3Ageneral&channel=channel-1');
    expect(screen.queryByRole('button', { name: 'Freeze' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Rotate' })).not.toBeInTheDocument();
  });
  await user.click(within(channelDialog).getByRole('button', { name: 'Close dialog' }));

  const settingsDialog = await openChannelSettings(user, 'friends+');
  await user.click(getChannelShareButton(settingsDialog, 'friends+', 'Mutuals+'));

  await waitFor(() => {
    expect(within(settingsDialog).getByText('Copy share link')).toBeInTheDocument();
    expect(within(settingsDialog).queryByText(/share:kukuri:topic:general:channel-1/)).not.toBeInTheDocument();
  });

  await user.click(within(settingsDialog).getByRole('button', { name: 'Copy link' }));
  expect(writeText).toHaveBeenLastCalledWith(
    buildChannelAccessPreviewDeepLink('share:kukuri:topic:general:channel-1')
  );
});

test('share token smart link previews before import and joins only after confirmation', async () => {
  const user = userEvent.setup();
  const ownerPubkey = 'f'.repeat(64);
  const inviteToken = JSON.stringify({
    envelope: {
      kind: 'channel-invite',
      pubkey: ownerPubkey,
      content: JSON.stringify({
        channel_id: 'channel-imported',
        topic_id: 'kukuri:topic:private-imported',
        channel_label: 'Imported',
        owner_pubkey: ownerPubkey,
        epoch_id: 'epoch-imported-1',
        namespace_secret_hex: 'a'.repeat(64),
        expires_at: null,
      }),
    },
  });
  const api = createDesktopMockApi({
    seedPosts: {
      'kukuri:topic:general': [
        {
          object_id: 'share-post',
          envelope_id: 'envelope-share-post',
          author_pubkey: 'a'.repeat(64),
          author_name: 'alice',
          author_display_name: null,
          following: false,
          followed_by: false,
          mutual: false,
          friend_of_friend: false,
          object_kind: 'post',
          content: buildChannelAccessPreviewDeepLink(inviteToken),
          content_status: 'Available',
          attachments: [],
          created_at: 1,
          reply_to: null,
          root_id: 'share-post',
          channel_id: null,
          audience_label: 'Public',
        },
      ],
    },
    invitePreview: {
      channel_id: 'channel-imported',
      topic_id: 'kukuri:topic:private-imported',
      channel_label: 'Imported',
      inviter_pubkey: ownerPubkey,
      owner_pubkey: ownerPubkey,
      epoch_id: 'epoch-imported-1',
      expires_at: null,
      namespace_secret_hex: 'a'.repeat(64),
    },
  });
  const previewSpy = vi.spyOn(api, 'previewChannelAccessToken');
  const importSpy = vi.spyOn(api, 'importChannelAccessToken');

  render(<App api={api} />);

  const tokenChip = (await screen.findAllByRole('button', { name: /Imported.*Invite only/ }))
    .find((button) => button.classList.contains('smart-reference-chip'));
  if (!(tokenChip instanceof HTMLButtonElement)) {
    throw new Error('expected access preview chip');
  }
  expect(tokenChip).not.toHaveAttribute('title');
  await user.hover(tokenChip);
  expect(await screen.findByRole('tooltip')).toHaveTextContent(inviteToken);
  await user.unhover(tokenChip);

  await user.click(tokenChip);

  const dialog = await screen.findByRole('dialog', { name: 'Review access' });
  await waitFor(() => {
    expect(previewSpy).toHaveBeenCalledTimes(1);
    expect(previewSpy).toHaveBeenCalledWith(inviteToken);
    expect(importSpy).not.toHaveBeenCalled();
  });
  expect(within(dialog).getByText('Imported')).toBeInTheDocument();
  expect(within(dialog).queryByText(/channel-imported/)).not.toBeInTheDocument();
  expect(within(dialog).queryByText(/epoch-imported-1/)).not.toBeInTheDocument();
  const channelPreviewItem = within(dialog).getByText('Imported').closest('div');
  if (!(channelPreviewItem instanceof HTMLElement)) {
    throw new Error('expected channel preview item');
  }
  expect(channelPreviewItem).not.toHaveAttribute('title');
  await user.hover(channelPreviewItem);
  expect(await screen.findByRole('tooltip')).toHaveTextContent('channel-imported');

  await user.click(within(dialog).getByRole('button', { name: 'Join' }));

  await waitFor(() => {
    expect(importSpy).toHaveBeenCalledTimes(1);
    expect(window.location.hash).toBe(
      '#/timeline?topic=kukuri%3Atopic%3Aprivate-imported&channel=channel-imported'
    );
  });
});

test('copy link actions write canonical hash routes for topic, post, and live', async () => {
  const user = userEvent.setup();
  const writeText = vi.fn().mockResolvedValue(undefined);
  Object.defineProperty(window.navigator, 'clipboard', {
    configurable: true,
    value: {
      writeText,
    },
  });

  render(
    <App
      api={createDesktopMockApi({
        seedPosts: {
          'kukuri:topic:general': [
            {
              object_id: 'copy-post',
              envelope_id: 'envelope-copy-post',
              author_pubkey: 'a'.repeat(64),
              author_name: 'alice',
              author_display_name: null,
              following: false,
              followed_by: false,
              mutual: false,
              friend_of_friend: false,
              object_kind: 'post',
              content: 'copy this post',
              content_status: 'Available',
              attachments: [],
              created_at: 1,
              reply_to: null,
              root_id: 'copy-post',
              channel_id: null,
              audience_label: 'Public',
            },
          ],
        },
        seedLiveSessions: {
          'kukuri:topic:general': [
            {
              session_id: 'session-demo',
              host_pubkey: 'a'.repeat(64),
              title: 'Live Demo',
              description: 'watch here',
              status: 'Live',
              started_at: 1,
              viewer_count: 1,
              joined_by_me: false,
              channel_id: null,
              audience_label: 'Public',
            },
          ],
        },
        seedGameRooms: {
          'kukuri:topic:general': [
            {
              room_id: 'room-demo',
              host_pubkey: 'a'.repeat(64),
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
      })}
    />
  );

  const controlCenter = await openControlCenter(user);
  const topicItem = within(controlCenter).getByRole('button', { name: 'general' }).closest('li');
  if (!(topicItem instanceof HTMLElement)) {
    throw new Error('expected topic item');
  }
  await user.click(within(topicItem).getByRole('button', { name: 'Copy link' }));
  expect(writeText).toHaveBeenLastCalledWith('#/timeline?topic=kukuri%3Atopic%3Ageneral');
  await waitFor(() => {
    expect(screen.getByRole('status')).toHaveTextContent('Copied to clipboard.');
    expect(screen.getAllByRole('status')).toHaveLength(1);
  });

  const postArticle = screen.getByText('copy this post').closest('article');
  if (!(postArticle instanceof HTMLElement)) {
    throw new Error('expected post article');
  }
  await user.click(within(postArticle).getByRole('button', { name: 'Copy link' }));
  expect(writeText).toHaveBeenLastCalledWith(
    '#/timeline?topic=kukuri%3Atopic%3Ageneral&context=thread&threadId=copy-post&focusObjectId=copy-post'
  );
  await waitFor(() => {
    expect(screen.getAllByRole('status')).toHaveLength(1);
  });

  await selectWorkspace(user, 'Live');
  const liveArticle = screen.getByText('Live Demo').closest('article');
  if (!(liveArticle instanceof HTMLElement)) {
    throw new Error('expected live article');
  }
  await user.click(within(liveArticle).getByRole('button', { name: 'Copy link' }));
  expect(writeText).toHaveBeenLastCalledWith(
    '#/live?topic=kukuri%3Atopic%3Ageneral&sessionId=session-demo'
  );
  await waitFor(() => {
    expect(screen.getAllByRole('status')).toHaveLength(1);
  });

  await selectWorkspace(user, 'Metaverse');
  expect(screen.getByText('Metaverse Rooms')).toBeInTheDocument();
  expect(screen.queryByText('Room Demo')).not.toBeInTheDocument();
});

test('channel settings copy removes duplicate summary and share button icon', async () => {
  const user = userEvent.setup();
  render(<App api={createDesktopMockApi()} />);

  const channelDialog = await openChannelManager(user);
  await user.type(within(channelDialog).getByPlaceholderText('Channel name'), 'friends+');
  await user.selectOptions(within(channelDialog).getByLabelText('Audience'), 'friend_plus');
  await user.click(within(channelDialog).getByRole('button', { name: 'Create Channel' }));

  await waitFor(() => {
    expect(window.location.hash).toBe('#/timeline?topic=kukuri%3Atopic%3Ageneral&channel=channel-1');
  });
  expect(
    within(channelDialog).queryByRole('button', { name: 'Create share link' })
  ).not.toBeInTheDocument();
  await user.click(within(channelDialog).getByRole('button', { name: 'Close dialog' }));

  const settingsDialog = await openChannelSettings(user, 'friends+');
  const shareButton = await within(settingsDialog).findByRole('button', {
    name: 'Create share link',
  });
  expect(within(settingsDialog).getByText('Channel name: friends+')).toBeInTheDocument();
  expect(
    within(settingsDialog).getByText(
      'Who can join: Mutuals+: participants can share with users they mutually follow'
    )
  ).toBeInTheDocument();
  expect(within(settingsDialog).queryByText('friends+ / Mutuals+')).not.toBeInTheDocument();
  expect(shareButton).toHaveTextContent('Create share link');
  expect(shareButton.querySelector('svg')).not.toBeInTheDocument();
});

test('desktop shell metaverse workspace hides the legacy score game room list', async () => {
  const user = userEvent.setup();
  render(<App api={createDesktopMockApi()} />);

  await selectWorkspace(user, 'Metaverse');

  await waitFor(() => {
    expect(screen.getByText('Metaverse Rooms')).toBeInTheDocument();
  });
  expect(screen.queryByText('Game Rooms')).not.toBeInTheDocument();
  expect(screen.queryByText('No game rooms')).not.toBeInTheDocument();
  expect(screen.queryByLabelText(/game-.*-status/)).not.toBeInTheDocument();
});


test('joined channel list reads the next page and keeps it across periodic refresh', async () => {
  const user = userEvent.setup();
  const api = createDesktopMockApi();
  for (let index = 0; index < 130; index += 1) {
    await api.createPrivateChannel('kukuri:topic:general', `paged ${index}`);
  }
  const listJoinedPrivateChannels = vi.spyOn(api, 'listJoinedPrivateChannels');
  render(<App api={api} />);
  const channelDialog = await openChannelManager(user);
  const joinedRows = () => within(channelDialog).getAllByRole('button', { name: /^Open paged/ });

  await waitFor(() => expect(joinedRows()).toHaveLength(128));
  await user.click(within(channelDialog).getByRole('button', { name: 'Show more' }));
  await waitFor(() => expect(joinedRows()).toHaveLength(130));
  expect(within(channelDialog).queryByRole('button', { name: 'Show more' })).not.toBeInTheDocument();
  expect(new Set(joinedRows().map((row) => row.getAttribute('aria-label'))).size).toBe(130);

  // 定期の再読み込みは最初の page だけを読み直し、読み込んだ続きを消さない。
  const callsBeforeRefresh = listJoinedPrivateChannels.mock.calls.length;
  window.dispatchEvent(new Event('focus'));
  await waitFor(() =>
    expect(listJoinedPrivateChannels.mock.calls.length).toBeGreaterThan(callsBeforeRefresh)
  );
  expect(listJoinedPrivateChannels.mock.calls.at(-1)).toEqual(['kukuri:topic:general']);
  await waitFor(() => expect(joinedRows()).toHaveLength(130));
  expect(within(channelDialog).queryByRole('button', { name: 'Show more' })).not.toBeInTheDocument();
});
