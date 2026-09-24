import { useMemo } from 'react';
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

import { buildThreadTree } from './buildThreadTree';
import { UnavailablePostsNotice } from './UnavailablePostsNotice';
import { PostCard } from './PostCard';
import { type PostCardView } from './types';
import { useInfiniteScrollSentinel } from './useInfiniteScrollSentinel';
import { useWindowScrollAnchor } from './useWindowScrollAnchor';

const MAX_VISUAL_DEPTH = 6;

type ThreadTreeProps = {
  posts: PostCardView[];
  emptyCopy: string;
  onOpenAuthor: (authorPubkey: string) => void;
  onOpenThread: (threadId: string) => void;
  onOpenThreadInTopic?: (threadId: string, topicId: string) => void;
  onReply: (post: PostCardView['post']) => void;
  onRepost?: (post: PostCardView['post']) => void;
  onQuoteRepost?: (post: PostCardView['post']) => void;
  localAuthorPubkey?: string;
  mediaObjectUrls?: Record<string, string | null>;
  ownedReactionAssets?: CustomReactionAssetView[];
  bookmarkedReactionAssets?: BookmarkedCustomReactionView[];
  recentReactions?: RecentReactionView[];
  onToggleReaction?: (post: PostCardView['post'], reactionKey: ReactionKeyInput) => void;
  onBookmarkCustomReaction?: (asset: CustomReactionAssetView) => void;
  onReactionPickerOpen?: () => void;
  onRetryLocalPost?: (post: PostCardView['post']) => void;
  onRestoreLocalPost?: (post: PostCardView['post']) => void;
  onWithdraw?: (post: PostCardView['post']) => void;
  onActivateReference?: (reference: InternalSmartReference) => void;
  onCopyPostLink?: (link: string) => void;
  focusedPostObjectId?: string | null;
  hasMore?: boolean;
  /** 読んだ範囲にあるが、まだ取得できていない返信の数(#1239 AC-4)。 */
  unavailableCount?: number;
  loadingMore?: boolean;
  onLoadMore?: () => void;
  returnToLatest?: boolean;
  onReturnToLatest?: () => void;
  onSubmitReport?: (
    request: SubmitCommunityNodeReportRequest
  ) => Promise<SubmitCommunityNodeReportResult>;
  onCopyReportContact?: (value: string) => void;
  onFetchReportManifest?: (baseUrl: string) => Promise<CommunityNodeManifestFetch>;
  /// #1192: 権利侵害を選んだときに提示する権利侵害申出ポリシーの取得(読み取りのみ)。
  onFetchNodePolicies?: (baseUrl: string, language?: string) => Promise<CommunityNodePoliciesResponse>;
  onMuteReportAuthor?: (authorPubkey: string) => Promise<void> | void;
};

export function ThreadTree({
  posts,
  emptyCopy,
  onOpenAuthor,
  onOpenThread,
  onOpenThreadInTopic,
  onReply,
  onRepost,
  onQuoteRepost,
  localAuthorPubkey,
  mediaObjectUrls = {},
  ownedReactionAssets = [],
  bookmarkedReactionAssets = [],
  recentReactions = [],
  onToggleReaction,
  onBookmarkCustomReaction,
  onReactionPickerOpen,
  onRetryLocalPost,
  onRestoreLocalPost,
  onWithdraw,
  onActivateReference,
  onCopyPostLink,
  focusedPostObjectId,
  hasMore = false,
  unavailableCount = 0,
  loadingMore = false,
  onLoadMore,
  returnToLatest = false,
  onReturnToLatest,
  onSubmitReport,
  onCopyReportContact,
  onFetchReportManifest,
  onFetchNodePolicies,
  onMuteReportAuthor,
}: ThreadTreeProps) {
  const { t } = useTranslation('common');
  const nodes = useMemo(() => buildThreadTree(posts), [posts]);
  const { listRef, loadMore } = useWindowScrollAnchor(posts, onLoadMore);
  const { sentinelRef: loadMoreRef, canAutoLoad, manualFallback } = useInfiniteScrollSentinel({
    hasMore,
    loadingMore,
    onLoadMore: loadMore,
  });

  // 行が 0 件でも、続きがある(`hasMore`)あいだは、続きを読む手段を描く(#1239。`TimelineFeed` と同じ)。
  if (nodes.length === 0 && !hasMore && unavailableCount <= 0 && !returnToLatest) {
    return <p className='empty'>{emptyCopy}</p>;
  }

  return (
    <ul ref={listRef} className='thread-tree'>
      {returnToLatest && onReturnToLatest ? (
        <li className='thread-tree-item'>
          <Button variant='secondary' type='button' onClick={onReturnToLatest}>
            {t('feed.backToLatest')}
          </Button>
        </li>
      ) : null}
      {nodes.map(({ view, depth, rails, isLast }) => {
        const visualDepth = Math.min(depth, MAX_VISUAL_DEPTH);
        const visibleRails = rails.slice(0, Math.max(0, visualDepth - 1));
        return (
          <li key={view.post.object_id} className='thread-tree-item' data-post-id={view.post.object_id} data-depth={visualDepth}>
            {visualDepth > 0 ? (
              <span className='thread-tree-rails' aria-hidden='true'>
                {visibleRails.map((continues, railIndex) => (
                  <span
                    key={`${view.post.object_id}-rail-${railIndex}`}
                    className={continues ? 'thread-rail thread-rail-line' : 'thread-rail'}
                  />
                ))}
                <span
                  className='thread-rail thread-rail-elbow'
                  data-last={isLast ? 'true' : 'false'}
                />
              </span>
            ) : null}
            <div className='thread-tree-body'>
            <PostCard
              enableLinkPreview
              view={view}
              onOpenAuthor={onOpenAuthor}
              onOpenThread={onOpenThread}
              onOpenThreadInTopic={onOpenThreadInTopic}
              onReply={onReply}
              onRepost={onRepost}
              onQuoteRepost={onQuoteRepost}
              localAuthorPubkey={localAuthorPubkey}
              mediaObjectUrls={mediaObjectUrls}
              ownedReactionAssets={ownedReactionAssets}
              bookmarkedReactionAssets={bookmarkedReactionAssets}
              recentReactions={recentReactions}
              onToggleReaction={onToggleReaction}
              onBookmarkCustomReaction={onBookmarkCustomReaction}
              onReactionPickerOpen={onReactionPickerOpen}
              onRetryLocalPost={onRetryLocalPost}
              onRestoreLocalPost={onRestoreLocalPost}
              onWithdraw={onWithdraw}
              onActivateReference={onActivateReference}
              onCopyLink={onCopyPostLink}
              isFocused={focusedPostObjectId === view.post.object_id}
              onSubmitReport={onSubmitReport}
              onCopyReportContact={onCopyReportContact}
              onFetchReportManifest={onFetchReportManifest}
              onFetchNodePolicies={onFetchNodePolicies}
              onMuteReportAuthor={onMuteReportAuthor}
            />
            </div>
          </li>
        );
      })}
      {unavailableCount > 0 ? (
        <li className='thread-tree-item' data-depth={0}>
          <UnavailablePostsNotice count={unavailableCount} />
        </li>
      ) : null}
      {hasMore ? (
        <li className='thread-tree-item' data-depth={0}>
          {canAutoLoad ? <div ref={loadMoreRef} aria-hidden='true' /> : null}
          {(!canAutoLoad || manualFallback) && onLoadMore ? (
            <Button variant='secondary' type='button' disabled={loadingMore} onClick={loadMore}>
              {loadingMore ? t('fallbacks.loadingMore') : t('fallbacks.loadMore')}
            </Button>
          ) : null}
          {canAutoLoad && loadingMore ? (
            <p className='empty'>{t('fallbacks.loadingMore')}</p>
          ) : null}
        </li>
      ) : null}
    </ul>
  );
}
