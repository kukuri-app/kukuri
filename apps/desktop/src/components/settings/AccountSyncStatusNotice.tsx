import { useTranslation } from 'react-i18next';

import { Notice } from '@/components/ui/notice';
import type { AccountSyncStatus } from '@/lib/api/types.generated';

type AccountSyncState = 'rebuilding' | 'fetchFailed' | 'noPeers' | 'behind' | 'pendingWrites' | 'synced';

// いくつ立っていても 1 つの文にする。作り直し・取得の失敗・別の端末が無い・読み残し・送信待ちの順に先に出す。
function accountSyncState(status: AccountSyncStatus): AccountSyncState {
  if (status.rebuilding) return 'rebuilding';
  if (status.fetch_failed) return 'fetchFailed';
  if (status.no_peers) return 'noPeers';
  if (status.behind) return 'behind';
  if (status.pending_writes) return 'pendingWrites';
  return 'synced';
}

// #1220 AC-3b: 本人の別の端末との同期の状態（ADR 0061 §10 の `SyncStatus.account_sync`）。
export function AccountSyncStatusNotice({ status }: { status: AccountSyncStatus }) {
  const { t } = useTranslation('settings');
  const state = accountSyncState(status);
  return (
    <section className='space-y-2' data-testid='account-sync-status' data-state={state}>
      <h4 className='text-sm font-semibold text-foreground'>{t('accountKey.accountSync.title')}</h4>
      <Notice tone={state === 'fetchFailed' ? 'warning' : 'neutral'} role='status'>
        {t(`accountKey.accountSync.${state}`)}
      </Notice>
    </section>
  );
}
