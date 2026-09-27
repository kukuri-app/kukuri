import { useCallback, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { Button } from '@/components/ui/button';

import type { ConnectivityPeerQuery, LoadConnectivityPeers } from './types';

type PeerPageState = {
  ids: string[];
  cursor: string | null;
  loading: boolean;
  failed: boolean;
};

// 設定画面の詳細の peer の一覧。表示したときに 1 ページ目を読み、続きは「さらに読み込む」で読む(#1221 R2-D)。
export function PeerPageList({
  query,
  loadPeers,
}: {
  query: ConnectivityPeerQuery;
  loadPeers: LoadConnectivityPeers;
}) {
  const { t } = useTranslation(['common']);
  const [page, setPage] = useState<PeerPageState>({
    ids: [],
    cursor: null,
    loading: true,
    failed: false,
  });
  const { kind, topic } = query;
  const load = useCallback(
    async (cursor: string | null) => {
      setPage((current) => ({ ...current, loading: true, failed: false }));
      try {
        const next = await loadPeers({ kind, topic }, cursor);
        setPage((current) => ({
          ids: [...(cursor ? current.ids : []), ...next.peer_ids],
          cursor: next.next_cursor ?? null,
          loading: false,
          failed: false,
        }));
      } catch {
        setPage((current) => ({ ...current, loading: false, failed: true }));
      }
    },
    [kind, loadPeers, topic]
  );
  useEffect(() => {
    void load(null);
  }, [load]);

  return (
    <span className='flex min-w-0 flex-col items-start gap-2'>
      {page.ids.length > 0 || !(page.loading || page.failed) ? (
        <span>{page.ids.length > 0 ? page.ids.join(', ') : t('common:fallbacks.none')}</span>
      ) : null}
      {page.cursor || page.failed ? (
        <Button
          variant='secondary'
          type='button'
          disabled={page.loading}
          onClick={() => void load(page.failed && page.ids.length === 0 ? null : page.cursor)}
        >
          {page.loading
            ? t('common:fallbacks.loadingMore')
            : page.failed
              ? t('common:actions.retry')
              : t('common:fallbacks.loadMore')}
        </Button>
      ) : null}
    </span>
  );
}
