import { type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';

import { Button } from '@/components/ui/button';

export function BookmarkPage<T>({
  items,
  hasPrevious,
  hasNext,
  loading = false,
  onPrevious,
  onNext,
  children,
}: {
  items: T[];
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
        <nav className='bookmark-pagination' aria-label={t('fallbacks.bookmarkPages')}>
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
