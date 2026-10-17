import { createContext, type ReactNode, useContext } from 'react';

import type { PostView } from '@/lib/api';

/** #1839: 投稿の中に Live を描く関数。Live の取得と操作は shell が持つので、`PostCard` はここから受け取る。 */
// eslint-disable-next-line react-refresh/only-export-components
export const PostLiveSlot = createContext<((post: PostView) => ReactNode) | null>(null);

export function PostLiveOutlet({ post }: { post: PostView }) {
  const render = useContext(PostLiveSlot);
  return post.live && render ? render(post) : null;
}
