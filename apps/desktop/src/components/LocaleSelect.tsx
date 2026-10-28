import { useId } from 'react';
import { useTranslation } from 'react-i18next';
import type { SupportedLocale } from '@/i18n';
import { Label } from './ui/label';
import { Select } from './ui/select';
import { Button } from './ui/button';

type LocaleSelectProps = {
  value: SupportedLocale;
  onChange: (locale: SupportedLocale) => void;
  disabled?: boolean;
  saveFailed?: boolean;
};

// 同意gateと設定で同じ自称表記・保存失敗の説明を使う。
export function LocaleSelect({ value, onChange, disabled = false, saveFailed = false }: LocaleSelectProps) {
  const { t } = useTranslation('settings');
  const hintId = useId();
  return (
    <div className='space-y-2'>
      <Label>
        <span>{t('appearance.languageLabel')}</span>
        <Select
          value={value}
          disabled={disabled}
          aria-label={t('appearance.languageLabel')}
          aria-describedby={saveFailed ? hintId : undefined}
          onChange={(event) => onChange(event.target.value as SupportedLocale)}
        >
          <option value='ja'>{t('appearance.languageOptions.ja')}</option>
          <option value='en'>{t('appearance.languageOptions.en')}</option>
          <option value='zh-CN'>{t('appearance.languageOptions.zh-CN')}</option>
        </Select>
      </Label>
      {saveFailed ? (
        <>
          <p id={hintId} className='text-sm leading-5 text-muted-foreground' role='status'>
            {t('appearance.languageSaveFailed')}
          </p>
          <Button type='button' variant='secondary' disabled={disabled} onClick={() => onChange(value)}>
            {t('appearance.retryLanguageSave')}
          </Button>
        </>
      ) : null}
    </div>
  );
}
