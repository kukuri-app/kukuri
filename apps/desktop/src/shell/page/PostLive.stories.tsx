import type { Meta, StoryObj } from '@storybook/react-vite';

import type { DesktopApi, LiveSessionView, PostView } from '@/lib/api';
import { createDesktopShellStore, DesktopShellStoreContext } from '@/shell/store';

import { LiveActionsContext, type LiveActions } from './liveActions';
import { PostLive } from './PostLive';

const STORY_TIMESTAMP = 1_742_860_800_000;

const SESSION: LiveSessionView = {
  session_id: 'live-1',
  host_pubkey: 'f'.repeat(64),
  title: 'Launch Party',
  description: 'Watch along while we ship the release candidate together.',
  status: 'Live',
  started_at: STORY_TIMESTAMP,
  ended_at: null,
  viewer_count: 4,
  joined_by_me: false,
  channel_id: null,
  audience_label: 'Public',
  start_post_id: 'start-post',
};

function PostLiveStory({ postId, session, channelId = null }: {
  postId: string;
  session: LiveSessionView | null;
  channelId?: string | null;
}) {
  const api = {
    getLiveSession: async () => session,
    setSessionDisplay: async () => undefined,
  } as unknown as DesktopApi;
  const actions: LiveActions = {
    api,
    locale: 'en',
    localPubkey: 'e'.repeat(64),
    pendingBySessionId: {},
    join: async () => undefined,
    leave: async () => undefined,
    end: async () => undefined,
    introduce: () => undefined,
    broadcastingSessionId: null,
    startBroadcast: () => undefined,
    openPost: () => undefined,
    copyLink: () => undefined,
    activateReference: () => undefined,
    stage: () => undefined,
  };
  const post = {
    object_id: postId,
    live: { session_id: 'live-1', topic_id: 'kukuri:topic:general', channel_id: channelId },
  } as PostView;
  return (
    <DesktopShellStoreContext.Provider value={createDesktopShellStore()}>
      <LiveActionsContext.Provider value={actions}>
        <div className='shell-phase1' style={{ maxWidth: 560 }}>
          <PostLive post={post} />
        </div>
      </LiveActionsContext.Provider>
    </DesktopShellStoreContext.Provider>
  );
}

const meta = {
  title: 'Shell/PostLive',
  component: PostLiveStory,
} satisfies Meta<typeof PostLiveStory>;

export default meta;

type Story = StoryObj<typeof meta>;

/** 開始 post の中。元の投稿へのボタンは出さない。 */
export const StartPost: Story = { args: { postId: 'start-post', session: SESSION } };

/** 紹介 post の中。返信・リアクションを付ける元の投稿へ移れる。 */
export const Introduction: Story = { args: { postId: 'intro-post', session: SESSION } };

export const Ended: Story = {
  args: { postId: 'intro-post', session: { ...SESSION, status: 'Ended', ended_at: STORY_TIMESTAMP + 3_600_000, viewer_count: 0 } },
};

export const LimitedChannel: Story = {
  args: {
    postId: 'start-post',
    channelId: 'channel-1',
    session: { ...SESSION, channel_id: 'channel-1', audience_label: 'Private channel' },
  },
};

/** 読めない Live(権限が無い・取得できない)。 */
export const Unavailable: Story = { args: { postId: 'intro-post', session: null } };
