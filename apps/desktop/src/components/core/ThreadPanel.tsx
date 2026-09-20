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

import { ThreadTree } from './ThreadTree';
import { type PostCardView, type ThreadPanelState } from './types';

type ThreadPanelProps = {
  state: ThreadPanelState;
  posts: PostCardView[];
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

export function ThreadPanel({
  state,
  posts,
  onOpenAuthor,
  onOpenThread,
  onOpenThreadInTopic,
  onReply,
  onRepost,
  onQuoteRepost,
  localAuthorPubkey,
  mediaObjectUrls,
  ownedReactionAssets,
  bookmarkedReactionAssets,
  recentReactions,
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
}: ThreadPanelProps) {
  return (
    <div className='shell-main-stack'>
      <ThreadTree
        posts={posts}
        emptyCopy={state.emptyCopy}
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
        onCopyPostLink={onCopyPostLink}
        focusedPostObjectId={focusedPostObjectId}
        hasMore={hasMore}
        loadingMore={loadingMore}
        onLoadMore={onLoadMore}
        onSubmitReport={onSubmitReport}
        onCopyReportContact={onCopyReportContact}
        onFetchReportManifest={onFetchReportManifest}
        onFetchNodePolicies={onFetchNodePolicies}
        onMuteReportAuthor={onMuteReportAuthor}
      />
    </div>
  );
}
