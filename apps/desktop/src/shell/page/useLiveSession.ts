import { useCallback, useEffect, useRef, useState } from 'react';

import { useSessionDisplay } from '@/components/extended/useSessionDisplay';
import type { DesktopApi, LiveSessionView, PostLiveView } from '@/lib/api';
import { channelTimelineScope } from '@/shell/presentation';
import { useDesktopShellStore } from '@/shell/store';

/**
 * #1839: 参照(Live を作った topic と id)の Live を、表示要求で 1 件読み、手元の行を返す。`element` が画面に入っている間だけ
 * 表示を要求する。`undefined` は読込み中(表示要求が終わるまで、手元に無いことを「読めない」としない)。参照の channel と
 * Live の channel が違う(限定 Live を別の場所から指す)ときは、読めない Live(`null`)として扱う。
 */
export function useLiveSession<T extends HTMLElement>(api: DesktopApi | undefined, live: PostLiveView) {
  const channelId = live.channel_id ?? null;
  const lastSync = useDesktopShellStore((state) => state.syncStatus.last_sync_ts);
  const [session, setSession] = useState<LiveSessionView | null | undefined>(undefined);
  const displayed = useRef(false);
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
  const { element } = useSessionDisplay<T>({
    // 公開スレッドで始めた Live は、公開スレッドの scope で表示要求を出す(#1840 AC-4)。
    context: api ? { api, topic: live.topic_id, scope: channelTimelineScope(channelId) } : undefined,
    sessionId: live.session_id,
    kind: 'live',
    onDisplayed,
  });
  return { element, session };
}
