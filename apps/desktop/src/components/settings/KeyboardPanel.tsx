import { Fragment } from 'react';
import { useTranslation } from 'react-i18next';

import { Card, CardHeader } from '@/components/ui/card';
import { Notice } from '@/components/ui/notice';

type KeyboardGroupId = 'composer' | 'screens';

type KeyboardRowSpec = {
  id: string;
  keys: string[];
};

// #964: 割り当て済みのキーと有効条件の案内。キー名はロケールに依存しない定数として
// ここに置き、操作・条件の文言は i18n `settings:keyboard.groups.<group>.rows.<row>` が正本。
// 各行は実装済みの handler と test に対応する(未対応キーは `unassigned` で明示する)。
const KEYBOARD_GROUPS: Array<{ id: KeyboardGroupId; rows: KeyboardRowSpec[] }> = [
  {
    id: 'composer',
    rows: [
      { id: 'esc', keys: ['Esc'] },
      { id: 'ctrlEnter', keys: ['Ctrl', 'Enter'] },
      { id: 'tab', keys: ['Tab'] },
      { id: 'arrows', keys: ['↑', '↓'] },
      { id: 'enter', keys: ['Enter'] },
    ],
  },
  {
    id: 'screens',
    rows: [
      { id: 'esc', keys: ['Esc'] },
      { id: 'tab', keys: ['Tab'] },
      { id: 'columnMenu', keys: ['↑', '↓'] },
      { id: 'timelineTabs', keys: ['←', '→'] },
      { id: 'postMenu', keys: ['Shift', 'F10'] },
      { id: 'media', keys: ['←', '→'] },
    ],
  },
];

export function KeyboardPanel() {
  const { t } = useTranslation(['settings']);
  const columns = {
    keys: t('settings:keyboard.columns.keys'),
    action: t('settings:keyboard.columns.action'),
    condition: t('settings:keyboard.columns.condition'),
  };

  return (
    <Card className='space-y-4'>
      <CardHeader>
        <h3>{t('settings:keyboard.title')}</h3>
      </CardHeader>

      {KEYBOARD_GROUPS.map((group) => {
        const title = t(`settings:keyboard.groups.${group.id}.title`);
        return (
          <section key={group.id} className='space-y-3' aria-label={title}>
            <h4 className='text-sm font-semibold text-foreground'>{title}</h4>
            <ul className='m-0 grid list-none gap-2 p-0'>
              {group.rows.map((row) => (
                <li
                  key={row.id}
                  className='grid min-w-0 gap-1 rounded-[var(--radius-input)] border border-[var(--border-subtle)] bg-[var(--surface-panel-soft)] px-4 py-3 text-sm leading-6'
                >
                  <span className='flex flex-wrap items-center gap-1'>
                    <span className='sr-only'>{columns.keys}: </span>
                    {row.keys.map((key, keyIndex) => (
                      <Fragment key={`${key}-${keyIndex}`}>
                        {keyIndex > 0 ? <span aria-hidden='true'>+</span> : null}
                        <kbd className='rounded-[var(--radius-sm)] border border-[var(--border-subtle)] bg-[var(--surface-input)] px-2 py-0.5 text-[0.8rem] text-foreground'>
                          {key}
                        </kbd>
                      </Fragment>
                    ))}
                  </span>
                  <span className='min-w-0 [overflow-wrap:anywhere] text-foreground'>
                    <span className='sr-only'>{columns.action}: </span>
                    {t(`settings:keyboard.groups.${group.id}.rows.${row.id}.action`)}
                  </span>
                  <span className='min-w-0 [overflow-wrap:anywhere] text-[var(--muted-foreground)]'>
                    <span className='font-semibold'>{columns.condition}: </span>
                    {t(`settings:keyboard.groups.${group.id}.rows.${row.id}.condition`)}
                  </span>
                </li>
              ))}
            </ul>
          </section>
        );
      })}

      <Notice>{t('settings:keyboard.unassigned')}</Notice>
    </Card>
  );
}
