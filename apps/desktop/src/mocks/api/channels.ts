import {
  type ChannelAccessTokenExport,
  type ChannelAccessTokenPreview,
  type ChannelAudienceKind,
  type DesktopApi,
  type FriendOnlyGrantPreview,
  type FriendPlusSharePreview,
  type PrivateChannelInvitePreview,
  type PublicChannelView,
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
  | 'takePrivateChannelController'
  | 'setPrivateChannelEntryDome'
  | 'leavePrivateChannel'
  | 'listJoinedPrivateChannels'
  | 'createPublicChannel'
  | 'getPublicChannel'
  | 'listPublicChannels'
>;

const PUBLIC_CHANNEL_PAGE = 20;

export function createChannelsMock(runtime: MockRuntime): ChannelsMock {
  const { options, joinedChannelsByTopic, publicChannelsByTopic, syncStatus, liveSessionsByTopic, mutedAuthorPubkeys } =
    runtime;
  // native と同じく、行に配信中の印(非表示の主催者の Live は数えない。#1840 AC-4)を付ける。
  const liveNow = (channel: PublicChannelView) => {
    const muted = mutedAuthorPubkeys();
    return (liveSessionsByTopic[channel.topic_id] ?? []).some(
      (session) =>
        session.channel_id === channel.channel_id && session.status === 'Live' && !muted.has(session.host_pubkey)
    );
  };
  const withCreatorNames = (channel: PublicChannelView): PublicChannelView =>
    channel.creator_pubkey === syncStatus.local_author_pubkey
      ? {
          ...channel,
          ...(runtime.myProfile.name ? { creator_name: runtime.myProfile.name } : {}),
          ...(runtime.myProfile.display_name ? { creator_display_name: runtime.myProfile.display_name } : {}),
          live_now: liveNow(channel),
        }
      : { ...channel, live_now: liveNow(channel) };

  return {
    // 最初の投稿は投稿の mock（合成した 1 つの api の this）で作る。
    async createPublicChannel(this: DesktopApi, topic: string, title: string, firstPost: string) {
      const createdAt = Date.now();
      const channelId = `pc-${createdAt}-${syncStatus.local_author_pubkey.slice(0, 16)}`;
      // 最初の投稿は channel の投稿として先に作る（native と同じ順）。
      const firstPostId = firstPost.trim()
        ? await this.createPost(topic, firstPost, null, [], {
            kind: 'public_channel',
            channel_id: channelId,
          })
        : null;
      const channel: PublicChannelView = {
        channel_id: channelId,
        topic_id: topic,
        title: title.trim(),
        creator_pubkey: syncStatus.local_author_pubkey,
        created_at: createdAt,
        ...(firstPostId ? { first_post_id: firstPostId } : {}),
        last_post_at: createdAt,
        live_now: false,
      };
      publicChannelsByTopic[topic] = [...(publicChannelsByTopic[topic] ?? []), channel];
      return withCreatorNames(channel);
    },
    // 返す行は写し（native の IPC と同じく、後の投稿で変わる手元の行と同じ object を渡さない）。自分の行には
    // 自分の profile の名前を付ける（native は手元の profile から付ける）。
    async getPublicChannel(topic, channelId) {
      const channel = (publicChannelsByTopic[topic] ?? []).find((candidate) => candidate.channel_id === channelId);
      return channel ? withCreatorNames(channel) : null;
    },
    async listPublicChannels(topic, order, cursor = null, limit = PUBLIC_CHANNEL_PAGE) {
      const at = (channel: PublicChannelView) =>
        order === 'created' ? channel.created_at : channel.last_post_at;
      const sorted = [...(publicChannelsByTopic[topic] ?? [])].sort(
        (left, right) => at(right) - at(left) || right.channel_id.localeCompare(left.channel_id)
      );
      const after = cursor
        ? sorted.filter(
            (channel) =>
              at(channel) < cursor.at || (at(channel) === cursor.at && channel.channel_id < cursor.channel_id)
          )
        : sorted;
      const items = after.slice(0, limit).map(withCreatorNames);
      const last = items.at(-1);
      return {
        items,
        next_cursor:
          items.length === limit && last ? { at: at(last), channel_id: last.channel_id } : null,
        unavailable_count: 0,
      };
    },
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
    async takePrivateChannelController(topic, channelId) {
      joinedChannelsByTopic[topic] = (joinedChannelsByTopic[topic] ?? []).map((channel) =>
        channel.channel_id === channelId ? { ...channel, controller: 'this_device' } : channel
      );
      return 'taken';
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
