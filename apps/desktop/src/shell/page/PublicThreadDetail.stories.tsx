import type { Meta, StoryObj } from '@storybook/react-vite';
import { useState } from 'react';
import { App } from '@/App';
import type { PostView } from '@/lib/api';
import { createDesktopMockApi } from '@/mocks/desktopApiMock';

const topic = 'kukuri:topic:general';
const channel = 'pc-1700000000000-0123456789abcdef';
function post(index: number, content: string, parent?: PostView): PostView {
  const base: PostView = {
    object_id: `conversation-${index}`, envelope_id: `envelope-${index}`,
    author_pubkey: 'b'.repeat(64), author_name: 'reviewer', author_display_name: null,
    following: false, followed_by: false, mutual: false, friend_of_friend: false,
    object_kind: 'post', is_threadable: true, content, content_status: 'Available',
    attachments: [], created_at: 1_700_000_000 + index * 60,
    reply_to: null, root_id: `conversation-${index}`, channel_id: channel,
    audience_label: 'Public thread',
  };
  if (!parent) return base;
  return {
    ...base, reply_to: parent.object_id, root_id: parent.root_id,
    reply_preview: { object_id: parent.object_id, topic, author: { pubkey: parent.author_pubkey, name: parent.author_name },
      content: parent.content, content_status: 'Available', attachments: [] },
  };
}
const first = post(1, '最初の投稿です。ここから会話を読みます。');
const answer = post(3, '最初の投稿への返信です。親の下に字下げして並びます。', first);
function PublicThreadDetail() {
  const [api] = useState(() => {
    window.history.replaceState(null, '', `#/timeline?topic=${encodeURIComponent(topic)}&channel=${channel}`);
    return createDesktopMockApi({
      seedPublicChannels: { [topic]: [{ channel_id: channel, topic_id: topic, title: '会話の順序を確認', creator_pubkey: 'b'.repeat(64), created_at: 1_700_000_000, last_post_at: 1_700_000_240, live_now: false }] },
      seedPosts: { [topic]: [
        post(4, '返信への返信です。さらに一段下がります。', answer), answer,
        post(2, 'あとから届いた投稿です。下に続きます。'), first,
      ] },
    });
  });
  return <App api={api} />;
}
const meta = { title: 'Shell/PublicThreadDetail', component: PublicThreadDetail, parameters: { layout: 'fullscreen', ownRouter: true, reviewCanvas: true } } satisfies Meta<typeof PublicThreadDetail>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Conversation: Story = {};
