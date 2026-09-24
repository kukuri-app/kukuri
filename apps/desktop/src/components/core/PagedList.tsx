import { type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';

import { Button } from '@/components/ui/button';

export function PagedList<T>({
  items,
  ariaLabel,
  hasPrevious,
  hasNext,
  loading = false,
  onPrevious,
  onNext,
  children,
}: {
  items: T[];
  ariaLabel: string;
  hasPrevious: boolean;
  hasNext: boolean;
  loading?: boolean;
  onPrevious: () => void;
  onNext: () => void;
  children: (items: T[]) => ReactNode;
}) {
  const { t } = useTranslation('common');

  return (
    <>
      {children(items)}
      {hasPrevious || hasNext ? (
        <nav className='bookmark-pagination' aria-label={ariaLabel}>
          <Button
            variant='secondary'
            type='button'
            disabled={!hasPrevious || loading}
            onClick={onPrevious}
          >
            {t('actions.previousPage')}
          </Button>
          <Button
            variant='secondary'
            type='button'
            disabled={!hasNext || loading}
            onClick={onNext}
          >
            {t('actions.nextPage')}
          </Button>
        </nav>
      ) : null}
    </>
  );
}
