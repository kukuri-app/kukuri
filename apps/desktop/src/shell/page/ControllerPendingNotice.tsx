import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { Button } from '@/components/ui/button';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { onControllerPending } from '@/lib/api/invoke/dispatch';

// #1220 AC-3a: 新しいアクセスの配布を本人の別の端末が行うので保留した操作(共有リンクの作成・書き込み)を説明する。
export function ControllerPendingNotice() {
  const { t } = useTranslation(['channels', 'common']);
  const [open, setOpen] = useState(false);
  useEffect(() => onControllerPending(() => setOpen(true)), []);
  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{t('channels:controllerPending.title')}</DialogTitle>
          <DialogDescription>{t('channels:controllerPending.description')}</DialogDescription>
        </DialogHeader>
        <DialogFooter>
          <Button onClick={() => setOpen(false)}>{t('common:actions.close')}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
