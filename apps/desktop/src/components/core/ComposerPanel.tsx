import type {
  ChangeEventHandler,
  ClipboardEventHandler,
  FormEventHandler,
  KeyboardEventHandler,
} from 'react';
import { useId, useRef } from 'react';

import { X } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { Button } from '@/components/ui/button';
import { IconButton } from '@/components/ui/icon-button';
import { Input } from '@/components/ui/input';
import { Textarea } from '@/components/ui/textarea';
import { clipboardImageFiles } from '@/lib/attachments';

import { AuthorAvatar } from './AuthorAvatar';
import { ComposerDraftPreviewList } from './ComposerDraftPreviewList';
import { MentionHoverCard } from './MentionHoverCard';
import { PostCard } from './PostCard';
import { useMentionAutocomplete } from './useMentionAutocomplete';
import {
  type ComposerDraftMediaView,
  type MentionAuthorView,
  type MentionCandidate,
  type PostCardView,
} from './types';

function mentionAuthorFromCandidate(candidate: MentionCandidate): MentionAuthorView {
  return {
    pubkey: candidate.pubkey,
    label: candidate.label,
    displayName: candidate.displayName,
    name: candidate.name,
    aboutPreview: candidate.about?.slice(0, 50) ?? null,
    picture: candidate.picture,
  };
}

type ReplyTargetView = {
  content: string;
  audienceLabel: string;
};

type RepostTargetView = {
  content: string;
  authorLabel: string;
};

type ComposerPanelProps = {
  mode?: 'post' | 'reply' | 'message';
  value: string;
  onChange: ChangeEventHandler<HTMLTextAreaElement>;
  onSubmit: FormEventHandler<HTMLFormElement>;
  attachmentInputKey: number;
  onAttachmentSelection: ChangeEventHandler<HTMLInputElement>;
  onPasteImageFiles?: (files: File[]) => void | Promise<void>;
  draftMediaItems: ComposerDraftMediaView[];
  onRemoveDraftAttachment: (itemId: string) => void;
  composerError?: string | null;
  audienceLabel: string;
  replyTarget?: ReplyTargetView | null;
  repostTarget?: RepostTargetView | null;
  sourcePreview?: PostCardView | null;
  onClearReply: () => void;
  onClearRepost?: () => void;
  attachmentsDisabled?: boolean;
  submitDisabled?: boolean;
  mentionCandidates?: MentionCandidate[];
  onValueChange?: (next: string) => void;
  // #858: 成人向けの自己申告トグル。onAdultLabeledChange 未指定なら出さない
  // (メッセージ等ラベル非対応の surface)。
  adultLabeled?: boolean;
  onAdultLabeledChange?: (labeled: boolean) => void;
};

const EMPTY_MENTION_CANDIDATES: MentionCandidate[] = [];

export function ComposerPanel({
  mode = 'post',
  value,
  onChange,
  onSubmit,
  attachmentInputKey,
  onAttachmentSelection,
  onPasteImageFiles,
  draftMediaItems,
  onRemoveDraftAttachment,
  composerError,
  audienceLabel,
  replyTarget,
  repostTarget,
  sourcePreview,
  onClearReply,
  onClearRepost,
  attachmentsDisabled = false,
  submitDisabled = false,
  mentionCandidates = EMPTY_MENTION_CANDIDATES,
  onValueChange,
  adultLabeled = false,
  onAdultLabeledChange,
}: ComposerPanelProps) {
  const { t } = useTranslation(['common']);
  const attachmentInputRef = useRef<HTMLInputElement>(null);
  const attachmentStatusId = useId();
  // #965: 対応形式は選ぶ前に見える位置へ置き、ボタンの説明としても渡す。
  const attachmentFormatsId = useId();
  const clearActiveTarget = replyTarget ? onClearReply : onClearRepost;
  const bannerAriaLabel = replyTarget ? t('composer.clearReply') : t('composer.clearQuoteRepost');
  const {
    textareaRef: mentionTextareaRef,
    isOpen: mentionOpen,
    items: mentionItems,
    activeIndex: mentionActiveIndex,
    onKeyDown: onMentionKeyDown,
    onSelectionChange: onMentionSelectionChange,
    selectCandidate: selectMention,
    setActiveIndex: setMentionActiveIndex,
  } = useMentionAutocomplete({
    value,
    candidates: mentionCandidates,
    onValueChange,
  });
  const onComposerKeyDown: KeyboardEventHandler<HTMLTextAreaElement> = (event) => {
    onMentionKeyDown(event);
    // #964: IME 変換中(isComposing)の Ctrl+Enter は確定操作であり送信しない。
    if (
      event.defaultPrevented ||
      submitDisabled ||
      event.key !== 'Enter' ||
      !event.ctrlKey ||
      event.nativeEvent.isComposing
    ) {
      return;
    }
    event.preventDefault();
    event.currentTarget.form?.requestSubmit();
  };
  const onComposerPaste: ClipboardEventHandler<HTMLTextAreaElement> = (event) => {
    if (attachmentsDisabled || !onPasteImageFiles) {
      return;
    }
    const images = clipboardImageFiles(event.clipboardData);
    if (images.length === 0) {
      return;
    }
    event.preventDefault();
    void onPasteImageFiles(images);
  };

  return (
    <form className='composer' onSubmit={onSubmit}>
      {replyTarget || repostTarget ? (
        <div className='reply-banner'>
          <span className='composer-target-summary'>
            <strong>{replyTarget ? t('composer.replying') : t('composer.quoteReposting')}</strong>
            {replyTarget ? (
              <span className='post-copy-wrap'>{replyTarget.content}</span>
            ) : repostTarget ? (
              <span className='post-copy-wrap'>
                {t('composer.sourcePost')} · {repostTarget.authorLabel}: {repostTarget.content}
              </span>
            ) : null}
          </span>
          <IconButton
            className='shell-icon-button'
            variant='ghost'
            type='button'
            label={bannerAriaLabel}
            onClick={() => clearActiveTarget?.()}
          >
            <X className='size-5' aria-hidden='true' />
          </IconButton>
        </div>
      ) : null}

      {sourcePreview ? (
        <div className='composer-source-preview'>
          <div className='topic-diagnostic topic-diagnostic-secondary'>
            <span>{t('composer.sourcePost')}</span>
            <span>
              {sourcePreview.audience.kind === 'private'
                ? sourcePreview.audience.channelLabel ?? t('audience.privateChannel')
                : t(sourcePreview.audience.kind === 'public' ? 'audience.public' : 'audience.publicThread')}
            </span>
          </div>
          <PostCard
            view={sourcePreview}
            onOpenAuthor={() => undefined}
            onOpenThread={() => undefined}
            onReply={() => undefined}
            readOnly
          />
        </div>
      ) : null}

      <div className='composer-mention-anchor'>
        <Textarea
          ref={mentionTextareaRef}
          value={value}
          onChange={(event) => {
            onChange(event);
            onMentionSelectionChange();
          }}
          onPaste={onComposerPaste}
          onKeyDown={onComposerKeyDown}
          onKeyUp={onMentionSelectionChange}
          onClick={onMentionSelectionChange}
          onSelect={onMentionSelectionChange}
          aria-expanded={mentionOpen}
          aria-controls={mentionOpen ? 'composer-mention-listbox' : undefined}
          placeholder={
            replyTarget || mode === 'reply'
              ? t('composer.writeReply')
              : repostTarget
                ? t('composer.writeQuoteRepost')
                : mode === 'message'
                  ? t('composer.writeMessage')
                  : t('composer.writePost')
          }
        />
        {mentionOpen ? (
          <ul
            id='composer-mention-listbox'
            className='composer-mention-list'
            role='listbox'
            aria-label={t('composer.mentionSuggestionsLabel')}
          >
            {mentionItems.map((candidate, index) => (
              <li key={candidate.pubkey} role='presentation'>
                <MentionHoverCard
                  pubkey={candidate.pubkey}
                  label={candidate.label}
                  author={mentionAuthorFromCandidate(candidate)}
                >
                  <button
                    type='button'
                    role='option'
                    aria-selected={index === mentionActiveIndex}
                    className={
                      index === mentionActiveIndex
                        ? 'composer-mention-option composer-mention-option-active'
                        : 'composer-mention-option'
                    }
                    onMouseDown={(event) => {
                      if (event.button !== 0) return;
                      event.preventDefault();
                      selectMention(candidate);
                    }}
                    onMouseEnter={() => setMentionActiveIndex(index)}
                  >
                    <AuthorAvatar label={candidate.label} picture={candidate.picture ?? null} size='sm' />
                    <span className='composer-mention-option-text'>
                      <span className='composer-mention-option-label'>{candidate.label}</span>
                    </span>
                  </button>
                </MentionHoverCard>
              </li>
            ))}
          </ul>
        ) : null}
      </div>

      <div className='file-field file-field-compact flex flex-col'>
        <span>{t('common:fallbacks.attachment')}</span>
        <div className='flex min-w-0 flex-wrap items-center gap-2'>
          <Button
            type='button'
            variant='secondary'
            disabled={attachmentsDisabled}
            aria-describedby={`${attachmentStatusId} ${attachmentFormatsId}`}
            onClick={() => attachmentInputRef.current?.click()}
          >
            {t('composer.chooseFiles')}
          </Button>
          <span id={attachmentStatusId} role='status' className='text-sm text-muted-foreground'>
            {draftMediaItems.length === 0
              ? t('composer.noFilesSelected')
              : t('composer.selectedFiles', { count: draftMediaItems.length })}
          </span>
        </div>
        <p id={attachmentFormatsId} className='composer-attachment-formats'>
          {t('composer.supportedFormats')}
        </p>
        <Input
          key={attachmentInputKey}
          ref={attachmentInputRef}
          hidden
          className='hidden'
          aria-label={t('common:fallbacks.attachment')}
          type='file'
          accept='image/*,video/*'
          multiple
          disabled={attachmentsDisabled}
          onChange={onAttachmentSelection}
        />
      </div>

      {onAdultLabeledChange && mode !== 'message' ? (
        <label className='topic-diagnostic topic-diagnostic-secondary flex items-center gap-2'>
          <input
            type='checkbox'
            checked={adultLabeled}
            onChange={(event) => onAdultLabeledChange(event.currentTarget.checked)}
            data-testid='composer-adult-label-toggle'
          />
          <span>{t('composer.adultLabel')}</span>
        </label>
      ) : null}

      {composerError ? (
        <p className='error error-inline' role='alert'>
          {composerError}
        </p>
      ) : null}

      <ComposerDraftPreviewList items={draftMediaItems} onRemove={onRemoveDraftAttachment} />

      <div className='topic-diagnostic topic-diagnostic-secondary'>
        <span>{t('labels.audience')}: {audienceLabel}</span>
      </div>

      <Button type='submit' disabled={submitDisabled}>
        {replyTarget || mode === 'reply'
          ? t('actions.reply')
          : repostTarget
            ? t('actions.quoteRepost')
            : mode === 'message'
              ? t('actions.send')
              : t('actions.publish')}
      </Button>
    </form>
  );
}
