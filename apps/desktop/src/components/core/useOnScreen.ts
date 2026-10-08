import { useEffect, useState, type RefObject } from 'react';

/// 要素(投稿 card の中なら card)が画面内にあり、画面が表示中か。要素が無い間は観測しない。`resetKey` が変わったら観測し直す。
export function useOnScreen(targetRef: RefObject<HTMLElement | null>, resetKey?: unknown): boolean {
  const [intersecting, setIntersecting] = useState(
    () => typeof IntersectionObserver === 'undefined'
  );
  const [documentVisible, setDocumentVisible] = useState(
    () => typeof document === 'undefined' || document.visibilityState !== 'hidden'
  );

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
  }, [targetRef, resetKey]);

  useEffect(() => {
    if (typeof document === 'undefined') return;
    const update = () => setDocumentVisible(document.visibilityState !== 'hidden');
    document.addEventListener('visibilitychange', update);
    return () => document.removeEventListener('visibilitychange', update);
  }, []);

  return intersecting && documentVisible;
}
