import * as React from 'react';

import { Button, type ButtonProps } from '@/components/ui/button';
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from '@/components/ui/tooltip';

export interface IconButtonProps
  extends Omit<ButtonProps, 'aria-label' | 'asChild' | 'size'> {
  label: string;
}

type IconButtonTooltipProps = {
  children: React.ReactElement;
  label: string;
  className?: string;
};

export function IconButtonTooltip({ children, label, className }: IconButtonTooltipProps) {
  return (
    <TooltipProvider delayDuration={180}>
      <Tooltip>
        <TooltipTrigger asChild>{children}</TooltipTrigger>
        <TooltipContent className={className}>{label}</TooltipContent>
      </Tooltip>
    </TooltipProvider>
  );
}

export const IconButton = React.forwardRef<HTMLButtonElement, IconButtonProps>(
  ({ children, label, ...props }, ref) => (
    <IconButtonTooltip label={label}>
      <Button ref={ref} size='icon' aria-label={label} {...props}>
        {children}
      </Button>
    </IconButtonTooltip>
  )
);

IconButton.displayName = 'IconButton';
