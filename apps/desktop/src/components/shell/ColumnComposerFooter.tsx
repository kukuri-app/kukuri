import { MessageSquare, PenLine, Reply } from 'lucide-react';
import type { ChangeEvent, FormEvent, KeyboardEvent } from 'react';
import { useEffect, useRef } from 'react';
import { useTranslation } from 'react-i18next';

import { ComposerPanel } from '@/components/core/ComposerPanel';
import type { MentionCandidate } from '@/components/core/types';
import { Button } from '@/components/ui/button';
import { composerDraftMediaViews } from '@/shell/media';
import { authorDisplayLabel } from '@/shell/presentation';
import {
  columnDraftKey,
  createColumnDraft,
  setColumnDraft,
  type ColumnDraftTarget,
} from '@/shell/slices/columnDrafts';
import { useDesktopShellFieldSetter, useDesktopShellStore } from '@/shell/store';
import { useWebBackLayer } from '@/lib/useWebBackLayer';

type ColumnComposerFooterProps = {
  active: boolean;
  destinationLabel: string;
  locale: string;
  mentionCandidates?: MentionCandidate[];
  onActivate: () => void;
  /// #964: 案内(設定 > キーボード操作)を開く。未指定なら hint 文だけを表示する。
  onOpenKeyboardHelp?: () => void;
  onAttachmentSelection: (
    target: ColumnDraftTarget,
    event: ChangeEvent<HTMLInputElement>
  ) => Promise<void>;
  onAttachmentPaste: (target: ColumnDraftTarget, files: File[]) => Promise<void>;
  onRemoveAttachment: (target: ColumnDraftTarget, itemId: string) => void;
  onSubmit: (target: ColumnDraftTarget, event: FormEvent<HTMLFormElement>) => Promise<void>;
  target: ColumnDraftTarget;
};

const ICON_BY_ACTION = {
  post: PenLine,
  reply: Reply,
  message: MessageSquare,
} as const;

const LABEL_KEY_BY_ACTION = {
  post: 'actions.publish',
  reply: 'actions.reply',
  message: 'actions.message',
} as const;

export function ColumnComposerFooter({
  active,
  destinationLabel,
  locale,
  mentionCandidates,
  onActivate,
  onOpenKeyboardHelp,
  onAttachmentSelection,
  onAttachmentPaste,
  onRemoveAttachment,
  onSubmit,
  target,
}: ColumnComposerFooterProps) {
  const { t } = useTranslation(['common']);
  const key = columnDraftKey(target);
  const storedDraft = useDesktopShellStore((state) => state.columnDraftsByKey[key]);
  const draft = storedDraft ?? createColumnDraft(target);
  const setColumnDraftsByKey = useDesktopShellFieldSetter('columnDraftsByKey');
  const settingsOpen = useDesktopShellStore((state) => state.shellChromeState.settingsOpen);
  const Icon = ICON_BY_ACTION[target.action];
  const actionLabel = t(LABEL_KEY_BY_ACTION[target.action]);
  const draftMediaViews = composerDraftMediaViews(draft.mediaItems, locale);
  const updateDraft = (update: Parameters<typeof setColumnDraft>[2]) => {
    setColumnDraftsByKey((current) => setColumnDraft(current, target, update));
  };
  // #964: 閉じた後は開始元(折りたたみボタン)へ focus を戻す。restart 復元などで
  // expanded が外部から false になった場合は focus を奪わない。
  const primaryActionRef = useRef<HTMLButtonElement>(null);
  const restoreFocusRef = useRef(false);
  useEffect(() => {
    if (draft.expanded || !restoreFocusRef.current) return;
    restoreFocusRef.current = false;
    primaryActionRef.current?.focus();
  }, [draft.expanded]);
  const collapse = () => {
    restoreFocusRef.current = true;
    updateDraft((current) => ({ ...current, expanded: false }));
  };
  useWebBackLayer(active && draft.expanded, collapse);
  // #964: Esc は投稿作成を閉じる。メンション候補が消費した Escape(defaultPrevented)と
  // IME 変換中は扱わず、消費した場合は global の Escape cascade(#765)が pane を閉じないよう
  // preventDefault する。下書き・返信先・投稿先は store に残す。
  // 案内(設定 drawer)を hint link から開いた直後は focus が composer に残るため、drawer が
  // 開いている間の Escape は drawer 側(global cascade)に委ねる。
  const onComposerKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (
      event.key !== 'Escape' ||
      event.defaultPrevented ||
      event.nativeEvent.isComposing ||
      settingsOpen
    ) {
      return;
    }
    event.preventDefault();
    collapse();
  };

  if (!draft.expanded) {
    return (
      <Button
        ref={primaryActionRef}
        className='shell-column-primary-action min-h-11 min-w-11'
        variant='primary'
        size={active ? 'default' : 'icon'}
        type='button'
        aria-label={t('composer.actionTo', {
          action: actionLabel,
          destination: destinationLabel,
        })}
        onClick={() => {
          onActivate();
          updateDraft((current) => ({ ...current, expanded: true, error: null }));
        }}
      >
        <Icon className='size-4' aria-hidden='true' />
        {active ? <span>{actionLabel}</span> : null}
      </Button>
    );
  }

  return (
    <div className='shell-column-composer' onKeyDown={onComposerKeyDown}>
      <div className='shell-column-composer-heading'>
        <strong>{actionLabel}</strong>
        <Button variant='ghost' size='sm' className='min-h-11' type='button' onClick={collapse}>
          {t('actions.close')}
        </Button>
      </div>
      <p className='shell-column-composer-hint'>
        <span>{t('composer.keyboardHint')}</span>
        {onOpenKeyboardHelp ? (
          <button type='button' className='shell-column-composer-hint-link' onClick={onOpenKeyboardHelp}>
            {t('composer.keyboardHelp')}
          </button>
        ) : null}
      </p>
      <ComposerPanel
        mode={target.action}
        value={draft.content}
        onChange={(event) =>
          updateDraft((current) => ({ ...current, content: event.target.value, error: null }))
        }
        onValueChange={(content) =>
          updateDraft((current) => ({ ...current, content, error: null }))
        }
        onSubmit={(event) => void onSubmit(target, event)}
        attachmentInputKey={draft.attachmentInputKey}
        onAttachmentSelection={(event) => void onAttachmentSelection(target, event)}
        onPasteImageFiles={(files) => onAttachmentPaste(target, files)}
        draftMediaItems={draftMediaViews}
        onRemoveDraftAttachment={(itemId) => onRemoveAttachment(target, itemId)}
        adultLabeled={draft.adultLabeled}
        onAdultLabeledChange={
          target.action === 'message'
            ? undefined
            : (labeled) =>
                updateDraft((current) => ({ ...current, adultLabeled: labeled }))
        }
        composerError={draft.error}
        audienceLabel={destinationLabel}
        replyTarget={
          draft.replyTarget
            ? {
                content: draft.replyTarget.content,
                audienceLabel: draft.replyTarget.audience_label,
              }
            : null
        }
        repostTarget={
          draft.repostTarget
            ? {
                content: draft.repostTarget.content,
                authorLabel: authorDisplayLabel(
                  draft.repostTarget.author_pubkey,
                  draft.repostTarget.author_display_name,
                  draft.repostTarget.author_name
                ),
              }
            : null
        }
        onClearReply={() =>
          updateDraft((current) => ({ ...current, replyTarget: null, error: null }))
        }
        onClearRepost={() =>
          updateDraft((current) => ({ ...current, repostTarget: null, error: null }))
        }
        attachmentsDisabled={draft.pending || Boolean(draft.repostTarget)}
        submitDisabled={draft.pending}
        mentionCandidates={mentionCandidates}
      />
    </div>
  );
}
