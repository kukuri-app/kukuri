import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { encode } from 'uqr';
import type { AccountTransferLink, AccountTransferStatus } from '@/lib/api/types.generated';
import {
  cancelAccountTransfer,
  createAccountTransferInvite,
  decideAccountTransfer,
  getAccountTransferStatus,
  openAccountTransfer,
} from '@/lib/api/identity';
import { copyTextToClipboard } from '@/lib/utils';
import { Button } from '@/components/ui/button';
import { Field } from '@/components/ui/field';
import { Input } from '@/components/ui/input';
import { Notice } from '@/components/ui/notice';

const POLL_MS = 500;
const IDLE: AccountTransferStatus = { state: 'idle' };

// #1211: QR・専用リンクの移行（AC-1 は両端末の確認まで）。移行元は招待を出して QR とリンクを表示し、
// 移行先はリンクを貼り付けて接続する。閉じたら移行を取り消す。リンクは log・URL の query へ出さない。
export function AccountTransferPanel({ role, initialLink = '' }: {
  role: 'source' | 'target';
  initialLink?: string;
}) {
  const { t } = useTranslation('settings');
  const [invite, setInvite] = useState<AccountTransferLink | null>(null);
  const [link, setLink] = useState(initialLink);
  const [status, setStatus] = useState<AccountTransferStatus>(IDLE);
  const [error, setError] = useState<'prepareFailed' | 'invalid' | null>(null);
  const [pending, setPending] = useState(false);
  const [copied, setCopied] = useState(false);
  const [now, setNow] = useState(() => Date.now());

  // 開発時の StrictMode の再 mount では、取り消した側の結果を採らない。
  const issue = async (active: () => boolean = () => true) => {
    setPending(true);
    setCopied(false);
    setError(null);
    try {
      const next = await createAccountTransferInvite();
      if (!active()) return;
      setInvite(next);
      setStatus({ state: 'waiting', expires_at_ms: next.expires_at_ms });
      setNow(Date.now());
    } catch {
      setStatus(IDLE);
      setError('prepareFailed');
    } finally {
      setPending(false);
    }
  };

  const connect = async () => {
    setPending(true);
    setError(null);
    try {
      await openAccountTransfer(link.trim());
      setStatus({ state: 'connecting' });
    } catch {
      setError('invalid');
    } finally {
      setPending(false);
    }
  };

  useEffect(() => {
    let active = true;
    if (role === 'source') void issue(() => active);
    return () => { active = false; void cancelAccountTransfer().catch(() => undefined); };
    // 開いたときに招待を出し、閉じたら取り消す。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const active = ['waiting', 'connecting', 'confirming'].includes(status.state);
  useEffect(() => {
    if (!active) return;
    const timer = window.setInterval(() => {
      setNow(Date.now());
      void getAccountTransferStatus().then(setStatus).catch(() => undefined);
    }, POLL_MS);
    return () => window.clearInterval(timer);
  }, [active]);

  const decide = async (accept: boolean) => {
    setPending(true);
    try { await decideAccountTransfer(accept); setStatus(await getAccountTransferStatus()); }
    catch { setStatus(await getAccountTransferStatus().catch(() => status)); }
    finally { setPending(false); }
  };

  const retry = () => {
    setError(null);
    if (role === 'source') void issue();
    else setStatus(IDLE);
  };

  if (status.state === 'confirming') {
    return <section className='space-y-3' data-testid='account-transfer-confirming'>
      <p className='text-sm'>{t(`accountTransfer.confirm.${role}`)}</p>
      <p className='text-center font-mono text-3xl font-semibold tracking-[0.2em]' aria-label={t('accountTransfer.confirm.codeLabel')}>
        {`${status.code.slice(0, 3)} ${status.code.slice(3)}`}
      </p>
      {status.local_accepted
        ? <p role='status' className='text-sm text-muted-foreground'>{t('accountTransfer.confirm.waitingPeer')}</p>
        : <div className='flex justify-end gap-2'>
          <Button variant='secondary' disabled={pending} onClick={() => void decide(false)}>{t('accountTransfer.confirm.mismatch')}</Button>
          <Button disabled={pending} onClick={() => void decide(true)}>{t('accountTransfer.confirm.match')}</Button>
        </div>}
    </section>;
  }
  if (status.state === 'confirmed') {
    return <Notice tone='accent' data-testid='account-transfer-confirmed'>{t('accountTransfer.confirmed')}</Notice>;
  }
  if (status.state === 'failed') {
    return <section className='space-y-3' data-testid='account-transfer-failed'>
      <Notice tone='destructive'>{t(`accountTransfer.failure.${status.reason}`)}</Notice>
      <Button variant='secondary' disabled={pending} onClick={retry}>{t('accountTransfer.retry')}</Button>
    </section>;
  }
  if (error === 'prepareFailed') {
    return <section className='space-y-3'>
      <Notice tone='destructive'>{t('accountTransfer.failure.prepareFailed')}</Notice>
      <Button variant='secondary' disabled={pending} onClick={retry}>{t('accountTransfer.retry')}</Button>
    </section>;
  }
  if (role === 'source') {
    if (!invite || status.state !== 'waiting') return <p role='status'>{t('accountTransfer.source.preparing')}</p>;
    const remaining = Math.max(0, Math.ceil((invite.expires_at_ms - now) / 1000));
    return <section className='space-y-3' data-testid='account-transfer-source'>
      <p className='text-sm'>{t('accountTransfer.source.instructions')}</p>
      <div className='flex flex-wrap items-center gap-4'>
        <TransferQr link={invite.link} label={t('accountTransfer.source.qrLabel')} />
        <p role='timer' className='text-sm text-muted-foreground'>
          {t('accountTransfer.source.remaining', { time: `${Math.floor(remaining / 60)}:${String(remaining % 60).padStart(2, '0')}` })}
        </p>
      </div>
      <Field label={t('accountTransfer.source.linkLabel')}>
        <Input readOnly value={invite.link} onFocus={(event) => event.currentTarget.select()} />
      </Field>
      <Button variant='secondary' onClick={() => void copyTextToClipboard(invite.link).then(setCopied)}>
        {t(copied ? 'accountTransfer.source.copied' : 'accountTransfer.source.copy')}
      </Button>
    </section>;
  }
  if (status.state === 'connecting') return <p role='status'>{t('accountTransfer.target.connecting')}</p>;
  return <section className='space-y-3' data-testid='account-transfer-target'>
    <Field label={t('accountTransfer.target.linkLabel')} hint={t('accountTransfer.target.instructions')}
      tone={error ? 'danger' : 'default'} message={error ? t('accountTransfer.failure.invalid') : undefined}>
      <Input value={link} onChange={(event) => { setLink(event.target.value); setError(null); }} autoComplete='off' spellCheck={false} />
    </Field>
    <Button disabled={pending || !link.trim()} onClick={() => void connect()}>
      {t(pending ? 'accountTransfer.target.connecting' : 'accountTransfer.target.connect')}
    </Button>
  </section>;
}

// QR は theme に関わらず読み取りやすい白地・黒の module で描く（tokens の例外）。
function TransferQr({ link, label }: { link: string; label: string }) {
  const { data } = encode(link, { ecc: 'M', border: 4 });
  const size = data.length;
  const modules = data.flatMap((row, y) => row.map((dark, x) => (dark ? `M${x} ${y}h1v1h-1z` : ''))).join('');
  return <svg role='img' aria-label={label} viewBox={`0 0 ${size} ${size}`} shapeRendering='crispEdges' className='size-48 shrink-0'>
    <rect width={size} height={size} fill='#fff' />
    <path d={modules} fill='#000' />
  </svg>;
}
