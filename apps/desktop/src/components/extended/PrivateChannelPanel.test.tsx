import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, test, vi } from 'vitest';

import { PrivateChannelPanel, PrivateChannelSettingsPanel } from './PrivateChannelPanel';
import type { PrivateChannelListItemView } from './types';

// Issue #966: 作成・参加 Dialog の説明と参加済み一覧、設定 Dialog の理由表示。
// 既存 handler は変更せず、開く／説明するだけでは API を呼ばないことを固定する。

function channel(
  overrides: Partial<PrivateChannelListItemView['channel']> = {}
): PrivateChannelListItemView['channel'] {
  return {
    topic_id: 'kukuri:topic:demo',
    channel_id: 'channel-1',
    label: 'core',
    creator_pubkey: 'a'.repeat(64),
    owner_pubkey: 'a'.repeat(64),
    joined_via_pubkey: null,
    audience_kind: 'invite_only',
    is_owner: true,
    current_epoch_id: 'epoch-1',
    archived_epoch_ids: [],
    sharing_state: 'open',
    rotation_required: false,
    participant_count: 1,
    stale_participant_count: 0,
    ...overrides,
  };
}

const AUDIENCE_OPTIONS = [
  { value: 'invite_only', label: 'Invite only' },
  { value: 'friend_only', label: 'Mutuals' },
  { value: 'friend_plus', label: 'Mutuals+' },
] as const;

function renderPanel(overrides: Partial<React.ComponentProps<typeof PrivateChannelPanel>> = {}) {
  const onCreateChannel = vi.fn((event: React.FormEvent<HTMLFormElement>) => event.preventDefault());
  const onJoin = vi.fn((event: React.FormEvent<HTMLFormElement>) => event.preventDefault());
  const view = render(
    <PrivateChannelPanel
      status='ready'
      error={null}
      pendingAction={null}
      channelLabel=''
      channelAudience='invite_only'
      channelAudienceOptions={[...AUDIENCE_OPTIONS]}
      inviteTokenInput=''
      onChannelLabelChange={vi.fn()}
      onChannelAudienceChange={vi.fn()}
      onInviteTokenChange={vi.fn()}
      onCreateChannel={onCreateChannel}
      onJoin={onJoin}
      {...overrides}
    />
  );
  return { ...view, onCreateChannel, onJoin };
}

test('create/join panel explains the feature, the selected audience, and how to obtain an invite', () => {
  const { onCreateChannel, onJoin } = renderPanel();

  expect(
    screen.getByText(/A private channel is a space inside this topic where only participants/)
  ).toBeInTheDocument();
  expect(screen.getByText(/You become the owner and can create invite or share links/)).toBeInTheDocument();
  expect(
    screen.getByText(/Invite and share links \(or tokens\) come from a channel participant/)
  ).toBeInTheDocument();
  const audience = screen.getByLabelText('Audience');
  expect(audience).toHaveAccessibleDescription(
    'Invite only: only users who receive an invite link can join. Participants can also create invites'
  );
  // 参加済みが無い場合は一覧を出さない。
  expect(screen.queryByText('Joined in this topic')).not.toBeInTheDocument();
  expect(onCreateChannel).not.toHaveBeenCalled();
  expect(onJoin).not.toHaveBeenCalled();
});

test('audience description follows the selected audience', () => {
  renderPanel({ channelAudience: 'friend_plus' });
  expect(screen.getByLabelText('Audience')).toHaveAccessibleDescription(
    'Mutuals+: participants can share with users they mutually follow'
  );
});

test('joined channels are listed with open and settings actions', async () => {
  const user = userEvent.setup();
  const onSelectJoinedChannel = vi.fn();
  const onOpenJoinedChannelSettings = vi.fn();
  renderPanel({
    joinedChannels: [channel(), channel({ channel_id: 'channel-2', label: 'friends', audience_kind: 'friend_only' })],
    onSelectJoinedChannel,
    onOpenJoinedChannelSettings,
  });

  expect(screen.getByText('Joined in this topic')).toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Open friends' }));
  expect(onSelectJoinedChannel).toHaveBeenCalledWith('channel-2');
  await user.click(screen.getByRole('button', { name: 'core settings and sharing' }));
  expect(onOpenJoinedChannelSettings).toHaveBeenCalledWith('channel-1');
});

test('settings panel blocks mutuals share for non-owners with a reason instead of calling share', async () => {
  const user = userEvent.setup();
  const onShare = vi.fn();
  render(
    <PrivateChannelSettingsPanel
      error={null}
      pendingAction={null}
      channel={channel({ audience_kind: 'friend_only', is_owner: false })}
      inviteOutput={null}
      inviteOutputLabel='grant'
      onShare={onShare}
    />
  );

  const share = screen.getByRole('button', { name: 'Create share link' });
  expect(share).toBeDisabled();
  expect(share).toHaveAccessibleDescription(
    'Only the owner can create share links for a mutuals channel. Ask the owner for an invite.'
  );
  await user.click(share);
  expect(onShare).not.toHaveBeenCalled();
});

test('settings panel shows the participant count only on the owner device', () => {
  const { rerender } = render(
    <PrivateChannelSettingsPanel
      error={null}
      pendingAction={null}
      channel={channel({ audience_kind: 'friend_plus', participant_count: 3 })}
      inviteOutput={null}
      inviteOutputLabel='grant'
      onShare={vi.fn()}
    />
  );
  expect(screen.getByText('participants: 3')).toBeInTheDocument();

  rerender(
    <PrivateChannelSettingsPanel
      error={null}
      pendingAction={null}
      channel={channel({ audience_kind: 'friend_plus', is_owner: false, participant_count: null })}
      inviteOutput={null}
      inviteOutputLabel='grant'
      onShare={vi.fn()}
    />
  );
  expect(screen.queryByText(/participants:/)).not.toBeInTheDocument();
});

test('settings panel keeps invite-only sharing available to participants and explains the link', async () => {
  const user = userEvent.setup();
  const onShare = vi.fn();
  render(
    <PrivateChannelSettingsPanel
      error={null}
      pendingAction={null}
      channel={channel({ is_owner: false, joined_via_pubkey: 'b'.repeat(64) })}
      inviteOutput={null}
      inviteOutputLabel='invite'
      onShare={onShare}
    />
  );

  const share = screen.getByRole('button', { name: 'Create share link' });
  expect(share).toBeEnabled();
  expect(screen.getByText(/Send the share link only to people you want to invite/)).toBeInTheDocument();
  await user.click(share);
  expect(onShare).toHaveBeenCalledTimes(1);
});

test('settings panel shows a pending reason and the rotation next step', () => {
  const { rerender } = render(
    <PrivateChannelSettingsPanel
      error={null}
      pendingAction='share'
      channel={channel({ audience_kind: 'friend_only', is_owner: true, rotation_required: true })}
      inviteOutput={null}
      inviteOutputLabel='grant'
      onShare={vi.fn()}
    />
  );

  expect(screen.getByRole('status')).toHaveTextContent('Working… please wait until it finishes.');
  expect(screen.getByRole('button', { name: 'Create share link' })).toBeDisabled();
  expect(
    screen.getByText(/Creating a share link hands new access only to participants who still qualify/)
  ).toBeInTheDocument();

  rerender(
    <PrivateChannelSettingsPanel
      error={null}
      pendingAction={null}
      channel={channel({ audience_kind: 'friend_only', is_owner: true, rotation_required: true })}
      inviteOutput={null}
      inviteOutputLabel='grant'
      onShare={vi.fn()}
    />
  );
  expect(screen.queryByRole('status')).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Create share link' })).toBeEnabled();
});

test('joined list shows more while loading, and keeps rows and the button after a failure', async () => {
  const user = userEvent.setup();
  let rejectLoad: (error: Error) => void = () => undefined;
  const onLoadMoreJoinedChannels = vi.fn(
    () => new Promise<void>((_, reject) => { rejectLoad = reject; })
  );
  renderPanel({ joinedChannels: [channel()], onLoadMoreJoinedChannels });

  await user.click(screen.getByRole('button', { name: 'Show more' }));
  expect(screen.getByRole('button', { name: 'Loading…' })).toBeDisabled();
  rejectLoad(new Error('offline'));

  expect(await screen.findByText('Failed to load private channels.')).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Open core' })).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Show more' })).toBeEnabled();
  expect(onLoadMoreJoinedChannels).toHaveBeenCalledTimes(1);
});
