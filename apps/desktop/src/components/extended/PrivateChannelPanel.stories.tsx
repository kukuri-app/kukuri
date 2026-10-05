import { useState, type FormEvent } from 'react';

import type { Meta, StoryObj } from '@storybook/react-vite';
import { expect, userEvent, within } from 'storybook/test';

import type { PrivateChannelControllerTake } from '@/lib/api';

import { PrivateChannelPanel, PrivateChannelSettingsPanel } from './PrivateChannelPanel';
import type { PrivateChannelListItemView, PrivateChannelPendingAction } from './types';

const meta = {
  title: 'Extended/PrivateChannelPanel',
  component: PrivateChannelPanel,
} satisfies Meta<typeof PrivateChannelPanel>;

export default meta;

type Story = StoryObj<typeof meta>;

const BASE_CHANNELS: PrivateChannelListItemView[] = [
  {
    active: true,
    channel: {
      topic_id: 'kukuri:topic:demo',
      channel_id: 'channel-1',
      label: 'Core Contributors',
      creator_pubkey: 'a'.repeat(64),
      owner_pubkey: 'a'.repeat(64),
      joined_via_pubkey: null,
      audience_kind: 'friend_plus',
      is_owner: true,
      current_epoch_id: 'epoch-4',
      archived_epoch_ids: ['epoch-3'],
      sharing_state: 'open',
      rotation_required: false,
      participant_count: 3,
      stale_participant_count: 0,
    },
  },
  {
    active: false,
    channel: {
      topic_id: 'kukuri:topic:demo',
      channel_id: 'channel-2',
      label: 'Invite-only Review',
      creator_pubkey: 'b'.repeat(64),
      owner_pubkey: 'b'.repeat(64),
      joined_via_pubkey: 'c'.repeat(64),
      audience_kind: 'invite_only',
      is_owner: false,
      current_epoch_id: 'legacy',
      archived_epoch_ids: [],
      sharing_state: 'open',
      rotation_required: false,
      participant_count: 0,
      stale_participant_count: 0,
    },
  },
];

const STORY_ARGS = {
  status: 'ready',
  error: null,
  pendingAction: null,
  channelLabel: 'Core Contributors',
  channelAudience: 'friend_plus',
  channelAudienceOptions: [
    { value: 'invite_only', label: 'Invite only' },
    { value: 'friend_only', label: 'Friends' },
    { value: 'friend_plus', label: 'Friends+' },
  ],
  inviteTokenInput: '',
  onChannelLabelChange: () => undefined,
  onChannelAudienceChange: () => undefined,
  onInviteTokenChange: () => undefined,
  onCreateChannel: (event: FormEvent<HTMLFormElement>) => event.preventDefault(),
  onJoin: (event: FormEvent<HTMLFormElement>) => event.preventDefault(),
} satisfies React.ComponentProps<typeof PrivateChannelPanel>;

function ChannelStory({
  status = 'ready',
  error = null,
}: {
  status?: 'loading' | 'ready' | 'error';
  error?: string | null;
}) {
  const [label, setLabel] = useState('Core Contributors');
  const [audience, setAudience] = useState<'invite_only' | 'friend_only' | 'friend_plus'>(
    'friend_plus'
  );
  const [token, setToken] = useState('');

  return (
    <PrivateChannelPanel
      status={status}
      error={error}
      pendingAction={null}
      channelLabel={label}
      channelAudience={audience}
      channelAudienceOptions={[
        { value: 'invite_only', label: 'Invite only' },
        { value: 'friend_only', label: 'Friends' },
        { value: 'friend_plus', label: 'Friends+' },
      ]}
      inviteTokenInput={token}
      onChannelLabelChange={setLabel}
      onChannelAudienceChange={setAudience}
      onInviteTokenChange={setToken}
      onCreateChannel={(event) => event.preventDefault()}
      onJoin={(event) => event.preventDefault()}
    />
  );
}

export const Ready: Story = {
  args: STORY_ARGS,
  render: (args) => (
    <ChannelStory
      status={args.status}
      error={args.error}
    />
  ),
};

export const ErrorState: Story = {
  args: {
    ...STORY_ARGS,
    status: 'error',
    error: 'private channel refresh failed',
  },
  render: (args) => (
    <ChannelStory
      status={args.status}
      error={args.error}
    />
  ),
};

export const InviteOutputState: Story = {
  args: {
    ...STORY_ARGS,
  },
  render: () => (
    <PrivateChannelSettingsPanel
      error={null}
      pendingAction={null}
      channel={BASE_CHANNELS[0].channel}
      inviteOutput='share:kukuri:topic:demo:channel-1'
      inviteOutputLabel='share'
      onShare={() => undefined}
    />
  ),
};

// Issue #966: 参加済み一覧付きの作成・参加 Dialog と、設定 Dialog の理由表示。
export const WithJoinedChannels: Story = {
  args: STORY_ARGS,
  render: () => (
    <PrivateChannelPanel
      {...STORY_ARGS}
      channelLabel=''
      channelAudience='invite_only'
      joinedChannels={BASE_CHANNELS.map((item) => item.channel)}
      onSelectJoinedChannel={() => undefined}
      onOpenJoinedChannelSettings={() => undefined}
    />
  ),
};

// Issue #1218 AC-4d: 一覧に続きがあるときの「さらに表示」(押すと読み込み中、失敗で 1 行のエラー)。
export const WithMoreJoinedChannels: Story = {
  args: STORY_ARGS,
  render: () => (
    <PrivateChannelPanel
      {...STORY_ARGS}
      channelLabel=''
      channelAudience='invite_only'
      joinedChannels={BASE_CHANNELS.map((item) => item.channel)}
      onSelectJoinedChannel={() => undefined}
      onOpenJoinedChannelSettings={() => undefined}
      onLoadMoreJoinedChannels={() => new Promise((_, reject) => setTimeout(() => reject(new Error('offline')), 800))}
    />
  ),
};

export const SettingsOwnerOnlyBlocked: Story = {
  args: STORY_ARGS,
  render: () => (
    <PrivateChannelSettingsPanel
      error={null}
      pendingAction={null}
      channel={{ ...BASE_CHANNELS[0].channel, audience_kind: 'friend_only', is_owner: false }}
      inviteOutput={null}
      inviteOutputLabel='grant'
      onShare={() => undefined}
    />
  ),
};

export const SettingsRotationRequired: Story = {
  args: STORY_ARGS,
  render: () => (
    <PrivateChannelSettingsPanel
      error={null}
      pendingAction={null}
      channel={{
        ...BASE_CHANNELS[0].channel,
        audience_kind: 'friend_only',
        rotation_required: true,
        stale_participant_count: 1,
      }}
      inviteOutput={null}
      inviteOutputLabel='grant'
      onShare={() => undefined}
    />
  ),
};

export const SettingsPending: Story = {
  args: STORY_ARGS,
  render: () => (
    <PrivateChannelSettingsPanel
      error={null}
      pendingAction='share'
      channel={BASE_CHANNELS[0].channel}
      inviteOutput={null}
      inviteOutputLabel='share'
      onShare={() => undefined}
    />
  ),
};

// #1219 AC-4: 共有リンクの作成と新しいアクセスの配布を行う端末の案内(owner の端末だけ)。
function controllerSettings(
  controller: PrivateChannelListItemView['channel']['controller'],
  result: PrivateChannelControllerTake = 'not_connected',
  pendingAction: PrivateChannelPendingAction = null
) {
  return (
    <PrivateChannelSettingsPanel
      error={null}
      pendingAction={pendingAction}
      channel={{ ...BASE_CHANNELS[0].channel, controller }}
      inviteOutput={null}
      inviteOutputLabel='share'
      onShare={() => undefined}
      onTakeController={() => Promise.resolve(result)}
    />
  );
}

const TAKE_BUTTON = /Do this on this device|この端末で行う|在此设备上进行/;

export const SettingsOtherDevice: Story = {
  args: STORY_ARGS,
  render: () => controllerSettings('other_device'),
};

export const SettingsTaking: Story = {
  args: STORY_ARGS,
  render: () => controllerSettings('other_device', 'taken', 'take'),
};

export const SettingsTakeNotConnected: Story = {
  args: STORY_ARGS,
  render: () => controllerSettings('other_device', 'not_connected'),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(canvas.getByRole('button', { name: TAKE_BUTTON }));
    await expect(await canvas.findByRole('status')).toBeVisible();
  },
};

export const SettingsTakeWaiting: Story = {
  args: STORY_ARGS,
  render: () => controllerSettings('other_device', 'waiting'),
  play: SettingsTakeNotConnected.play,
};

export const SettingsMoving: Story = {
  args: STORY_ARGS,
  render: () => controllerSettings('moving'),
};
