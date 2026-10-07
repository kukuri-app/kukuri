import { useTranslation } from 'react-i18next';
import { RefreshCw } from 'lucide-react';

import { IconButton } from '@/components/ui/icon-button';
import { useAcknowledgedPending } from '@/lib/useAcknowledgedPending';

type ProfileRefreshButtonProps = {
  refreshing: boolean;
  saving: boolean;
  onRefresh: () => Promise<void>;
};

export function ProfileRefreshButton({ refreshing, saving, onRefresh }: ProfileRefreshButtonProps) {
  const { t } = useTranslation('profile');
  const { busy, acknowledge } = useAcknowledgedPending(refreshing);
  return (
    <IconButton
      variant='ghost'
      type='button'
      label={t(busy ? 'overview.refreshing' : 'overview.refresh')}
      aria-busy={busy}
      aria-disabled={busy || saving}
      onClick={() => {
        if (busy || saving) return;
        acknowledge();
        void onRefresh();
      }}
    >
      <RefreshCw className={`size-4${busy ? ' icon-spinning' : ''}`} aria-hidden='true' />
    </IconButton>
  );
}
