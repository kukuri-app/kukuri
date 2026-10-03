import { useCallback, useEffect, useMemo, useState } from 'react';
import { Download, ExternalLink, FileText, Power, RefreshCw } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { Button } from '@/components/ui/button';
import { Card, CardHeader } from '@/components/ui/card';
import { Notice } from '@/components/ui/notice';
import { copyTextToClipboard } from '@/lib/utils';
import { downloadTextFile } from '@/lib/downloadTextFile';
import {
  DESKTOP_DISTRIBUTION,
  type DesktopDistribution,
  usesSelfManagedUpdater,
} from '@/lib/distribution';
import { useAcknowledgedPending } from '@/lib/useAcknowledgedPending';
import { useExternalLinkOpener } from '@/lib/useExternalLinkOpener';
import { formatLocalizedTime } from '@/i18n/format';
import {
  buildSafeDiagnosticReport,
  classifyUpdateError,
  RELEASE_CHANNEL,
  RELEASE_FEEDBACK_URL,
  RELEASE_LATEST_URL,
  RELEASE_MANIFEST_NAME,
  RELEASE_QUICKSTART_URL,
  RELEASE_RUNBOOK_URL,
  THIRD_PARTY_NOTICES_URL,
} from '@/lib/releaseReadiness';
import { useOsNotificationPermission, useOsNotificationSettings } from '@/lib/useOsNotificationSettings';
import { buildCommunityNodeDisclosures } from '@/lib/communityNodeDisclosures';
import { useAppUpdateStore } from '@/shell/useAppUpdateStore';
import { useDesktopShellStore } from '@/shell/store';

import { SettingsActionRow } from './SettingsActionRow';
import { SettingsDiagnosticList } from './SettingsDiagnosticList';
import { formatUpdateStatus } from './releasePanelCopy';
import { IS_WEB_RUNTIME } from '@/lib/webRuntime';

function updateErrorTranslationKey(errorMessage?: string | null): string {
  return `settings:release.update.errors.${classifyUpdateError(errorMessage)}`;
}

type ReleasePanelProps = {
  showDiagnostics?: boolean;
  distribution?: DesktopDistribution;
};

export function ReleasePanel({
  showDiagnostics = true,
  distribution = DESKTOP_DISTRIBUTION,
}: ReleasePanelProps) {
  const { t } = useTranslation(['common', 'settings']);
  const externalLink = useExternalLinkOpener();
  const syncStatus = useDesktopShellStore((state) => state.syncStatus);
  const notificationStatus = useDesktopShellStore((state) => state.notificationStatus);
  const communityNodeStatuses = useDesktopShellStore((state) => state.communityNodeStatuses);
  const communityNodeConfig = useDesktopShellStore((state) => state.communityNodeConfig);
  const communityNodeManifests = useDesktopShellStore((state) => state.communityNodeManifests);
  const updateState = useAppUpdateStore((state) => state.updateState);
  const pendingUpdate = useAppUpdateStore((state) => state.pendingUpdate);
  const checkForUpdate = useAppUpdateStore((state) => state.checkForUpdate);
  const downloadUpdate = useAppUpdateStore((state) => state.downloadUpdate);
  const restartAndInstall = useAppUpdateStore((state) => state.restartAndInstall);
  const [diagnosticReport, setDiagnosticReport] = useState('');
  const [diagnosticMessage, setDiagnosticMessage] = useState<string | null>(null);
  const [restartPromptDismissed, setRestartPromptDismissed] = useState(false);
  const selfManagedUpdater = usesSelfManagedUpdater(distribution);
  // #962: 受信設定の編集は通知 section へ移した。ここでは診断レポート用に現在値だけ読む。
  const [osNotificationSettings] = useOsNotificationSettings();
  const { permission: osNotificationPermission } = useOsNotificationPermission();

  useEffect(() => {
    if (updateState.status !== 'ready_to_restart') {
      setRestartPromptDismissed(false);
    }
  }, [updateState.status]);

  const diagnosticReportText = useMemo(
    () =>
      buildSafeDiagnosticReport({
        appVersion: updateState.currentVersion,
        updateState,
        osNotificationPermission,
        osNotificationSettings,
        userAgent: typeof navigator === 'undefined' ? 'unknown' : navigator.userAgent,
        platform: typeof navigator === 'undefined' ? 'unknown' : navigator.platform,
        syncConnected: syncStatus.connected,
        deliveryState: syncStatus.delivery_state,
        discoveryMode: syncStatus.discovery.mode,
        activePath: syncStatus.active_path,
        peerCount: syncStatus.peer_count,
        subscribedTopicCount: syncStatus.subscribed_topics.length,
        unreadNotificationCount: notificationStatus.unread_count,
        communityNodeStatuses,
        lastSyncError: syncStatus.last_error,
        lastDiscoveryError: syncStatus.discovery.last_discovery_error,
      }),
    [
      communityNodeStatuses,
      notificationStatus.unread_count,
      osNotificationPermission,
      osNotificationSettings,
      syncStatus.active_path,
      syncStatus.connected,
      syncStatus.delivery_state,
      syncStatus.discovery.last_discovery_error,
      syncStatus.discovery.mode,
      syncStatus.last_error,
      syncStatus.peer_count,
      syncStatus.subscribed_topics.length,
      updateState,
    ]
  );

  useEffect(() => {
    if (diagnosticReport) {
      setDiagnosticReport(diagnosticReportText);
    }
  }, [diagnosticReport, diagnosticReportText]);

  const copyDiagnosticReport = useCallback(async () => {
    const copied = await copyTextToClipboard(diagnosticReportText);
    setDiagnosticReport(diagnosticReportText);
    setDiagnosticMessage(
      copied
        ? t('settings:release.diagnostics.copied')
        : t('settings:release.diagnostics.copyUnavailable')
    );
  }, [diagnosticReportText, t]);

  const exportDiagnosticReport = useCallback(() => {
    downloadTextFile('kukuri-diagnostics.txt', diagnosticReportText);
    setDiagnosticReport(diagnosticReportText);
    setDiagnosticMessage(t('settings:release.diagnostics.exported'));
  }, [diagnosticReportText, t]);

  const updateDiagnostics = [
    {
      label: t('settings:release.update.version'),
      value: updateState.currentVersion,
      monospace: true,
    },
    {
      label: t('settings:release.update.channel'),
      value: selfManagedUpdater
        ? RELEASE_CHANNEL
        : t('settings:release.update.storeChannel'),
    },
    ...(selfManagedUpdater
      ? [{
          label: t('settings:release.update.manifest'),
          value: RELEASE_MANIFEST_NAME,
          monospace: true,
        }]
      : []),
    {
      label: t('settings:release.update.status'),
      value: selfManagedUpdater
        ? formatUpdateStatus(updateState.status, t)
        : t('settings:release.update.storeManagedStatus'),
      tone: selfManagedUpdater && updateState.status === 'failed'
        ? ('danger' as const)
        : ('default' as const),
    },
  ];
  const updateErrorMessage = updateState.status === 'failed'
    ? t(updateErrorTranslationKey(updateState.lastError))
    : null;
  const updateBusy = ['checking', 'downloading', 'installing'].includes(updateState.status);
  const updateReadyToRestart = updateState.status === 'ready_to_restart';
  // #956: 高速完了でも確認の受付を最低1秒示す。結果はstoreの状態をそのまま反映する。
  const { busy: checkPending, acknowledge: acknowledgeCheck } = useAcknowledgedPending(
    updateState.status === 'checking'
  );
  // #956: 結果が前回と同じでも、この確認がいつ完了したかを時:分:秒で示す。
  const checkedAtLabel = updateState.lastCheckedAt != null
    ? t('settings:release.update.checkedAt', { time: formatLocalizedTime(updateState.lastCheckedAt) })
    : null;
  const communityNodeDisclosures = useMemo(
    () => buildCommunityNodeDisclosures(communityNodeConfig, communityNodeManifests),
    [communityNodeConfig, communityNodeManifests]
  );

  return (
    <Card className='min-w-0 space-y-5'>
      <CardHeader>
        <h3>{t('settings:release.title')}</h3>
        <small>{t(selfManagedUpdater ? 'settings:release.summary' : 'settings:release.storeSummary')}</small>
      </CardHeader>

      {externalLink.pending ? <Notice role='status'>{t('common:externalLink.opening')}</Notice> : null}
      {externalLink.failed ? <Notice tone='destructive' role='alert'>{t('common:externalLink.failed')}</Notice> : null}

      {/* Web は配信元から最新を読むので、アプリの更新は無い（ADR 0060 §3）。 */}
      {IS_WEB_RUNTIME ? null : <section className='min-w-0 space-y-3'>
        <h4 className='text-base font-semibold text-foreground'>
          {t('settings:release.update.title')}
        </h4>
        {showDiagnostics ? <SettingsDiagnosticList items={updateDiagnostics} columns={2} /> : null}
        {!selfManagedUpdater ? (
          <Notice role='status'>{t('settings:release.update.storeManaged')}</Notice>
        ) : null}
        {selfManagedUpdater ? <>
        <div role='status' aria-live='polite' aria-atomic='true'>
          {['checking', 'up_to_date', 'downloading', 'installing'].includes(updateState.status) ? (
            <Notice>
              <p>{formatUpdateStatus(updateState.status, t)}</p>
              {updateState.status === 'checking' && updateState.availableVersion ? (
                <p className='break-words text-xs'>
                  {t('settings:release.update.previouslyAvailable', { version: updateState.availableVersion })}
                </p>
              ) : null}
              {updateState.status === 'up_to_date' && checkedAtLabel ? (
                <p className='text-xs'>{checkedAtLabel}</p>
              ) : null}
            </Notice>
          ) : null}
          {updateState.availableVersion && ['available', 'downloading'].includes(updateState.status) ? (
            <Notice className='break-words' tone='accent'>
              <p>{t('settings:release.update.available', { version: updateState.availableVersion })}</p>
              {updateState.status === 'available' && checkedAtLabel ? (
                <p className='text-xs'>{checkedAtLabel}</p>
              ) : null}
            </Notice>
          ) : null}
        </div>
        {updateErrorMessage ? (
          <Notice className='break-words' tone='destructive' role='alert'>
            <div className='space-y-1'>
              <p>{updateErrorMessage}</p>
              {checkedAtLabel ? <p className='text-xs'>{checkedAtLabel}</p> : null}
              {showDiagnostics ? (
                <small className='font-mono'>{updateState.lastError}</small>
              ) : null}
            </div>
          </Notice>
        ) : null}
        {updateReadyToRestart && !restartPromptDismissed ? (
          <Notice tone='warning'>
            <div className='space-y-3'>
              <div className='space-y-1'>
                <p className='font-semibold'>{t('settings:release.update.ready')}</p>
                <p>{t('settings:release.update.readyDescription')}</p>
                <small>{t('settings:release.update.restartWarning')}</small>
              </div>
              <SettingsActionRow className='flex-col sm:flex-row'>
                <Button
                  className='whitespace-normal [&>svg]:shrink-0'
                  type='button'
                  onClick={() => void restartAndInstall()}
                  disabled={!pendingUpdate}
                >
                  <Power className='size-4' aria-hidden='true' />
                  {t('settings:release.update.restartNow')}
                </Button>
                <Button
                  variant='secondary'
                  type='button'
                  onClick={() => setRestartPromptDismissed(true)}
                >
                  {t('settings:release.update.later')}
                </Button>
              </SettingsActionRow>
            </div>
          </Notice>
        ) : null}
        <SettingsActionRow className='flex-col sm:flex-row'>
          <Button
            variant='secondary'
            type='button'
            disabled={updateBusy || updateReadyToRestart || checkPending}
            aria-busy={checkPending}
            onClick={() => {
              acknowledgeCheck();
              void checkForUpdate();
            }}
          >
            <RefreshCw className='size-4' aria-hidden='true' />
            {t(checkPending
              ? 'settings:release.update.statuses.checking'
              : 'settings:release.update.check')}
          </Button>
          {updateReadyToRestart ? (
            <Button
              className='whitespace-normal [&>svg]:shrink-0'
              variant='secondary'
              type='button'
              disabled={!pendingUpdate}
              onClick={() => void restartAndInstall()}
            >
              <Power className='size-4' aria-hidden='true' />
              {t('settings:release.update.restartNow')}
            </Button>
          ) : (
            <Button
              variant='secondary'
              type='button'
              disabled={!pendingUpdate || updateBusy}
              onClick={() => void downloadUpdate()}
            >
              <Download className='size-4' aria-hidden='true' />
              {t('settings:release.update.install')}
            </Button>
          )}
        </SettingsActionRow>
        </> : null}
      </section>}

      <section className='min-w-0 space-y-3'>
        <h4 className='text-base font-semibold text-foreground'>
          {t('settings:release.externalTransmission.title')}
        </h4>
        <p className='text-sm text-[var(--muted-foreground-soft)]'>
          {t('settings:release.externalTransmission.summary')}
        </p>
        <div className='grid gap-3 lg:grid-cols-2'>
          {[
            ...(selfManagedUpdater ? [{
              key: 'update',
              destination: t('settings:release.externalTransmission.updateDestination'),
              purpose: t('settings:release.externalTransmission.updatePurpose'),
              items: t('settings:release.externalTransmission.updateItems'),
              retention: t('settings:release.externalTransmission.updateRetention'),
              href: RELEASE_LATEST_URL,
            }] : []),
            {
              key: 'p2p',
              destination: t('settings:release.externalTransmission.p2pDestination'),
              purpose: t('settings:release.externalTransmission.p2pPurpose'),
              items: t('settings:release.externalTransmission.p2pItems'),
              retention: t('settings:release.externalTransmission.p2pRetention'),
              href: null,
            },
          ].map((entry) => (
            <dl
              key={entry.key}
              className='grid min-w-0 gap-2 rounded-[var(--radius-input)] border border-[var(--border-subtle)] bg-[var(--surface-panel-soft)] p-3 text-sm'
            >
              <div className='min-w-0'>
                <dt className='font-medium text-foreground'>
                  {t('settings:release.externalTransmission.destination')}
                </dt>
                <dd className='break-words text-[var(--muted-foreground-soft)]'>
                  {entry.href ? (
                    <a
                      className='underline underline-offset-2'
                      href={entry.href}
                      {...externalLink.linkProps}
                      target='_blank'
                      rel='noreferrer'
                    >
                      {entry.destination}
                    </a>
                  ) : (
                    entry.destination
                  )}
                </dd>
              </div>
              <div>
                <dt className='font-medium text-foreground'>
                  {t('settings:release.externalTransmission.purpose')}
                </dt>
                <dd className='text-[var(--muted-foreground-soft)]'>{entry.purpose}</dd>
              </div>
              <div>
                <dt className='font-medium text-foreground'>
                  {t('settings:release.externalTransmission.items')}
                </dt>
                <dd className='text-[var(--muted-foreground-soft)]'>{entry.items}</dd>
              </div>
              <div>
                <dt className='font-medium text-foreground'>
                  {t('settings:release.externalTransmission.retention')}
                </dt>
                <dd className='text-[var(--muted-foreground-soft)]'>{entry.retention}</dd>
              </div>
            </dl>
          ))}
        </div>
        <Notice>{t('settings:release.externalTransmission.diagnosticNotice')}</Notice>
      </section>

      <section className='min-w-0 space-y-3'>
        <h4 className='text-base font-semibold text-foreground'>
          {t('settings:release.resources.title')}
        </h4>
        <p className='text-sm text-[var(--muted-foreground-soft)]'>
          {t('settings:release.resources.summary')}
        </p>
        <SettingsActionRow>
          {[
            [t('settings:release.resources.latestRelease'), RELEASE_LATEST_URL],
            [t('settings:release.resources.quickstart'), RELEASE_QUICKSTART_URL],
            [t('settings:release.resources.releaseRunbook'), RELEASE_RUNBOOK_URL],
            [t('settings:release.resources.thirdPartyNotices'), THIRD_PARTY_NOTICES_URL],
          ].map(([label, href]) => (
            <Button key={href} asChild variant='secondary'>
              <a href={href} target='_blank' rel='noreferrer' {...externalLink.linkProps}>
                {label}
                <ExternalLink className='size-4' aria-hidden='true' />
              </a>
            </Button>
          ))}
        </SettingsActionRow>
        <p className='text-sm font-medium text-foreground'>
          {t('settings:release.resources.communityNodeDisclosures')}
        </p>
        {communityNodeDisclosures.length === 0 ? (
          <p className='text-sm text-[var(--muted-foreground-soft)]'>
            {t('settings:release.resources.noCommunityNodes')}
          </p>
        ) : (
          <div className='space-y-3'>
            {communityNodeDisclosures.map((disclosure) => (
              <div
                key={disclosure.baseUrl}
                className='space-y-2 rounded-[var(--radius-input)] border border-[var(--border-subtle)] bg-[var(--surface-panel-soft)] p-3'
              >
                <div className='min-w-0'>
                  <p className='font-medium text-foreground'>
                    {disclosure.nodeName ?? disclosure.baseUrl}
                  </p>
                  {disclosure.nodeName ? (
                    <small className='break-all font-mono'>{disclosure.baseUrl}</small>
                  ) : null}
                </div>
                {disclosure.manifestAvailable && disclosure.links.length > 0 ? (
                  <SettingsActionRow>
                    {disclosure.links.map(({ key, href }) => (
                      <Button key={`${key}:${href}`} asChild variant='secondary' size='sm'>
                        <a href={href} target='_blank' rel='noreferrer' {...externalLink.linkProps}>
                          {t(`settings:release.resources.${key}`)}
                          <ExternalLink className='size-4' aria-hidden='true' />
                        </a>
                      </Button>
                    ))}
                  </SettingsActionRow>
                ) : (
                  <small>{t('settings:release.resources.manifestUnavailable')}</small>
                )}
              </div>
            ))}
          </div>
        )}
      </section>

      {showDiagnostics ? (
      <section className='min-w-0 space-y-3'>
        <h4 className='text-base font-semibold text-foreground'>
          {t('settings:release.diagnostics.title')}
        </h4>
        <SettingsActionRow>
          <Button variant='secondary' type='button' onClick={() => void copyDiagnosticReport()}>
            <FileText className='size-4' aria-hidden='true' />
            {t('settings:release.diagnostics.copy')}
          </Button>
          <Button variant='secondary' type='button' onClick={exportDiagnosticReport}>
            <Download className='size-4' aria-hidden='true' />
            {t('settings:release.diagnostics.export')}
          </Button>
          <Button variant='secondary' asChild>
            <a href={RELEASE_FEEDBACK_URL} target='_blank' rel='noreferrer' {...externalLink.linkProps}>
              {t('settings:release.diagnostics.feedback')}
            </a>
          </Button>
        </SettingsActionRow>
        {diagnosticMessage ? <Notice tone='accent'>{diagnosticMessage}</Notice> : null}
        {diagnosticReport ? (
          <textarea
            className='min-h-44 w-full resize-y rounded-[var(--radius-input)] border border-[var(--border-subtle)] bg-[var(--surface-panel-soft)] p-3 font-mono text-xs text-[var(--muted-foreground-soft)]'
            readOnly
            value={diagnosticReport}
            aria-label={t('settings:release.diagnostics.previewLabel')}
          />
        ) : null}
      </section>
      ) : null}
    </Card>
  );
}
