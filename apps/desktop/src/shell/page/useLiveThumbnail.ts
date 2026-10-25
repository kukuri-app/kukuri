import { useCallback, useEffect, useRef, useState } from 'react';
import { useOnScreen } from '@/components/core/useOnScreen';
import { LIVE_THUMBNAIL_INTERVAL_MS } from '@/components/extended/liveMedia';
import type { DesktopApi, LiveThumbnailView } from '@/lib/api';

export function useLiveThumbnail(api: DesktopApi | undefined, topic: string, sessionId: string, live: boolean) {
  const element = useRef<HTMLDivElement>(null);
  const visible = useOnScreen(element);
  const [image, setImage] = useState<LiveThumbnailView | null>(null);
  const load = useCallback(async () => {
    if (!api?.getLiveThumbnail) return null;
    return api.getLiveThumbnail(topic, sessionId).catch(() => null);
  }, [api, topic, sessionId]);
  useEffect(() => {
    if (!visible) return;
    let current = true;
    let timer: ReturnType<typeof setTimeout>;
    const refresh = async () => {
      const next = await load();
      if (!current) return;
      setImage(next);
      if (live) timer = setTimeout(() => void refresh(), LIVE_THUMBNAIL_INTERVAL_MS);
    };
    void refresh();
    return () => { current = false; clearTimeout(timer); };
  }, [load, visible, live]);
  return { element, image, refresh: async () => setImage(await load()) };
}

