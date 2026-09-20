import {
  type ButtonHTMLAttributes,
  type KeyboardEvent,
  type MouseEvent,
  type ReactElement,
  type ReactNode,
  useMemo,
  useState,
} from 'react';
import { useTranslation } from 'react-i18next';
import { Flag } from 'lucide-react';

import { Card, CardHeader } from '@/components/ui/card';
import { Button } from '@/components/ui/button';
import { IconButton } from '@/components/ui/icon-button';
import {
  ContextActionMenu,
  contextActionMenuPositionFromKeyboard,
  contextActionMenuPositionFromPointer,
  type ContextActionMenuPosition,
} from '@/components/ui/context-action-menu';
import type {
  CommunityNodeManifestFetch,
  CommunityNodePoliciesResponse,
  SubmitCommunityNodeReportRequest,
  SubmitCommunityNodeReportResult,
} from '@/lib/api';
import { contentProvenanceFromView } from '@/lib/api/provenance';
import { planReportRouting } from '@/lib/api/reportRouting';
import { copyTextToClipboard } from '@/lib/utils';

import { AuthorAvatar } from './AuthorAvatar';
import { BlockedActionTooltip } from './BlockedActionTooltip';
import { RelationshipBadge } from './RelationshipBadge';
import { ReportRoutingDialog, type ReportSubmitInput } from './ReportRoutingDialog';
import { useReportManifests } from './useReportManifests';
import { type AuthorDetailView } from './types';

type AuthorDetailCardProps = {
  view: AuthorDetailView;
  localAuthorPubkey: string;
  onToggleRelationship: (authorPubkey: string, following: boolean) => void;
  onToggleMute: (authorPubkey: string, muted: boolean) => void;
  onToggleBlock?: (authorPubkey: string, blocking: boolean) => void;
  onOpenDirectMessage?: (authorPubkey: string) => void;
  communityNodeAdvisory?: ReactNode;
  /// #1061: 信頼値による折りたたみの例外設定（作者詳細から設定・解除する）。
  trustDisplayException?: ReactNode;
  onSubmitReport?: (
    request: SubmitCommunityNodeReportRequest
  ) => Promise<SubmitCommunityNodeReportResult>;
  onCopyReportContact?: (value: string) => void;
  onFetchReportManifest?: (baseUrl: string) => Promise<CommunityNodeManifestFetch>;
  /// #1192: 権利侵害を選んだときに提示する権利侵害申出ポリシーの取得(読み取りのみ)。
  onFetchNodePolicies?: (baseUrl: string, language?: string) => Promise<CommunityNodePoliciesResponse>;
};

export function AuthorDetailCard({
  view,
  localAuthorPubkey,
  onToggleRelationship,
  onToggleMute,
  onToggleBlock,
  onOpenDirectMessage,
  communityNodeAdvisory,
  trustDisplayException,
  onSubmitReport,
  onCopyReportContact,
  onFetchReportManifest,
  onFetchNodePolicies,
}: AuthorDetailCardProps) {
  const { t } = useTranslation(['common']);
  const author = view.author;
  const relationshipLabel = view.summary?.label ?? null;
  const showFollowAction = author?.author_pubkey !== localAuthorPubkey;
  const showMessageAction = Boolean(
    author &&
      author.author_pubkey !== localAuthorPubkey &&
      view.canMessage &&
      onOpenDirectMessage
  );
  const showMuteAction = Boolean(author && author.author_pubkey !== localAuthorPubkey);
  // #992: ブロック中はフォローとメッセージを無効にし、理由を tooltip で示す。フォロー解除は残す。
  const blocking = Boolean(author?.blocking);
  const followBlocked = blocking && !author?.following;
  const [reportOpen, setReportOpen] = useState(false);
  const [identifierMenuPosition, setIdentifierMenuPosition] =
    useState<ContextActionMenuPosition | null>(null);
  const provenance = useMemo(
    () => contentProvenanceFromView(author?.provenance),
    [author?.provenance]
  );
  // 通報画面を開いた時に取得成功した最新 manifest だけを候補源にする(#696)。
  const {
    manifests: reportManifests,
    resolving: reportResolving,
    resolveError: reportResolveError,
  } = useReportManifests({
    open: reportOpen,
    provenance,
    fetchManifest: onFetchReportManifest,
  });
  const reportPlan = useMemo(
    () => planReportRouting(provenance, reportManifests),
    [provenance, reportManifests]
  );
  const identifierMenuItems = useMemo(
    () =>
      author
        ? [
            {
              id: 'copy-author-id',
              label: t('actions.copyAuthorId'),
              onSelect: async () => {
                await copyTextToClipboard(author.author_pubkey);
              },
            },
          ]
        : [],
    [author, t]
  );

  const submitReport = async (input: ReportSubmitInput) => {
    if (!author || !onSubmitReport) throw new Error('report submission is not available');
    return onSubmitReport({
      node_base_url: input.candidate.target.nodeBaseUrl,
      report_endpoint: input.candidate.target.reportEndpoint ?? '',
      subject_kind: 'profile',
      subject_id: author.author_pubkey,
      capability: input.candidate.target.capability,
      reason: input.reason,
      details: input.details.trim() || null,
      reporter_contact: input.reporterContact.trim() || null,
    });
  };

  const actionButton = (label: string, onClick: () => void) => (
    <button className='button button-secondary' type='button' onClick={onClick}>
      {label}
    </button>
  );
  const withBlockedReason = (
    blocked: boolean,
    reason: string,
    button: ReactElement<ButtonHTMLAttributes<HTMLButtonElement>>
  ) =>
    blocked ? <BlockedActionTooltip reason={reason}>{button}</BlockedActionTooltip> : button;

  return (
    <Card
      className='author-detail'
      data-testid={author ? 'author-identifier-target' : undefined}
      tabIndex={author ? 0 : undefined}
      onContextMenu={
        author
          ? (event: MouseEvent<HTMLElement>) =>
              setIdentifierMenuPosition(contextActionMenuPositionFromPointer(event))
          : undefined
      }
      onKeyDown={
        author
          ? (event: KeyboardEvent<HTMLElement>) => {
              const position = contextActionMenuPositionFromKeyboard(event);
              if (position) setIdentifierMenuPosition(position);
            }
          : undefined
      }
    >
      {author ? (
        <>
          <CardHeader className='author-detail-toolbar'>
            <div className='author-detail-summary'>
              <div className='author-detail-hero'>
                <AuthorAvatar
                  label={view.displayLabel}
                  picture={view.pictureSrc ?? null}
                  size='sm'
                  testId='author-detail-avatar'
                />
                <div className='author-detail-identity'>
                  <div className='author-detail-heading'>
                    <strong className='author-detail-name author-detail-break'>{view.displayLabel}</strong>
                    {relationshipLabel ? (
                      <RelationshipBadge
                        label={relationshipLabel}
                        className='author-detail-relationship'
                      />
                    ) : null}
                    {blocking ? (
                      <span className='relationship-badge relationship-badge-direct author-detail-relationship'>
                        {t('relationships.blocking')}
                      </span>
                    ) : null}
                  </div>
                </div>
              </div>
              <div className='author-detail-copy-stack'>
                <p className='author-detail-copy author-detail-break'>
                  {author.about?.trim() || t('fallbacks.noBio')}
                </p>
              </div>
            </div>
          </CardHeader>

          {showFollowAction || showMuteAction || showMessageAction ? (
            <div className='author-detail-actions'>
              <div className='author-detail-action-buttons'>
                {showMessageAction
                  ? withBlockedReason(
                      blocking,
                      t('relationships.blockedMessageReason'),
                      actionButton(t('actions.message', { defaultValue: 'Message' }), () =>
                        onOpenDirectMessage?.(author.author_pubkey)
                      )
                    )
                  : null}
                {showFollowAction
                  ? withBlockedReason(
                      followBlocked,
                      t('relationships.blockedFollowReason'),
                      actionButton(
                        view.summary?.followActionLabel === 'Unfollow'
                          ? t('actions.unfollow')
                          : t('actions.follow'),
                        () => onToggleRelationship(author.author_pubkey, author.following)
                      )
                    )
                  : null}
                {showMuteAction ? (
                  <button
                    className='button button-secondary'
                    type='button'
                    onClick={() => onToggleMute(author.author_pubkey, author.muted)}
                  >
                    {view.summary?.muteActionLabel === 'Unmute'
                      ? t('actions.unmute', { defaultValue: 'Unmute' })
                      : t('actions.mute', { defaultValue: 'Mute' })}
                  </button>
                ) : null}
                {showMuteAction && onToggleBlock ? (
                  <button
                    className='button button-secondary'
                    type='button'
                    onClick={() => onToggleBlock(author.author_pubkey, author.blocking)}
                  >
                    {t(author.blocking ? 'actions.unblock' : 'actions.block')}
                  </button>
                ) : null}
                {author && onSubmitReport ? (
                  <IconButton
                    variant='secondary'
                    type='button'
                    label={t('report.actionLabel', { ns: 'shell' })}
                    onClick={() => setReportOpen(true)}
                  >
                    <Flag className='size-4' aria-hidden='true' />
                  </IconButton>
                ) : null}
              </div>
            </div>
          ) : null}
          {trustDisplayException}
          {communityNodeAdvisory}
          {author && onSubmitReport ? (
            <ReportRoutingDialog
              open={reportOpen}
              onOpenChange={setReportOpen}
              subject={{ kind: 'profile', id: author.author_pubkey, label: view.displayLabel }}
              plan={reportPlan}
              onSubmit={submitReport}
              onCopyContact={onCopyReportContact}
              onFetchNodePolicies={onFetchNodePolicies}
              resolving={reportResolving}
              resolveError={reportResolveError}
              localActions={
                showMuteAction ? (
                  <>
                    <Button
                      variant='secondary'
                      type='button'
                      onClick={() => onToggleMute(author.author_pubkey, author.muted)}
                    >
                      {t(author.muted ? 'actions.unmute' : 'actions.mute')}
                    </Button>
                    {onToggleBlock ? (
                      <Button
                        variant='secondary'
                        type='button'
                        onClick={() => onToggleBlock(author.author_pubkey, author.blocking)}
                      >
                        {t(author.blocking ? 'actions.unblock' : 'actions.block')}
                      </Button>
                    ) : null}
                  </>
                ) : undefined
              }
            />
          ) : null}
        </>
      ) : (
        <p className='empty'>{t('fallbacks.selectAuthor')}</p>
      )}

      {view.authorError ? <p className='error error-inline'>{view.authorError}</p> : null}
      <ContextActionMenu
        open={identifierMenuPosition !== null && identifierMenuItems.length > 0}
        position={identifierMenuPosition}
        items={identifierMenuItems}
        onClose={() => setIdentifierMenuPosition(null)}
      />
    </Card>
  );
}
