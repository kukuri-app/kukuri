import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { encode } from 'uqr';
import type { AccountTransferHistory, AccountTransferLink, AccountTransferStatus } from '@/lib/api/types.generated';
import {
  cancelAccountTransfer,
  createAccountTransferInvite,
  decideAccountTransfer,
  getAccountTransferStatus,
  openAccountTransfer,
} from '@/lib/api/identity';
import { isTauriRuntime } from '@/lib/releaseReadiness';
import { copyTextToClipboard } from '@/lib/utils';
import { TRANSFER_LINK_PREFIX } from '@/shell/page/useAccountTransferLink';
import { Button } from '@/components/ui/button';
import { Field } from '@/components/ui/field';
import { Input } from '@/components/ui/input';
import { Notice } from '@/components/ui/notice';
import { Select } from '@/components/ui/select';

const POLL_MS = 500;
const SCAN_MS = 200;
const IDLE: AccountTransferStatus = { state: 'idle' };
const HISTORY_CHOICES = ['none', 'month', 'year', 'all'] as const;
type HistoryChoice = (typeof HISTORY_CHOICES)[number];

// #1211: QR・専用リンクの移行。移行元は招待を出して QR とリンクを表示し、移行先はリンクを貼り付けて接続する。
// 両端末の確認の後に鍵と設定を送り、移行先が保存を終えたら両端末を完了にする。移行先は完了の画面で「このアカウントを
// 使う」を押したら、受け取ったアカウントを `onCompleted` へ渡す（切替は呼び出し側。AC-5: 切替は画面を読み込み直すので、
// 完了の画面の説明を読めるように、すぐには切り替えない）。閉じたら移行を取り消す。リンクは log・URL の query へ出さない。
// AC-3: 移行先は接続の前に投稿の履歴の範囲を選ぶ（既定は移さない）。履歴は必須の移行の後に受け、受けている間は
// 完了にしない（切替は履歴が終わってから）。止めても必須の移行は完了のまま。移行先は履歴を受けている間を
// `onReceivingHistory` で知らせる（呼び出し側はその間は閉じさせない。終えるのは「やめる」だけ）。
// #1628: Web 版の移行先は、移行元の QR をカメラで読んで入力欄へ入れる（desktop は貼り付けと OS のリンク起動だけ）。
export function AccountTransferPanel({ role, initialLink = '', onCompleted, onReceivingHistory }: {
  role: 'source' | 'target';
  initialLink?: string;
  onCompleted?: (accountId: string) => void;
  onReceivingHistory?: (receiving: boolean) => void;
}) {
  const { t } = useTranslation('settings');
  const [invite, setInvite] = useState<AccountTransferLink | null>(null);
  const [link, setLink] = useState(initialLink);
  const [history, setHistory] = useState<HistoryChoice>('none');
  const [status, setStatus] = useState<AccountTransferStatus>(IDLE);
  const [error, setError] = useState<'prepareFailed' | 'invalid' | null>(null);
  const [pending, setPending] = useState(false);
  const [copied, setCopied] = useState(false);
  const [now, setNow] = useState(() => Date.now());
  const [scan, setScan] = useState<'off' | 'on' | 'failed'>('off');

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
      await openAccountTransfer(link.trim(), history === 'none' ? null : history as AccountTransferHistory);
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

  const active = ['waiting', 'connecting', 'confirming', 'transferring', 'history'].includes(status.state);
  useEffect(() => {
    if (!active) return;
    const timer = window.setInterval(() => {
      setNow(Date.now());
      void getAccountTransferStatus().then(setStatus).catch(() => undefined);
    }, POLL_MS);
    return () => window.clearInterval(timer);
  }, [active]);

  const receivingHistory = role === 'target' && status.state === 'history';
  // 描いた後の paint・操作より前に知らせる（呼び出し側が閉じる操作を隠すのを、履歴の画面と同じ時点にそろえる）。
  useLayoutEffect(() => {
    onReceivingHistory?.(receivingHistory);
    return () => onReceivingHistory?.(false);
    // 履歴を受け始めた・終えた・閉じたときだけ知らせる。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [receivingHistory]);

  const decide = async (accept: boolean) => {
    setPending(true);
    try { await decideAccountTransfer(accept); setStatus(await getAccountTransferStatus()); }
    catch { setStatus(await getAccountTransferStatus().catch(() => status)); }
    finally { setPending(false); }
  };

  // 履歴だけを止める（必須の移行は完了のまま。backend が完了の状態へ移す）。
  const stopHistory = async () => {
    setPending(true);
    try { await cancelAccountTransfer(); setStatus(await getAccountTransferStatus()); }
    catch { /* 次の状態の読み出しで確かめる */ }
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
  if (status.state === 'transferring') {
    return <p role='status' data-testid='account-transfer-transferring'>{t(`accountTransfer.transferring.${role}`, { count: status.items })}</p>;
  }
  if (status.state === 'history') {
    return <section className='space-y-3' data-testid='account-transfer-history'>
      <p className='text-sm'>{t('accountTransfer.history.bundleDone')}</p>
      <p role='status'>{t(`accountTransfer.history.progress.${role}`, { count: status.posts })}</p>
      <Button variant='secondary' disabled={pending} onClick={() => void stopHistory()}>{t(`accountTransfer.history.stop.${role}`)}</Button>
    </section>;
  }
  if (status.state === 'completed') {
    const result = status.history;
    const received = role === 'target' ? status.account_id : null;
    return <section className='space-y-3' data-testid='account-transfer-completed'>
      <Notice tone='accent'>{t(`accountTransfer.completed.${role}`)}</Notice>
      {result ? <div className='space-y-1 text-sm' data-testid='account-transfer-history-result'>
        <p>{t(`accountTransfer.history.${result.stopped ? 'stopped' : 'done'}.${role}`, { count: result.posts })}</p>
        {result.unavailable > 0 ? <p>{t('accountTransfer.history.unavailable', { count: result.unavailable })}</p> : null}
        {result.stopped ? <p className='text-muted-foreground'>{t('accountTransfer.history.resume')}</p> : null}
      </div> : null}
      {role === 'target' ? <TransferScope completed /> : null}
      {received ? <div className='flex justify-end'>
        <Button onClick={() => onCompleted?.(received)}>{t('accountTransfer.completed.use')}</Button>
      </div> : null}
    </section>;
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
      <TransferScope />
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
    {isTauriRuntime() ? null : scan === 'on'
      ? <div className='space-y-2'>
        <TransferQrScanner onRead={(read) => { setLink(read); setError(null); setScan('off'); }} onFailed={() => setScan('failed')} />
        <Button variant='secondary' onClick={() => setScan('off')}>{t('accountTransfer.target.stopScan')}</Button>
      </div>
      : <div className='space-y-2'>
        {scan === 'failed' ? <Notice tone='destructive'>{t('accountTransfer.target.cameraUnavailable')}</Notice> : null}
        <Button variant='secondary' onClick={() => setScan('on')}>{t('accountTransfer.target.scan')}</Button>
      </div>}
    <Field label={t('accountTransfer.history.label')} hint={t('accountTransfer.history.hint')}>
      <Select value={history} onChange={(event) => setHistory(event.target.value as HistoryChoice)}>
        {HISTORY_CHOICES.map((choice) => <option key={choice} value={choice}>{t(`accountTransfer.history.choice.${choice}`)}</option>)}
      </Select>
    </Field>
    <TransferScope />
    <Button disabled={pending || !link.trim()} onClick={() => void connect()}>
      {t(pending ? 'accountTransfer.target.connecting' : 'accountTransfer.target.connect')}
    </Button>
  </section>;
}

// #1211 AC-5: 接続の前に移るもの・移らないものを示し、移行先の完了の後に移っていないものを示す。
function TransferScope({ completed = false }: { completed?: boolean }) {
  const { t } = useTranslation('settings');
  return <div className='space-y-1 text-sm' data-testid='account-transfer-scope'>
    {completed ? null : <p><span className='font-semibold'>{t('accountTransfer.scope.movesLabel')}</span> {t('accountTransfer.scope.moves')}</p>}
    <p>
      <span className='font-semibold'>{t(completed ? 'accountTransfer.scope.notMovedDoneLabel' : 'accountTransfer.scope.notMovedLabel')}</span>{' '}
      {t('accountTransfer.scope.notMoved')}
    </p>
    {completed ? null : <p className='text-muted-foreground'>{t('accountTransfer.scope.sourceKept')}</p>}
  </div>;
}

// #1628: カメラ（背面を優先）の映像を一定間隔で読み、移行用のリンクの QR を認識したら渡す。映像の frame は
// メモリの中の canvas でだけ読み、保存・送信しない。読み取り・やめる・閉じる（unmount）でカメラを止める。
// decoder は読み取りを開いたときだけ読み込む。
function TransferQrScanner({ onRead, onFailed }: { onRead: (link: string) => void; onFailed: () => void }) {
  const { t } = useTranslation('settings');
  const videoRef = useRef<HTMLVideoElement>(null);
  useEffect(() => {
    let stopped = false;
    let stream: MediaStream | undefined;
    let timer = 0;
    const stop = () => {
      stopped = true;
      window.clearTimeout(timer);
      stream?.getTracks().forEach((track) => track.stop());
    };
    void (async () => {
      const { default: jsQR } = await import('jsqr');
      if (stopped) return;
      const media = await navigator.mediaDevices.getUserMedia({ video: { facingMode: 'environment' }, audio: false });
      stream = media;
      const video = videoRef.current;
      if (stopped || !video) { stop(); return; }
      video.srcObject = media;
      const canvas = document.createElement('canvas');
      const context = canvas.getContext('2d', { willReadFrequently: true });
      const read = () => {
        if (stopped) return;
        const { videoWidth: width, videoHeight: height } = video;
        if (context && width > 0) {
          canvas.width = width;
          canvas.height = height;
          context.drawImage(video, 0, 0, width, height);
          const code = jsQR(context.getImageData(0, 0, width, height).data, width, height, { inversionAttempts: 'dontInvert' });
          if (code?.data.startsWith(TRANSFER_LINK_PREFIX)) { stop(); onRead(code.data); return; }
        }
        timer = window.setTimeout(read, SCAN_MS);
      };
      read();
    })().catch(() => { if (!stopped) { stop(); onFailed(); } });
    return stop;
    // 開いている間に 1 回だけカメラを起動する（渡す先は呼び出し側の state の setter だけ）。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  return <div className='space-y-2'>
    <p role='status' className='text-sm'>{t('accountTransfer.target.scanning')}</p>
    <video ref={videoRef} autoPlay muted playsInline aria-hidden='true'
      className='aspect-square w-full max-w-xs rounded-[var(--radius-input)] bg-[var(--surface-panel-muted)] object-cover' />
  </div>;
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
