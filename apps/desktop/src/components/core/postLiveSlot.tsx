import { createContext, type ReactNode, useContext } from 'react';

import type { PostView } from '@/lib/api';
import { IS_WEB_RUNTIME } from '@/lib/webRuntime';

/** #1839: 投稿の中に Live を描く関数。Live の取得と操作は shell が持つので、`PostCard` はここから受け取る。 */
// eslint-disable-next-line react-refresh/only-export-components
export const PostLiveSlot = createContext<((post: PostView) => ReactNode) | null>(null);

/** Web は Live を扱わない(ADR 0060)ので、開始・紹介 post を通常の投稿として出す。 */
export function PostLiveOutlet({ post }: { post: PostView }) {
  const render = useContext(PostLiveSlot);
  return post.live && render && !IS_WEB_RUNTIME ? render(post) : null;
}
