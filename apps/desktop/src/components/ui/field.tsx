import * as React from 'react';

import { cn } from '@/lib/utils';

type FieldProps = React.HTMLAttributes<HTMLElement> & {
  // 子が入力欄でない（button など）ときは 'div'。label で包むと文字が子の読み上げ名になり、文字の押下で子が押される（#1550）。
  as?: 'label' | 'div';
  label: string;
  hint?: string;
  message?: string;
  tone?: 'default' | 'danger';
};

export function Field({
  as: Component = 'label',
  label,
  hint,
  message,
  tone = 'default',
  className,
  children,
  ...props
}: FieldProps) {
  return (
    <Component className={cn('field flex flex-col gap-2', className)} {...props}>
      <span>{label}</span>
      {children}
      {hint ? <small className='text-[0.78rem] text-[var(--muted-foreground-soft)]'>{hint}</small> : null}
      {message ? (
        <small
          className={cn(
            'text-[0.78rem]',
            tone === 'danger' ? 'text-[var(--destructive)]' : 'text-[var(--muted-foreground)]'
          )}
        >
          {message}
        </small>
      ) : null}
    </Component>
  );
}
