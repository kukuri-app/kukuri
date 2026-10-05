import { useEffect, useEffectEvent, useId, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { MarkdownDocument } from '@/components/MarkdownDocument';
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
import type { CommunityNodeObservationSharingStatus } from '@/lib/api';
import { TRUST_OBSERVATION_SHARING_POLICY_SLUG } from '@/lib/api/observationSharing';

/// #1061: Community Node ごとに、この端末のブロック・ミュートを評価へ提供するかを選ぶ。
///
/// 提供はノードの任意同意文書への同意で成立する（ADR 0026 §8.5）。文書を公開していない
/// ノードでは選択肢を出さない。やめると、そのノードに保存された観測の削除を要求する。
export type CommunityNodeObservationSharingHandlers = {
  /// 設定を開いている間だけ照会し、開くたびに照会し直す（省略時は開いている扱い）。
  settingsOpen?: boolean;
  getObservationSharing: (baseUrl: string) => Promise<CommunityNodeObservationSharingStatus>;
  enableObservationSharing: (request: {
    base_url: string;
    policy_version: number;
    policy_snapshot_revision: string | null;
    language: string;
    include_existing: boolean;
  }) => Promise<CommunityNodeObservationSharingStatus>;
  disableObservationSharing: (baseUrl: string) => Promise<CommunityNodeObservationSharingStatus>;
};

export type CommunityNodeObservationSharingFieldProps = {
  nodeId: string;
  baseUrl: string;
  disabled?: boolean;
  language: string;
} & CommunityNodeObservationSharingHandlers;

export function CommunityNodeObservationSharingField({
  nodeId,
  baseUrl,
  disabled = false,
  language,
  settingsOpen = true,
  getObservationSharing,
  enableObservationSharing,
  disableObservationSharing,
}: CommunityNodeObservationSharingFieldProps) {
  const { t } = useTranslation(['common', 'settings']);
  const descriptionId = useId();
  const titleRef = useRef<HTMLHeadingElement>(null);
  const [status, setStatus] = useState<CommunityNodeObservationSharingStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [dialogOpen, setDialogOpen] = useState(false);
  const [includeExisting, setIncludeExisting] = useState(false);

  // handlers は親が描画し直すたびに作り直されるので、照会の契機にしない（#1546）。
  const load = useEffectEvent(async (url: string) => {
    try {
      setStatus(await getObservationSharing(url));
    } catch {
      setStatus(null);
    }
  });

  useEffect(() => {
    if (settingsOpen && baseUrl.trim()) void load(baseUrl);
  }, [baseUrl, settingsOpen]);

  // 文書を公開していないノードでは提供の選択肢を出さない。
  if (
    !status?.offered ||
    !status.policy ||
    status.policy.policy_slug !== TRUST_OBSERVATION_SHARING_POLICY_SLUG
  ) {
    return null;
  }

  const policy = status.policy;
  const openDialog = () => {
    setIncludeExisting(false);
    setError(null);
    setDialogOpen(true);
  };

  async function run(action: () => Promise<CommunityNodeObservationSharingStatus>) {
    setBusy(true);
    setError(null);
    try {
      setStatus(await action());
      setDialogOpen(false);
    } catch {
      setError(t('settings:communityNode.observationSharing.failed'));
    } finally {
      setBusy(false);
    }
  }

  return (
    <section
      className='mt-4 space-y-3 rounded-[16px] border border-[var(--border-subtle)] p-4'
      data-testid={`community-node-observation-sharing-${nodeId}`}
    >
      <div className='space-y-1'>
        <h5 className='text-sm font-semibold text-foreground'>
          {t('settings:communityNode.observationSharing.title')}
        </h5>
        <p id={descriptionId} className='text-sm text-[var(--muted-foreground)]'>
          {t('settings:communityNode.observationSharing.description')}
        </p>
        <p className='text-sm text-[var(--muted-foreground)]'>
          {status.enabled
            ? t('settings:communityNode.observationSharing.enabled')
            : t('settings:communityNode.observationSharing.disabled')}
        </p>
      </div>
      {status.needs_reconsent ? (
        <Notice tone='warning'>
          {t('settings:communityNode.observationSharing.needsReconsent')}
        </Notice>
      ) : null}
      {status.revocation_pending ? (
        <Notice tone='warning'>
          {t('settings:communityNode.observationSharing.revocationPending')}
        </Notice>
      ) : null}
      {error ? (
        <Notice tone='destructive' role='alert'>
          {error}
        </Notice>
      ) : null}
      <Button
        variant='secondary'
        disabled={disabled || busy || !baseUrl.trim()}
        aria-describedby={descriptionId}
        data-testid={`community-node-observation-sharing-toggle-${nodeId}`}
        onClick={() => {
          if (status.enabled) {
            void run(() => disableObservationSharing(baseUrl));
            return;
          }
          openDialog();
        }}
      >
        {status.enabled
          ? t('settings:communityNode.observationSharing.stop')
          : t('settings:communityNode.observationSharing.start')}
      </Button>

      <Dialog open={dialogOpen} onOpenChange={(open) => !busy && setDialogOpen(open)}>
        <DialogContent
          className='max-h-[88vh] w-[min(40rem,92vw)] overflow-hidden'
          hideClose={busy}
          onOpenAutoFocus={(event) => {
            event.preventDefault();
            titleRef.current?.focus();
          }}
        >
          <DialogHeader>
            <DialogTitle ref={titleRef} tabIndex={-1}>
              {policy.title}
            </DialogTitle>
            <p className='break-all font-mono text-xs text-[var(--muted-foreground)]'>{baseUrl}</p>
          </DialogHeader>
          <DialogBody className='max-h-[60vh] space-y-4 overflow-y-auto'>
            <DialogDescription className='text-sm leading-6 text-[var(--muted-foreground)]'>
              {t('settings:communityNode.observationSharing.dialogIntro')}
            </DialogDescription>
            {policy.body_markdown.trim() ? (
              <MarkdownDocument source={policy.body_markdown} headingLevel={4} />
            ) : (
              <p className='text-sm italic text-[var(--muted-foreground)]'>
                {t('settings:communityNode.consent.noBody')}
              </p>
            )}
            <label className='flex min-w-0 items-start gap-3 text-sm text-foreground'>
              <input
                type='checkbox'
                className='mt-0.5 size-4 shrink-0'
                checked={includeExisting}
                disabled={busy}
                data-testid={`community-node-observation-sharing-existing-${nodeId}`}
                onChange={(event) => setIncludeExisting(event.currentTarget.checked)}
              />
              <span className='min-w-0'>
                {t('settings:communityNode.observationSharing.includeExisting')}
              </span>
            </label>
            {error ? (
              <Notice tone='destructive' role='alert'>
                {error}
              </Notice>
            ) : null}
          </DialogBody>
          <DialogFooter>
            <Button variant='secondary' disabled={busy} onClick={() => setDialogOpen(false)}>
              {t('common:actions.cancel')}
            </Button>
            <Button
              disabled={busy}
              data-testid={`community-node-observation-sharing-accept-${nodeId}`}
              onClick={() =>
                void run(() =>
                  enableObservationSharing({
                    base_url: baseUrl,
                    policy_version: policy.policy_version,
                    policy_snapshot_revision: policy.policy_snapshot_revision ?? null,
                    language,
                    include_existing: includeExisting,
                  })
                )
              }
            >
              {t('settings:communityNode.observationSharing.accept')}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </section>
  );
}
