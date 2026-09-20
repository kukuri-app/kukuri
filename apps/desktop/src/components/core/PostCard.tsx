import { useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Bookmark, Flag, Link2, Reply, Repeat2, Trash2 } from 'lucide-react';

import { formatPostDateTime } from '@/i18n/format';
import type {
  BookmarkedCustomReactionView,
  CommunityNodeManifestFetch,
  CommunityNodePoliciesResponse,
  ContentProvenance,
  CustomReactionAssetView,
  LinkPreviewFetcher,
  ReactionKeyInput,
  ReactionKeyView,
  RecentReactionView,
  SubmitCommunityNodeReportRequest,
  SubmitCommunityNodeReportResult,
} from '@/lib/api';
import { planAppealReportRouting, planReportRouting } from '@/lib/api/reportRouting';
import { usePostTrustGateCollapse } from './usePostTrustGateCollapse';
import { PostAdvisoryDetailsDialog, PostGatedContent } from './PostAdvisoryNotice';
import { usePostAdvisoryDetails } from './usePostAdvisoryDetails';
import { useReportManifests } from './useReportManifests';
import { copyTextToClipboard } from '@/lib/utils';
import {
  buildPostLink,
  type InternalSmartReference,
} from '@/lib/internalLinks';

import { Button } from '@/components/ui/button';
import { IconButton } from '@/components/ui/icon-button';
import {
  ContextActionMenu,
  contextActionMenuPositionFromKeyboard,
  contextActionMenuPositionFromPointer,
  type ContextActionMenuItem,
  type ContextActionMenuPosition,
} from '@/components/ui/context-action-menu';
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover';

import { AuthorAvatar } from './AuthorAvatar';
import { AuthorIdentityButton } from './AuthorIdentityButton';
import { MediaViewerDialog } from './MediaViewerDialog';
import { LinkPreviewCard } from './LinkPreviewCard';
import { PostMedia } from './PostMedia';
import { PostReactionChip } from './PostReactionChip';
import { ReactionPickerPopover } from './ReactionPickerPopover';
import {
  ReportRoutingDialog,
  type ReportAppealContext,
  type ReportSubmitInput,
} from './ReportRoutingDialog';
import type { ReportRoutingSubject } from './ReportRoutingDialog';
import { RelationshipBadge } from './RelationshipBadge';
import { SmartReferenceText } from './SmartReferenceText';
import { type ContentAdvisoryView, type PostCardView } from './types';

function sourceAuthorLabel(
  view: PostCardView['post']['repost_of'],
  unknownAuthorLabel: string
): string | null {
  if (!view) {
    return null;
  }
  return (
    view.source_author_display_name?.trim() ||
    view.source_author_name?.trim() ||
    unknownAuthorLabel
  );
}

type PostCardProps = {
  view: PostCardView;
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
  showBookmarkAction?: boolean;
  isBookmarked?: boolean;
  onToggleBookmark?: (post: PostCardView['post']) => void;
  onWithdraw?: (post: PostCardView['post']) => void;
  onRetryLocalPost?: (post: PostCardView['post']) => void;
  onRestoreLocalPost?: (post: PostCardView['post']) => void;
  onReactionPickerOpen?: () => void;
  onActivateReference?: (reference: InternalSmartReference) => void;
  onCopyLink?: (link: string) => void;
  isFocused?: boolean;
  // 解決済みの通報先 node へ通報を送信する。未指定なら通報導線を表示しない。
  onSubmitReport?: (
    request: SubmitCommunityNodeReportRequest
  ) => Promise<SubmitCommunityNodeReportResult>;
  // abuse contact（endpoint 無し node）の案内用コピー。
  onCopyReportContact?: (value: string) => void;
  // 通報画面を開いた時に観測元ノードの最新 manifest を取得する。未指定なら候補は作らない(#696)。
  onFetchReportManifest?: (baseUrl: string) => Promise<CommunityNodeManifestFetch>;
  /// #1192: 権利侵害を選んだときに提示する権利侵害申出ポリシーの取得(読み取りのみ)。
  onFetchNodePolicies?: (baseUrl: string, language?: string) => Promise<CommunityNodePoliciesResponse>;
  onMuteReportAuthor?: (authorPubkey: string) => Promise<void> | void;
  enableLinkPreview?: boolean;
  linkPreviewFetcher?: LinkPreviewFetcher;
};

function reactionKeyInputFromView(reaction: ReactionKeyView): ReactionKeyInput | null {
  if (reaction.reaction_key_kind === 'emoji' && reaction.emoji?.trim()) {
    return { kind: 'emoji', emoji: reaction.emoji };
  }
  if (reaction.reaction_key_kind === 'custom_asset' && reaction.custom_asset) {
    return { kind: 'custom_asset', asset: reaction.custom_asset };
  }
  return null;
}

export function PostCard({
  view,
  onOpenAuthor,
  onOpenThread,
  onOpenThreadInTopic,
  onReply,
  onRepost,
  onQuoteRepost,
  readOnly = false,
  onOpenOriginalTopic,
  localAuthorPubkey,
  mediaObjectUrls = {},
  ownedReactionAssets = [],
  bookmarkedReactionAssets = [],
  recentReactions = [],
  onToggleReaction,
  onBookmarkCustomReaction,
  showBookmarkAction = false,
  isBookmarked = false,
  onToggleBookmark,
  onWithdraw,
  onRetryLocalPost,
  onRestoreLocalPost,
  onReactionPickerOpen,
  onActivateReference,
  onCopyLink,
  isFocused = false,
  onSubmitReport,
  onCopyReportContact,
  onFetchReportManifest,
  onFetchNodePolicies,
  onMuteReportAuthor,
  enableLinkPreview = false,
  linkPreviewFetcher,
}: PostCardProps) {
  const { t } = useTranslation(['common', 'profile']);
  const { post, context } = view;
  const actionPost = view.actionPost ?? post;
  // #1061: 信頼値による折りたたみ（「表示する」はこの投稿だけに効く）。
  const trustGateCollapse = usePostTrustGateCollapse(view.trustGate, onOpenAuthor);
  const [repostMenuOpen, setRepostMenuOpen] = useState(false);
  const [reportDialogOpen, setReportDialogOpen] = useState(false);
  const [reportSubject, setReportSubject] = useState<ReportRoutingSubject>({
    kind: view.reportSubjectKind ?? 'post',
    id: post.object_id,
    label: view.authorLabel,
  });
  const [reportProvenance, setReportProvenance] = useState<ContentProvenance | undefined>(
    view.provenance
  );
  // #1055: Community Node の content advisory に対する異議申し立て。通報と同じ dialog を
  // appeal mode で使い、対象 risk signal を発行した node だけを送信先候補にする。
  const [reportAppeal, setReportAppeal] = useState<ReportAppealContext | null>(null);
  const advisoryDetails = usePostAdvisoryDetails();
  const [mediaViewerOpen, setMediaViewerOpen] = useState(false);
  const [mediaViewerIndex, setMediaViewerIndex] = useState(view.media.currentImageIndex ?? 0);
  const [reactionMenuPosition, setReactionMenuPosition] = useState<ContextActionMenuPosition | null>(
    null
  );
  const [reactionMenuAsset, setReactionMenuAsset] = useState<CustomReactionAssetView | null>(null);
  const [postMenuPosition, setPostMenuPosition] = useState<ContextActionMenuPosition | null>(null);
  const isUnavailableText = post.content_status === 'Missing' && post.content === '[blob pending]';
  const localState = post.local_state ?? null;
  const isWithdrawn = post.withdrawal != null;
  const interactionDisabled = localState !== null || isWithdrawn;
  const audienceChipLabel = view.audienceChipLabel ?? post.audience_label;
  const publishedTopicId = post.published_topic_id?.trim() || post.origin_topic_id?.trim() || null;
  const canonicalPostTopicId = view.threadTopicId?.trim() || publishedTopicId;
  const canonicalPostLink = canonicalPostTopicId
    ? buildPostLink(canonicalPostTopicId, view.threadTargetId, post.object_id)
    : null;
  const repostSource = post.repost_of ?? null;
  const replyPreview = post.reply_preview ?? null;
  const isQuoteRepost = post.object_kind === 'repost' && Boolean(post.repost_commentary?.trim());
  const isPureRepost = post.object_kind === 'repost' && !isQuoteRepost;
  // X-style: a pure repost renders the original post as the primary content, with the
  // reposter demoted to a small attribution header above the (source) author identity.
  const showRepostAsPrimary = isPureRepost && repostSource !== null && view.repostSourceAuthor != null;
  const showReplyContext = replyPreview !== null && !view.suppressReplyPreview;
  const primaryAuthor =
    showRepostAsPrimary && view.repostSourceAuthor
      ? view.repostSourceAuthor
      : { pubkey: post.author_pubkey, label: view.authorLabel, picture: view.authorPicture ?? null };
  const canReply = view.canReply ?? true;
  const canRepost = view.canRepost ?? false;
  const canReact = view.canReact ?? true;
  const canOpenThread = view.canOpenThread ?? !readOnly;
  const localStateLabel =
    localState === 'pending'
      ? t('feed.localPosting')
      : localState === 'syncing'
        ? t('feed.localSyncing')
        : localState === 'failed'
          ? t('feed.localFailed')
          : null;
  const primaryContent = showRepostAsPrimary && repostSource ? repostSource.content : post.content;
  const hasPrimaryContent = !isUnavailableText && primaryContent.trim().length > 0;
  const linkPreviewEligible =
    enableLinkPreview &&
    post.channel_id == null &&
    !view.adultContentGated &&
    !isWithdrawn &&
    !isUnavailableText &&
    localState === null &&
    hasPrimaryContent;
  const reactionSummary = post.reaction_summary ?? [];
  const myReactionKeys = useMemo(
    () => new Set((post.my_reactions ?? []).map((reaction) => reaction.normalized_reaction_key)),
    [post.my_reactions]
  );
  const pickerAssets = useMemo(() => {
    const deduped = new Map<string, CustomReactionAssetView>();
    for (const asset of [...ownedReactionAssets, ...bookmarkedReactionAssets]) {
      deduped.set(asset.asset_id, asset);
    }
    return [...deduped.values()];
  }, [bookmarkedReactionAssets, ownedReactionAssets]);

  const openPrimaryTarget = () => {
    const topicId = view.threadTopicId?.trim();
    if (topicId && onOpenThreadInTopic) {
      onOpenThreadInTopic(view.threadTargetId, topicId);
      return;
    }
    onOpenThread(view.threadTargetId);
  };

  // 通報先は現在選択中の対象の provenance（観測経路）と、通報画面を開いた時に取得成功した
  // 最新 manifest だけから解決する(#696)。provenance 不明 / 通報先未解決でも dialog は開き、
  // local action のみ案内する。
  const {
    manifests: reportManifests,
    resolving: reportResolving,
    resolveError: reportResolveError,
  } = useReportManifests({
    open: reportDialogOpen,
    provenance: reportProvenance,
    fetchManifest: onFetchReportManifest,
  });
  const reportPlan = useMemo(
    () =>
      reportAppeal
        ? planAppealReportRouting(reportAppeal.issuerNodeId, reportManifests)
        : planReportRouting(reportProvenance, reportManifests),
    [reportAppeal, reportProvenance, reportManifests]
  );
  const showReportAction = Boolean(onSubmitReport) && (!readOnly || view.allowReadOnlyReport === true);
  const gatedAdvisory = view.adultContentGated ? (view.contentAdvisory ?? null) : null;
  const hasGatedMediaFrame = !isWithdrawn && Boolean(view.media.kind) && view.media.state === 'gated';

  /// #1055: content advisory への異議申し立てを、通報と同じ dialog で開く。
  /// 対象は添付そのものへの判定なら media、投稿への判定なら post(#707 と同じ subject 規則)。
  const openAdvisoryAppeal = (advisory: ContentAdvisoryView) => {
    setReportSubject(
      advisory.subjectKind === 'blob_cid'
        ? { kind: 'media', id: advisory.subjectId, label: view.authorLabel }
        : { kind: 'post', id: advisory.subjectId, label: view.authorLabel }
    );
    setReportProvenance(view.provenance);
    setReportAppeal({
      riskSignalId: advisory.signalId,
      issuerNodeId: advisory.issuerNodeId,
    });
    setReportDialogOpen(true);
  };

  const handleSubmitReport = async (
    input: ReportSubmitInput
  ): Promise<SubmitCommunityNodeReportResult> => {
    if (!onSubmitReport) {
      throw new Error('report submission is not available');
    }
    const { candidate, reason, details, reporterContact } = input;
    const request: SubmitCommunityNodeReportRequest = {
      node_base_url: candidate.target.nodeBaseUrl,
      report_endpoint: candidate.target.reportEndpoint ?? '',
      subject_kind: reportSubject.kind,
      subject_id: reportSubject.id,
      capability: candidate.target.capability,
      reason,
      details: details.trim() ? details.trim() : null,
      reporter_contact: reporterContact.trim() ? reporterContact.trim() : null,
      // #1055: 異議申し立ては対象 risk signal を伴う(ADR 0046 §6.3)。
      appeal: input.appeal,
    };
    return onSubmitReport(request);
  };

  const reactionMenuItems = useMemo(() => {
    if (!reactionMenuAsset) {
      return [];
    }
    const canSaveReaction =
      Boolean(onBookmarkCustomReaction) &&
      reactionMenuAsset.owner_pubkey !== localAuthorPubkey;
    return [
      {
        id: 'save',
        label: t('actions.save'),
        disabled: !canSaveReaction,
        onSelect: async () => {
          if (canSaveReaction && onBookmarkCustomReaction) {
            onBookmarkCustomReaction(reactionMenuAsset);
          }
        },
      },
      {
        id: 'copy-hash',
        label: t('actions.copyHash'),
        onSelect: async () => {
          await copyTextToClipboard(reactionMenuAsset.blob_hash);
        },
      },
    ];
  }, [localAuthorPubkey, onBookmarkCustomReaction, reactionMenuAsset, t]);
  const postMenuItems = useMemo(() => {
    const identifiers = view.identifierCopy ?? {
      postId: post.object_id,
      envelopeId: post.envelope_id,
      authorId: primaryAuthor.pubkey,
    };
    const items: ContextActionMenuItem[] = [];
    const { postId, envelopeId, authorId } = identifiers;
    if (postId) {
      items.push({
        id: 'copy-post-id',
        label: t('actions.copyPostId'),
        onSelect: async () => {
          await copyTextToClipboard(postId);
        },
      });
    }
    if (envelopeId) {
      items.push({
        id: 'copy-envelope-id',
        label: t('actions.copyEnvelopeId'),
        onSelect: async () => {
          await copyTextToClipboard(envelopeId);
        },
      });
    }
    if (authorId) {
      items.push({
        id: 'copy-author-id',
        label: t('actions.copyAuthorId'),
        onSelect: async () => {
          await copyTextToClipboard(authorId);
        },
      });
    }
    return items;
  }, [post.envelope_id, post.object_id, primaryAuthor.pubkey, t, view.identifierCopy]);

  const renderReferencedCard = (
    source:
      | {
          authorLabel: string | null;
          content: string;
          topic: string;
          attachments: { hash: string }[];
          replyTo?: string | null;
        }
      | null,
    eyebrow: string,
    author?: PostCardView['repostSourceAuthor']
  ) => {
    if (!source) {
      return null;
    }
    return (
      <div className='repost-source-card post-layout-safe'>
        <div className='repost-source-meta'>
          <span className='repost-source-eyebrow'>{eyebrow}</span>
          <span className='repost-source-topic'>
            <span>{t('labels.sourceTopic')}</span>
            <SmartReferenceText
              text={source.topic}
              className='shell-topic-link-label'
              onActivateReference={onActivateReference}
            />
          </span>
          {source.attachments.length > 0 ? (
            <span className='repost-source-attachments'>
              {t('feed.moreMedia', { count: source.attachments.length })}
            </span>
          ) : null}
        </div>
        <div className='post-body repost-source-body post-layout-safe'>
          {author ? (
            <button
              type='button'
              className='repost-source-author author-link'
              onClick={(event) => {
                event.stopPropagation();
                onOpenAuthor(author.pubkey);
              }}
            >
              <AuthorAvatar label={author.label} picture={author.picture ?? null} size='sm' />
              <span>{author.label}</span>
            </button>
          ) : source.authorLabel ? (
            <strong className='post-title post-copy-wrap'>{source.authorLabel}</strong>
          ) : null}
          {source.content.trim().length > 0 ? (
            <SmartReferenceText
              text={source.content}
              className='post-copy-wrap'
              onActivateReference={onActivateReference}
              mentionAuthors={view.mentionAuthors}
              onOpenMention={onOpenAuthor}
              externalLinks
            />
          ) : null}
        </div>
      </div>
    );
  };

  const replyContext = (
    <>
      {!view.adultContentGated && showReplyContext && view.replyParentAuthor && replyPreview ? (
        <div className='post-reply-context'>
          <button
            type='button'
            className='post-reply-context-avatar'
            aria-label={view.replyParentAuthor.label}
            onClick={(event) => {
              event.stopPropagation();
              onOpenAuthor(view.replyParentAuthor!.pubkey);
            }}
          >
            <AuthorAvatar
              label={view.replyParentAuthor.label}
              picture={view.replyParentAuthor.picture ?? null}
              size='sm'
            />
          </button>
          <div className='post-reply-context-main'>
            <button
              type='button'
              className='post-reply-context-author author-link'
              onClick={(event) => {
                event.stopPropagation();
                onOpenAuthor(view.replyParentAuthor!.pubkey);
              }}
            >
              {t('feed.replyingTo', { author: view.replyParentAuthor.label })}
            </button>
            {replyPreview.content.trim().length > 0 ? (
              <div
                className='post-reply-context-body post-copy-wrap'
                role={!readOnly && canOpenThread ? 'button' : undefined}
                tabIndex={!readOnly && canOpenThread ? 0 : undefined}
                onClick={!readOnly && canOpenThread ? openPrimaryTarget : undefined}
                onKeyDown={(event) => {
                  if (readOnly || !canOpenThread || event.target !== event.currentTarget) return;
                  if (event.key === 'Enter' || event.key === ' ') {
                    event.preventDefault();
                    openPrimaryTarget();
                  }
                }}
              >
                <SmartReferenceText
                  text={replyPreview.content}
                  className='post-copy-wrap'
                  onActivateReference={onActivateReference}
                  mentionAuthors={view.mentionAuthors}
                  onOpenMention={onOpenAuthor}
                  externalLinks
                />
              </div>
            ) : replyPreview.attachments.length > 0 ? (
              <span className='post-reply-context-body'>{t('feed.moreMedia', { count: replyPreview.attachments.length })}</span>
            ) : null}
          </div>
        </div>
      ) : null}
    </>
  );

  const contentBlock = (
    <>
      <div className='post-body post-layout-safe'>
        {isWithdrawn ? (
          <p className='topic-diagnostic topic-diagnostic-secondary' role='status'>
            {t('feed.withdrawnPost')}
          </p>
        ) : view.adultContentGated ? (
          // #858: 成人向けとして申告された投稿は、表示設定 OFF の間は本文も代替表示にする。
          // #1055: 判定元が Community Node の推定のときは、断定せず発行元・根拠を示し、
          // 異議申し立てへの導線を添える(ADR 0046 §6.3)。
          // #1108: その説明は詳細 dialog に置き、一覧には開く操作だけを出す。
          <PostGatedContent
            objectId={post.object_id}
            gatedBy={view.gatedBy}
            bodyText={view.gatedBodyText}
            advisory={gatedAdvisory}
            onOpenDetails={gatedAdvisory && !hasGatedMediaFrame ? advisoryDetails.openDetails : undefined}
          />
        ) : isUnavailableText ? (
          view.showUnavailableDiagnostics ? (
            <p className='topic-diagnostic topic-diagnostic-secondary' role='status'>
              {t('feed.contentUnavailable')}
            </p>
          ) : null
        ) : hasPrimaryContent ? (
          <strong className='post-title post-copy-wrap'>
            <SmartReferenceText
              text={primaryContent}
              className='post-copy-wrap'
              onActivateReference={onActivateReference}
              mentionAuthors={view.mentionAuthors}
              onOpenMention={onOpenAuthor}
              externalLinks
            />
          </strong>
        ) : null}

        {view.adultContentGated ? null : showRepostAsPrimary && repostSource ? (
          <div className='post-source-topic'>
            <span>{t('labels.sourceTopic')}</span>
            <SmartReferenceText
              text={repostSource.source_topic_id}
              className='shell-topic-link-label'
              onActivateReference={onActivateReference}
            />
            {repostSource.attachments.length > 0 ? (
              <span className='repost-source-attachments'>
                {t('feed.moreMedia', { count: repostSource.attachments.length })}
              </span>
            ) : null}
          </div>
        ) : repostSource ? (
          renderReferencedCard(
            {
              authorLabel: sourceAuthorLabel(repostSource, t('fallbacks.unknownAuthor')),
              content: repostSource.content,
              topic: repostSource.source_topic_id,
              attachments: repostSource.attachments,
              replyTo: repostSource.reply_to ?? null,
            },
            isQuoteRepost ? t('feed.quoteRepost') : t('feed.reposted'),
            view.repostSourceAuthor
          )
        ) : null}
        <LinkPreviewCard
          content={primaryContent}
          enabled={linkPreviewEligible}
          fetcher={linkPreviewFetcher}
        />
      </div>
      {readOnly && publishedTopicId ? (
        <div className='topic-diagnostic topic-diagnostic-secondary'>
          <span>{t('feed.originTopic', { ns: 'profile' })}</span>
          <SmartReferenceText
            text={publishedTopicId}
            className='shell-topic-link-label'
            onActivateReference={onActivateReference}
          />
        </div>
      ) : null}
    </>
  );

  const card = (
    <article
      className={
        context === 'thread'
          ? `post-card post-card-thread post-layout-safe${isFocused ? ' post-card-targeted' : ''}`
          : `post-card post-layout-safe${isFocused ? ' post-card-targeted' : ''}`
      }
      aria-busy={localState === 'pending' || localState === 'syncing'}
      data-post-object-id={post.object_id}
      tabIndex={isFocused ? -1 : undefined}
    >
      {showRepostAsPrimary ? (
        <button
          type='button'
          className='post-repost-attribution author-link'
          onClick={() => onOpenAuthor(post.author_pubkey)}
        >
          <Repeat2 className='size-3.5' aria-hidden='true' />
          <AuthorAvatar
            label={view.authorLabel}
            picture={view.authorPicture ?? null}
            size='sm'
            className='post-repost-attribution-avatar'
          />
          <span>{t('feed.repostedBy', { author: view.authorLabel })}</span>
        </button>
      ) : null}

      <div className='post-meta'>
        <AuthorIdentityButton
          label={primaryAuthor.label}
          picture={primaryAuthor.picture ?? null}
          avatarTestId={`${post.object_id}-author-avatar`}
          onClick={() => onOpenAuthor(primaryAuthor.pubkey)}
        />
        <div className='post-meta-trailing'>
          <RelationshipBadge label={view.relationshipLabel} />
          <span className='post-meta-chip'>{audienceChipLabel}</span>
          <time className='post-timestamp' dateTime={new Date(post.created_at * 1000).toISOString()}>{formatPostDateTime(post.created_at * 1000)}</time>
        </div>
      </div>

      {!isWithdrawn && view.media.kind ? (
        <PostMedia
          media={view.media}
          showUnavailableDiagnostic={view.showUnavailableDiagnostics}
          onOpenGatedDetails={gatedAdvisory ? advisoryDetails.openDetails : undefined}
          onOpenImage={(index) => {
            setMediaViewerIndex(index);
            setMediaViewerOpen(true);
          }}
          onReportVideo={
            showReportAction
              ? (hash) => {
                  // 動画添付そのものを media として通報する。観測元は親投稿から引き継いだ
                  // media.provenance(正本 blob)をそのまま使う(#697)。
                  setReportSubject({ kind: 'media', id: hash, label: view.authorLabel });
                  setReportProvenance(view.media.provenance);
                  setReportDialogOpen(true);
                }
              : undefined
          }
        />
      ) : null}

      {readOnly || !canOpenThread ? (
        <div
          className='post-link post-layout-safe'
          role='group'
          tabIndex={0}
          data-testid='post-identifier-target'
          onContextMenu={(event) =>
            setPostMenuPosition(contextActionMenuPositionFromPointer(event))
          }
          onKeyDown={(event) => {
            const position = contextActionMenuPositionFromKeyboard(event);
            if (position) setPostMenuPosition(position);
          }}
        >
          {contentBlock}
        </div>
      ) : (
        <div
          className='post-link post-layout-safe'
          role='button'
          tabIndex={0}
          data-testid='post-identifier-target'
          onContextMenu={(event) =>
            setPostMenuPosition(contextActionMenuPositionFromPointer(event))
          }
          onClick={openPrimaryTarget}
          onKeyDown={(event) => {
            const position = contextActionMenuPositionFromKeyboard(event);
            if (position) {
              setPostMenuPosition(position);
              return;
            }
            if (event.key === 'Enter' || event.key === ' ') {
              event.preventDefault();
              openPrimaryTarget();
            }
          }}
        >
          {contentBlock}
        </div>
      )}

      {localStateLabel ? (
        <div className='topic-diagnostic topic-diagnostic-secondary' aria-live='polite'>
          <span>{localStateLabel}</span>
          {post.local_error ? <span>{post.local_error}</span> : null}
          {localState === 'failed' && onRetryLocalPost ? (
            <Button variant='secondary' type='button' onClick={() => onRetryLocalPost(post)}>
              {t('actions.retry')}
            </Button>
          ) : null}
          {localState === 'failed' && onRestoreLocalPost ? (
            <Button variant='secondary' type='button' onClick={() => onRestoreLocalPost(post)}>
              {t('actions.restoreDraft')}
            </Button>
          ) : null}
        </div>
      ) : null}

      <div className='post-actions'>
        {readOnly ? (
          <>
            {publishedTopicId && onOpenOriginalTopic ? (
              <Button
                variant='secondary'
                type='button'
                onClick={() => onOpenOriginalTopic(publishedTopicId)}
              >
                {t('feed.openOriginalTopic', { ns: 'profile' })}
              </Button>
            ) : null}
            {canonicalPostLink && onCopyLink ? (
              <IconButton
                variant='secondary'
                className='post-action-button'
                type='button'
                label={t('actions.copyLink')}
                onClick={() => onCopyLink(canonicalPostLink)}
              >
                <Link2 className='size-4' aria-hidden='true' />
              </IconButton>
            ) : null}
            {showReportAction ? (
              <IconButton
                variant='secondary'
                className='post-action-button'
                type='button'
                label={t('report.actionLabel', { ns: 'shell' })}
                onClick={() => {
                  setReportSubject({
                    kind: view.reportSubjectKind ?? 'post',
                    id: post.object_id,
                    label: view.authorLabel,
                  });
                  setReportProvenance(view.provenance);
                  setReportDialogOpen(true);
                }}
              >
                <Flag className='size-4' aria-hidden='true' />
              </IconButton>
            ) : null}
          </>
        ) : (
          <>
            {reactionSummary.length > 0 && canReact && !interactionDisabled ? (
              <div className='post-reaction-summary'>
                {reactionSummary.map((reaction) => {
                  const reactionKey = reactionKeyInputFromView(reaction);
                  const customAsset = reaction.custom_asset ?? null;
                  const previewUrl =
                    customAsset && typeof mediaObjectUrls[customAsset.blob_hash] === 'string'
                      ? mediaObjectUrls[customAsset.blob_hash]
                      : null;
                  return (
                    <PostReactionChip
                      key={reaction.normalized_reaction_key}
                      reaction={reaction}
                      active={myReactionKeys.has(reaction.normalized_reaction_key)}
                      previewUrl={previewUrl}
                      onToggle={
                        reactionKey && onToggleReaction
                          ? () => onToggleReaction(actionPost, reactionKey)
                          : undefined
                      }
                      onOpenContextMenu={
                        customAsset
                          ? (position) => {
                              setReactionMenuAsset(customAsset);
                              setReactionMenuPosition(position);
                            }
                          : undefined
                      }
                    />
                  );
                })}
              </div>
            ) : null}
            {canReact && !interactionDisabled ? (
              <ReactionPickerPopover
                post={actionPost}
                recentReactions={recentReactions}
                assets={pickerAssets}
                mediaObjectUrls={mediaObjectUrls}
                onToggleReaction={onToggleReaction}
                onOpen={() => onReactionPickerOpen?.()}
              />
            ) : null}
            {!interactionDisabled && canRepost && (onRepost || onQuoteRepost) ? (
              <Popover open={repostMenuOpen} onOpenChange={setRepostMenuOpen}>
                <PopoverTrigger asChild>
                  <IconButton
                    variant='secondary'
                    className='post-action-button'
                    type='button'
                    label={t('actions.repost')}
                  >
                    <Repeat2 className='size-4' aria-hidden='true' />
                  </IconButton>
                </PopoverTrigger>
                <PopoverContent align='end' className='post-action-popover'>
                  <div className='post-action-popover-stack'>
                    {onRepost ? (
                      <Button
                        variant='secondary'
                        type='button'
                        onClick={() => {
                          setRepostMenuOpen(false);
                          onRepost(actionPost);
                        }}
                      >
                        {t('actions.repost')}
                      </Button>
                    ) : null}
                    {onQuoteRepost ? (
                      <Button
                        variant='secondary'
                        type='button'
                        onClick={() => {
                          setRepostMenuOpen(false);
                          onQuoteRepost(actionPost);
                        }}
                      >
                        {t('actions.quoteRepost')}
                      </Button>
                    ) : null}
                  </div>
                </PopoverContent>
              </Popover>
            ) : null}
            {canReply && !interactionDisabled ? (
              <IconButton
                variant='secondary'
                className='post-action-button'
                type='button'
                label={t('actions.reply')}
                onClick={() => onReply(actionPost)}
              >
                <Reply className='size-4' aria-hidden='true' />
              </IconButton>
            ) : null}
            {canonicalPostLink && onCopyLink ? (
              <IconButton
                variant='secondary'
                className='post-action-button'
                type='button'
                label={t('actions.copyLink')}
                onClick={() => onCopyLink(canonicalPostLink)}
              >
                <Link2 className='size-4' aria-hidden='true' />
              </IconButton>
            ) : null}
            {showBookmarkAction && onToggleBookmark ? (
              <IconButton
                variant='secondary'
                className={`post-action-button${isBookmarked ? ' post-action-button-active' : ''}`}
                type='button'
                label={isBookmarked ? t('actions.removeBookmark') : t('actions.bookmark')}
                aria-pressed={isBookmarked}
                onClick={() => onToggleBookmark(actionPost)}
              >
                <Bookmark
                  className='size-4'
                  fill={isBookmarked ? 'currentColor' : 'none'}
                  aria-hidden='true'
                />
              </IconButton>
            ) : null}
            {!isWithdrawn && actionPost.author_pubkey === localAuthorPubkey && onWithdraw ? (
              <IconButton
                variant='secondary'
                className='post-action-button'
                type='button'
                label={t('actions.withdrawPost')}
                onClick={() => {
                  if (window.confirm(t('actions.confirmWithdrawPost'))) {
                    onWithdraw(actionPost);
                  }
                }}
              >
                <Trash2 className='size-4' aria-hidden='true' />
              </IconButton>
            ) : null}
            {showReportAction ? (
              <IconButton
                variant='secondary'
                className='post-action-button'
                type='button'
                label={t('report.actionLabel', { ns: 'shell' })}
                onClick={() => {
                  setReportSubject({
                    kind: view.reportSubjectKind ?? 'post',
                    id: post.object_id,
                    label: view.authorLabel,
                  });
                  setReportProvenance(view.provenance);
                  setReportDialogOpen(true);
                }}
              >
                <Flag className='size-4' aria-hidden='true' />
              </IconButton>
            ) : null}
          </>
        )}
      </div>

      <MediaViewerDialog
        items={view.media.imageGalleryItems ?? []}
        index={mediaViewerIndex}
        open={mediaViewerOpen}
        onOpenChange={setMediaViewerOpen}
        onIndexChange={setMediaViewerIndex}
        onReportCurrent={
          showReportAction
            ? (hash) => {
                setMediaViewerOpen(false);
                const attachment = view.media.imageGalleryItems?.find((item) => item.hash === hash);
                setReportSubject({ kind: 'media', id: hash, label: view.authorLabel });
                setReportProvenance(attachment?.provenance);
                setReportDialogOpen(true);
              }
            : undefined
        }
      />
      <ContextActionMenu
        open={postMenuPosition !== null}
        position={postMenuPosition}
        items={postMenuItems}
        onClose={() => setPostMenuPosition(null)}
      />
      <ContextActionMenu
        open={reactionMenuAsset !== null}
        position={reactionMenuPosition}
        items={reactionMenuItems}
        onClose={() => {
          setReactionMenuAsset(null);
          setReactionMenuPosition(null);
        }}
      />
      {gatedAdvisory ? (
        <PostAdvisoryDetailsDialog
          details={advisoryDetails}
          objectId={post.object_id}
          gatedBy={view.gatedBy}
          advisory={gatedAdvisory}
          onAppeal={showReportAction && onSubmitReport ? () => openAdvisoryAppeal(gatedAdvisory) : undefined}
        />
      ) : null}
      {showReportAction && onSubmitReport ? (
        <ReportRoutingDialog
          open={reportDialogOpen}
          onCloseAutoFocus={advisoryDetails.onReportCloseAutoFocus}
          onOpenChange={(open) => {
            setReportDialogOpen(open);
            // 通常の通報へ戻すため、閉じるときに appeal 文脈を捨てる(#1055)。
            if (!open) setReportAppeal(null);
          }}
          appeal={reportAppeal}
          subject={reportSubject}
          plan={reportPlan}
          onSubmit={handleSubmitReport}
          onCopyContact={onCopyReportContact}
          onFetchNodePolicies={onFetchNodePolicies}
          resolving={reportResolving}
          resolveError={reportResolveError}
          localActions={
            onMuteReportAuthor && post.author_pubkey !== localAuthorPubkey ? (
              <Button
                variant='secondary'
                type='button'
                onClick={() => void onMuteReportAuthor(post.author_pubkey)}
              >
                {t('actions.mute')}
              </Button>
            ) : undefined
          }
        />
      ) : null}
    </article>
  );

  if (trustGateCollapse) return trustGateCollapse;

  return (
    <div className={showReplyContext && !view.adultContentGated ? 'post-reply-group post-layout-safe' : 'post-layout-safe'}>
      {replyContext}
      {card}
    </div>
  );
}
