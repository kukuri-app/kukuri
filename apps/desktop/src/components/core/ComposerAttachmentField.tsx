import type { ChangeEventHandler } from 'react';
import { useId, useRef } from 'react';

import { useTranslation } from 'react-i18next';

import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { MediaBookmarkPicker } from '@/shell/page/MediaBookmarkPicker';

type ComposerAttachmentFieldProps = {
  inputKey: number;
  count: number;
  disabled?: boolean;
  onChange: ChangeEventHandler<HTMLInputElement>;
  onBookmarkSelection?: (item: import('@/shell/store').DraftMediaItem) => void;
};

/** 投稿の添付欄(選ぶボタン・選んだ数・対応形式)。投稿の composer と公開スレッドの作成で共用する(#1889)。 */
export function ComposerAttachmentField({ inputKey, count, disabled = false, onChange, onBookmarkSelection }: ComposerAttachmentFieldProps) {
  const { t } = useTranslation(['common']);
  const inputRef = useRef<HTMLInputElement>(null);
  const statusId = useId();
  // #965: 対応形式は選ぶ前に見える位置へ置き、ボタンの説明としても渡す。
  const formatsId = useId();
  return (
    <div className='file-field file-field-compact flex flex-col'>
      <span>{t('common:fallbacks.attachment')}</span>
      <div className='flex min-w-0 flex-wrap items-center gap-2'>
        <Button
          type='button'
          variant='secondary'
          disabled={disabled}
          aria-describedby={`${statusId} ${formatsId}`}
          onClick={() => inputRef.current?.click()}
        >
          {t('composer.chooseFiles')}
        </Button>
        {onBookmarkSelection ? <MediaBookmarkPicker disabled={disabled} onSelect={onBookmarkSelection} /> : null}
        <span id={statusId} role='status' className='text-sm text-muted-foreground'>
          {count === 0 ? t('composer.noFilesSelected') : t('composer.selectedFiles', { count })}
        </span>
      </div>
      <p id={formatsId} className='composer-attachment-formats'>
        {t('composer.supportedFormats')}
      </p>
      <Input
        key={inputKey}
        ref={inputRef}
        hidden
        className='hidden'
        aria-label={t('common:fallbacks.attachment')}
        type='file'
        accept='image/*,video/*'
        multiple
        disabled={disabled}
        onChange={onChange}
      />
    </div>
  );
}
