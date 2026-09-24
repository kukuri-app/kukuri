import { useCallback, useLayoutEffect, useRef } from 'react';

type PostIdentity = { post: { object_id: string } };

export function useWindowScrollAnchor(posts: readonly PostIdentity[], onLoadMore?: () => void) {
  const listRef = useRef<HTMLUListElement | null>(null);
  const anchorRef = useRef<{
    id: string;
    offset: number;
    first: string | undefined;
    last: string | undefined;
    length: number;
  } | null>(null);

  const loadMore = useCallback(() => {
    const list = listRef.current;
    const scroll = list?.closest<HTMLElement>('.shell-column-body');
    if (list && scroll) {
      const viewport = scroll.getBoundingClientRect();
      const item = [...list.children].find((child) => {
        const rect = child.getBoundingClientRect();
        return child instanceof HTMLElement && child.dataset.postId &&
          rect.bottom > viewport.top && rect.top < viewport.bottom;
      }) as HTMLElement | undefined;
      if (item?.dataset.postId) {
        anchorRef.current = {
          id: item.dataset.postId,
          offset: item.getBoundingClientRect().top - viewport.top,
          first: posts[0]?.post.object_id,
          last: posts.at(-1)?.post.object_id,
          length: posts.length,
        };
      }
    }
    onLoadMore?.();
  }, [onLoadMore, posts]);

  useLayoutEffect(() => {
    const anchor = anchorRef.current;
    if (!anchor || (anchor.first === posts[0]?.post.object_id &&
      anchor.last === posts.at(-1)?.post.object_id && anchor.length === posts.length)) return;
    anchorRef.current = null;
    const list = listRef.current;
    const scroll = list?.closest<HTMLElement>('.shell-column-body');
    const item = [...(list?.children ?? [])].find((child) =>
      child instanceof HTMLElement && child.dataset.postId === anchor.id
    );
    if (scroll && item) {
      scroll.scrollTop += item.getBoundingClientRect().top -
        scroll.getBoundingClientRect().top - anchor.offset;
    }
  }, [posts]);

  return { listRef, loadMore };
}
