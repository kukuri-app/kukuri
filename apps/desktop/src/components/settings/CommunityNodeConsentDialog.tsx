import { useId, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { ChevronRight } from 'lucide-react';

import { MarkdownDocument } from '@/components/MarkdownDocument';

import { Badge } from '@/components/ui/badge';
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

import { type CommunityNodeConsentPolicyView, type CommunityNodeConsentView } from './types';

type CommunityNodeConsentDialogProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  baseUrl: string;
  consent: CommunityNodeConsentView;
  busy: boolean;
  error?: string | null;
  onCloseAutoFocus?: (event: Event) => void;
  onAccept: () => void;
  // #857: 取得失敗（オフライン等）時の再試行。
  onRetry: () => void;
  // #857: 同意の撤回。同意済みのときだけ渡す。
  onWithdraw?: () => void;
};

// #857: Node 同意モーダル。提示内容は認証不要の公開 policy カタログから組み立て、
// 認証(JWT 発行)は同意成立後にのみ始まる。不同意(閉じる)でも非 Node 機能は使える。
export function CommunityNodeConsentDialog({
  open,
  onOpenChange,
  baseUrl,
  consent,
  busy,
  error,
  onCloseAutoFocus,
  onAccept,
  onRetry,
  onWithdraw,
}: CommunityNodeConsentDialogProps) {
  const { t } = useTranslation(['common', 'settings']);
  const titleRef = useRef<HTMLHeadingElement>(null);

  const acceptDisabled = busy || !consent.loaded || consent.policies.length === 0 || consent.allRequiredAccepted;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        className='max-h-[88vh] w-[min(40rem,92vw)] overflow-hidden'
        hideClose={busy}
        onCloseAutoFocus={onCloseAutoFocus}
        onOpenAutoFocus={(event) => {
          event.preventDefault();
          titleRef.current?.focus();
        }}
      >
        <DialogHeader>
          <DialogTitle ref={titleRef} tabIndex={-1}>{t('settings:communityNode.consent.title')}</DialogTitle>
          <p className='break-all font-mono text-xs text-[var(--muted-foreground)]'>{baseUrl}</p>
        </DialogHeader>

        <DialogBody className='max-h-[60vh] space-y-4 overflow-y-auto'>
          {error ? <Notice tone='destructive' role='alert'>
            <p>{error}</p>
            <Button variant='secondary' disabled={busy} onClick={onRetry}>{t('common:actions.retry')}</Button>
          </Notice> : null}
          <DialogDescription className='text-sm leading-6 text-[var(--muted-foreground)]'>
            {t('settings:communityNode.consent.intro')}
          </DialogDescription>

          {consent.loading ? (
            <Notice aria-live='polite'>{t('settings:communityNode.consent.loading')}</Notice>
          ) : null}

          {consent.loadError ? (
            <Notice tone='destructive'>
              <div className='flex flex-wrap items-center justify-between gap-3'>
                <span>{t('settings:communityNode.consent.loadFailed')}</span>
                <Button variant='secondary' type='button' disabled={busy} onClick={onRetry}>
                  {t('common:actions.retry')}
                </Button>
              </div>
              <small className='font-mono'>{consent.loadError}</small>
            </Notice>
          ) : null}

          {consent.withdrawn ? (
            <Notice tone='warning'>{t('settings:communityNode.consent.withdrawnNotice')}</Notice>
          ) : null}

          {consent.loaded && consent.hasPendingUpdate ? (
            <Notice tone='warning'>{t('settings:communityNode.consent.updatedNotice')}</Notice>
          ) : null}

          {consent.loaded && consent.policies.length === 0 ? (
            <Notice>{t('settings:communityNode.consent.noPolicies')}</Notice>
          ) : null}

          {consent.loaded && consent.policies.length > 0 ? (
            <ul className='space-y-3'>
              {consent.policies.map((policy) => (
                <ConsentPolicyItem key={`${baseUrl}:${policy.policySlug}`} policy={policy} />
              ))}
            </ul>
          ) : null}
        </DialogBody>

        <DialogFooter className='flex flex-wrap justify-end gap-2'>
          {onWithdraw && consent.hasLocalConsent ? (
            <Button variant='secondary' disabled={busy} onClick={onWithdraw}>
              {t('settings:communityNode.consent.withdraw')}
            </Button>
          ) : null}
          <Button variant='secondary' disabled={busy} onClick={() => onOpenChange(false)}>
            {consent.allRequiredAccepted
              ? t('common:actions.close')
              : t('settings:communityNode.consent.decline')}
          </Button>
          <Button disabled={acceptDisabled} onClick={onAccept}>
            {consent.allRequiredAccepted
              ? t('settings:communityNode.consent.allAccepted')
              : t('common:actions.accept')}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

// #1106: 文書ごとの折りたたみ。見出し行だけで必須・更新・同意状況が分かり、本文は展開時に描画する。
// 折りたたみは表示だけの状態で、同意対象(一覧のすべての文書)は変えない。
function ConsentPolicyItem({ policy }: { policy: CommunityNodeConsentPolicyView }) {
  const { t } = useTranslation('settings');
  const [expanded, setExpanded] = useState(false);
  const id = useId();
  const buttonId = `${id}-toggle`;
  const panelId = `${id}-panel`;
  const summaryId = `${id}-summary`;
  const previous = policy.previouslyAcceptedVersion;
  const updateDetail = policy.updated && previous != null
    ? previous < policy.policyVersion
      ? t('communityNode.consent.updatedDetail', { previous, current: policy.policyVersion })
      : t('communityNode.consent.updatedContentDetail')
    : null;
  const metadata = [
    policy.effectiveDate ? t('communityNode.consent.effectiveDate', { date: policy.effectiveDate }) : null,
    policy.language ? t('communityNode.consent.language', { language: policy.language }) : null,
  ].filter(Boolean).join(' / ');

  return (
    <li className='rounded-[16px] border border-[var(--border-subtle)] bg-[var(--surface-panel-soft)]'>
      <h3 className='text-sm font-semibold text-foreground'>
        <button
          id={buttonId}
          type='button'
          aria-expanded={expanded}
          aria-controls={expanded ? panelId : undefined}
          aria-describedby={summaryId}
          className='flex min-h-11 w-full items-start gap-2 rounded-[16px] px-4 pb-1 pt-3 text-left'
          onClick={() => setExpanded((value) => !value)}
        >
          <ChevronRight
            aria-hidden='true'
            className={`mt-0.5 size-4 shrink-0 text-[var(--muted-foreground)] motion-safe:transition-transform ${expanded ? 'rotate-90' : ''}`}
          />
          <span className='min-w-0 break-words [overflow-wrap:anywhere]'>{policy.title}</span>
        </button>
      </h3>
      <div id={summaryId} className='flex flex-wrap items-center gap-x-2 gap-y-1 pb-3 pl-10 pr-4 text-xs text-[var(--muted-foreground)]'>
        {/* #1192: 一覧の文書はまとめて同意するため、必須 / 任意のバッジは出さない。
            提示する文書の選別は communityNodeConsentView が持つ。 */}
        {policy.updated ? <Badge tone='warning'>{t('communityNode.consent.updatedBadge')}</Badge> : null}
        <span className='font-semibold uppercase tracking-[0.08em]'>v{policy.policyVersion}</span>
        <span>
          {policy.acceptedAtLabel
            ? t('communityNode.consent.acceptedAt', { timestamp: policy.acceptedAtLabel })
            : t('communityNode.consent.notAccepted')}
        </span>
        {updateDetail ? <span className='basis-full'>{updateDetail}</span> : null}
      </div>
      {expanded ? (
        <div
          id={panelId}
          role='region'
          aria-labelledby={buttonId}
          className='space-y-3 border-t border-[var(--border-subtle)] px-4 py-3'
        >
          {metadata || policy.fallback || policy.referenceTranslation ? (
            <div className='space-y-1 text-xs text-[var(--muted-foreground)]'>
              {metadata ? <p>{metadata}</p> : null}
              {policy.fallback ? (
                <p>
                  {t('communityNode.consent.authoritativeFallback', {
                    language: policy.authoritativeLanguage ?? policy.language ?? 'unknown',
                  })}
                </p>
              ) : policy.referenceTranslation ? (
                <p>{t('communityNode.consent.referenceTranslation')}</p>
              ) : null}
            </div>
          ) : null}
          {policy.body.trim() ? (
            <MarkdownDocument source={policy.body} headingLevel={4} />
          ) : (
            <p className='text-sm italic text-[var(--muted-foreground)]'>
              {t('communityNode.consent.noBody')}
            </p>
          )}
        </div>
      ) : null}
    </li>
  );
}
