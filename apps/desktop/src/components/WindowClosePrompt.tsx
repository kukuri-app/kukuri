import { useEffect, useState } from 'react';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { useTranslation } from 'react-i18next';

import { Button } from '@/components/ui/button';
import {
  Dialog,
  DialogBody,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { Notice } from '@/components/ui/notice';
import {
  getPendingWindowCloseRequest,
  respondWindowCloseRequest,
  type WindowCloseBehavior,
  type WindowClosePrompt as WindowClosePromptPayload,
} from '@/lib/api/windowClosePreference';
import { isTauriRuntime } from '@/lib/releaseReadiness';

const WINDOW_CLOSE_REQUESTED_EVENT = 'kukuri://window-close-requested';

type WindowClosePromptViewProps = {
  open: boolean;
  remember: boolean;
  pending: boolean;
  error: string | null;
  onRememberChange: (remember: boolean) => void;
  onChoose: (behavior: WindowCloseBehavior) => void;
  onCancel: () => void;
};

export function WindowClosePromptView({
  open,
  remember,
  pending,
  error,
  onRememberChange,
  onChoose,
  onCancel,
}: WindowClosePromptViewProps) {
  const { t } = useTranslation('settings');
  return (
    <Dialog open={open} onOpenChange={(nextOpen) => !nextOpen && !pending && onCancel()}>
      <DialogContent hideClose className='w-[min(32rem,92vw)]'>
        <DialogHeader>
          <DialogTitle>{t('windowClosePrompt.title')}</DialogTitle>
          <DialogDescription>{t('windowClosePrompt.description')}</DialogDescription>
        </DialogHeader>
        <DialogBody className='space-y-4'>
          <label className='flex min-w-0 items-start gap-3 text-sm text-foreground'>
            <input
              type='checkbox'
              className='mt-1 size-4 shrink-0 accent-[var(--accent)]'
              checked={remember}
              disabled={pending}
              onChange={(event) => onRememberChange(event.target.checked)}
            />
            <span>{t('windowClosePrompt.remember')}</span>
          </label>
          {error ? <Notice tone='destructive'>{t('windowClosePrompt.error')}</Notice> : null}
        </DialogBody>
        <DialogFooter>
          <Button type='button' variant='secondary' disabled={pending} onClick={onCancel}>
            {t('windowClosePrompt.cancel')}
          </Button>
          <Button
            type='button'
            variant='secondary'
            disabled={pending}
            onClick={() => onChoose('tray')}
          >
            {t('windowClosePrompt.tray')}
          </Button>
          <Button type='button' disabled={pending} onClick={() => onChoose('quit')}>
            {pending ? t('windowClosePrompt.pending') : t('windowClosePrompt.quit')}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

export function WindowClosePrompt() {
  const [request, setRequest] = useState<WindowClosePromptPayload | null>(null);
  const [remember, setRemember] = useState(false);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!isTauriRuntime()) return;
    let unlisten: UnlistenFn | undefined;
    let cancelled = false;
    void (async () => {
      try {
        const dispose = await listen<WindowClosePromptPayload>(
          WINDOW_CLOSE_REQUESTED_EVENT,
          (event) => {
            setRequest(event.payload);
            setRemember(false);
            setError(null);
          }
        );
        if (cancelled) {
          dispose();
          return;
        }
        unlisten = dispose;
        const outstanding = await getPendingWindowCloseRequest();
        if (!cancelled && outstanding) setRequest(outstanding);
      } catch {
        // Startup remains usable. A later close re-emits no side effect and the backend keeps it pending.
      }
    })();
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  const respond = async (behavior: WindowCloseBehavior | null) => {
    if (!request || pending) return;
    setPending(true);
    setError(null);
    try {
      await respondWindowCloseRequest(request.request_id, behavior, behavior !== null && remember);
      setRequest(null);
      setRemember(false);
    } catch (responseError) {
      setError(responseError instanceof Error ? responseError.message : String(responseError));
    } finally {
      setPending(false);
    }
  };

  return (
    <WindowClosePromptView
      open={request !== null}
      remember={remember}
      pending={pending}
      error={error}
      onRememberChange={setRemember}
      onChoose={(behavior) => void respond(behavior)}
      onCancel={() => void respond(null)}
    />
  );
}
