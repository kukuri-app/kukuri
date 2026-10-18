import { useContext } from 'react';
import { useTranslation } from 'react-i18next';

import { Notice } from '@/components/ui/notice';
import type { PostView } from '@/lib/api';
import { publishedTopicIdForPost } from '@/shell/presentation';

import { LiveCard } from './LiveCard';
import { LiveActionsContext } from './liveActions';
import { useLiveSurfacesEnabled } from './publicThreadActions';
import { useLiveSession } from './useLiveSession';

/**
 * #1839: 開始 post・紹介 post の中の Live。Live を作った topic と id で 1 件を表示要求で読み、手元の行を出す。
 * 参照の channel と Live の channel が違う(限定 Live を別の場所から指す)ときは、読めない Live として扱う。
 */
export function PostLive({ post }: { post: PostView }) {
  const { t } = useTranslation('live');
  const actions = useContext(LiveActionsContext);
  const live = post.live!;
  const { element, session } = useLiveSession<HTMLDivElement>(actions?.api, live);
  // #1841: 「大きく表示」は、この投稿の場所を会話にしたライブの列を開く。
  const stageable = useLiveSurfacesEnabled();
  const conversation = { topicId: publishedTopicIdForPost(post) ?? live.topic_id, channelId: post.channel_id ?? null };
  return (
    <div ref={element} className='post-live'>
      {session ? (
        <LiveCard
          topic={live.topic_id}
          session={session}
          postId={post.object_id}
          onStage={actions && stageable ? () => actions.stage(live, conversation) : undefined}
        />
      ) : (
        <Notice>{t(session === null ? 'unavailable' : 'loading')}</Notice>
      )}
    </div>
  );
}
