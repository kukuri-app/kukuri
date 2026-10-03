import { useTranslation } from 'react-i18next';

import { Notice } from '@/components/ui/notice';

// Web だけ: このブラウザの保存の状態と、消えたときの戻し方（ADR 0059 §6）。許可が無いのに「消えない」と示さない。
export function BrowserStorageNotice({ persisted }: { persisted: boolean }) {
  const { t } = useTranslation('settings');
  return (
    <Notice tone={persisted ? 'neutral' : 'warning'} className='space-y-2' data-testid='browser-storage-notice'>
      <p>{t(persisted ? 'accountKey.browserStorage.persisted' : 'accountKey.browserStorage.notPersisted')}</p>
      <p>{t('accountKey.browserStorage.recovery')}</p>
    </Notice>
  );
}
