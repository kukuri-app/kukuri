import { useCallback, useState } from 'react';

import { Download, ExternalLink, FileText, RefreshCw } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { Button } from '@/components/ui/button';
import { Notice } from '@/components/ui/notice';
import {
  DESKTOP_LOGS_EXPORT_FILE_NAME,
  buildDesktopLogsExport,
  formatDesktopLogBytes,
  formatDesktopLogRepeat,
  formatDesktopLogTimestamp,
  type DesktopLogsView,
} from '@/lib/desktopLogs';
import { downloadTextFile } from '@/lib/downloadTextFile';
import { useExternalLinkOpener } from '@/lib/useExternalLinkOpener';
import { cn, copyTextToClipboard } from '@/lib/utils';
import type { DesktopLogsStatus } from '@/shell/useDesktopLogs';
import { SettingsActionRow } from './SettingsActionRow';

export const TROUBLESHOOTING_RUNBOOK_URL =
  'https://github.com/kukuri-app/kukuri/blob/main/docs/runbooks/mvp-troubleshooting.md';

export type DeveloperLogViewerProps = {
  status: DesktopLogsStatus;
  view: DesktopLogsView | null;
  errorMessage?: string | null;
  onRefresh: () => void;
};

function levelClassName(level: string): string {
  switch (level) {
    case 'ERROR':
      return 'text-[var(--destructive)]';
    case 'WARN':
      return 'text-[var(--warning)]';
    default:
      return 'text-[var(--muted-foreground)]';
  }
}

// #978: 開発者モード ON 時だけ mount される。取得は親の hook が担い、ここは表示と
// 利用者操作(更新・コピー・書き出し)だけを持つ。コピー／書き出しは click 時にしか起きない。
export function DeveloperLogViewer({ status, view, errorMessage, onRefresh }: DeveloperLogViewerProps) {
  const { t } = useTranslation(['common', 'settings']);
  const externalLink = useExternalLinkOpener();
  const [message, setMessage] = useState<string | null>(null);
  const entries = view?.entries ?? [];
  const busy = status === 'loading' || status === 'refreshing';

  const copyLogs = useCallback(async () => {
    if (!view) return;
    const copied = await copyTextToClipboard(buildDesktopLogsExport(view));
    setMessage(t(copied ? 'settings:developer.logs.copied' : 'settings:developer.logs.copyUnavailable'));
  }, [t, view]);

  const exportLogs = useCallback(() => {
    if (!view) return;
    downloadTextFile(DESKTOP_LOGS_EXPORT_FILE_NAME, buildDesktopLogsExport(view));
    setMessage(t('settings:developer.logs.exported'));
  }, [t, view]);

  return (
    <section className='min-w-0 space-y-3' aria-labelledby='developer-logs-heading'>
      <h4 id='developer-logs-heading' className='text-base font-semibold text-foreground'>
        {t('settings:developer.logs.title')}
      </h4>
      <p className='text-sm text-muted-foreground'>
        {view
          ? t('settings:developer.logs.summary', {
              maxEntries: view.maxEntries.toLocaleString(),
              maxBytes: formatDesktopLogBytes(view.maxBytes),
            })
          : t('settings:developer.logs.summaryUnknown')}
      </p>
      <SettingsActionRow>
        <Button variant='secondary' type='button' onClick={onRefresh} disabled={busy} aria-busy={busy}>
          <RefreshCw className={cn('size-4', busy && 'animate-spin')} aria-hidden='true' />
          {t('settings:developer.logs.refresh')}
        </Button>
        <Button variant='secondary' type='button' onClick={() => void copyLogs()} disabled={!view}>
          <FileText className='size-4' aria-hidden='true' />
          {t('settings:developer.logs.copy')}
        </Button>
        <Button variant='secondary' type='button' onClick={exportLogs} disabled={!view}>
          <Download className='size-4' aria-hidden='true' />
          {t('settings:developer.logs.export')}
        </Button>
      </SettingsActionRow>
      <div aria-live='polite'>{message ? <Notice tone='accent'>{message}</Notice> : null}</div>
      {status === 'error' ? (
        <Notice tone='destructive' role='alert'>
          {t('settings:developer.logs.error')}
          {errorMessage ? <span className='block break-words font-mono text-xs'>{errorMessage}</span> : null}
        </Notice>
      ) : null}
      {view?.gapSinceLastRefresh ? <Notice tone='warning'>{t('settings:developer.logs.gap')}</Notice> : null}
      {view?.droppedOlder ? <Notice>{t('settings:developer.logs.dropped')}</Notice> : null}
      {status === 'loading' ? (
        <p className='text-sm text-muted-foreground'>{t('settings:developer.logs.loading')}</p>
      ) : null}
      {view && entries.length === 0 && status !== 'loading' ? (
        <p className='text-sm text-muted-foreground'>{t('settings:developer.logs.empty')}</p>
      ) : null}
      {entries.length > 0 ? (
        <div
          role='region'
          aria-label={t('settings:developer.logs.listLabel', { count: entries.length })}
          tabIndex={0}
          className='max-h-72 min-w-0 overflow-auto rounded-[var(--radius-input)] border border-[var(--border-subtle)] bg-[var(--surface-panel-soft)] p-3'
        >
          <p className='mb-2 text-xs text-[var(--muted-foreground)]'>
            {t('settings:developer.logs.count', { count: entries.length })}
          </p>
          <ol className='m-0 list-none space-y-1 p-0 font-mono text-xs leading-5'>
            {entries.map((entry) => (
              <li key={entry.seq} className='min-w-0 [overflow-wrap:anywhere] text-[var(--muted-foreground-soft)]'>
                <span className='text-[var(--muted-foreground)]'>{formatDesktopLogTimestamp(entry.timestamp_ms)}</span>{' '}
                <span className={cn('font-semibold', levelClassName(entry.level))}>{entry.level}</span>{' '}
                <span className='text-[var(--muted-foreground)]'>{entry.target}:</span> {entry.message}
                {entry.repeat_count > 1 ? (
                  <span className='text-[var(--muted-foreground)]'> {formatDesktopLogRepeat(entry)}</span>
                ) : null}
              </li>
            ))}
          </ol>
        </div>
      ) : null}
      <p className='text-sm text-muted-foreground'>{t('settings:developer.logs.share')}</p>
      <p className='break-words font-mono text-xs text-[var(--muted-foreground-soft)]'>
        {t('settings:developer.logs.capture')}
      </p>
      {externalLink.pending ? <Notice role='status'>{t('common:externalLink.opening')}</Notice> : null}
      {externalLink.failed ? (
        <Notice tone='destructive' role='alert'>{t('common:externalLink.failed')}</Notice>
      ) : null}
      <SettingsActionRow>
        <Button variant='secondary' asChild>
          <a href={TROUBLESHOOTING_RUNBOOK_URL} target='_blank' rel='noreferrer' {...externalLink.linkProps}>
            {t('settings:developer.logs.runbook')}
            <ExternalLink className='size-4' aria-hidden='true' />
          </a>
        </Button>
      </SettingsActionRow>
    </section>
  );
}
