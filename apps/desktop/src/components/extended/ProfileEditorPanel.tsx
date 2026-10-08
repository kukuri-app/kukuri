import type { ChangeEventHandler, FormEventHandler } from 'react';
import { useTranslation } from 'react-i18next';

import { Button } from '@/components/ui/button';
import { Card, CardHeader } from '@/components/ui/card';
import { Field } from '@/components/ui/field';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { Notice } from '@/components/ui/notice';
import { Textarea } from '@/components/ui/textarea';
import { parseProfileNip05 } from '@/lib/profileNip05';
import { copyTextToClipboard } from '@/lib/utils';

import { type ExtendedPanelStatus, type ProfileEditorFields } from './types';

type ProfileEditorPanelProps = {
  authorLabel: string;
  status: ExtendedPanelStatus;
  saving: boolean;
  dirty: boolean;
  error: string | null;
  fields: ProfileEditorFields;
  /// 自分の公開鍵。渡したときだけ NIP-05 の識別子の欄を出す(ADR 0064)。
  localPubkey?: string;
  picturePreviewSrc?: string | null;
  hasPicture: boolean;
  pictureInputKey: number;
  onFieldChange: (field: keyof ProfileEditorFields, value: string) => void;
  onPictureSelect: ChangeEventHandler<HTMLInputElement>;
  onPictureClear: () => void;
  onBack?: () => void;
  onSave: FormEventHandler<HTMLFormElement>;
  onReset: () => void;
  hideActions?: boolean;
};

export function ProfileEditorPanel({
  authorLabel,
  status,
  saving,
  dirty,
  error,
  fields,
  localPubkey,
  picturePreviewSrc,
  hasPicture,
  pictureInputKey,
  onFieldChange,
  onPictureSelect,
  onPictureClear,
  onBack,
  onSave,
  onReset,
  hideActions = false,
}: ProfileEditorPanelProps) {
  const { t } = useTranslation(['profile', 'common']);
  const disabled = status === 'loading' || saving;
  const nip05 = parseProfileNip05(fields.nip05 ?? '');
  const nip05Invalid = Boolean(fields.nip05?.trim()) && !nip05;
  const nip05Document = localPubkey
    ? JSON.stringify({ names: { [nip05?.name ?? 'name']: localPubkey } })
    : '';

  return (
    <Card className='panel-subsection'>
      <CardHeader>
        <div>
          <h3>{t('editor.title')}</h3>
          <small>{authorLabel}</small>
        </div>
        {onBack ? (
          <Button variant='secondary' type='button' onClick={onBack}>
            {t('editor.back')}
          </Button>
        ) : null}
      </CardHeader>

      {status === 'loading' ? <Notice>{t('editor.loading')}</Notice> : null}
      {status === 'error' && error ? <Notice tone='destructive'>{error}</Notice> : null}

      <form className='composer composer-compact' onSubmit={onSave} aria-busy={saving}>
        <Label>
          <span>{t('editor.displayName')}</span>
          <Input
            value={fields.displayName}
            onChange={(event) => onFieldChange('displayName', event.target.value)}
            placeholder={t('editor.placeholders.displayName')}
            disabled={disabled}
          />
        </Label>
        <Label>
          <span>{t('editor.name')}</span>
          <Input
            value={fields.name}
            onChange={(event) => onFieldChange('name', event.target.value)}
            placeholder={t('editor.placeholders.name')}
            disabled={disabled}
          />
        </Label>
        <Label>
          <span>{t('editor.about')}</span>
          <Textarea
            value={fields.about}
            onChange={(event) => onFieldChange('about', event.target.value)}
            className='ticket-output'
            placeholder={t('editor.placeholders.about')}
            disabled={disabled}
          />
        </Label>
        {localPubkey ? (
          <>
            <Field
              label={t('editor.nip05')}
              hint={t('editor.nip05Help', { domain: nip05?.domain ?? 'example.com' })}
              message={nip05Invalid ? t('editor.nip05Invalid') : undefined}
              tone={nip05Invalid ? 'danger' : 'default'}
            >
              <Input
                value={fields.nip05 ?? ''}
                onChange={(event) => onFieldChange('nip05', event.target.value)}
                placeholder='name@example.com'
                aria-invalid={nip05Invalid}
                disabled={disabled}
              />
            </Field>
            <p className='break-all font-mono text-xs text-[var(--muted-foreground)]'>
              {nip05Document}
            </p>
            <Button
              variant='secondary'
              type='button'
              onClick={() => void copyTextToClipboard(nip05Document).catch(() => undefined)}
            >
              {t('editor.nip05Copy')}
            </Button>
          </>
        ) : null}
        <div className='profile-editor-picture-panel'>
          <Label>
            <span>{t('editor.picture')}</span>
            <Input
              key={pictureInputKey}
              type='file'
              accept='image/*'
              disabled={disabled}
              onChange={onPictureSelect}
            />
          </Label>
          {picturePreviewSrc ? (
            <div className='profile-editor-picture-preview'>
              <img
                src={picturePreviewSrc}
                alt={t('editor.picturePreviewAlt', { name: authorLabel })}
                className='profile-overview-image'
              />
            </div>
          ) : null}
          <Button
            variant='secondary'
            type='button'
            disabled={!hasPicture || disabled}
            onClick={onPictureClear}
          >
            {t('common:actions.clear', { ns: 'common' })}
          </Button>
        </div>

        {status !== 'error' && error ? <p className='error error-inline'>{error}</p> : null}

        {!hideActions ? <div className='discovery-actions'>
          <Button variant='secondary' type='submit' disabled={!dirty || disabled || nip05Invalid}>
            {t('editor.save')}
          </Button>
          <Button
            variant='secondary'
            type='button'
            disabled={!dirty || disabled}
            onClick={onReset}
          >
            {t('editor.reset')}
          </Button>
        </div> : null}
      </form>
    </Card>
  );
}
