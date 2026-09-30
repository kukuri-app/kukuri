import type * as React from 'react';
import { memo, type ReactNode, useInsertionEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import type {
  BookmarkedCustomReactionView,
  CommunityNodeManifestFetch,
  CommunityNodePoliciesResponse,
  CustomReactionAssetView,
  ReactionKeyInput,
  RecentReactionView,
  SubmitCommunityNodeReportRequest,
  SubmitCommunityNodeReportResult,
} from '@/lib/api';
import type { InternalSmartReference } from '@/lib/internalLinks';

import { Button } from '@/components/ui/button';

import { PostCard } from './PostCard';
import { UnavailablePostsNotice } from './UnavailablePostsNotice';
import { type PostCardView } from './types';
import { useInfiniteScrollSentinel } from './useInfiniteScrollSentinel';
import { useWindowScrollAnchor } from './useWindowScrollAnchor';

// 行の props が変わらない限り、既存の行を描き直さない。続きの読み込みや定期更新のたびに窓の全行
// (最大 200 行)を描き直すと、その処理が行数に比例して操作の応答を遅らせる(#1425)。
const TimelineFeedRow = memo(PostCard);
const NO_MEDIA_OBJECT_URLS: Record<string, string | null> = {};
const NO_ITEMS: never[] = [];

type Callback = (...args: never[]) => unknown;

/**
 * 行へ渡す callback を、描画のたびに作り直されても同じ参照に保つ(呼ばれたときは最新の callback を使う)。
 * 未指定の callback は未指定のまま渡し、操作の有無の表示を変えない。
 */
function useStableCallbacks<T extends Record<string, Callback | undefined>>(callbacks: T): T {
  const [{ latest, wrappers }] = useState(() => ({
    latest: new Map<string, Callback | undefined>(),
    wrappers: new Map<string, Callback>(),
  }));
  useInsertionEffect(() => {
    for (const [key, callback] of Object.entries(callbacks)) latest.set(key, callback);
  });
  return Object.fromEntries(
    Object.entries(callbacks).map(([key, callback]) => {
      if (!callback) return [key, callback];
      let wrapper = wrappers.get(key);
      if (!wrapper) {
        wrapper = (...args: never[]) => (latest.get(key) as Callback)(...args);
        wrappers.set(key, wrapper);
      }
      return [key, wrapper];
    })
  ) as T;
}

type TimelineFeedProps = {
  posts: PostCardView[];
  emptyCopy: string;
  /**
   * #994: 0 件時の描画を呼出し側が差し替える。`null` は空文言も出さない(loading / error を呼出し側が示す)。
   * 未指定なら従来どおり `emptyCopy` を 1 行表示する。
   */
  emptyState?: ReactNode;
  listClassName?: string;
  itemClassName?: string;
  onOpenAuthor: (authorPubkey: string) => void;
  onOpenThread: (threadId: string) => void;
  onOpenThreadInTopic?: (threadId: string, topicId: string) => void;
  onReply: (post: PostCardView['post']) => void;
  onRepost?: (post: PostCardView['post']) => void;
  onQuoteRepost?: (post: PostCardView['post']) => void;
  readOnly?: boolean;
  onOpenOriginalTopic?: (topicId: string) => void;
  localAuthorPubkey?: string;
  mediaObjectUrls?: Record<string, string | null>;
  ownedReactionAssets?: CustomReactionAssetView[];
  bookmarkedReactionAssets?: BookmarkedCustomReactionView[];
  recentReactions?: RecentReactionView[];
  onToggleReaction?: (post: PostCardView['post'], reactionKey: ReactionKeyInput) => void;
  onBookmarkCustomReaction?: (asset: CustomReactionAssetView) => void;
  onReactionPickerOpen?: () => void;
  showBookmarkAction?: boolean;
  bookmarkedPostIds?: Set<string>;
  onToggleBookmark?: (post: PostCardView['post']) => void;
  onWithdraw?: (post: PostCardView['post']) => void;
  onRetryLocalPost?: (post: PostCardView['post']) => void;
  onRestoreLocalPost?: (post: PostCardView['post']) => void;
  onActivateReference?: (reference: InternalSmartReference) => void;
  onCopyPostLink?: (link: string) => void;
  focusedPostObjectId?: string | null;
  hasMore?: boolean;
  loadingMore?: boolean;
  loadMoreError?: string | null;
  onLoadMore?: () => void;
  /** 読んだ範囲にあるが、まだ取得できていない投稿の数(#1239 AC-4)。続きを読む操作は止めない。 */
  unavailableCount?: number;
  pendingCount?: number;
  returnToLatest?: boolean;
  onApplyPending?: () => void;
  // 分散通報ルーティング（#310）。取得済み community node manifest（ok のみ）と送信導線。
  onSubmitReport?: (
    request: SubmitCommunityNodeReportRequest
  ) => Promise<SubmitCommunityNodeReportResult>;
  onCopyReportContact?: (value: string) => void;
  onFetchReportManifest?: (baseUrl: string) => Promise<CommunityNodeManifestFetch>;
  /// #1192: 権利侵害を選んだときに提示する権利侵害申出ポリシーの取得(読み取りのみ)。
  onFetchNodePolicies?: (baseUrl: string, language?: string) => Promise<CommunityNodePoliciesResponse>;
  onMuteReportAuthor?: (authorPubkey: string) => Promise<void> | void;
};

export function TimelineFeed({
  posts,
  emptyCopy,
  emptyState,
  listClassName = 'post-list',
  itemClassName,
  onOpenAuthor,
  onOpenThread,
  onOpenThreadInTopic,
  onReply,
  onRepost,
  onQuoteRepost,
  readOnly = false,
  onOpenOriginalTopic,
  localAuthorPubkey,
  mediaObjectUrls = NO_MEDIA_OBJECT_URLS,
  ownedReactionAssets = NO_ITEMS,
  bookmarkedReactionAssets = NO_ITEMS,
  recentReactions = NO_ITEMS,
  onToggleReaction,
  onBookmarkCustomReaction,
  onReactionPickerOpen,
  showBookmarkAction = false,
  bookmarkedPostIds,
  onToggleBookmark,
  onWithdraw,
  onRetryLocalPost,
  onRestoreLocalPost,
  onActivateReference,
  onCopyPostLink,
  focusedPostObjectId,
  hasMore = false,
  loadingMore = false,
  loadMoreError = null,
  onLoadMore,
  unavailableCount = 0,
  pendingCount = 0,
  returnToLatest = false,
  onApplyPending,
  onSubmitReport,
  onCopyReportContact,
  onFetchReportManifest,
  onFetchNodePolicies,
  onMuteReportAuthor,
}: TimelineFeedProps) {
  const { t } = useTranslation('common');
  const { listRef, loadMore } = useWindowScrollAnchor(posts, onLoadMore);
  const { sentinelRef: loadMoreRef, canAutoLoad, manualFallback } = useInfiniteScrollSentinel({
    // A failed automatic request must not immediately reconnect the observer and retry forever.
    // Keep the cursor, but require an explicit retry after an error.
    hasMore: hasMore && !loadMoreError,
    loadingMore,
    onLoadMore: loadMore,
  });
  const rowCallbacks = useStableCallbacks({
    onOpenAuthor,
    onOpenThread,
    onOpenThreadInTopic,
    onReply,
    onRepost,
    onQuoteRepost,
    onOpenOriginalTopic,
    onToggleReaction,
    onBookmarkCustomReaction,
    onReactionPickerOpen,
    onToggleBookmark,
    onWithdraw,
    onRetryLocalPost,
    onRestoreLocalPost,
    onActivateReference,
    onCopyLink: onCopyPostLink,
    onSubmitReport,
    onCopyReportContact,
    onFetchReportManifest,
    onFetchNodePolicies,
    onMuteReportAuthor,
  });
  const overscrollAccumulationRef = useRef(0);
  const touchStartYRef = useRef<number | null>(null);

  const canApplyPending = (pendingCount > 0 || returnToLatest) && typeof onApplyPending === 'function';

  const handleOverscrollIntent = () => {
    if (!onApplyPending) {
      return;
    }
    onApplyPending();
    overscrollAccumulationRef.current = 0;
  };

  const handleWheel = (event: React.WheelEvent<HTMLUListElement>) => {
    if (!onApplyPending || posts.length === 0) {
      return;
    }
    const target = event.currentTarget;
    if (target.scrollTop > 0 || event.deltaY >= 0) {
      overscrollAccumulationRef.current = 0;
      return;
    }
    overscrollAccumulationRef.current += Math.abs(event.deltaY);
    if (overscrollAccumulationRef.current >= 120) {
      handleOverscrollIntent();
    }
  };

  const handleTouchStart = (event: React.TouchEvent<HTMLUListElement>) => {
    touchStartYRef.current = event.touches[0]?.clientY ?? null;
  };

  const handleTouchMove = (event: React.TouchEvent<HTMLUListElement>) => {
    if (!onApplyPending || posts.length === 0) {
      return;
    }
    const startY = touchStartYRef.current;
    const currentY = event.touches[0]?.clientY ?? null;
    if (startY === null || currentY === null || event.currentTarget.scrollTop > 0) {
      return;
    }
    if (currentY - startY >= 80) {
      touchStartYRef.current = currentY;
      handleOverscrollIntent();
    }
  };

  // 行が 0 件でも、続きがある(`hasMore`)あいだは、続きを読む手段(sentinel か button)を描く。
  // 非表示の著者の投稿が続く範囲では、取得が空のページと `next_cursor` を返す(#1239)。ここで空の文言だけを
  // 返すと、その先の表示できる投稿へ進めない。
  // まだ取得できていない投稿があるときも、空の文言ではなく、その旨を描く(#1239 AC-4)。
  if (posts.length === 0 && !canApplyPending && !hasMore && unavailableCount <= 0) {
    if (emptyState !== undefined) return <>{emptyState}</>;
    return <p className='empty'>{emptyCopy}</p>;
  }

  return (
    <ul
      ref={listRef}
      className={listClassName}
      onWheel={handleWheel}
      onTouchStart={handleTouchStart}
      onTouchMove={handleTouchMove}
    >
      {canApplyPending ? (
        <li className={itemClassName}>
          <Button
            variant='secondary'
            type='button'
            className='timeline-feed-refresh-banner'
            onClick={() => onApplyPending()}
          >
            {returnToLatest ? t('feed.backToLatest') : t('feed.pendingPosts', { count: pendingCount })}
          </Button>
        </li>
      ) : null}
      {posts.map((view) => (
        <li key={view.post.object_id} className={itemClassName} data-post-id={view.post.object_id}>
        <TimelineFeedRow
          {...rowCallbacks}
          enableLinkPreview
          view={view}
          readOnly={readOnly}
          localAuthorPubkey={localAuthorPubkey}
          mediaObjectUrls={mediaObjectUrls}
          ownedReactionAssets={ownedReactionAssets}
          bookmarkedReactionAssets={bookmarkedReactionAssets}
          recentReactions={recentReactions}
          showBookmarkAction={showBookmarkAction}
          isBookmarked={bookmarkedPostIds?.has(view.post.object_id) ?? false}
          isFocused={focusedPostObjectId === view.post.object_id}
        />
        </li>
      ))}
      {unavailableCount > 0 ? (
        <li className={itemClassName}>
          <UnavailablePostsNotice count={unavailableCount} />
        </li>
      ) : null}
      {hasMore ? (
        <li className={itemClassName}>
          {loadMoreError ? <p className='error'>{loadMoreError}</p> : null}
          {canAutoLoad && !loadMoreError ? <div ref={loadMoreRef} aria-hidden='true' /> : null}
          {(!canAutoLoad || manualFallback || loadMoreError) && onLoadMore ? (
            <Button variant='secondary' type='button' disabled={loadingMore} onClick={loadMore}>
              {loadingMore
                ? t('fallbacks.loadingMore')
                : loadMoreError
                  ? t('actions.retry')
                  : t('fallbacks.loadMore')}
            </Button>
          ) : null}
          {canAutoLoad && !loadMoreError && loadingMore ? (
            <p className='empty'>{t('fallbacks.loadingMore')}</p>
          ) : null}
        </li>
      ) : null}
    </ul>
  );
}
