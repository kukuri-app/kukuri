import { render, screen } from '@testing-library/react';
import { expect, test, vi } from 'vitest';

import type { PostView } from '@/lib/api';

import { PostLiveOutlet, PostLiveSlot } from './postLiveSlot';

vi.mock('@/lib/webRuntime', () => ({ IS_WEB_RUNTIME: true }));

// #1839: Web は Live を読めないので、Live を参照する投稿でも投稿の中に Live を出さない(読込み中のまま残さない)。
test('the web runtime does not render a live session inside a post', () => {
  const post = { object_id: 'start-post', live: { session_id: 'live-1', topic_id: 'topic', channel_id: null } } as PostView;
  render(
    <PostLiveSlot.Provider value={() => <p>live session</p>}>
      <PostLiveOutlet post={post} />
    </PostLiveSlot.Provider>
  );
  expect(screen.queryByText('live session')).toBeNull();
});
