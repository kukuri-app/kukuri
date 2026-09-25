import { useCallback, useEffect, useId, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import type { DesktopApi } from '@/lib/api';
import { topicDisplayName } from '@/lib/topicId';

import { Button } from '@/components/ui/button';
import {
  Dialog,
  DialogBody,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { Label } from '@/components/ui/label';
import { Notice } from '@/components/ui/notice';
import { Select } from '@/components/ui/select';

import {
  type CommunityIndexingStatusSummary,
  communityIndexingErrorKey,
  communityIndexingTargetId,
  summarizeCommunityIndexingStatus,
} from './communityIndexingStatus';

export type CommunityIndexingTarget =
  | { kind: 'public_topic'; topicId: string }
  | { kind: 'private_channel'; topicId: string; channelId: string; channelLabel: string };

type CommunityIndexingRequestDialogProps = {
  api: DesktopApi;
  target: CommunityIndexingTarget | null;
  eligibleNodeBaseUrls: readonly string[];
  onOpenChange: (open: boolean) => void;
  onOpenCommunityNodeSettings: () => void;
};

/**
 * #975: 選択ノードでの索引状況。取得中 / 確定 / 未確認(理由つき)を区別し、未確認を 0 件や対象外と
 * 断定しない(DESIGN 4.2)。応答は保持せず、node・対象が変わるたびに取り直す。
 */
type IndexStatusState =
  | { kind: 'loading' }
  | { kind: 'known'; summary: CommunityIndexingStatusSummary }
  | { kind: 'unknown'; errorKey: string };

export function CommunityIndexingRequestDialog({
  api,
  target,
  eligibleNodeBaseUrls,
  onOpenChange,
  onOpenCommunityNodeSettings,
}: CommunityIndexingRequestDialogProps) {
  const { t } = useTranslation(['shell', 'common']);
  const statusHeadingId = useId();
  const [selectedNode, setSelectedNode] = useState('');
  const [confirmed, setConfirmed] = useState(false);
  const [pending, setPending] = useState(false);
  const [indexStatus, setIndexStatus] = useState<IndexStatusState | null>(null);
  const [errorKey, setErrorKey] = useState<string | null>(null);
  const requestVersionRef = useRef(0);
  const statusVersionRef = useRef(0);

  const privateTarget = target?.kind === 'private_channel';
  const targetLabel = target
    ? privateTarget
      ? target.channelLabel
      : topicDisplayName(target.topicId)
    : '';

  // 索引状況の読取り(#975)。公開 topic は対象付きで読む。非公開チャンネルは、明示確認なしでは
  // 所属証明(秘密値)を送らないため、自分の申請一覧だけを読む(`withTarget = false`)。
  const loadIndexStatus = useCallback(
    async (baseUrl: string, statusTarget: CommunityIndexingTarget, withTarget: boolean) => {
      const version = statusVersionRef.current + 1;
      statusVersionRef.current = version;
      setIndexStatus({ kind: 'loading' });
      try {
        const response = await api.readCommunityNodeIndexingStatus({
          base_url: baseUrl,
          scope_kind: withTarget ? statusTarget.kind : null,
          topic_id: withTarget ? statusTarget.topicId : null,
          channel_id:
            withTarget && statusTarget.kind === 'private_channel' ? statusTarget.channelId : null,
          confirm_private_channel_secret_disclosure:
            withTarget && statusTarget.kind === 'private_channel',
        });
        if (statusVersionRef.current !== version) return;
        setIndexStatus({
          kind: 'known',
          summary: summarizeCommunityIndexingStatus(response, statusTarget),
        });
      } catch (error) {
        if (statusVersionRef.current !== version) return;
        setIndexStatus({
          kind: 'unknown',
          errorKey: communityIndexingErrorKey(error, 'statusFailed'),
        });
      }
    },
    [api]
  );

  // 適格一覧は定期更新のたびに新しい配列になり得るため、参照ではなく内容の変化で初期化する(#698)。
  const eligibleKey = JSON.stringify(eligibleNodeBaseUrls);
  useEffect(() => {
    requestVersionRef.current += 1;
    statusVersionRef.current += 1;
    const firstNode = (JSON.parse(eligibleKey) as string[])[0] ?? '';
    setSelectedNode(firstNode);
    setConfirmed(false);
    setPending(false);
    setIndexStatus(null);
    setErrorKey(null);
    if (target && firstNode) {
      void loadIndexStatus(firstNode, target, target.kind === 'public_topic');
    }
  }, [eligibleKey, loadIndexStatus, target]);
  const selectedNodeEligible = selectedNode !== '' && eligibleNodeBaseUrls.includes(selectedNode);

  function selectNode(baseUrl: string) {
    requestVersionRef.current += 1;
    statusVersionRef.current += 1;
    setSelectedNode(baseUrl);
    setConfirmed(false);
    setIndexStatus(null);
    setErrorKey(null);
    if (target && eligibleNodeBaseUrls.includes(baseUrl)) {
      void loadIndexStatus(baseUrl, target, target.kind === 'public_topic');
    }
  }

  const summary = indexStatus?.kind === 'known' ? indexStatus.summary : null;
  // 申請済み・索引対象の対象は再申請しても状態が変わらない(server は冪等)ため、送信を塞いで理由を示す。
  const submitBlockedKey = summary?.ownRequest
    ? 'submitBlockedRequested'
    : summary?.supported
      ? 'submitBlockedSupported'
      : null;
  const canCheckPrivateStatus =
    privateTarget && summary !== null && summary.supported === null && !summary.ownRequest;

  function checkPrivateStatus() {
    // 所属証明の送信は明示確認を 1 回だけ消費する(申請と同じ扱い)。
    if (!target || !selectedNodeEligible || !confirmed || pending) return;
    setConfirmed(false);
    void loadIndexStatus(selectedNode, target, true);
  }

  function retryIndexStatus() {
    if (!target || !selectedNodeEligible) return;
    void loadIndexStatus(selectedNode, target, target.kind === 'public_topic');
  }

  async function submit() {
    // 選択ノードが適格一覧から外れた瞬間から申請(非公開チャンネルでは秘密値)を送らない(#698)。
    if (!target || !selectedNodeEligible || (privateTarget && !confirmed) || submitBlockedKey) return;
    const requestVersion = requestVersionRef.current + 1;
    requestVersionRef.current = requestVersion;
    if (privateTarget) setConfirmed(false);
    setPending(true);
    setErrorKey(null);
    try {
      const response = await api.submitCommunityNodeIndexingRequest({
        base_url: selectedNode,
        scope_kind: target.kind,
        topic_id: target.topicId,
        channel_id: privateTarget ? target.channelId : null,
        confirm_private_channel_secret_disclosure: privateTarget && confirmed,
      });
      if (requestVersionRef.current !== requestVersion) return;
      // 申請応答は自分の申請の現在状態そのものなので、再取得せずに表示 state へ反映する。
      statusVersionRef.current += 1;
      setIndexStatus((previous) => ({
        kind: 'known',
        summary: {
          supported: previous?.kind === 'known' ? previous.summary.supported : null,
          ownRequest: {
            request_id: response.request_id,
            scope_kind: target.kind,
            target_id: communityIndexingTargetId(target),
            status: response.status,
            created_at: Date.now(),
            decided_at: null,
          },
        },
      }));
    } catch (error) {
      if (requestVersionRef.current !== requestVersion) return;
      setErrorKey(communityIndexingErrorKey(error, 'requestFailed'));
    } finally {
      if (requestVersionRef.current === requestVersion) setPending(false);
    }
  }

  async function revoke() {
    if (!target || !selectedNodeEligible || target.kind !== 'private_channel' || pending) return;
    const requestVersion = requestVersionRef.current + 1;
    requestVersionRef.current = requestVersion;
    setPending(true);
    setErrorKey(null);
    try {
      await api.revokeCommunityNodeIndexingRequest({
        base_url: selectedNode,
        scope_kind: target.kind,
        topic_id: target.topicId,
        channel_id: target.channelId,
        confirm_private_channel_secret_disclosure: false,
      });
      if (requestVersionRef.current !== requestVersion) return;
      statusVersionRef.current += 1;
      setIndexStatus({ kind: 'known', summary: { ownRequest: null, supported: null } });
    } catch (error) {
      if (requestVersionRef.current !== requestVersion) return;
      setErrorKey(communityIndexingErrorKey(error, 'requestFailed'));
    } finally {
      if (requestVersionRef.current === requestVersion) setPending(false);
    }
  }

  return (
    <Dialog open={target !== null} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{t('shell:indexingRequest.title')}</DialogTitle>
          <DialogDescription>
            {t('shell:indexingRequest.target', { target: targetLabel })}
          </DialogDescription>
        </DialogHeader>
        <DialogBody>
          <div className='extended-module-stack'>
            {eligibleNodeBaseUrls.length === 0 ? (
              <Notice>
                <p>{t('shell:indexingRequest.noEligibleNode')}</p>
                <Button type='button' variant='secondary' onClick={onOpenCommunityNodeSettings}>
                  {t('shell:indexingRequest.openSettings')}
                </Button>
              </Notice>
            ) : (
              <Label>
                <span>{t('shell:indexingRequest.nodeLabel')}</span>
                <Select
                  value={selectedNode}
                  disabled={pending}
                  onChange={(event) => selectNode(event.target.value)}
                >
                  {eligibleNodeBaseUrls.map((baseUrl) => (
                    <option key={baseUrl} value={baseUrl}>{baseUrl}</option>
                  ))}
                </Select>
              </Label>
            )}

            {indexStatus ? (
              <section aria-labelledby={statusHeadingId} className='space-y-2'>
                <p id={statusHeadingId} className='text-sm font-medium'>
                  {t('shell:indexingRequest.indexStatus.heading')}
                </p>
                {indexStatus.kind === 'loading' ? (
                  <p className='muted' role='status'>{t('shell:indexingRequest.indexStatus.loading')}</p>
                ) : null}
                {indexStatus.kind === 'known' ? (
                  <>
                    {indexStatus.summary.ownRequest ? (
                      <Notice
                        tone={indexStatus.summary.ownRequest.status === 'rejected' ? 'destructive' : 'accent'}
                      >
                        {t(`shell:indexingRequest.status.${indexStatus.summary.ownRequest.status}`)}
                      </Notice>
                    ) : null}
                    {indexStatus.summary.supported === true ? (
                      <Notice tone='accent'>{t('shell:indexingRequest.indexStatus.supported')}</Notice>
                    ) : indexStatus.summary.supported === false && !indexStatus.summary.ownRequest ? (
                      <Notice>{t('shell:indexingRequest.indexStatus.notSupported')}</Notice>
                    ) : indexStatus.summary.supported === null && !indexStatus.summary.ownRequest ? (
                      <Notice>{t('shell:indexingRequest.indexStatus.noRequest')}</Notice>
                    ) : null}
                    {canCheckPrivateStatus ? (
                      <p className='muted'>{t('shell:indexingRequest.indexStatus.privateCheckHint')}</p>
                    ) : null}
                    {submitBlockedKey ? (
                      <p className='muted'>{t(`shell:indexingRequest.indexStatus.${submitBlockedKey}`)}</p>
                    ) : null}
                    {privateTarget && indexStatus.summary.ownRequest ? (
                      <div>
                        <p className='muted'>{t('shell:indexingRequest.privateStopHint')}</p>
                        <Button type='button' variant='secondary' disabled={pending || !selectedNodeEligible}
                          onClick={() => void revoke()}>
                          {t('shell:indexingRequest.privateStop')}
                        </Button>
                      </div>
                    ) : null}
                  </>
                ) : null}
                {indexStatus.kind === 'unknown' ? (
                  <Notice tone='warning'>
                    <p>{t('shell:indexingRequest.indexStatus.unavailable')}</p>
                    <p className='mb-0 mt-1'>{t(`shell:indexingRequest.errors.${indexStatus.errorKey}`)}</p>
                    <Button
                      type='button'
                      variant='secondary'
                      className='mt-2'
                      disabled={pending || !selectedNodeEligible}
                      onClick={retryIndexStatus}
                    >
                      {t('shell:indexingRequest.indexStatus.retry')}
                    </Button>
                  </Notice>
                ) : null}
              </section>
            ) : null}

            {privateTarget ? (
              <Notice tone='warning'>
                <label className='flex items-start gap-3'>
                  <input
                    type='checkbox'
                    className='mt-1 size-4 shrink-0'
                    checked={confirmed}
                    disabled={pending}
                    onChange={(event) => setConfirmed(event.target.checked)}
                  />
                  <span className='min-w-0'>{t('shell:indexingRequest.privateConfirmation')}</span>
                </label>
                <p className='mb-0 mt-2'>{t('shell:indexingRequest.privateWarning')}</p>
                {canCheckPrivateStatus ? (
                  <Button
                    type='button'
                    variant='secondary'
                    className='mt-2'
                    disabled={pending || !selectedNodeEligible || !confirmed}
                    onClick={checkPrivateStatus}
                  >
                    {t('shell:indexingRequest.indexStatus.check')}
                  </Button>
                ) : null}
              </Notice>
            ) : (
              <Notice>{t('shell:indexingRequest.publicNotice')}</Notice>
            )}
            <p className='muted'>{t('shell:indexingRequest.gateNotice')}</p>

            {errorKey ? <Notice tone='destructive'>{t(`shell:indexingRequest.errors.${errorKey}`)}</Notice> : null}

            <div className='ui-dialog-footer'>
              <Button type='button' variant='secondary' onClick={() => onOpenChange(false)}>
                {t('common:actions.close')}
              </Button>
              <Button
                type='button'
                disabled={
                  pending ||
                  !selectedNodeEligible ||
                  (privateTarget && !confirmed) ||
                  submitBlockedKey !== null
                }
                onClick={() => void submit()}
              >
                {pending ? t('shell:indexingRequest.submitting') : t('shell:indexingRequest.submit')}
              </Button>
            </div>
          </div>
        </DialogBody>
      </DialogContent>
    </Dialog>
  );
}
