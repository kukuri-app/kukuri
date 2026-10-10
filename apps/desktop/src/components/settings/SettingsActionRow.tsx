import type * as React from 'react';

import { cn } from '@/lib/utils';

type SettingsActionRowProps = {
  children: React.ReactNode;
  className?: string;
};

export function SettingsActionRow({ children, className }: SettingsActionRowProps) {
  return (
    <div
      className={cn(
        'flex flex-wrap items-stretch gap-3 [&>*]:flex-auto [&>button]:whitespace-normal sm:[&>*]:flex-1 sm:[&>*]:min-w-[9.5rem]',
        className
      )}
    >
      {children}
    </div>
  );
}
