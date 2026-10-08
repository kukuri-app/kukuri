import { Fragment, type MouseEvent, type KeyboardEvent } from 'react';
import { useTranslation } from 'react-i18next';
import { DoorOpen, SmilePlus } from 'lucide-react';

import {
  parseSmartText,
  shortenReferenceId,
  splitReactionSetLinks,
  type InternalSmartReference,
} from '@/lib/internalLinks';
import { topicDisplayName } from '@/lib/topicId';
import { useExternalLinkOpener } from '@/lib/useExternalLinkOpener';
import { cn } from '@/lib/utils';
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from '@/components/ui/tooltip';

import { MentionHoverCard } from './MentionHoverCard';
import { type MentionAuthorView } from './types';

type SmartReferenceTextProps = {
  text: string;
  className?: string;
  onActivateReference?: (reference: InternalSmartReference) => void;
  mentionAuthors?: Record<string, MentionAuthorView>;
  onOpenMention?: (pubkey: string) => void;
  externalLinks?: boolean;
};

function tokenKindLabel(
  tokenKind: 'invite' | 'grant' | 'share',
  t: ReturnType<typeof useTranslation>['t']
): string {
  if (tokenKind === 'invite') {
    return t('channels:previewDialog.tokenKinds.invite');
  }
  if (tokenKind === 'grant') {
    return t('channels:previewDialog.tokenKinds.grant');
  }
  return t('channels:previewDialog.tokenKinds.share');
}

function tokenAudienceLabel(
  tokenKind: 'invite' | 'grant' | 'share',
  t: ReturnType<typeof useTranslation>['t']
): string {
  if (tokenKind === 'invite') {
    return t('channels:audienceOptions.invite_only');
  }
  if (tokenKind === 'grant') {
    return t('channels:audienceOptions.friend_only');
  }
  return t('channels:audienceOptions.friend_plus');
}

function referenceLabel(
  reference: InternalSmartReference,
  t: ReturnType<typeof useTranslation>['t']
): string {
  if (reference.kind === 'topic') {
    return topicDisplayName(reference.topic);
  }
  if (reference.kind === 'post') {
    return `${t('common:labels.post')} ${shortenReferenceId(
      reference.focusObjectId ?? reference.threadId
    )}`;
  }
  if (reference.kind === 'live') {
    return `${t('shell:primarySections.live')} ${shortenReferenceId(reference.sessionId)}`;
  }
  if (reference.kind === 'game') {
    return `${t('shell:primarySections.game')} ${shortenReferenceId(reference.roomId)}`;
  }
  if (reference.kind === 'reaction_set') {
    return t('common:reactions.importSet');
  }
  const channelLabel =
    reference.metadata.channelLabel?.trim() || reference.metadata.channelId?.trim();
  if (channelLabel) {
    return `${channelLabel} / ${tokenAudienceLabel(reference.tokenKind, t)}`;
  }
  return `${tokenKindLabel(reference.tokenKind, t)} ${t('channels:previewDialog.tokenLabelSuffix')}`;
}

function handleReferenceAction(
  event: MouseEvent<HTMLButtonElement> | KeyboardEvent<HTMLButtonElement>,
  reference: InternalSmartReference,
  onActivateReference?: (reference: InternalSmartReference) => void
) {
  event.preventDefault();
  event.stopPropagation();
  onActivateReference?.(reference);
}

export function SmartReferenceText({
  text,
  className,
  onActivateReference,
  mentionAuthors,
  onOpenMention,
  externalLinks = false,
}: SmartReferenceTextProps) {
  const { t } = useTranslation(['channels', 'common', 'shell']);
  const externalLink = useExternalLinkOpener();
  const lines = parseSmartText(text);

  return (
    <span className={cn('smart-reference-text', className)}>
      {lines.map((segments, lineIndex) => (
        <Fragment key={`${lineIndex}-${segments.length}`}>
          {segments.map((segment, segmentIndex) => {
            if (segment.kind === 'text') {
              return (
                <span key={`${lineIndex}-${segmentIndex}`} className={className}>
                  {segment.text}
                </span>
              );
            }
            if (segment.kind === 'mention') {
              const author = mentionAuthors?.[segment.pubkey] ?? null;
              const mentionLabel = `@${author?.label ?? segment.label}`;
              if (!author) {
                return (
                  <span key={`${lineIndex}-${segmentIndex}`} className='smart-mention-plain'>
                    {mentionLabel}
                  </span>
                );
              }
              return (
                <MentionHoverCard
                  key={`${lineIndex}-${segmentIndex}`}
                  pubkey={segment.pubkey}
                  label={segment.label}
                  author={author}
                >
                  <button
                    type='button'
                    className='smart-mention-chip'
                    onClick={(event) => {
                      event.preventDefault();
                      event.stopPropagation();
                      onOpenMention?.(segment.pubkey);
                    }}
                  >
                    {mentionLabel}
                  </button>
                </MentionHoverCard>
              );
            }
            if (segment.kind === 'external_url') {
              if (!externalLinks) {
                return (
                  <span key={`${lineIndex}-${segmentIndex}`} className={className}>
                    {segment.href}
                  </span>
                );
              }
              return (
                <a
                  key={`${lineIndex}-${segmentIndex}`}
                  className='smart-external-link'
                  href={segment.href}
                  target='_blank'
                  rel='noopener noreferrer'
                  aria-disabled={externalLink.pending || undefined}
                  onClick={(event) => {
                    event.stopPropagation();
                    externalLink.linkProps.onClick(event);
                  }}
                  onAuxClick={(event) => {
                    event.stopPropagation();
                    externalLink.linkProps.onAuxClick(event);
                  }}
                  onKeyDown={(event) => event.stopPropagation()}
                >
                  {segment.href}
                </a>
              );
            }
            const label = referenceLabel(segment.reference, t);
            const button = (
              <button
                key={`${lineIndex}-${segmentIndex}`}
                type='button'
                className={cn(
                  'smart-reference-chip',
                  segment.reference.kind === 'share_token' && 'smart-reference-chip-access-preview'
                )}
                title={'route' in segment.reference ? segment.reference.route : undefined}
                onClick={(event) =>
                  handleReferenceAction(event, segment.reference, onActivateReference)
                }
                onKeyDown={(event) => {
                  if (event.key === 'Enter' || event.key === ' ') {
                    handleReferenceAction(event, segment.reference, onActivateReference);
                  }
                }}
              >
                <span>{label}</span>
                {segment.reference.kind === 'share_token' ? (
                  <DoorOpen className='size-3.5' aria-hidden='true' />
                ) : null}
                {segment.reference.kind === 'reaction_set' ? (
                  <SmilePlus className='size-3.5' aria-hidden='true' />
                ) : null}
              </button>
            );
            if (segment.reference.kind !== 'share_token') {
              return button;
            }
            return (
              <TooltipProvider key={`${lineIndex}-${segmentIndex}`} delayDuration={180}>
                <Tooltip>
                  <TooltipTrigger asChild>{button}</TooltipTrigger>
                  <TooltipContent>{segment.reference.token}</TooltipContent>
                </Tooltip>
              </TooltipProvider>
            );
          })}
          {lineIndex < lines.length - 1 ? <br /> : null}
        </Fragment>
      ))}
    </span>
  );
}

/** #1232 AC-4: DM の本文。リアクションのセットの共有用の文字列だけを「取り込む」ボタンにし、他は文字のまま示す。 */
export function ReactionSetText({
  text,
  onImport,
}: {
  text: string;
  onImport: (setHash: string) => Promise<void>;
}) {
  const { t } = useTranslation('common');
  return splitReactionSetLinks(text).map(({ text: part, setHash }, index) =>
    setHash ? (
      <button key={index} type='button' className='smart-reference-chip' onClick={() => void onImport(setHash)}>
        <span>{t('common:reactions.importSet')}</span>
        <SmilePlus className='size-3.5' aria-hidden='true' />
      </button>
    ) : (
      <Fragment key={index}>{part}</Fragment>
    )
  );
}
