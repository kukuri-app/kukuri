import { useTranslation } from 'react-i18next';

import { Button } from '@/components/ui/button';
import { Card, CardHeader } from '@/components/ui/card';
import { Notice } from '@/components/ui/notice';

import { SettingsActionRow } from './SettingsActionRow';

type SafetyPanelProps = {
  adultContentEnabled: boolean;
  onAdultContentEnabledChange: (enabled: boolean) => void;
  onOpenMutedUsers: () => void;
  onOpenBlockedUsers: () => void;
};

// #858: 成人向け表現の表示設定(ADR 0046)。既定 OFF で、18歳以上の自己申告とは
// 別の状態。ON にしない限り、対象メディアのバイト列は取得もデコードもされない。
// #961: ミュート(端末内)とブロック(署名済み・同期・双方向の表示非表示)の違いを説明し、
// 一覧はプロフィール配下の connections 画面へ誘導する。一覧をここへ複製しない。
export function SafetyPanel({
  adultContentEnabled,
  onAdultContentEnabledChange,
  onOpenMutedUsers,
  onOpenBlockedUsers,
}: SafetyPanelProps) {
  const { t } = useTranslation(['settings']);

  return (
    <Card className='space-y-4'>
      <CardHeader>
        <h3>{t('settings:safety.title')}</h3>
      </CardHeader>

      <label className='flex min-w-0 items-center gap-3 rounded-[var(--radius-input)] border border-[var(--border-subtle)] bg-[var(--surface-panel-soft)] px-4 py-3 text-sm text-foreground'>
        <input
          type='checkbox'
          checked={adultContentEnabled}
          onChange={(event) => onAdultContentEnabledChange(event.currentTarget.checked)}
          data-testid='adult-content-display-toggle'
        />
        <span>{t('settings:safety.adultContent.label')}</span>
      </label>

      <Notice>{t('settings:safety.adultContent.description')}</Notice>

      <section className='space-y-3' aria-labelledby='safety-social-title'>
        <h4 id='safety-social-title' className='text-sm font-semibold text-foreground'>
          {t('settings:safety.social.title')}
        </h4>
        <Notice>{t('settings:safety.social.mute')}</Notice>
        <Notice>{t('settings:safety.social.block')}</Notice>
        <SettingsActionRow>
          <Button variant='secondary' type='button' onClick={onOpenMutedUsers}>
            {t('settings:safety.social.openMuted')}
          </Button>
          <Button variant='secondary' type='button' onClick={onOpenBlockedUsers}>
            {t('settings:safety.social.openBlocked')}
          </Button>
        </SettingsActionRow>
      </section>
    </Card>
  );
}
