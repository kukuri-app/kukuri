import { useCallback, useContext, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { useSessionDisplay } from '@/components/extended/useSessionDisplay';
import { Notice } from '@/components/ui/notice';
import type { LiveSessionView, PostView } from '@/lib/api';
import { channelTimelineScope } from '@/shell/presentation';
import { useDesktopShellStore } from '@/shell/store';

import { LiveCard } from './LiveCard';
import { LiveActionsContext } from './liveActions';

/**
 * #1839: 開始 post・紹介 post の中の Live。Live を作った topic と id で 1 件を表示要求で読み、手元の行を出す。
 * 参照の channel と Live の channel が違う(限定 Live を別の場所から指す)ときは、読めない Live として扱う。
 */
export function PostLive({ post }: { post: PostView }) {
  const { t } = useTranslation('live');
  const actions = useContext(LiveActionsContext);
  const live = post.live!;
  const channelId = live.channel_id ?? null;
  // 公開スレッドで始めた Live は、公開スレッドの scope で表示要求を出す(#1840 AC-4)。
  const scope = channelTimelineScope(channelId);
  const lastSync = useDesktopShellStore((state) => state.syncStatus.last_sync_ts);
  // undefined は読込み中(表示要求が終わるまで、手元に無いことを「読めない」としない)。
  const [session, setSession] = useState<LiveSessionView | null | undefined>(undefined);
  const displayed = useRef(false);
  const api = actions?.api;
  const load = useCallback(async () => {
    if (!api) return;
    const found = await api.getLiveSession(live.topic_id, live.session_id).catch(() => null);
    const matches = found && (found.channel_id ?? null) === channelId;
    setSession(matches ? found : displayed.current ? null : undefined);
  }, [api, live.topic_id, live.session_id, channelId]);
  const onDisplayed = useCallback(() => {
    displayed.current = true;
    void load();
  }, [load]);
  useEffect(() => {
    void load();
  }, [load, lastSync]);
  const { element } = useSessionDisplay<HTMLDivElement>({
    context: api ? { api, topic: live.topic_id, scope } : undefined,
    sessionId: live.session_id,
    kind: 'live',
    onDisplayed,
  });
  return (
    <div ref={element} className='post-live'>
      {session ? (
        <LiveCard topic={live.topic_id} session={session} postId={post.object_id} />
      ) : (
        <Notice>{t(session === null ? 'unavailable' : 'loading')}</Notice>
      )}
    </div>
  );
}
