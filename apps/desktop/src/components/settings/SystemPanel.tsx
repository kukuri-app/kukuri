import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { Card, CardHeader } from '@/components/ui/card';
import { Notice } from '@/components/ui/notice';
import { Select } from '@/components/ui/select';
import {
  getWindowClosePreference,
  setWindowClosePreference,
  type WindowCloseBehavior,
} from '@/lib/api/windowClosePreference';
import { isDesktopMockActive } from '@/lib/api/invoke/dispatch';
import { isTauriRuntime } from '@/lib/releaseReadiness';

export type WindowCloseSetting = WindowCloseBehavior | 'ask';

type SystemPanelViewProps = {
  value: WindowCloseSetting;
  pending: boolean;
  error: boolean;
  onChange: (value: WindowCloseSetting) => void;
};

export function SystemPanelView({ value, pending, error, onChange }: SystemPanelViewProps) {
  const { t } = useTranslation('settings');
  return (
    <Card className='space-y-4'>
      <CardHeader>
        <h3>{t('system.title')}</h3>
        <small>{t('system.summary')}</small>
      </CardHeader>
      <label className='grid gap-2 text-sm font-semibold text-foreground'>
        <span>{t('system.windowCloseBehavior')}</span>
        <Select
          value={value}
          disabled={pending}
          onChange={(event) => onChange(event.target.value as WindowCloseSetting)}
        >
          <option value='ask'>{t('system.options.ask')}</option>
          <option value='quit'>{t('system.options.quit')}</option>
          <option value='tray'>{t('system.options.tray')}</option>
        </Select>
      </label>
      <Notice>{t('system.hint')}</Notice>
      {error ? <Notice tone='destructive'>{t('system.saveError')}</Notice> : null}
    </Card>
  );
}

export function SystemPanel() {
  const [value, setValue] = useState<WindowCloseSetting>('ask');
  const [pending, setPending] = useState(true);
  const [error, setError] = useState(false);

  useEffect(() => {
    if (!isTauriRuntime() && !isDesktopMockActive()) {
      setPending(false);
      return;
    }
    let active = true;
    void getWindowClosePreference()
      .then((preference) => {
        if (active) setValue(preference.behavior ?? 'ask');
      })
      .catch(() => {
        if (active) setError(true);
      })
      .finally(() => {
        if (active) setPending(false);
      });
    return () => {
      active = false;
    };
  }, []);

  const update = async (next: WindowCloseSetting) => {
    if (pending) return;
    const previous = value;
    setValue(next);
    setPending(true);
    setError(false);
    try {
      const saved = await setWindowClosePreference({ behavior: next === 'ask' ? null : next });
      setValue(saved.behavior ?? 'ask');
    } catch {
      setValue(previous);
      setError(true);
    } finally {
      setPending(false);
    }
  };

  return (
    <SystemPanelView
      value={value}
      pending={pending}
      error={error}
      onChange={(next) => void update(next)}
    />
  );
}
