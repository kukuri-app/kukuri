import { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useShallow } from 'zustand/react/shallow';
import { Check, RefreshCw } from 'lucide-react';
import { AuthorAvatar } from '@/components/core/AuthorAvatar';
import { Button } from '@/components/ui/button';
import { IconButton } from '@/components/ui/icon-button';
import { Notice } from '@/components/ui/notice';
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover';
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription, DialogBody, DialogFooter } from '@/components/ui/dialog';
import { AccountKeyImportForm } from '@/components/settings/AccountKeyImportForm';
import { AccountTransferPanel } from '@/components/settings/AccountTransferPanel';
import { useAccountTransferLink } from '@/shell/page/useAccountTransferLink';
import { onAccountAddRequest } from '@/shell/page/accountAddRequest';
import { getAccountDisplay, listAccounts } from '@/lib/api/identity';
import type { AccountDisplay, AccountsSnapshot } from '@/lib/api/types.generated';
import { accountCreationOperationId, changeAccountSession, takeUnseenAccountFailure } from '@/lib/accountSession';
import { useDesktopShellStore } from '@/shell/store';
import { resolveProfilePictureSrc } from '@/shell/presentation';

const dialogTitle = {
  import: 'accountMenu.add', logout: 'accountMenu.logoutTitle', sync: 'settings:accountTransfer.sync.title',
  'transfer-source': 'settings:accountTransfer.source.title', 'transfer-target': 'settings:accountTransfer.target.title',
} as const;
const dialogDescription = {
  import: 'accountMenu.importDescription', logout: 'accountMenu.logoutDescription', sync: 'settings:accountTransfer.sync.description',
  'transfer-source': 'settings:accountTransfer.source.description', 'transfer-target': 'settings:accountTransfer.target.description',
} as const;

export function AccountMenu({ onProfile, onManage, onOpen }: {
  onProfile: () => void;
  onManage: () => void;
  onOpen: () => void;
}) {
  const { t } = useTranslation('shell');
  const [open, setOpen] = useState(false);
  const [dialog, setDialog] = useState<'import' | 'logout' | 'sync' | 'transfer-source' | 'transfer-target' | null>(null);
  const [transferLink, setTransferLink] = useState('');
  const [snapshot, setSnapshot] = useState<AccountsSnapshot | null>(null);
  const [display, setDisplay] = useState<AccountDisplay[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [receivingHistory, setReceivingHistory] = useState(false);
  const [loading, setLoading] = useState(false);
  const menu = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const moveFocus = useRef(false);
  const { localProfile, mediaObjectUrls, pubkey } = useDesktopShellStore(useShallow((s) => ({ localProfile: s.localProfile, mediaObjectUrls: s.mediaObjectUrls, pubkey: s.syncStatus.local_author_pubkey })));
  const label = localProfile?.display_name || localProfile?.name || t('accountMenu.unknown');
  // 版の更新でレイアウトを外していた間に切替が失敗したら、戻ったときにメニューを開いて知らせる（ADR 0059 §1）。
  const unseenFailure = useRef(false);
  useEffect(() => { if (takeUnseenAccountFailure()) { unseenFailure.current = true; setOpen(true); } }, []);
  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const [accounts, profiles] = await Promise.all([listAccounts(), getAccountDisplay()]);
      setSnapshot(accounts); setDisplay(profiles); setError(unseenFailure.current ? t('accountMenu.actionFailed') : null);
      unseenFailure.current = false;
    } catch { setError(t('accountMenu.loadFailed')); }
    finally { setLoading(false); }
  }, [t]);
  useEffect(() => { if (open) void refresh(); }, [open, refresh]);
  const switchTo = async (id: string, logout = false) => {
    if (pending) return;
    setPending(true); setError(null);
    try { await changeAccountSession(id, logout); }
    catch { setError(t('accountMenu.actionFailed')); setPending(false); }
  };
  const active = snapshot?.accounts.find((a) => a.id === snapshot.active_account_id && a.pubkey === pubkey);
  // #1211: OS のリンク起動で受けた移行用のリンクは、移行先の入力欄へ入れるだけで接続はしない。
  useAccountTransferLink((link) => { setOpen(false); setError(null); setTransferLink(link); setDialog('transfer-target'); });
  useEffect(() => onAccountAddRequest((next) => { setOpen(false); setError(null); setDialog(next); void refresh(); }), [refresh]);
  const openDialog = (next: 'import' | 'logout' | 'sync') => { moveFocus.current = true; setOpen(false); setError(null); setDialog(next); };
  const closeDialog = () => { if (!pending && !receivingHistory) { setDialog(null); setTransferLink(''); moveFocus.current = false; } };
  return <>
    <Popover open={open} onOpenChange={(next) => { if (next) onOpen(); moveFocus.current = false; setOpen(next); }}>
      <PopoverTrigger asChild>
        <IconButton ref={trigger} type='button' variant='secondary' className='shell-account-menu-trigger' label={t('accountMenu.open')} aria-haspopup='menu' aria-expanded={open} data-testid='account-menu-trigger'>
          <AuthorAvatar label={label} picture={resolveProfilePictureSrc(localProfile, mediaObjectUrls)} />
        </IconButton>
      </PopoverTrigger>
      <PopoverContent ref={menu} role='menu' aria-label={t('accountMenu.open')} align='start' className='shell-account-menu w-[min(22rem,calc(100vw-1rem))] max-h-[min(80vh,36rem)] overflow-y-auto space-y-1'
        onOpenAutoFocus={(event) => { event.preventDefault(); menu.current?.querySelector<HTMLButtonElement>('[role="menuitem"]')?.focus(); }}
        onCloseAutoFocus={(event) => { if (moveFocus.current) event.preventDefault(); }}
        onKeyDown={(event) => {
          if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) return;
          event.preventDefault();
          const items = [...(menu.current?.querySelectorAll<HTMLButtonElement>('button[role^="menuitem"]:not(:disabled)') ?? [])];
          const index = items.indexOf(document.activeElement as HTMLButtonElement);
          const next = event.key === 'Home' ? 0 : event.key === 'End' ? items.length - 1 : (index + (event.key === 'ArrowUp' ? -1 : 1) + items.length) % items.length;
          items[next]?.focus();
        }}>
        <Button role='menuitem' variant='ghost' className='w-full justify-start' onClick={() => { moveFocus.current = true; setOpen(false); onProfile(); }}>{t('accountMenu.profile')}</Button>
        <div role='separator' className='border-t border-[var(--border-subtle)]' />
        {loading ? <p role='status'>{t('accountMenu.loading')}</p> : null}
        {snapshot?.accounts.map((account) => {
          const profile = display.find((entry) => entry.id === account.id);
          const current = account.id === snapshot.active_account_id;
          const name = (current ? localProfile?.display_name || localProfile?.name : null) || profile?.display_name || profile?.name || t('accountMenu.unknown');
          // #1650: 同期できるのは起動中のアカウントだけなので、使用中の行にだけ同期のボタンを置く（チェックの右）。
          return <div key={account.id} className='flex min-w-0 items-center gap-1'>
            <button type='button' role='menuitemradio' aria-checked={current} disabled={pending} className='flex min-w-0 flex-1 items-center gap-3 rounded-[var(--radius-input)] p-2 text-left hover:bg-[var(--surface-button-ghost-hover)] focus-visible:outline focus-visible:outline-2 focus-visible:outline-[var(--ring)]'
              onClick={() => { if (!current) void switchTo(account.id); }}>
              <AuthorAvatar label={name} picture={current ? resolveProfilePictureSrc(localProfile, mediaObjectUrls) : profile?.picture} />
              <span className='min-w-0 flex-1'><span className='block truncate'>{name}</span><span className='block truncate text-xs text-muted-foreground'>{profile?.unavailable ? t('accountMenu.unavailable') : (current ? localProfile?.name : profile?.name) ? `@${current ? localProfile?.name : profile?.name}` : t('accountMenu.noUsername')}</span></span>
              {current ? <Check className='size-4' aria-label={t('accountMenu.current')} /> : null}
            </button>
            {active?.id === account.id ? <IconButton role='menuitem' variant='ghost' label={t('accountMenu.sync')} disabled={pending} onClick={() => openDialog('sync')}>
              <RefreshCw className='size-4' aria-hidden='true' />
            </IconButton> : null}
          </div>;
        })}
        {error ? <Notice tone='destructive'>{error}<Button variant='ghost' onClick={() => void refresh()}>{t('accountMenu.retry')}</Button></Notice> : null}
        <div role='separator' className='border-t border-[var(--border-subtle)]' />
        <Button role='menuitem' variant='ghost' className='w-full justify-start' disabled={pending} onClick={() => openDialog('import')}>{t('accountMenu.add')}</Button>
        <Button role='menuitem' variant='ghost' className='w-full justify-start' onClick={() => { moveFocus.current = true; setOpen(false); onManage(); }}>{t('accountMenu.manage')}</Button>
        <Button role='menuitem' variant='ghost' className='w-full justify-start text-destructive' disabled={pending || !active} onClick={() => openDialog('logout')}>{t('accountMenu.logout')}</Button>
      </PopoverContent>
    </Popover>
    <Dialog open={dialog !== null} onOpenChange={(next) => { if (!next) closeDialog(); }}>
      <DialogContent hideClose={receivingHistory} className='w-[min(34rem,94vw)] max-h-[90vh] overflow-y-auto' onCloseAutoFocus={(event) => { event.preventDefault(); trigger.current?.focus(); }}
        onOpenAutoFocus={(event) => { if (dialog === 'logout') { event.preventDefault(); document.querySelector<HTMLButtonElement>('[data-testid="logout-cancel"]')?.focus(); } }}>
        <DialogHeader><DialogTitle>{t(dialogTitle[dialog ?? 'import'])}</DialogTitle><DialogDescription>{t(dialogDescription[dialog ?? 'import'])}</DialogDescription></DialogHeader>
        <DialogBody>
          {dialog === 'sync' ? <AccountTransferPanel role='sync' /> : dialog === 'transfer-source' || dialog === 'transfer-target' ? <>
            {/* #1211: 移行先は完了の画面の「このアカウントを使う」で、受け取ったアカウントへ切り替える（使っているアカウントなら
                閉じるだけ）。
                履歴を受けている間は閉じさせず、「戻る」も出さない（終えるのは「やめる」だけ。2026-10-04 ユーザー決定）。 */}
            <AccountTransferPanel key={`${dialog}:${transferLink}`} role={dialog === 'transfer-source' ? 'source' : 'target'} initialLink={transferLink}
              onCompleted={(id) => void listAccounts().then((accounts) => accounts.active_account_id === id ? closeDialog() : switchTo(id)).catch(() => setError(t('accountMenu.actionFailed')))}
              onReceivingHistory={setReceivingHistory} />
            {receivingHistory ? null : <Button variant='ghost' className='mt-4' onClick={() => { setTransferLink(''); setDialog('import'); }}>{t('accountMenu.back')}</Button>}
          </> : dialog === 'import' ? <>
            <Button disabled={pending || !active} className='mb-4 w-full' data-testid='create-new-account' onClick={() => {
              if (!active || pending) return;
              setPending(true); setError(null);
              void Promise.resolve().then(() => changeAccountSession(active.id, false, accountCreationOperationId(active.id))).catch(() => { setError(t('accountMenu.actionFailed')); setPending(false); });
            }}>{t(pending ? 'accountMenu.pending' : 'accountMenu.create')}</Button>
            <section className='mb-4 space-y-2'>
              <h4 className='text-sm font-semibold text-foreground'>{t('accountMenu.transferHeading')}</h4>
              <div className='flex flex-wrap gap-2'>
                <Button variant='secondary' disabled={pending || !active} onClick={() => setDialog('transfer-source')}>{t('accountMenu.transferOut')}</Button>
                <Button variant='secondary' disabled={pending} onClick={() => setDialog('transfer-target')}>{t('accountMenu.transferIn')}</Button>
              </div>
            </section>
            <fieldset disabled={pending}><AccountKeyImportForm onImported={refresh} onSwitch={(id) => switchTo(id)} switching={pending} /></fieldset>
          </> : <>
            <p className='font-semibold'>{label}</p><p>{localProfile?.name ? `@${localProfile.name}` : t('accountMenu.noUsername')}</p>
            <p>{t(snapshot?.accounts.length === 1 ? 'accountMenu.logoutCreates' : 'accountMenu.logoutReturns')}</p>
          </>}
          {error ? <Notice tone='destructive'>{error}</Notice> : null}
        </DialogBody>
        {dialog === 'logout' ? <DialogFooter>
          <Button variant='secondary' disabled={pending} data-testid='logout-cancel' onClick={closeDialog}>{t('accountMenu.cancel')}</Button>
          <Button disabled={pending || !active} onClick={() => { if (active) void switchTo(active.id, true); }}>{t(pending ? 'accountMenu.pending' : 'accountMenu.yes')}</Button>
        </DialogFooter> : null}
      </DialogContent>
    </Dialog>
  </>;
}
