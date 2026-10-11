import { useTranslation } from 'react-i18next';

import { Button } from '@/components/ui/button';
import { Notice } from '@/components/ui/notice';
import { installPwa, type PwaInstallPhase, usePwaInstallStore } from '@/lib/pwaInstall';

export function PwaInstallNotice() {
  const phase = usePwaInstallStore((state) => state.phase);
  return <PwaInstallView phase={phase} onInstall={() => void installPwa()} />;
}

export function PwaInstallView({ phase, onInstall }: { phase: PwaInstallPhase; onInstall: () => void }) {
  const { t } = useTranslation('settings');
  return (
    <section className='min-w-0 space-y-2' aria-label={t('pwa.title')}>
      <h4 className='text-base font-semibold text-foreground'>{t('pwa.title')}</h4>
      <p>{t(phase === 'installed' ? 'pwa.installed' : 'pwa.summary')}</p>
      {phase === 'available' || phase === 'prompting' ? (
        <Button type='button' variant='secondary' disabled={phase === 'prompting'}
          aria-busy={phase === 'prompting'} onClick={onInstall}>
          {t(phase === 'prompting' ? 'pwa.pending' : 'pwa.install')}
        </Button>
      ) : phase !== 'installed' ? <p>{t('pwa.menu')}</p> : null}
      {phase === 'failed' ? <Notice tone='warning'>{t('pwa.failed')}</Notice> : null}
    </section>
  );
}
