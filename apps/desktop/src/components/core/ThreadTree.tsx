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
import { PostCard } from './PostCard';
import { type PostCardView } from './types';
import { useInfiniteScrollSentinel } from './useInfiniteScrollSentinel';

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
  loadingMore?: boolean;
  onLoadMore?: () => void;
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
  loadingMore = false,
  onLoadMore,
  onSubmitReport,
  onCopyReportContact,
  onFetchReportManifest,
  onFetchNodePolicies,
  onMuteReportAuthor,
}: ThreadTreeProps) {
  const { t } = useTranslation('common');
  const nodes = useMemo(() => buildThreadTree(posts), [posts]);
  const { sentinelRef: loadMoreRef, canAutoLoad } = useInfiniteScrollSentinel({
    hasMore,
    loadingMore,
    onLoadMore,
  });

  // 行が 0 件でも、続きがある(`hasMore`)あいだは、続きを読む手段を描く(#1239。`TimelineFeed` と同じ)。
  if (nodes.length === 0 && !hasMore) {
    return <p className='empty'>{emptyCopy}</p>;
  }

  return (
    <ul className='thread-tree'>
      {nodes.map(({ view, depth, rails, isLast }) => {
        const visualDepth = Math.min(depth, MAX_VISUAL_DEPTH);
        const visibleRails = rails.slice(0, Math.max(0, visualDepth - 1));
        return (
          <li key={view.post.object_id} className='thread-tree-item' data-depth={visualDepth}>
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
      {hasMore ? (
        <li className='thread-tree-item' data-depth={0}>
          {canAutoLoad ? <div ref={loadMoreRef} aria-hidden='true' /> : null}
          {!canAutoLoad && onLoadMore ? (
            <Button variant='secondary' type='button' onClick={() => onLoadMore()}>
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
