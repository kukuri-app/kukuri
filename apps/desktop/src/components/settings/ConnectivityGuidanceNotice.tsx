import { useTranslation } from 'react-i18next';
import type { ConnectivityGuidance } from '@/shell/connectivityGuidance';
import { Notice } from '@/components/ui/notice';
import { Button } from '@/components/ui/button';
import { SettingsActionRow } from './SettingsActionRow';

export type DiagnosticActions = {
  onRefreshDiagnostics?: () => void;
  onOpenCommunityNode?: () => void;
};

export function ConnectivityGuidanceNotice({ guidance, onRefreshDiagnostics, onOpenCommunityNode }:
  DiagnosticActions & { guidance?: ConnectivityGuidance }) {
  const { t } = useTranslation('settings');
  if (!guidance) return null;
  return <div className='min-w-0 space-y-3 [overflow-wrap:anywhere]'>
    {guidance.description ? <p>{guidance.description}</p> : null}
    {guidance.readError ? <Notice tone='destructive'>{guidance.readError}</Notice> : null}
    {guidance.errorSummary ? <p className='text-sm text-muted-foreground'>
      {t('connectionGuidance.lastRecordedError', { error: guidance.errorSummary })}
    </p> : null}
    <p className='text-sm text-muted-foreground'>{guidance.nextStep}</p>
    <SettingsActionRow className='flex-col sm:flex-row'>
      {onRefreshDiagnostics ? <Button className='whitespace-normal' variant='secondary' disabled={guidance.refreshing}
        onClick={onRefreshDiagnostics} aria-busy={guidance.refreshing}>
        {t(`connectionGuidance.${guidance.refreshing ? 'refreshing' : 'refresh'}`)}
      </Button> : null}
      {onOpenCommunityNode ? <Button className='whitespace-normal' variant='ghost' onClick={onOpenCommunityNode}>
        {t('connectionGuidance.openNode')}
      </Button> : null}
    </SettingsActionRow>
  </div>;
}
