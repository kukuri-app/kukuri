import {
  type ChannelAccessTokenExport,
  type ChannelAccessTokenPreview,
  type ChannelAudienceKind,
  type DesktopApi,
  type FriendOnlyGrantPreview,
  type FriendPlusSharePreview,
  type PrivateChannelInvitePreview,
} from '@/lib/api';

import { parseMockChannelAccessTokenPreview, withJoinedChannelDefaults } from '../desktopMockModel';
import { type MockRuntime } from '../mockRuntime';

const JOINED_CHANNEL_PAGE = 128;

type ChannelsMock = Pick<
  DesktopApi,
  | 'createPrivateChannel'
  | 'exportPrivateChannelInvite'
  | 'importPrivateChannelInvite'
  | 'exportChannelAccessToken'
  | 'previewChannelAccessToken'
  | 'importChannelAccessToken'
  | 'exportFriendOnlyGrant'
  | 'importFriendOnlyGrant'
  | 'exportFriendPlusShare'
  | 'importFriendPlusShare'
  | 'freezePrivateChannel'
  | 'rotatePrivateChannel'
  | 'setPrivateChannelEntryDome'
  | 'leavePrivateChannel'
  | 'listJoinedPrivateChannels'
>;

export function createChannelsMock(runtime: MockRuntime): ChannelsMock {
  const { options, joinedChannelsByTopic, syncStatus } = runtime;

  return {
    async createPrivateChannel(
      topic,
      label,
      audienceKind: ChannelAudienceKind = 'invite_only'
    ) {
      runtime.sequence += 1;
      const channelId = `channel-${runtime.sequence}`;
      const channel = withJoinedChannelDefaults({
        topic_id: topic,
        channel_id: channelId,
        label,
        creator_pubkey: syncStatus.local_author_pubkey,
        owner_pubkey: syncStatus.local_author_pubkey,
        audience_kind: audienceKind,
        is_owner: true,
        current_epoch_id: `epoch-${runtime.sequence}`,
        archived_epoch_ids: [],
        sharing_state: 'open',
        rotation_required: false,
        participant_count: 1,
        stale_participant_count: 0,
      });
      joinedChannelsByTopic[topic] = [...(joinedChannelsByTopic[topic] ?? []), channel];
      return channel;
    },
    async exportPrivateChannelInvite(topic, channelId) {
      return `invite:${topic}:${channelId}`;
    },
    async importPrivateChannelInvite() {
      const preview: PrivateChannelInvitePreview = options?.invitePreview ?? {
        channel_id: 'channel-imported',
        topic_id: 'kukuri:topic:general',
        channel_label: 'Imported',
        inviter_pubkey: syncStatus.local_author_pubkey,
        owner_pubkey: syncStatus.local_author_pubkey,
        epoch_id: 'epoch-imported-1',
        expires_at: null,
        namespace_secret_hex: 'a'.repeat(64),
      };
      joinedChannelsByTopic[preview.topic_id] = [
        ...(joinedChannelsByTopic[preview.topic_id] ?? []),
        withJoinedChannelDefaults({
          topic_id: preview.topic_id,
          channel_id: preview.channel_id,
          label: preview.channel_label,
          creator_pubkey: preview.inviter_pubkey,
          owner_pubkey: preview.owner_pubkey,
          audience_kind: 'invite_only',
          is_owner: false,
          current_epoch_id: preview.epoch_id,
          archived_epoch_ids: [],
          sharing_state: 'open',
          rotation_required: false,
          stale_participant_count: 0,
        }),
      ];
      return preview;
    },
    async exportChannelAccessToken(topic, channelId) {
      const channel = (joinedChannelsByTopic[topic] ?? []).find(
        (item) => item.channel_id === channelId
      );
      if (!channel) {
        throw new Error('private channel is not joined');
      }
      const kind =
        channel.audience_kind === 'invite_only'
          ? 'invite'
          : channel.audience_kind === 'friend_only'
            ? 'grant'
            : 'share';
      return {
        kind,
        token: `${kind}:${topic}:${channelId}`,
      } satisfies ChannelAccessTokenExport;
    },
    async previewChannelAccessToken(token) {
      return parseMockChannelAccessTokenPreview(token, options ?? {}, syncStatus.local_author_pubkey);
    },
    async importChannelAccessToken(token) {
      const preview = parseMockChannelAccessTokenPreview(token, options ?? {}, syncStatus.local_author_pubkey);
      if (preview.kind === 'grant') {
        const preview = await this.importFriendOnlyGrant(token);
        return {
          kind: 'grant',
          topic_id: preview.topic_id,
          channel_id: preview.channel_id,
          channel_label: preview.channel_label,
          owner_pubkey: preview.owner_pubkey,
          inviter_pubkey: null,
          sponsor_pubkey: preview.owner_pubkey,
          epoch_id: preview.epoch_id,
        } satisfies ChannelAccessTokenPreview;
      }
      if (preview.kind === 'share') {
        const preview = await this.importFriendPlusShare(token);
        return {
          kind: 'share',
          topic_id: preview.topic_id,
          channel_id: preview.channel_id,
          channel_label: preview.channel_label,
          owner_pubkey: preview.owner_pubkey,
          inviter_pubkey: null,
          sponsor_pubkey: preview.sponsor_pubkey,
          epoch_id: preview.epoch_id,
        } satisfies ChannelAccessTokenPreview;
      }
      const invitePreview = await this.importPrivateChannelInvite(token);
      return {
        kind: 'invite',
        topic_id: invitePreview.topic_id,
        channel_id: invitePreview.channel_id,
        channel_label: invitePreview.channel_label,
        owner_pubkey: invitePreview.owner_pubkey,
        inviter_pubkey: invitePreview.inviter_pubkey,
        sponsor_pubkey: null,
        epoch_id: invitePreview.epoch_id,
      } satisfies ChannelAccessTokenPreview;
    },
    async exportFriendOnlyGrant(topic, channelId) {
      return `grant:${topic}:${channelId}`;
    },
    async importFriendOnlyGrant() {
      const preview: FriendOnlyGrantPreview = {
        channel_id: 'channel-friends',
        topic_id: 'kukuri:topic:general',
        channel_label: 'Friends',
        owner_pubkey: syncStatus.local_author_pubkey,
        epoch_id: 'epoch-1',
        expires_at: null,
        namespace_secret_hex: 'b'.repeat(64),
      };
      joinedChannelsByTopic[preview.topic_id] = [
        ...(joinedChannelsByTopic[preview.topic_id] ?? []),
        withJoinedChannelDefaults({
          topic_id: preview.topic_id,
          channel_id: preview.channel_id,
          label: preview.channel_label,
          creator_pubkey: preview.owner_pubkey,
          owner_pubkey: preview.owner_pubkey,
          audience_kind: 'friend_only',
          is_owner: false,
          current_epoch_id: preview.epoch_id,
          archived_epoch_ids: [],
          sharing_state: 'open',
          rotation_required: false,
          stale_participant_count: 0,
        }),
      ];
      return preview;
    },
    async exportFriendPlusShare(topic, channelId) {
      return `share:${topic}:${channelId}`;
    },
    async importFriendPlusShare() {
      const preview: FriendPlusSharePreview = {
        channel_id: 'channel-friends-plus',
        topic_id: 'kukuri:topic:general',
        channel_label: 'Friends+',
        owner_pubkey: syncStatus.local_author_pubkey,
        sponsor_pubkey: 'sponsor-pubkey-1234',
        epoch_id: 'epoch-plus-1',
        expires_at: null,
        namespace_secret_hex: 'c'.repeat(64),
        share_token_id: 'share-token-1',
      };
      joinedChannelsByTopic[preview.topic_id] = [
        ...(joinedChannelsByTopic[preview.topic_id] ?? []),
        withJoinedChannelDefaults({
          topic_id: preview.topic_id,
          channel_id: preview.channel_id,
          label: preview.channel_label,
          creator_pubkey: preview.owner_pubkey,
          owner_pubkey: preview.owner_pubkey,
          joined_via_pubkey: preview.sponsor_pubkey,
          audience_kind: 'friend_plus',
          is_owner: false,
          current_epoch_id: preview.epoch_id,
          archived_epoch_ids: [],
          sharing_state: 'open',
          rotation_required: false,
          stale_participant_count: 0,
        }),
      ];
      return preview;
    },
    async freezePrivateChannel(topic, channelId) {
      const channels = joinedChannelsByTopic[topic] ?? [];
      const next = channels.map((channel) =>
        channel.channel_id === channelId
          ? withJoinedChannelDefaults({ ...channel, sharing_state: 'frozen' })
          : channel
      );
      joinedChannelsByTopic[topic] = next;
      return next.find((channel) => channel.channel_id === channelId)!;
    },
    async rotatePrivateChannel(topic, channelId) {
      const channels = joinedChannelsByTopic[topic] ?? [];
      const next = channels.map((channel) =>
        channel.channel_id === channelId
          ? withJoinedChannelDefaults({
              ...channel,
              current_epoch_id: `${channel.current_epoch_id}-rotated`,
              archived_epoch_ids: [...channel.archived_epoch_ids, channel.current_epoch_id],
              rotation_required: false,
              stale_participant_count: 0,
            })
          : channel
      );
      joinedChannelsByTopic[topic] = next;
      return next.find((channel) => channel.channel_id === channelId)!;
    },
    async setPrivateChannelEntryDome(topic, channelId, entryDomeInstanceId) {
      const channels = joinedChannelsByTopic[topic] ?? [];
      const next = channels.map((channel) =>
        channel.channel_id === channelId
          ? withJoinedChannelDefaults({
              ...channel,
              entry_dome_instance_id: entryDomeInstanceId,
            })
          : channel
      );
      joinedChannelsByTopic[topic] = next;
      return next.find((channel) => channel.channel_id === channelId)!;
    },
    async leavePrivateChannel(topic, channelId) {
      joinedChannelsByTopic[topic] = (joinedChannelsByTopic[topic] ?? []).filter(
        (channel) => channel.channel_id !== channelId
      );
    },
    // runtime と同じく channel id の順に 128 件の page を返す。cursor は page の最後の channel id。
    async listJoinedPrivateChannels(topic, cursor) {
      const items = (joinedChannelsByTopic[topic] ?? [])
        .filter((channel) => !cursor || channel.channel_id > cursor)
        .sort((left, right) => (left.channel_id < right.channel_id ? -1 : 1))
        .slice(0, JOINED_CHANNEL_PAGE);
      const last = items.at(-1);
      return {
        items,
        next_cursor: items.length === JOINED_CHANNEL_PAGE && last ? last.channel_id : null,
      };
    },
  };
}
