import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { Button } from '@/components/ui/button';
import { Card, CardHeader } from '@/components/ui/card';
import { Field } from '@/components/ui/field';
import { Input } from '@/components/ui/input';
import { Notice } from '@/components/ui/notice';
import {
  cancelDeviceBackup,
  capturePortableFrontendState,
  chooseDeviceBackupDestination,
  chooseDeviceBackupSource,
  createDeviceBackup,
  listenDeviceBackupProgress,
  previewDeviceBackup,
  restoreDeviceBackup,
} from '@/lib/api/deviceBackup';
import type {
  DeviceBackupPreview,
  DeviceBackupProgress,
  DeviceBackupSummary,
} from '@/lib/api/types.generated';

const MIN_PASSPHRASE_CHARS = 8;

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function formatBytes(value: number): string {
  if (value < 1024) return `${value} B`;
  if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} KiB`;
  if (value < 1024 * 1024 * 1024) return `${(value / 1024 / 1024).toFixed(1)} MiB`;
  return `${(value / 1024 / 1024 / 1024).toFixed(1)} GiB`;
}

type DeviceBackupPanelProps = {
  // #967: 鍵だけの移行(設定 > アカウント)への案内。設定内の section 移動だけを行う。
  onOpenAccountKeys?: () => void;
};

export function DeviceBackupPanel({ onOpenAccountKeys }: DeviceBackupPanelProps = {}) {
  const { t } = useTranslation(['settings']);
  const [progress, setProgress] = useState<DeviceBackupProgress | null>(null);

  const [createAcknowledged, setCreateAcknowledged] = useState(false);
  const [createPassphrase, setCreatePassphrase] = useState('');
  const [createConfirm, setCreateConfirm] = useState('');
  const [createPending, setCreatePending] = useState(false);
  const [createResult, setCreateResult] = useState<DeviceBackupSummary | null>(null);
  const [createError, setCreateError] = useState<string | null>(null);

  const [restorePath, setRestorePath] = useState<string | null>(null);
  const [restorePassphrase, setRestorePassphrase] = useState('');
  const [restorePreview, setRestorePreview] = useState<DeviceBackupPreview | null>(null);
  const [restorePreferences, setRestorePreferences] = useState(true);
  const [replaceConfirmed, setReplaceConfirmed] = useState(false);
  const [restorePending, setRestorePending] = useState(false);
  const [restoreError, setRestoreError] = useState<string | null>(null);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listenDeviceBackupProgress((next) => {
      if (!disposed) setProgress(next);
    }).then((next) => {
      if (disposed) next();
      else unlisten = next;
    }).catch(() => {
      // Progress is supplementary; command results still report success or failure.
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  const createReady =
    createAcknowledged &&
    createPassphrase.length >= MIN_PASSPHRASE_CHARS &&
    createPassphrase === createConfirm &&
    !createPending;
  const existingAccount = Boolean(restorePreview?.existing_account_id);
  const restoreReady =
    restorePreview !== null &&
    restorePassphrase.length > 0 &&
    (!existingAccount || replaceConfirmed) &&
    !restorePending;
  const progressPercent =
    progress && progress.total_bytes > 0
      ? Math.min(100, Math.round((progress.completed_bytes / progress.total_bytes) * 100))
      : null;
  const cancellationAllowed = !(restorePending && progress?.phase === 'installing');

  const handleCreate = async () => {
    setCreateError(null);
    setCreateResult(null);
    setProgress(null);
    try {
      const path = await chooseDeviceBackupDestination();
      if (!path) return;
      setCreatePending(true);
      const result = await createDeviceBackup(
        path,
        createPassphrase,
        capturePortableFrontendState()
      );
      setCreateResult(result);
      setCreatePassphrase('');
      setCreateConfirm('');
    } catch (error) {
      setCreateError(errorMessage(error));
    } finally {
      setCreatePending(false);
    }
  };

  const handleChooseRestore = async () => {
    setRestoreError(null);
    try {
      const path = await chooseDeviceBackupSource();
      if (!path) return;
      setRestorePath(path);
      setRestorePreview(null);
      setReplaceConfirmed(false);
    } catch (error) {
      setRestoreError(errorMessage(error));
    }
  };

  const handlePreview = async () => {
    if (!restorePath) return;
    setRestoreError(null);
    try {
      setRestorePreview(await previewDeviceBackup(restorePath, restorePassphrase));
    } catch (error) {
      setRestorePreview(null);
      setRestoreError(errorMessage(error));
    }
  };

  const handleRestore = async () => {
    if (!restorePath || !restorePreview) return;
    setRestorePending(true);
    setRestoreError(null);
    setProgress(null);
    try {
      await restoreDeviceBackup(
        restorePath,
        restorePassphrase,
        existingAccount && replaceConfirmed,
        restorePreferences
      );
      // frontend stateはbackend activation完了後のdurable markerから適用する。
      // この応答時点は再同意待ちなのでlocalStorageへ触れない。
      window.location.reload();
    } catch (error) {
      setRestoreError(errorMessage(error));
      setRestorePending(false);
    }
  };

  return (
    <Card className='space-y-5'>
      <CardHeader>
        <h3>{t('settings:deviceBackup.title')}</h3>
      </CardHeader>

      <Notice>{t('settings:deviceBackup.oneFileNotice')}</Notice>
      <Notice tone='destructive'>{t('settings:deviceBackup.secretNotice')}</Notice>
      <div className='space-y-2'>
        <p className='text-sm text-[var(--muted-foreground)]'>{t('settings:deviceBackup.keyOnlyHint')}</p>
        {onOpenAccountKeys ? (
          <Button variant='secondary' type='button' onClick={onOpenAccountKeys}>
            {t('settings:deviceBackup.openAccount')}
          </Button>
        ) : null}
      </div>

      <section className='space-y-3'>
        <h4 className='text-sm font-semibold text-foreground'>
          {t('settings:deviceBackup.create.title')}
        </h4>
        <label className='flex min-w-0 items-start gap-3 text-sm text-foreground'>
          <input
            type='checkbox'
            checked={createAcknowledged}
            onChange={(event) => setCreateAcknowledged(event.currentTarget.checked)}
            data-testid='device-backup-acknowledge'
          />
          <span>{t('settings:deviceBackup.create.acknowledge')}</span>
        </label>
        <Field
          label={t('settings:deviceBackup.passphrase')}
          hint={t('settings:deviceBackup.passphraseHint', { min: MIN_PASSPHRASE_CHARS })}
        >
          <Input
            type='password'
            value={createPassphrase}
            disabled={!createAcknowledged}
            onChange={(event) => setCreatePassphrase(event.currentTarget.value)}
            data-testid='device-backup-passphrase'
          />
        </Field>
        <Field
          label={t('settings:deviceBackup.confirmPassphrase')}
          message={
            createConfirm.length > 0 && createConfirm !== createPassphrase
              ? t('settings:deviceBackup.passphraseMismatch')
              : undefined
          }
          tone={
            createConfirm.length > 0 && createConfirm !== createPassphrase ? 'danger' : 'default'
          }
        >
          <Input
            type='password'
            value={createConfirm}
            disabled={!createAcknowledged}
            onChange={(event) => setCreateConfirm(event.currentTarget.value)}
            data-testid='device-backup-passphrase-confirm'
          />
        </Field>
        <Button disabled={!createReady} onClick={() => void handleCreate()}>
          {createPending
            ? t('settings:deviceBackup.create.pending')
            : t('settings:deviceBackup.create.submit')}
        </Button>
        {createError ? <Notice tone='destructive'>{createError}</Notice> : null}
        {createResult ? (
          <Notice tone='accent' data-testid='device-backup-created'>
            {t('settings:deviceBackup.create.success', { size: formatBytes(createResult.bytes) })}
          </Notice>
        ) : null}
      </section>

      <section className='space-y-3 border-t border-[var(--border-subtle)] pt-5'>
        <h4 className='text-sm font-semibold text-foreground'>
          {t('settings:deviceBackup.restore.title')}
        </h4>
        <p className='text-sm text-[var(--muted-foreground)]'>
          {t('settings:deviceBackup.restore.description')}
        </p>
        <Button variant='secondary' onClick={() => void handleChooseRestore()}>
          {t('settings:deviceBackup.restore.choose')}
        </Button>
        {restorePath ? (
          <p className='break-all font-mono text-xs text-[var(--muted-foreground)]'>{restorePath}</p>
        ) : null}
        <Field label={t('settings:deviceBackup.passphrase')}>
          <Input
            type='password'
            value={restorePassphrase}
            disabled={!restorePath}
            onChange={(event) => {
              setRestorePassphrase(event.currentTarget.value);
              setRestorePreview(null);
              setReplaceConfirmed(false);
            }}
            data-testid='device-restore-passphrase'
          />
        </Field>
        <Button
          variant='secondary'
          disabled={!restorePath || restorePassphrase.length === 0}
          onClick={() => void handlePreview()}
        >
          {t('settings:deviceBackup.restore.preview')}
        </Button>

        {restorePreview ? (
          <div className='space-y-3 rounded-[var(--radius-input)] border border-[var(--border-subtle)] p-4'>
            <Field label={t('settings:deviceBackup.restore.fingerprint')}>
              <p className='break-all font-mono text-xs text-[var(--muted-foreground)]'>
                {restorePreview.public_key}
              </p>
            </Field>
            <p className='text-sm text-foreground'>
              {t('settings:deviceBackup.restore.contents', {
                size: formatBytes(restorePreview.content_bytes),
                version: restorePreview.app_version,
              })}
            </p>
            <Notice>{t('settings:deviceBackup.restore.reconsent')}</Notice>
            <label className='flex min-w-0 items-start gap-3 text-sm text-foreground'>
              <input
                type='checkbox'
                checked={restorePreferences}
                onChange={(event) => setRestorePreferences(event.currentTarget.checked)}
              />
              <span>{t('settings:deviceBackup.restore.applyPreferences')}</span>
            </label>
            {existingAccount ? (
              <label className='flex min-w-0 items-start gap-3 text-sm text-[var(--danger-foreground)]'>
                <input
                  type='checkbox'
                  checked={replaceConfirmed}
                  onChange={(event) => setReplaceConfirmed(event.currentTarget.checked)}
                  data-testid='device-restore-replace-confirm'
                />
                <span>{t('settings:deviceBackup.restore.replaceExisting')}</span>
              </label>
            ) : null}
            <Button disabled={!restoreReady} onClick={() => void handleRestore()}>
              {restorePending
                ? t('settings:deviceBackup.restore.pending')
                : t('settings:deviceBackup.restore.submit')}
            </Button>
          </div>
        ) : null}
        {restoreError ? <Notice tone='destructive'>{restoreError}</Notice> : null}
      </section>

      {createPending || restorePending ? (
        <div className='space-y-2' aria-live='polite'>
          <p className='text-sm text-foreground'>
            {t(`settings:deviceBackup.phase.${progress?.phase ?? 'scanning'}`)}
            {progressPercent === null ? '' : ` ${progressPercent}%`}
          </p>
          <progress className='w-full' max={100} value={progressPercent ?? undefined} />
          {cancellationAllowed ? (
            <Button variant='secondary' onClick={() => void cancelDeviceBackup()}>
              {t('settings:deviceBackup.cancel')}
            </Button>
          ) : null}
        </div>
      ) : null}
    </Card>
  );
}
