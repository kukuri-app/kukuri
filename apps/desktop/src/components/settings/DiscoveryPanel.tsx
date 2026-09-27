import { useTranslation } from 'react-i18next';
import { ConnectivityGuidanceNotice, type DiagnosticActions } from './ConnectivityGuidanceNotice';

import { Button } from '@/components/ui/button';
import { Card, CardHeader } from '@/components/ui/card';
import { Notice } from '@/components/ui/notice';
import { Textarea } from '@/components/ui/textarea';

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
  showDiagnostics = true,
  loadPeers,
}: DiscoveryPanelProps) {
  const { t } = useTranslation(['common', 'settings']);

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
