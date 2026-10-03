import { useEffect, useRef, useState, type RefObject } from 'react';

import type { LinkPreview, LinkPreviewFetcher } from '@/lib/api';

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
  const [intersecting, setIntersecting] = useState(
    () => typeof IntersectionObserver === 'undefined'
  );
  const [documentVisible, setDocumentVisible] = useState(
    () => typeof document === 'undefined' || document.visibilityState !== 'hidden'
  );
  const [state, setState] = useState<LinkPreviewState>({ status: 'idle', preview: null });
  const generation = useRef(0);

  useEffect(() => {
    if (typeof IntersectionObserver === 'undefined') {
      setIntersecting(true);
      return;
    }
    const slot = targetRef.current;
    if (!slot) return;
    const target = slot.closest('article') ?? slot;
    const observer = new IntersectionObserver(
      (entries) => setIntersecting(entries.some((entry) => entry.isIntersecting)),
      { rootMargin: '0px' }
    );
    observer.observe(target);
    return () => observer.disconnect();
  }, [targetRef, url]);

  useEffect(() => {
    if (typeof document === 'undefined') return;
    const update = () => setDocumentVisible(document.visibilityState !== 'hidden');
    document.addEventListener('visibilitychange', update);
    return () => document.removeEventListener('visibilitychange', update);
  }, []);

  useEffect(() => {
    const request = ++generation.current;
    if (!enabled || !fetcher || !url || !intersecting || !documentVisible) {
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
  }, [documentVisible, enabled, fetcher, intersecting, objectId, url]);

  return state;
}
