import type * as React from 'react';

import { Field } from '@/components/ui/field';

type SettingsEditorFieldProps = {
  as?: 'label' | 'div';
  label: string;
  hint?: string;
  message?: string;
  tone?: 'default' | 'danger';
  children: React.ReactNode;
};

export function SettingsEditorField({
  as,
  label,
  hint,
  message,
  tone = 'default',
  children,
}: SettingsEditorFieldProps) {
  return (
    <Field as={as} label={label} hint={hint} message={message} tone={tone} className='gap-3'>
      {children}
    </Field>
  );
}
