import { useEffect, useRef, useState, type RefObject } from 'react';

import type { LinkPreview, LinkPreviewFetcher } from '@/lib/api';

import { useOnScreen } from './useOnScreen';

type LinkPreviewState =
  | { status: 'idle' | 'loading' | 'unavailable'; preview: null }
  | { status: 'available'; preview: LinkPreview };

export function useLinkPreview({
  enabled,
  fetcher,
  objectId,
  targetRef,
  url,
}: {
  enabled: boolean;
  fetcher: LinkPreviewFetcher | null;
  objectId: string;
  targetRef: RefObject<HTMLElement | null>;
  url: string | null;
}): LinkPreviewState {
  const onScreen = useOnScreen(targetRef, url);
  const [state, setState] = useState<LinkPreviewState>({ status: 'idle', preview: null });
  const generation = useRef(0);

  useEffect(() => {
    const request = ++generation.current;
    if (!enabled || !fetcher || !url || !onScreen) {
      setState({ status: 'idle', preview: null });
      return;
    }
    setState({ status: 'loading', preview: null });
    void fetcher(url, objectId)
      .then((outcome) => {
        if (generation.current !== request) return;
        setState(
          outcome.status === 'available'
            ? { status: 'available', preview: outcome.preview }
            : { status: 'unavailable', preview: null }
        );
      })
      .catch(() => {
        if (generation.current === request) {
          setState({ status: 'unavailable', preview: null });
        }
      });
    return () => {
      generation.current += 1;
    };
  }, [enabled, fetcher, objectId, onScreen, url]);

  return state;
}
