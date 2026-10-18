import type { Meta, StoryObj } from '@storybook/react-vite';
import { useEffect, useState } from 'react';

import type { DesktopApi, LiveSessionView, PostView, ReactionSummaryView } from '@/lib/api';
import { announceReactionReceived } from '@/shell/data/useRuntimeEventBridge';
import { createDesktopShellStore, DesktopShellStoreContext, timelineStorageKeyForChannel } from '@/shell/store';

import { LiveActionsContext, type LiveActions } from './liveActions';
import { LiveStage } from './LiveStage';

const TOPIC = 'kukuri:topic:general';
const THREAD = 'pc-1742860800000-cccccccccccccccc';
const PRIVATE_CHANNEL = 'channel-1742860800000-dddddddddddddddd';
const ME = 'e'.repeat(64);
const STORY_SECONDS = 1_742_860_800;
// custom reaction の画像の代わり。
const IMAGE =
  "data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><circle cx='8' cy='8' r='7' fill='%23f2a93b'/><circle cx='5.5' cy='6.5' r='1' fill='%23222'/><circle cx='10.5' cy='6.5' r='1' fill='%23222'/><path d='M5 10q3 2.5 6 0' stroke='%23222' fill='none'/></svg>";
// 視聴中の映像の代わり(暗い所と明るい所の両方で読めるかを見る)。映像の bytes は送らない。
const VIDEO_STAND_IN = '.live-stage canvas { background: linear-gradient(160deg, #1b2735 0%, #4f6d8f 45%, #f9d976 100%) !important; }';

const SESSION: LiveSessionView = {
  session_id: 'live-1',
  host_pubkey: 'f'.repeat(64),
  title: 'Windows 11 で通しの確認',
  description: '',
  status: 'Live',
  started_at: STORY_SECONDS * 1000,
  ended_at: null,
  viewer_count: 3,
  joined_by_me: false,
  channel_id: THREAD,
  audience_label: 'Public thread',
  start_post_id: 'start',
};

const custom = (count: number): ReactionSummaryView => ({
  reaction_key_kind: 'custom_asset',
  normalized_reaction_key: 'custom_asset:neko',
  emoji: null,
  custom_asset: {
    asset_id: 'neko',
    owner_pubkey: 'c'.repeat(64),
    blob_hash: 'neko-hash',
    search_key: 'neko',
    mime: 'image/png',
    bytes: 68,
    width: 128,
    height: 128,
  },
  count,
});

const emoji = (value: string, count: number): ReactionSummaryView => ({
  reaction_key_kind: 'emoji',
  normalized_reaction_key: value,
  emoji: value,
  custom_asset: null,
  count,
});

const post = (id: string, name: string, content: string, overrides: Partial<PostView> = {}): PostView =>
  ({
    object_id: id,
    author_pubkey: id.padEnd(64, '0'),
    author_display_name: name,
    content,
    content_status: 'Available',
    attachments: [],
    created_at: STORY_SECONDS,
    withdrawal: null,
    reply_to: null,
    root_id: null,
    reaction_summary: [],
    ...overrides,
  }) as PostView;

const POSTS = [
  post('p5', 'けん', '音が少し小さいです。マイクの位置を変えられますか？', { reply_to: 'start', root_id: 'start' }),
  post('p4', 'あおい', '見に行きます', { reaction_summary: [emoji('👍', 2)] }),
  post(
    'p3',
    'みな',
    'これから Windows 11 で通しの確認を配信します。インストールから最初の投稿までを順に見ていきます。途中で気になったところがあれば、このスレッドに書いてください。'
  ),
  post('p2', 'りく', '@[みな](' + 'a'.repeat(64) + ') ありがとうございます', { reaction_summary: [custom(1)] }),
  post('p1', 'そら', '待機中'),
];

// 視聴の状態を「映像が届いた」にする応答(状態 0・時刻 0・chunk 0 件)。
const WATCHING_BATCH = new Uint8Array(11).buffer;

type StoryProps = {
  session?: LiveSessionView;
  posts?: PostView[];
  floating?: boolean;
  images?: boolean;
  conversationChannel?: string;
};

function LiveStageStory({
  session = SESSION,
  posts = POSTS,
  floating = false,
  images = true,
  conversationChannel = THREAD,
}: StoryProps) {
  const [store] = useState(() => {
    const created = createDesktopShellStore();
    created.getState().setField('timelinesByKey', { [timelineStorageKeyForChannel(TOPIC, conversationChannel)]: posts });
    created.getState().setField('joinedChannelsByTopic', { [TOPIC]: [] });
    created.getState().setField('mediaObjectUrls', images ? { 'neko-hash': IMAGE } : {});
    return created;
  });
  const [actions] = useState(() => {
    const startPost = post('start', 'みな', 'これから配信します', {
      reaction_summary: [emoji('🎉', 5), custom(4), emoji('👏', 2), emoji('🔥', 1)],
    });
    let watching = false;
    const api = {
      getLiveSession: async () => session,
      setSessionDisplay: async () => undefined,
      listThread: async () => ({ items: [startPost], next_cursor: null }),
      watchLiveMedia: async () => undefined,
      setLiveVolume: async () => undefined,
      stopWatchingLiveMedia: async () => undefined,
      nextLiveVideo: () => {
        if (watching) return new Promise<ArrayBuffer>(() => undefined);
        watching = true;
        return Promise.resolve(WATCHING_BATCH);
      },
    } as unknown as DesktopApi;
    return { api, locale: 'ja', localPubkey: ME, pendingBySessionId: {} } as unknown as LiveActions;
  });
  useEffect(() => {
    if (!floating) return;
    const timer = window.setInterval(() => {
      announceReactionReceived({
        target_object_id: 'start',
        reaction_id: `story-${Date.now()}`,
        reaction: Math.random() < 0.5 ? emoji('🎉', 1) : custom(1),
        updated_at: Date.now(),
      });
    }, 400);
    return () => window.clearInterval(timer);
  }, [floating]);
  return (
    <DesktopShellStoreContext.Provider value={store}>
      <LiveActionsContext.Provider value={actions}>
        <style>{VIDEO_STAND_IN}</style>
        <div style={{ maxWidth: 560 }}>
          <LiveStage
            source={{
              live: { session_id: 'live-1', topic_id: TOPIC, channel_id: THREAD },
              conversation: { topic: TOPIC, channelId: conversationChannel },
            }}
            conversationLabel='v0.5 の配布候補の確認'
            visible
            recentReactions={[]}
            reactionAssets={[]}
            onReactionPickerOpen={() => undefined}
            onToggleReaction={() => undefined}
            onReply={() => undefined}
          />
        </div>
      </LiveActionsContext.Provider>
    </DesktopShellStoreContext.Provider>
  );
}

const meta = {
  title: 'Shell/LiveStage',
  component: LiveStageStory,
} satisfies Meta<typeof LiveStageStory>;

export default meta;

type Story = StoryObj<typeof meta>;

/** 視聴中。映像の上に投稿と開始 post のリアクションを重ねる(映像は代わりの背景)。 */
export const Watching: Story = { args: { session: { ...SESSION, joined_by_me: true } } };

/** 新しく届いたリアクションを浮かべる。 */
export const FloatingReactions: Story = { args: { session: { ...SESSION, joined_by_me: true }, floating: true } };

/** 参加前。映像の代わりに案内を出し、投稿と開始 post のリアクションを重ねる。 */
export const BeforeJoining: Story = {};

/** 配信者本人の画面。 */
export const Broadcaster: Story = { args: { session: { ...SESSION, host_pubkey: ME } } };

/** custom reaction の画像を取得できない。名前の先頭 2 文字で出す。 */
export const MissingImage: Story = { args: { images: false } };

/** 投稿が無い会話。 */
export const NoPosts: Story = { args: { posts: [] } };

/** 参加していない private channel の会話。投稿を重ねない。 */
export const Unreadable: Story = { args: { conversationChannel: PRIVATE_CHANNEL } };

export const Ended: Story = { args: { session: { ...SESSION, status: 'Ended', ended_at: STORY_SECONDS * 1000 } } };
