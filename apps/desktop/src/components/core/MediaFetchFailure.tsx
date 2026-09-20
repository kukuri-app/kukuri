import { useContext } from 'react';
import { useTranslation } from 'react-i18next';
import { RefreshCw } from 'lucide-react';

import { IconButton } from '@/components/ui/icon-button';

import { MediaRetryContext } from './mediaRetryContext';

type MediaFetchFailureProps = {
  /// 再取得の対象 hash。空なら再試行の操作を出さない。
  hashes: readonly string[];
  /// 再取得を試行中。操作を止めるが focus は残す(`disabled` にしない)。
  retrying?: boolean;
  testId?: string;
  className?: string;
};

/// 自動取得が上限に達したメディアの代替表示。失敗した部分だけを置き換える。
export function MediaFetchFailure({
  hashes,
  retrying = false,
  testId,
  className,
}: MediaFetchFailureProps) {
  const { t } = useTranslation('common');
  const retry = useContext(MediaRetryContext);

  return (
    <div
      className={className ? `media-fetch-failure ${className}` : 'media-fetch-failure'}
      data-testid={testId}
    >
      <p className='topic-diagnostic topic-diagnostic-secondary' role='status'>
        {retrying ? t('media.retryingFetch') : t('media.fetchFailed')}
      </p>
      {retry && hashes.length > 0 ? (
        <IconButton
          variant='secondary'
          type='button'
          className='media-fetch-retry'
          aria-disabled={retrying}
          aria-busy={retrying}
          onClick={() => {
            if (!retrying) {
              retry(hashes);
            }
          }}
          label={t('media.retryFetch')}
          data-testid={testId ? `${testId}-retry` : undefined}
        >
          <RefreshCw
            className={retrying ? 'size-4 media-fetch-retry-spin' : 'size-4'}
            aria-hidden='true'
          />
        </IconButton>
      ) : null}
    </div>
  );
}
