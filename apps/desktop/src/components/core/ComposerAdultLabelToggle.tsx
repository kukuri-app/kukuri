import { useTranslation } from 'react-i18next';

type ComposerAdultLabelToggleProps = {
  checked: boolean;
  onChange: (labeled: boolean) => void;
};

/** #858: 成人向けの自己申告。投稿の composer と公開スレッドの作成で共用する(#1889)。 */
export function ComposerAdultLabelToggle({ checked, onChange }: ComposerAdultLabelToggleProps) {
  const { t } = useTranslation(['common']);
  return (
    <label className='topic-diagnostic topic-diagnostic-secondary flex items-center gap-2'>
      <input
        type='checkbox'
        checked={checked}
        onChange={(event) => onChange(event.currentTarget.checked)}
        data-testid='composer-adult-label-toggle'
      />
      <span>{t('composer.adultLabel')}</span>
    </label>
  );
}
