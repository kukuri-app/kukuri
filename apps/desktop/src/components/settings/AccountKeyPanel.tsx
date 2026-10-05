import { AccountKeyImportForm } from './AccountKeyImportForm';
import { AccountSyncStatusNotice } from './AccountSyncStatusNotice';
import { BrowserStorageNotice } from './BrowserStorageNotice';
import { useCallback, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';

import type {
  AccountKeyExport,
  AccountsSnapshot,
  AccountSyncStatus,
} from '@/lib/api/types.generated';

import { Button } from '@/components/ui/button';
import { Card, CardHeader } from '@/components/ui/card';
import { Field } from '@/components/ui/field';
import { Input } from '@/components/ui/input';
import { Notice } from '@/components/ui/notice';
import { Textarea } from '@/components/ui/textarea';
import {
  exportAccountKey,
  listAccounts,
} from '@/lib/api/identity';
import { copyTextToClipboard } from '@/lib/utils';
import { changeAccountSession } from '@/lib/accountSession';

const MIN_PASSPHRASE_CHARS = 8;

function errorMessage(error: unknown): string {
  if (error instanceof Error) return error.message;
  return String(error);
}

// #859: アカウント鍵の export / import と複数アカウント管理(ADR 0047)。
// 平文秘密鍵はこのパネルにも IPC にも一切現れない。export は暗号化 envelope のみ。
type AccountKeyPanelProps = {
  // #967: 端末全体のバックアップ／復元(設定 > バックアップと復元)への案内。section 移動だけを行う。
  onOpenDeviceBackup?: () => void;
  // Web だけ（#1217 AC-5）: このブラウザの保存の状態（`navigator.storage.persisted()`）と、消えたときの戻し方を示す。
  showBrowserStorage?: boolean;
  // #1220 AC-3b: 本人の別の端末との同期の状態（通信状態の `account_sync`。無ければ出さない）。
  accountSync?: AccountSyncStatus | null;
};

export function AccountKeyPanel({ onOpenDeviceBackup, showBrowserStorage, accountSync }: AccountKeyPanelProps = {}) {
  const { t } = useTranslation(['settings']);

  const [accounts, setAccounts] = useState<AccountsSnapshot | null>(null);
  const [accountsError, setAccountsError] = useState<string | null>(null);

  const [exportAcknowledged, setExportAcknowledged] = useState(false);
  const [exportPassphrase, setExportPassphrase] = useState('');
  const [exportPassphraseConfirm, setExportPassphraseConfirm] = useState('');
  const [exportPending, setExportPending] = useState(false);
  const [exportResult, setExportResult] = useState<AccountKeyExport | null>(null);
  const [exportError, setExportError] = useState<string | null>(null);
  const [exportCopied, setExportCopied] = useState(false);

  const [switchPendingId, setSwitchPendingId] = useState<string | null>(null);
  const [switchError, setSwitchError] = useState<string | null>(null);
  const [persisted, setPersisted] = useState<boolean | null>(null);

  const refreshAccounts = useCallback(async () => {
    try {
      setAccounts(await listAccounts());
      setAccountsError(null);
    } catch (error) {
      setAccountsError(errorMessage(error));
    }
  }, []);

  useEffect(() => {
    void refreshAccounts();
  }, [refreshAccounts]);

  useEffect(() => {
    if (!showBrowserStorage) return;
    void navigator.storage.persisted().then(setPersisted, () => setPersisted(false));
  }, [showBrowserStorage]);

  const passphraseTooShort =
    exportPassphrase.length > 0 && exportPassphrase.length < MIN_PASSPHRASE_CHARS;
  const passphraseMismatch =
    exportPassphraseConfirm.length > 0 && exportPassphrase !== exportPassphraseConfirm;
  const exportReady =
    exportAcknowledged &&
    exportPassphrase.length >= MIN_PASSPHRASE_CHARS &&
    exportPassphrase === exportPassphraseConfirm &&
    !exportPending;

  const handleExport = async () => {
    setExportPending(true);
    setExportError(null);
    setExportCopied(false);
    try {
      setExportResult(await exportAccountKey(exportPassphrase));
    } catch (error) {
      setExportError(errorMessage(error));
    } finally {
      setExportPending(false);
    }
  };

  const handleCopyExport = async () => {
    if (!exportResult) return;
    // 暗号化 envelope のコピー。平文秘密鍵はクリップボードに載らない。
    const copied = await copyTextToClipboard(exportResult.export);
    setExportCopied(copied);
  };

  const handleSwitch = async (accountId: string) => {
    setSwitchPendingId(accountId);
    setSwitchError(null);
    try {
      await changeAccountSession(accountId);
    } catch (error) {
      setSwitchError(errorMessage(error));
      setSwitchPendingId(null);
    }
  };

  return (
    <Card className='space-y-4'>
      <CardHeader>
        <h3>{t('settings:accountKey.title')}</h3>
        <small>{t('settings:accountKey.summary')}</small>
      </CardHeader>

      {/* #1211 AC-5: 別の端末で使う 3 つの方法の対象の差。backup・restore は desktop だけ（Web は section を出さない）。 */}
      <Notice data-testid='account-key-methods'>
        <p>{t('settings:accountKey.methods.intro')}</p>
        <ul className='mt-1 list-disc ps-5'>
          <li>{t('settings:accountKey.methods.transfer')}</li>
          <li>{t('settings:accountKey.methods.export')}</li>
          {onOpenDeviceBackup ? <li>{t('settings:accountKey.methods.backup')}</li> : null}
        </ul>
      </Notice>
      {persisted === null ? null : <BrowserStorageNotice persisted={persisted} />}
      {accountSync ? <AccountSyncStatusNotice status={accountSync} /> : null}
      {onOpenDeviceBackup ? (
        <Button variant='secondary' type='button' onClick={onOpenDeviceBackup}>
          {t('settings:accountKey.openBackup')}
        </Button>
      ) : null}

      <section className='space-y-3'>
        <h4 className='text-sm font-semibold text-foreground'>
          {t('settings:accountKey.accountsTitle')}
        </h4>
        {accountsError ? <Notice tone='destructive'>{accountsError}</Notice> : null}
        {switchError ? <Notice tone='destructive'>{switchError}</Notice> : null}
        <ul className='space-y-2' data-testid='account-list'>
          {(accounts?.accounts ?? []).map((account) => {
            const active = account.id === accounts?.active_account_id;
            return (
              <li
                key={account.id}
                className='flex min-w-0 items-center gap-3 rounded-[var(--radius-input)] border border-[var(--border-subtle)] bg-[var(--surface-panel-soft)] px-4 py-3'
              >
                <div className='min-w-0 flex-1'>
                  <p className='truncate text-sm text-foreground'>
                    {account.label ?? t('settings:accountKey.unnamedAccount')}
                    {active ? (
                      <span className='ml-2 rounded bg-[var(--surface-accent-soft)] px-2 py-0.5 text-xs text-[var(--accent-foreground)]'>
                        {t('settings:accountKey.activeBadge')}
                      </span>
                    ) : null}
                  </p>
                  <p className='break-all font-mono text-xs text-[var(--muted-foreground)]'>
                    {account.pubkey}
                  </p>
                </div>
                {active ? null : (
                  <Button
                    variant='secondary'
                    disabled={switchPendingId !== null}
                    onClick={() => void handleSwitch(account.id)}
                    data-testid={`switch-account-${account.id}`}
                  >
                    {switchPendingId === account.id
                      ? t('settings:accountKey.switchPending')
                      : t('settings:accountKey.switchButton')}
                  </Button>
                )}
              </li>
            );
          })}
        </ul>
      </section>

      <section className='space-y-3'>
        <h4 className='text-sm font-semibold text-foreground'>
          {t('settings:accountKey.export.title')}
        </h4>
        <Notice tone='destructive'>{t('settings:accountKey.export.warning')}</Notice>
        <label className='flex min-w-0 items-center gap-3 text-sm text-foreground'>
          <input
            type='checkbox'
            checked={exportAcknowledged}
            onChange={(event) => setExportAcknowledged(event.currentTarget.checked)}
            data-testid='export-acknowledge'
          />
          <span>{t('settings:accountKey.export.acknowledge')}</span>
        </label>
        <Field
          label={t('settings:accountKey.export.passphraseLabel')}
          hint={t('settings:accountKey.export.passphraseHint', {
            min: MIN_PASSPHRASE_CHARS,
          })}
          message={passphraseTooShort ? t('settings:accountKey.export.tooShort') : undefined}
          tone={passphraseTooShort ? 'danger' : 'default'}
        >
          <Input
            type='password'
            value={exportPassphrase}
            disabled={!exportAcknowledged}
            onChange={(event) => setExportPassphrase(event.currentTarget.value)}
            data-testid='export-passphrase'
          />
        </Field>
        <Field
          label={t('settings:accountKey.export.confirmLabel')}
          message={passphraseMismatch ? t('settings:accountKey.export.mismatch') : undefined}
          tone={passphraseMismatch ? 'danger' : 'default'}
        >
          <Input
            type='password'
            value={exportPassphraseConfirm}
            disabled={!exportAcknowledged}
            onChange={(event) => setExportPassphraseConfirm(event.currentTarget.value)}
            data-testid='export-passphrase-confirm'
          />
        </Field>
        <Button disabled={!exportReady} onClick={() => void handleExport()} data-testid='export-submit'>
          {exportPending
            ? t('settings:accountKey.export.pending')
            : t('settings:accountKey.export.submit')}
        </Button>
        {exportError ? <Notice tone='destructive'>{exportError}</Notice> : null}
        {exportResult ? (
          <div className='space-y-2' data-testid='export-result'>
            <Field label={t('settings:accountKey.export.fingerprintLabel')}>
              <p className='break-all font-mono text-xs text-[var(--muted-foreground)]'>
                {exportResult.public_key}
              </p>
            </Field>
            <Field label={t('settings:accountKey.export.resultLabel')}>
              <Textarea
                readOnly
                rows={4}
                value={exportResult.export}
                data-testid='export-envelope'
              />
            </Field>
            <Button variant='secondary' onClick={() => void handleCopyExport()}>
              {exportCopied
                ? t('settings:accountKey.export.copied')
                : t('settings:accountKey.export.copy')}
            </Button>
          </div>
        ) : null}
      </section>

      <AccountKeyImportForm onImported={refreshAccounts} onSwitch={handleSwitch} switching={switchPendingId !== null} />
    </Card>
  );
}
