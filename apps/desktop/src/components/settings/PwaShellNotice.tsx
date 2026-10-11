import { useTranslation } from 'react-i18next';
import { Button } from '@/components/ui/button';
import { Notice } from '@/components/ui/notice';
import { useAcknowledgedPending } from '@/lib/useAcknowledgedPending';
import { formatLocalizedTime } from '@/i18n/format';
import { applyPwaShell, checkPwaShell, usePwaShellStore, type PwaShellState } from '@/lib/pwaShell';

export function PwaShellNotice() {
  const state = usePwaShellStore();
  return <PwaShellView state={state} onCheck={() => void checkPwaShell()} onApply={() => void applyPwaShell()} />;
}

export function PwaShellView({ state, onCheck, onApply }: {
  state: PwaShellState; onCheck: () => void; onApply: () => void;
}) {
  const { t } = useTranslation('settings');
  const { busy, acknowledge } = useAcknowledgedPending(state.checking);
  const status = state.applying ? 'applying' : state.available ? 'available' : state.ready ? 'ready' : 'preparing';
  return <section className='min-w-0 space-y-3' aria-label={t('pwaShell.title')}>
    <h4 className='text-base font-semibold text-foreground'>{t('pwaShell.title')}</h4>
    <Notice role='status' aria-live='polite'>
      <p>{t(`pwaShell.${status}`)}</p>
      {state.checkedAt !== null ? <p className='text-xs'>{t('pwaShell.checkedAt', { time: formatLocalizedTime(state.checkedAt) })}</p> : null}
    </Notice>
    {state.error ? <Notice role='alert' tone='destructive'>{t(`pwaShell.${state.error}Failed`)}</Notice> : null}
    <div className='flex flex-wrap gap-2'>
      <Button type='button' variant='secondary' disabled={busy || state.applying} aria-busy={busy}
        onClick={() => { acknowledge(); onCheck(); }}>
        {t(busy ? 'pwaShell.checking' : 'pwaShell.check')}
      </Button>
      {state.available ? <Button type='button' disabled={state.applying} aria-busy={state.applying} onClick={onApply}>
        {t(state.applying ? 'pwaShell.applying' : 'pwaShell.apply')}
      </Button> : null}
    </div>
  </section>;
}
