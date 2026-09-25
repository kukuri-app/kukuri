import { useContext, useEffect, useRef } from 'react';
import { MediaDemandContext } from './mediaRetryContext';

export function MediaDemandObserver({ hash }: { hash: string | null }) {
  const demand = useContext(MediaDemandContext);
  const marker = useRef<HTMLSpanElement>(null);

  useEffect(() => {
    if (!hash || !demand || !marker.current?.parentElement) return;
    let active = false;
    const setVisible = (visible: boolean) => {
      if (active === visible) return;
      active = visible;
      demand(hash, visible);
    };
    if (typeof IntersectionObserver === 'undefined') {
      setVisible(true);
      return () => setVisible(false);
    }
    // 1 回の callback に複数の entry が届いたときは、最後の entry が現在の状態を表す。
    const observer = new IntersectionObserver((entries) => setVisible(Boolean(entries.at(-1)?.isIntersecting)));
    observer.observe(marker.current.parentElement);
    return () => {
      observer.disconnect();
      setVisible(false);
    };
  }, [demand, hash]);

  return hash && demand ? <span ref={marker} className='media-demand-marker' aria-hidden='true' /> : null;
}
