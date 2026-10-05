import { useId } from 'react';
import { useTranslation } from 'react-i18next';

import { Button } from './ui/button';
import { Notice } from './ui/notice';

export type RuntimeInUseViewProps = {
  pending: boolean;
  failed: boolean;
  onTakeOver: () => void;
};

// Web だけ: 同じ origin の別の tab が kukuri を動かしている（ADR 0059 §4）。引継ぎの判定は App が所有する。
export function RuntimeInUseView({ pending, failed, onTakeOver }: RuntimeInUseViewProps) {
  const { t } = useTranslation('common');
  const titleId = useId();

  return (
    <main className='startup-error-screen'>
      <section className='startup-error-panel content-start' aria-labelledby={titleId}>
        <header className='space-y-2'>
          <h1 id={titleId} className='text-xl font-semibold text-foreground'>
            {t('startup.inUseElsewhere.title')}
          </h1>
          <p className='text-sm leading-6 text-[var(--muted-foreground)]'>
            {t('startup.inUseElsewhere.description')}
          </p>
        </header>
        {failed ? (
          <Notice tone='destructive' role='alert'>
            {t('startup.inUseElsewhere.failed')}
          </Notice>
        ) : null}
        <div className='startup-error-actions'>
          <Button type='button' disabled={pending} aria-busy={pending} onClick={onTakeOver}>
            {pending ? t('startup.inUseElsewhere.takingOver') : t('startup.inUseElsewhere.takeOver')}
          </Button>
        </div>
      </section>
    </main>
  );
}
