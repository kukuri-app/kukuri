/**
 * WP-S5: useDesktopShellViewModels の characterization テスト。
 *
 * 後続 WP-H6(shell store のスライス化・selector 化・prop drilling 除去)の
 * 安全網として「現時点で観測した挙動」をそのまま固定する。
 * このテストが落ちた場合、疑うべきは加えた変更でありテストではない。
 *
 * 方針:
 * - t / translate は key をそのまま返す stub を注入し、locale リソースの変更で
 *   割れないよう文言ではなくキー/構造で assert する。
 * - 返り値 52 キーの全量比較・snapshot・参照同一性 assert は書かない
 *   (H6 のリファクタで無意味に割れるため)。キー/フィールド単位の値のみ固定する。
 */
import { renderHook } from '@testing-library/react';
import { beforeEach, describe, expect, test } from 'vitest';

import type {
  AuthorSocialView,
  DirectMessageConversationView,
  DirectMessageStatusView,
  JoinedPrivateChannelView,
  PostView,
  TopicSyncStatus,
} from '@/lib/api';
import type { DesktopShellState } from '@/shell/store';
import { useDesktopShellViewModels } from '@/shell/useDesktopShellViewModels';
import {
  actPatchState,
  createShellHookHarness,
  resetWindowHash,
} from '@/shell/testSupport/renderShellHook';
import { columnIdentityId, openTransientColumn } from '@/shell/slices/workspace';

const SELF_PUBKEY = 'f'.repeat(64);
const AUTHOR_A_PUBKEY = 'a'.repeat(64);
const AUTHOR_B_PUBKEY = 'b'.repeat(64);
const DM_PEER_PUBKEY = 'd'.repeat(64);
const UNKNOWN_PEER_PUBKEY = 'e'.repeat(64);

// key をそのまま返す stub。文言 assert を locale リソースから切り離す。
const stubTranslate = (key: string) => key;

function buildPost(overrides: Partial<PostView> = {}): PostView {
  return {
    object_id: 'post-1',
    envelope_id: 'envelope-post-1',
    author_pubkey: AUTHOR_A_PUBKEY,
    author_name: null,
    author_display_name: null,
    following: false,
    followed_by: false,
    mutual: false,
    friend_of_friend: false,
    object_kind: 'post',
    is_threadable: true,
    content: 'hello world',
    content_status: 'Available',
    attachments: [],
    created_at: 1,
    reply_to: null,
    root_id: 'post-1',
    channel_id: null,
    audience_label: 'Public',
    ...overrides,
  };
}

function buildAuthor(
  pubkey: string,
  overrides: Partial<AuthorSocialView> = {}
): AuthorSocialView {
  return {
    author_pubkey: pubkey,
    name: null,
    display_name: null,
    about: null,
    picture_asset: null,
    updated_at: null,
    following: false,
    followed_by: false,
    mutual: false,
    friend_of_friend: false,
    friend_of_friend_via_pubkeys: [],
    muted: false,
    blocking: false,
    blocked_by: false,
    ...overrides,
  };
}

function buildJoinedChannel(
  channelId: string,
  label: string,
  overrides: Partial<JoinedPrivateChannelView> = {}
): JoinedPrivateChannelView {
  return {
    topic_id: 'kukuri:topic:general',
    channel_id: channelId,
    label,
    creator_pubkey: 'c'.repeat(64),
    owner_pubkey: 'c'.repeat(64),
    joined_via_pubkey: null,
    audience_kind: 'invite_only',
    is_owner: false,
    current_epoch_id: 'epoch-1',
    archived_epoch_ids: [],
    sharing_state: 'open',
    rotation_required: false,
    participant_count: 2,
    stale_participant_count: 0,
    ...overrides,
  };
}

function buildTopicDiagnostic(
  topic: string,
  overrides: Partial<TopicSyncStatus> = {}
): TopicSyncStatus {
  return {
    topic,
    joined: true,
    delivery_state: 'Live',
    peer_count: 0,
    connected_peers: [],
    docs_assist_peer_ids: [],
    configured_peer_ids: [],
    missing_peer_ids: [],
    active_path: 'direct_p2p',
    rendezvous_peer_ids: [],
    fallback_peer_ids: [],
    last_received_at: null,
    last_docs_activity_at: null,
    status_detail: '',
    last_error: null,
    ...overrides,
  };
}

function buildDmStatus(
  peerPubkey: string,
  overrides: Partial<DirectMessageStatusView> = {}
): DirectMessageStatusView {
  return {
    peer_pubkey: peerPubkey,
    dm_id: 'dm-1',
    mutual: true,
    send_enabled: false,
    peer_count: 1,
    pending_outbox_count: 2,
    ...overrides,
  };
}

function buildConversation(
  peerPubkey: string,
  overrides: Partial<DirectMessageConversationView> = {}
): DirectMessageConversationView {
  return {
    dm_id: 'dm-1',
    peer_pubkey: peerPubkey,
    peer_name: 'dave',
    peer_display_name: 'Dave',
    peer_picture_asset: null,
    updated_at: 10,
    last_message_at: null,
    last_message_id: null,
    last_message_preview: null,
    status: buildDmStatus(peerPubkey),
    ...overrides,
  };
}

// store プリセット → renderHook → 即 assert の型。preset は現在の state を受け取り
// patch を返す(syncStatus 等ネストの深いフィールドを spread で部分更新するため)。
function renderViewModels(
  preset?: (current: DesktopShellState) => Partial<DesktopShellState>,
  options: {
    locale?: 'en' | 'ja' | 'zh-CN';
    t?: (key: string) => string;
  } = {}
) {
  const harness = createShellHookHarness();
  if (preset) {
    harness.store.getState().patchState(preset(harness.store.getState()));
  }
  const rendered = renderHook(
    () =>
      useDesktopShellViewModels({
        t: options.t ?? stubTranslate,
        translate: options.t ?? stubTranslate,
        locale: options.locale ?? 'en',
        theme: 'dark',
        profileAvatarPreviewUrl: null,
      }),
    { wrapper: harness.wrapper }
  );
  return { ...rendered, store: harness.store };
}

beforeEach(() => {
  resetWindowHash();
});

describe('useDesktopShellViewModels', () => {
  test('localizes transport status details and discovery assist-peer labels for Japanese UI', () => {
    const translations: Record<string, string> = {
      'settings:connectivity.diagnostics.connectionDetail': '接続詳細',
      'settings:connectivity.statusDetails.noPeersConfigured': 'ピアが設定されていません',
      'settings:connectivity.statusDetails.topicNoPeersConfigured':
        'このトピックにはピアが設定されていません',
      'settings:discovery.diagnostics.docsAssistPeers': 'ドキュメント補助ピア',
      'settings:discovery.diagnostics.blobAssistPeers': '添付補助ピア',
    };
    const t = (key: string) => translations[key] ?? key;
    const view = renderViewModels(
      (current) => ({
        syncStatus: {
          ...current.syncStatus,
          status_detail: 'No peers configured',
          topic_diagnostics: [
            buildTopicDiagnostic('kukuri:topic:general', {
              status_detail: 'No peers configured for this topic',
            }),
          ],
        },
      }),
      { locale: 'ja', t }
    );

    expect(
      view.result.current.connectivityPanelView.diagnostics.find(
        (item) => item.label === '接続詳細'
      )?.value
    ).toBe('ピアが設定されていません');
    expect(view.result.current.connectivityPanelView.topics[0]?.statusDetail).toBe(
      'このトピックにはピアが設定されていません'
    );
    expect(view.result.current.discoveryPanelView.diagnostics.map((item) => item.label)).toEqual(
      expect.arrayContaining(['ドキュメント補助ピア', '添付補助ピア'])
    );

    view.unmount();
  });

  test('builds an image post card whose media state follows mediaObjectUrls availability', () => {
    const imageHash = '1'.repeat(64);
    const post = buildPost({
      object_id: 'image-post-1',
      root_id: 'image-post-1',
      attachments: [
        {
          hash: imageHash,
          mime: 'image/png',
          bytes: 2048,
          role: 'image_original',
          status: 'Missing',
          provenance: {
            canonical_source: 'blob',
            observed_via: [
              {
                node_base_url: 'https://node.example',
                capability: 'community_index',
                observed_at: 123,
              },
            ],
          },
        },
        // primary image でも video 系でもない添付は extraAttachmentCount に数えられる
        {
          hash: '2'.repeat(64),
          mime: 'application/pdf',
          bytes: 512,
          role: 'document',
          status: 'Missing',
        },
      ],
    });
    const view = renderViewModels(() => ({
      // 既定の activeTopic='kukuri:topic:general' + public scope の storage key
      timelinesByKey: { 'kukuri:topic:general::public': [post] },
      // #1056: 照会フックを起動しない構成なので、照会先なし(確定)として扱う。
      timelineAdvisoryLookup: { active: false, settled: {} },
    }));

    const card = view.result.current.activeTimelinePostViews[0];
    expect(view.result.current.activeTimelinePostViews).toHaveLength(1);
    expect(card.media.kind).toBe('image');
    // object url 未取得の間は loading / preview なし
    expect(card.media.state).toBe('loading');
    expect(card.media.imagePreviewSrc).toBeNull();
    expect(card.media.extraAttachmentCount).toBe(1);
    expect(card.media.metaMime).toBe('image/png');
    // gallery は image mime のみ(video_poster 以外)。src は object url 未取得なら null
    expect(card.media.imageGalleryItems).toEqual([
      {
        hash: imageHash,
        src: null,
        failed: false,
        retrying: false,
        mime: 'image/png',
        provenance: {
          canonicalSource: 'blob',
          observedVia: [
            {
              nodeBaseUrl: 'https://node.example',
              capability: 'community_index',
              observedAt: 123,
            },
          ],
          responsibleReportTargets: [],
        },
      },
    ]);
    expect(card.media.currentImageIndex).toBe(0);

    actPatchState(view.store, {
      mediaObjectUrls: { [imageHash]: null },
    });

    const unavailableCard = view.result.current.activeTimelinePostViews[0];
    expect(unavailableCard.media.state).toBe('unavailable');

    actPatchState(view.store, {
      mediaObjectUrls: { [imageHash]: 'blob:image-preview-1' },
    });

    const readyCard = view.result.current.activeTimelinePostViews[0];
    expect(readyCard.media.state).toBe('ready');
    expect(readyCard.media.imagePreviewSrc).toBe('blob:image-preview-1');
    expect(readyCard.media.imageGalleryItems).toEqual([
      {
        hash: imageHash,
        src: 'blob:image-preview-1',
        failed: false,
        retrying: false,
        mime: 'image/png',
        provenance: {
          canonicalSource: 'blob',
          observedVia: [
            {
              nodeBaseUrl: 'https://node.example',
              capability: 'community_index',
              observedAt: 123,
            },
          ],
          responsibleReportTargets: [],
        },
      },
    ]);

    view.unmount();
  });

  test('re-targets thread navigation of a non-quote repost to the repost source', () => {
    const repostOfBase = {
      source_object_id: 'source-post-1',
      source_topic_id: 'kukuri:topic:source',
      source_author_pubkey: AUTHOR_B_PUBKEY,
      source_author_name: 'bob',
      source_author_display_name: 'Bob Display',
      source_author_picture_asset: null,
      source_object_kind: 'post',
      content: 'original content',
      attachments: [],
      reply_to: null,
      root_id: 'source-root-1',
    };
    const repostWithRoot = buildPost({
      object_id: 'repost-1',
      root_id: 'repost-1',
      object_kind: 'repost',
      repost_commentary: null,
      // 実 API が is_threadable を省略した場合の fallback 分岐
      // (non-quote repost は canReply=false)を固定する
      is_threadable: undefined as unknown as boolean,
      content: '',
      repost_of: { ...repostOfBase },
    });
    const repostWithoutRoot = buildPost({
      object_id: 'repost-2',
      root_id: 'repost-2',
      object_kind: 'repost',
      repost_commentary: null,
      // wire 実態の直値 false(fallback を経ない分岐)も固定する
      is_threadable: false,
      content: '',
      repost_of: { ...repostOfBase, root_id: null },
    });
    const quoteRepost = buildPost({
      object_id: 'repost-3',
      root_id: null,
      object_kind: 'repost',
      repost_commentary: 'my take',
      is_threadable: undefined as unknown as boolean,
      content: 'my take',
      repost_of: { ...repostOfBase },
    });
    const view = renderViewModels(() => ({
      timelinesByKey: {
        'kukuri:topic:general::public': [repostWithRoot, repostWithoutRoot, quoteRepost],
      },
      // repost_of に asset が無い場合は knownAuthors から解決される
      knownAuthorsByPubkey: {
        [AUTHOR_B_PUBKEY]: buildAuthor(AUTHOR_B_PUBKEY, {
          picture_asset: {
            hash: 'avatar-bob',
            mime: 'image/png',
            bytes: 42,
            role: 'profile_avatar',
          },
        }),
      },
      mediaObjectUrls: { 'avatar-bob': 'blob:avatar-bob' },
    }));

    const [withRootCard, withoutRootCard, quoteCard] =
      view.result.current.activeTimelinePostViews;
    // non-quote repost: thread 遷移先は repost_of.root_id、topic は source_topic_id
    expect(withRootCard.threadTargetId).toBe('source-root-1');
    expect(withRootCard.threadTopicId).toBe('kukuri:topic:source');
    expect(withRootCard.canReply).toBe(false);
    expect(withRootCard.canRepost).toBe(false);
    expect(withRootCard.repostSourceAuthor).toEqual({
      pubkey: AUTHOR_B_PUBKEY,
      label: 'Bob Display',
      picture: 'blob:avatar-bob',
    });
    // repost_of.root_id が無い場合は source_object_id に fallback
    expect(withoutRootCard.threadTargetId).toBe('source-post-1');
    // is_threadable: false の直値はそのまま canReply=false になる
    expect(withoutRootCard.canReply).toBe(false);
    // quote repost は自分自身の thread に遷移し返信可能
    expect(quoteCard.threadTargetId).toBe('repost-3');
    expect(quoteCard.threadTopicId).toBeNull();
    expect(quoteCard.canReply).toBe(true);

    view.unmount();
  });

  test('builds topicNavItems from diagnostics and exposes joined channels for every topic', () => {
    const view = renderViewModels((current) => ({
      syncStatusRead: { loaded: true, refreshing: false, error: false },
      syncStatus: {
        ...current.syncStatus,
        topic_diagnostics: [
          buildTopicDiagnostic('kukuri:topic:general', {
            peer_count: 3,
            connected_peers: ['peer-1', 'peer-2', 'peer-3'],
          }),
        ],
        gossip_disabled_topics: ['kukuri:topic:dev'],
        gossip_disabled_channels: ['kukuri:topic:general::channel-alpha'],
      },
      joinedChannelsByTopic: {
        ...current.joinedChannelsByTopic,
        'kukuri:topic:general': [
          buildJoinedChannel('channel-alpha', 'Alpha Channel'),
          buildJoinedChannel('channel-beta', 'Beta Channel'),
        ],
        'kukuri:topic:dev': [
          buildJoinedChannel('channel-dev', 'Dev Channel', {
            topic_id: 'kukuri:topic:dev',
          }),
        ],
      },
      workspaceState: openTransientColumn(current.workspaceState, {
        id: columnIdentityId('timeline', {
          topicId: 'kukuri:topic:general',
          channelId: 'channel-alpha',
        }),
        kind: 'timeline',
        scope: { topicId: 'kukuri:topic:general', channelId: 'channel-alpha' },
        pinned: false,
      }),
    }));

    const items = view.result.current.topicNavItems;
    // starter topics 3 件がそのまま nav item になる
    expect(items.map((item) => item.topic)).toEqual([
      'kukuri:topic:general',
      'kukuri:topic:dev',
      'kukuri:topic:test',
    ]);

    const generalItem = items[0];
    expect(generalItem.active).toBe(true);
    // channel 選択中は public 行は非アクティブ
    expect(generalItem.publicActive).toBe(false);
    expect(generalItem.removable).toBe(true);
    // Control Center and settings share the current connection explanation.
    expect(generalItem.connectionLabel).toBe('settings:connectionGuidance.states.live · settings:diagnostics.values.path.direct_p2p');
    expect(generalItem.peerCount).toBe(3);
    expect(generalItem.gossipJoined).toBe(true);
    // `topic::channel_id` が gossip_disabled_channels にあれば
    // channel 単位で gossipJoined=false
    expect(generalItem.channels).toEqual([
      {
        channelId: 'channel-alpha',
        label: 'Alpha Channel',
        audienceKind: 'invite_only',
        active: true,
        gossipJoined: false,
      },
      {
        channelId: 'channel-beta',
        label: 'Beta Channel',
        audienceKind: 'invite_only',
        active: false,
        gossipJoined: true,
      },
    ]);

    const devItem = items[1];
    expect(devItem.active).toBe(false);
    // A paused topic with no diagnostic has no confirmed peer count.
    expect(devItem.connectionLabel).toBe('settings:connectionGuidance.states.paused');
    expect(devItem.peerCount).toBeNull();
    // gossip_disabled_topics に載っている topic は gossipJoined=false
    expect(devItem.gossipJoined).toBe(false);
    // Control Center の検索と直接選択のため、inactive topic の joined channel も公開する。
    expect(devItem.channels).toEqual([
      {
        channelId: 'channel-dev',
        label: 'Dev Channel',
        audienceKind: 'invite_only',
        active: false,
        gossipJoined: false,
      },
    ]);

    view.unmount();
  });

  test('resolves compose channel and audience from the active scope target', () => {
    const view = renderViewModels((current) => ({
      composeChannelByTopic: {
        ...current.composeChannelByTopic,
        'kukuri:topic:general': {
          kind: 'private_channel',
          channel_id: 'channel-alpha',
        },
      },
      joinedChannelsByTopic: {
        ...current.joinedChannelsByTopic,
        'kukuri:topic:general': [buildJoinedChannel('channel-alpha', 'Alpha Channel')],
      },
    }));

    expect(view.result.current.activeComposeChannel).toEqual({
      kind: 'private_channel',
      channel_id: 'channel-alpha',
    });
    expect(view.result.current.activeComposeAudienceLabel).toBe('Alpha Channel');

    view.unmount();
  });

  test('falls back selectedDirectMessageStatus from statusByPeer to conversation.status to null', () => {
    const view = renderViewModels(() => ({
      directMessages: [buildConversation(DM_PEER_PUBKEY)],
      selectedDirectMessagePeerPubkey: DM_PEER_PUBKEY,
    }));

    // statusByPeer 未取得の間は conversation.status
    expect(view.result.current.selectedDirectMessageConversation?.dm_id).toBe('dm-1');
    expect(view.result.current.selectedDirectMessageStatus?.send_enabled).toBe(false);
    expect(view.result.current.selectedDirectMessageStatus?.pending_outbox_count).toBe(2);
    // peer_display_name > peer_name の順で label 解決
    expect(view.result.current.selectedDirectMessagePeerLabel).toBe('Dave');

    // statusByPeer が入ると conversation.status より優先
    actPatchState(view.store, {
      directMessageStatusByPeer: {
        [DM_PEER_PUBKEY]: buildDmStatus(DM_PEER_PUBKEY, {
          send_enabled: true,
          pending_outbox_count: 0,
        }),
      },
    });
    expect(view.result.current.selectedDirectMessageStatus?.send_enabled).toBe(true);
    expect(view.result.current.selectedDirectMessageStatus?.pending_outbox_count).toBe(0);

    // conversation が見つからない peer を選ぶと null に fallback
    actPatchState(view.store, {
      selectedDirectMessagePeerPubkey: UNKNOWN_PEER_PUBKEY,
    });
    expect(view.result.current.selectedDirectMessageConversation).toBeNull();
    expect(view.result.current.selectedDirectMessageStatus).toBeNull();
    expect(view.result.current.selectedDirectMessagePeerLabel).toBeNull();
    expect(view.result.current.selectedDirectMessageTimeline).toEqual([]);

    view.unmount();
  });

  test('merges mentionCandidates from following + knownAuthors excluding self and duplicates', () => {
    const view = renderViewModels((current) => ({
      syncStatus: {
        ...current.syncStatus,
        local_author_pubkey: SELF_PUBKEY,
      },
      socialConnections: {
        following: [
          buildAuthor(AUTHOR_A_PUBKEY, {
            display_name: 'Alice',
            about: 'alice about',
            following: true,
          }),
          // 自分自身は following に居ても候補から除外される
          buildAuthor(SELF_PUBKEY, { display_name: 'Me', following: true }),
        ],
        followed: [],
        muted: [],
        blocking: [],
      },
      knownAuthorsByPubkey: {
        // following と重複する author は following 側が勝つ(先勝ち)
        [AUTHOR_A_PUBKEY]: buildAuthor(AUTHOR_A_PUBKEY, {
          display_name: 'Alice Known',
        }),
        [AUTHOR_B_PUBKEY]: buildAuthor(AUTHOR_B_PUBKEY, {
          name: 'bob',
          picture_asset: {
            hash: 'avatar-bob',
            mime: 'image/png',
            bytes: 42,
            role: 'profile_avatar',
          },
        }),
        [SELF_PUBKEY]: buildAuthor(SELF_PUBKEY, { display_name: 'Me' }),
      },
      mediaObjectUrls: { 'avatar-bob': 'blob:avatar-bob' },
    }));

    const candidates = view.result.current.mentionCandidates;
    expect(candidates.map((candidate) => candidate.pubkey)).toEqual([
      AUTHOR_A_PUBKEY,
      AUTHOR_B_PUBKEY,
    ]);
    expect(candidates[0]).toEqual({
      pubkey: AUTHOR_A_PUBKEY,
      label: 'Alice',
      displayName: 'Alice',
      name: null,
      about: 'alice about',
      picture: null,
    });
    // display_name が無ければ name が label になる
    expect(candidates[1]).toEqual({
      pubkey: AUTHOR_B_PUBKEY,
      label: 'bob',
      displayName: null,
      name: 'bob',
      about: null,
      picture: 'blob:avatar-bob',
    });

    view.unmount();
  });
});

describe('createShellHookHarness', () => {
  // T4-T7 が依存するハーネス契約: hash は store 生成前に反映される
  // (createInitialShellState が window.location.hash を直読するため)。
  test('applies the hash option before creating the store so initial state is seeded from it', () => {
    const harness = createShellHookHarness({
      hash: '#/timeline?topic=kukuri%3Atopic%3Ageneral&settings=appearance',
    });

    expect(window.location.hash).toBe(
      '#/timeline?topic=kukuri%3Atopic%3Ageneral&settings=appearance'
    );
    const state = harness.store.getState();
    expect(state.shellChromeState.activeSettingsSection).toBe('appearance');
    expect(state.shellChromeState.settingsOpen).toBe(true);
  });
});
