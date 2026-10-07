import { useId, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { ConnectivityGuidanceNotice, type DiagnosticActions } from './ConnectivityGuidanceNotice';

import { Button } from '@/components/ui/button';
import { Card, CardHeader } from '@/components/ui/card';
import { Notice } from '@/components/ui/notice';
import { Textarea } from '@/components/ui/textarea';
import { IS_WEB_RUNTIME } from '@/lib/webRuntime';

import { SettingsActionRow } from './SettingsActionRow';
import { SettingsDiagnosticList } from './SettingsDiagnosticList';
import { SettingsEditorField } from './SettingsEditorField';
import { SettingsMetricGrid } from './SettingsMetricGrid';
import { SettingsDetails } from './SettingsDetails';
import { type DiscoveryPanelView, type LoadConnectivityPeers } from './types';

type DiscoveryPanelProps = DiagnosticActions & {
  view: DiscoveryPanelView;
  loadPeers?: LoadConnectivityPeers;
  saveDisabled: boolean;
  resetDisabled: boolean;
  onSeedPeersChange: (value: string) => void;
  onSave: () => void;
  onReset: () => void;
  // #1632: 切替が終わると（失敗でも）解決する。失敗の表示と保存済みの値の更新は呼出元が持つ。
  onPublicBlobDiscoveryChange: (enabled: boolean) => Promise<void>;
  showDiagnostics?: boolean;
};

export function DiscoveryPanel({
  view,
  onRefreshDiagnostics,
  onOpenCommunityNode,
  saveDisabled,
  resetDisabled,
  onSeedPeersChange,
  onSave,
  onReset,
  onPublicBlobDiscoveryChange,
  showDiagnostics = true,
  loadPeers,
}: DiscoveryPanelProps) {
  const { t } = useTranslation(['common', 'settings']);
  const publicBlobDescriptionId = useId();
  // 切替中は求めた値を示して操作を受け付けず、終わったら保存済みの値（失敗なら元の値）へ戻す。
  // focus を失わないよう、切替中は disabled にせず aria-disabled で止める（env の固定だけ disabled）。
  const [publicBlobSwitchingTo, setPublicBlobSwitchingTo] = useState<boolean | null>(null);
  const switchPublicBlobDiscovery = async (enabled: boolean) => {
    setPublicBlobSwitchingTo(enabled);
    try {
      await onPublicBlobDiscoveryChange(enabled);
    } finally {
      setPublicBlobSwitchingTo(null);
    }
  };

  return (
    <Card className='space-y-4'>
      <CardHeader>
        <h3>{t('settings:discovery.title')}</h3>
        <small>{view.summaryLabel}</small>
      </CardHeader>

      {view.status === 'loading' ? <Notice>{t('settings:discovery.loading')}</Notice> : null}
      {view.panelError ? <Notice tone='destructive'>{view.panelError}</Notice> : null}

      <ConnectivityGuidanceNotice guidance={view.guidance} onRefreshDiagnostics={onRefreshDiagnostics} onOpenCommunityNode={onOpenCommunityNode} />
      {showDiagnostics && view.guidance?.loaded !== false ? (
        <SettingsDetails summary={t('settings:connectionGuidance.details')}>
          <SettingsMetricGrid items={view.metrics} />
          <SettingsDiagnosticList items={view.diagnostics} columns={2} loadPeers={loadPeers} />
        </SettingsDetails>
      ) : null}

      {/* #1632: Web は DHT を使えないので、公開コンテンツの発見を切り替えられない。 */}
      {IS_WEB_RUNTIME ? null : (
        <>
          <label className='flex min-w-0 items-center gap-3 rounded-[var(--radius-input)] border border-[var(--border-subtle)] bg-[var(--surface-panel-soft)] px-4 py-3 text-sm text-foreground'>
            <input
              type='checkbox'
              checked={publicBlobSwitchingTo ?? view.publicBlobDiscovery}
              disabled={view.envLocked}
              aria-disabled={publicBlobSwitchingTo !== null || undefined}
              aria-busy={publicBlobSwitchingTo !== null}
              aria-describedby={publicBlobDescriptionId}
              onChange={(event) => {
                if (publicBlobSwitchingTo === null) void switchPublicBlobDiscovery(event.currentTarget.checked);
              }}
            />
            {/* 切替中の表示は名前に続けて読み、狭い幅では名前の後ろでまとめて折り返す。 */}
            <span>
              {t('settings:discovery.publicBlobDiscovery.label')}{' '}
              {publicBlobSwitchingTo === null ? null : (
                <span className='whitespace-nowrap text-[var(--muted-foreground)]'>{t('settings:discovery.publicBlobDiscovery.pending')}</span>
              )}
            </span>
          </label>
          <Notice id={publicBlobDescriptionId}>{t('settings:discovery.publicBlobDiscovery.description')}</Notice>
        </>
      )}

      <SettingsEditorField
        label={t('settings:discovery.seedPeersLabel')}
        hint={t('settings:discovery.seedPeersHint')}
        message={view.seedPeersMessage}
        tone={view.seedPeersMessageTone}
      >
        <Textarea
          aria-label={t('settings:discovery.seedPeersLabel')}
          value={view.seedPeersInput}
          onChange={(event) => onSeedPeersChange(event.target.value)}
          readOnly={view.envLocked}
          className='min-h-[120px] resize-y font-mono text-[0.8rem]'
          placeholder={t('settings:discovery.seedPeersPlaceholder')}
        />
      </SettingsEditorField>

      <SettingsActionRow>
        <Button variant='secondary' disabled={saveDisabled} onClick={onSave}>
          {t('settings:discovery.actions.saveSeeds')}
        </Button>
        <Button variant='secondary' disabled={resetDisabled} onClick={onReset}>
          {t('common:actions.reset')}
        </Button>
      </SettingsActionRow>
    </Card>
  );
}
