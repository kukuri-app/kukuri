import { type ReactNode, useEffect, useId, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { AlertTriangle, ChevronRight, ExternalLink, ShieldAlert } from 'lucide-react';

import {
  REPORT_REASONS,
  type ReportReason,
  type ReportRoutingCandidate,
  type ReportRoutingPlan,
  isCriticalSafetyReason,
} from '@/lib/api/reportRouting';
import {
  type CommunityNodePoliciesResponse,
  type SubmitCommunityNodeReportResult,
} from '@/lib/api';
import { POLICY_KIND_RIGHTS_INFRINGEMENT } from '@/lib/api/policyKind';
import { InvokeError } from '@/lib/api/invoke/error';
import { useExternalLinkOpener } from '@/lib/useExternalLinkOpener';
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

/// 通報対象の種別。provenance を持ち得る surface（post / profile / media 等）に対応する。
export type ReportSubjectKind = 'post' | 'profile' | 'media' | 'search_result' | 'recommendation';

export type ReportRoutingSubject = {
  kind: ReportSubjectKind;
  id: string;
  /// 表示用の短いラベル（author / 抜粋など）。送信内容には含めない。
  label?: string;
};

export type ReportSubmitInput = {
  candidate: ReportRoutingCandidate;
  reason: ReportReason;
  details: string;
  reporterContact: string;
  appeal: { risk_signal_id: string } | null;
};

export type ReportAppealContext = {
  riskSignalId: string;
  issuerNodeId: string;
};

export type ReportRoutingDialogProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  subject: ReportRoutingSubject;
  plan: ReportRoutingPlan;
  /// report endpoint を持つ候補への送信。contact のみの候補は onCopyContact で案内する。
  onSubmit: (input: ReportSubmitInput) => Promise<SubmitCommunityNodeReportResult>;
  /// abuse contact（mailto / copyable）の案内。
  onCopyContact?: (value: string) => void;
  /// provenance 不明 / 通報先未解決時に出す local action（block / mute / local hide）導線。
  localActions?: ReactNode;
  resolving?: boolean;
  resolveError?: string | null;
  /// リスク判定への異議申し立てとして表示する場合の対象。
  appeal?: ReportAppealContext | null;
  /// 受付後の再取得など、成功応答を確認してから行う処理。
  onSubmitted?: (result: SubmitCommunityNodeReportResult) => void | Promise<void>;
  /// 閉じた後の focus 移動。別の dialog から開いた場合に、呼出元が元の操作へ戻す(#1108)。
  onCloseAutoFocus?: (event: Event) => void;
  /// #1192: 権利侵害を選んだときに、選択ノードの権利侵害申出ポリシーを提示するための取得。
  /// 読み取りのみで、同意記録は作らない。
  onFetchNodePolicies?: (
    baseUrl: string,
    language?: string
  ) => Promise<CommunityNodePoliciesResponse>;
};

function nodeHost(url: string): string {
  try {
    return new URL(url).host;
  } catch {
    return url.replace(/^https?:\/\//, '');
  }
}

function candidateKey(candidate: ReportRoutingCandidate): string {
  return `${candidate.target.nodeBaseUrl} ${candidate.target.capability}`;
}

export function ReportRoutingDialog({
  open,
  onOpenChange,
  subject,
  plan,
  onSubmit,
  onCopyContact,
  localActions,
  resolving = false,
  resolveError,
  appeal = null,
  onSubmitted,
  onCloseAutoFocus,
  onFetchNodePolicies,
}: ReportRoutingDialogProps) {
  const { t } = useTranslation(['shell', 'common', 'profile']);
  const externalLink = useExternalLinkOpener();
  const { candidates } = plan;
  const appealRiskSignalId = appeal?.riskSignalId ?? null;
  const isAppeal = appealRiskSignalId !== null;

  const [selectedKey, setSelectedKey] = useState<string | null>(null);
  const [reason, setReason] = useState<ReportReason>('spam');
  const [details, setDetails] = useState('');
  const [reporterContact, setReporterContact] = useState('');
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<SubmitCommunityNodeReportResult | null>(null);

  // ダイアログを開くたびに入力状態を初期化する。候補は取得完了後に届くため、
  // 候補の変化では入力を消さず、選択候補だけを追随させる(#696)。
  useEffect(() => {
    if (open) {
      setReason(isAppeal ? 'other' : 'spam');
      setDetails('');
      setReporterContact('');
      setSubmitting(false);
      setError(null);
      setResult(null);
    }
  }, [open, appealRiskSignalId, isAppeal]);

  useEffect(() => {
    if (!open) return;
    setSelectedKey((current) =>
      current && candidates.some((candidate) => candidateKey(candidate) === current)
        ? current
        : candidates.length > 0
          ? candidateKey(candidates[0])
          : null,
    );
  }, [open, candidates]);

  const selectedCandidate = useMemo(
    () => candidates.find((candidate) => candidateKey(candidate) === selectedKey) ?? null,
    [candidates, selectedKey],
  );

  const isCriticalSafety = isCriticalSafetyReason(reason);
  const isRightsInfringement = !isAppeal && reason === 'rights_infringement';
  const rightsRequestUrl = selectedCandidate?.target.rightsRequestUrl;
  // #1192: 権利侵害申出ポリシーは同意一覧ではなくここで提示する。申出画面へ進む前に
  // ノードの対応範囲を読めるようにするだけで、同意操作も申出送信もここでは行わない。
  const rightsPolicy = useRightsInfringementPolicy({
    open: open && isRightsInfringement,
    baseUrl: selectedCandidate?.target.nodeBaseUrl ?? null,
    fetchPolicies: onFetchNodePolicies,
  });

  const handleSubmit = async () => {
    // 最新の manifest を取得し終えるまで、古い候補への送信も連絡先の複写もしない(#696)。
    if (!selectedCandidate || resolving) {
      return;
    }
    const contact = selectedCandidate.contact;
    // endpoint が無い候補は POST せず、abuse contact を案内する（#310 初期実装方針）。
    if (contact.kind === 'contact') {
      onCopyContact?.(contact.value);
      return;
    }
    if (contact.kind !== 'endpoint') {
      return;
    }
    setSubmitting(true);
    setError(null);
    try {
      const submitted = await onSubmit({
        candidate: selectedCandidate,
        reason: appeal ? 'other' : reason,
        details,
        reporterContact: appeal ? '' : reporterContact,
        appeal: appeal ? { risk_signal_id: appeal.riskSignalId } : null,
      });
      if (appeal && submitted.disputed_risk_signal_id !== appeal.riskSignalId) {
        throw new Error(t('profile:communityNodeAdvisory.appeal.responseMismatch'));
      }
      await onSubmitted?.(submitted);
      setResult(submitted);
    } catch (cause) {
      setError(
        appeal && cause instanceof InvokeError && cause.code === 'INVALID_APPEAL'
          ? t('profile:communityNodeAdvisory.appeal.invalidAppeal')
          : cause instanceof Error
            ? cause.message
            : String(cause),
      );
    } finally {
      setSubmitting(false);
    }
  };

  const canRoute = candidates.length > 0;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        className='report-routing-dialog'
        onCloseAutoFocus={onCloseAutoFocus}
        closeLabel={appeal ? t('profile:communityNodeAdvisory.appeal.close') : undefined}
      >
        <DialogHeader>
          <DialogTitle>
            {appeal ? t('profile:communityNodeAdvisory.appeal.title') : t('report.title')}
          </DialogTitle>
          <DialogDescription>
            {appeal
              ? t('profile:communityNodeAdvisory.appeal.description')
              : t('report.boundaryNotice')}
          </DialogDescription>
        </DialogHeader>
        <DialogBody className='report-routing-body'>
          {externalLink.pending ? <Notice role='status'>{t('common:externalLink.opening')}</Notice> : null}
          {externalLink.failed ? <Notice tone='destructive' role='alert'>{t('common:externalLink.failed')}</Notice> : null}
          <p className='report-subject'>
            {appeal
              ? t(`profile:communityNodeAdvisory.appeal.subject.${subject.kind}`)
              : t(`report.subject.${subject.kind}`)}
            {subject.label ? ` · ${subject.label}` : ''}
          </p>

          {/* 通報先が kukuri 全体ではないことを常に明示する。 */}
          <Notice tone='accent' className='report-boundary-notice'>
            <p>
              {appeal
                ? t('profile:communityNodeAdvisory.appeal.boundary', {
                    issuer: appeal.issuerNodeId,
                  })
                : t('report.boundaryDetail')}
            </p>
            <p className='report-identity-boundary'>
              {appeal
                ? t('profile:communityNodeAdvisory.appeal.anonymous')
                : t('report.identityBoundaryNote')}
            </p>
          </Notice>

          {resolveError ? (
            <Notice tone='destructive'>{t('report.resolveFailed')}</Notice>
          ) : null}

          {resolving ? (
            <Notice aria-live='polite'>{t('report.resolvingTargets')}</Notice>
          ) : result ? (
            <Notice tone='accent' className='report-result'>
              <p>
                {appeal ? t('profile:communityNodeAdvisory.appeal.success') : t('report.success')}
              </p>
              {result.reference_id ? (
                <p className='report-result-reference'>
                  {appeal
                    ? t('profile:communityNodeAdvisory.appeal.reportReference', {
                        id: result.reference_id,
                      })
                    : t('report.referenceId', { id: result.reference_id })}
                </p>
              ) : null}
              {appeal && result.disputed_risk_signal_id ? (
                <p className='report-result-reference'>
                  {t('profile:communityNodeAdvisory.appeal.disputedId', {
                    id: result.disputed_risk_signal_id,
                  })}
                </p>
              ) : null}
            </Notice>
          ) : canRoute ? (
            <>
              <fieldset className='report-target-list'>
                <legend>
                  {appeal
                    ? t('profile:communityNodeAdvisory.appeal.targetsHeading')
                    : t('report.targetsHeading')}
                </legend>
                {candidates.map((candidate) => {
                  const key = candidateKey(candidate);
                  const { target, contact } = candidate;
                  return (
                    <label key={key} className='report-target-option'>
                      <input
                        type='radio'
                        name='report-target'
                        value={key}
                        checked={selectedKey === key}
                        onChange={() => setSelectedKey(key)}
                      />
                      <span className='report-target-main'>
                        <span className='report-target-host'>{nodeHost(target.nodeBaseUrl)}</span>
                        <span className='report-target-capability'>
                          {appeal
                            ? t('profile:communityNodeAdvisory.appeal.capability')
                            : t(`report.capability.${target.capability}`)}
                        </span>
                        <span className='report-target-contact'>
                          {contact.kind === 'endpoint'
                            ? appeal
                              ? t('profile:communityNodeAdvisory.appeal.endpoint')
                              : t('report.contact.endpoint')
                            : contact.kind === 'contact'
                              ? t('report.contact.mailto', { contact: contact.value })
                              : null}
                        </span>
                        {target.policyUrl ? (
                          <a
                            className='report-target-policy'
                          href={target.policyUrl}
                          {...externalLink.linkProps}
                            target='_blank'
                            rel='noreferrer'
                          >
                            <ExternalLink className='size-3.5' aria-hidden='true' />
                            {appeal
                              ? t('profile:communityNodeAdvisory.appeal.openPolicy')
                              : t('report.openPolicy')}
                          </a>
                        ) : null}
                      </span>
                    </label>
                  );
                })}
              </fieldset>

              {!appeal ? (
                <label className='report-field'>
                  <span>{t('report.reasonLabel')}</span>
                  <select
                    className='report-reason-select'
                    value={reason}
                    onChange={(event) => setReason(event.target.value as ReportReason)}
                  >
                    {REPORT_REASONS.map((value) => (
                      <option key={value} value={value}>
                        {t(`report.reasons.${value}`)}
                      </option>
                    ))}
                  </select>
                </label>
              ) : null}

              {!appeal && isCriticalSafety ? (
                <Notice tone='warning' className='report-critical-safety'>
                  <ShieldAlert className='size-4' aria-hidden='true' />
                  <span>{t('report.criticalSafetyNote')}</span>
                </Notice>
              ) : null}

              {isRightsInfringement ? (
                <>
                  <Notice tone='warning' className='report-rights-request'>
                    <div>
                      <p>{t('report.rightsRequest.boundary')}</p>
                      {rightsRequestUrl ? (
                        <p>{t('report.rightsRequest.openDedicated')}</p>
                      ) : (
                        <p>{t('report.rightsRequest.unavailable')}</p>
                      )}
                    </div>
                  </Notice>
                  <RightsInfringementPolicy state={rightsPolicy} />
                </>
              ) : null}

              {!isRightsInfringement ? <label className='report-field'>
                <span>
                  {appeal
                    ? t('profile:communityNodeAdvisory.appeal.detailsLabel')
                    : t('report.detailsLabel')}
                </span>
                <textarea
                  className='report-details-input'
                  rows={3}
                  value={details}
                  placeholder={
                    appeal
                      ? t('profile:communityNodeAdvisory.appeal.detailsPlaceholder')
                      : t('report.detailsPlaceholder')
                  }
                  onChange={(event) => setDetails(event.target.value)}
                />
              </label> : null}

              {!appeal && !isRightsInfringement ? (
                <label className='report-field'>
                  <span>{t('report.reporterContactLabel')}</span>
                  <input
                    className='report-reporter-contact-input'
                    type='text'
                    value={reporterContact}
                    placeholder={t('report.reporterContactPlaceholder')}
                    onChange={(event) => setReporterContact(event.target.value)}
                  />
                  <small className='report-field-hint'>{t('report.reporterContactHint')}</small>
                </label>
              ) : null}

              {error ? (
                <Notice tone='destructive' className='report-error'>
                  {error}
                </Notice>
              ) : null}
            </>
          ) : (
            // provenance 不明 / 通報先未解決：default node へ向けず local action のみ案内する。
            <Notice tone='warning' className='report-unresolved'>
              <AlertTriangle className='size-4' aria-hidden='true' />
              <div className='report-unresolved-body'>
                <p className='report-unresolved-title'>
                  {appeal
                    ? t('profile:communityNodeAdvisory.appeal.unresolvedTitle')
                    : plan.provenanceUnknown
                      ? t('report.unknownTitle')
                      : t('report.observedUnresolvedTitle')}
                </p>
                <p>
                  {appeal
                    ? t('profile:communityNodeAdvisory.appeal.unresolvedBody')
                    : plan.provenanceUnknown
                      ? t('report.unknownBody')
                      : t('report.observedUnresolvedBody')}
                </p>
                {!appeal ? (
                  <p className='report-local-actions-hint'>{t('report.localActionsHint')}</p>
                ) : null}
                {localActions ? <div className='report-local-actions'>{localActions}</div> : null}
              </div>
            </Notice>
          )}
        </DialogBody>
        <DialogFooter className='report-routing-footer'>
          <Button variant='secondary' type='button' onClick={() => onOpenChange(false)}>
            {appeal
              ? t(
                  result
                    ? 'profile:communityNodeAdvisory.appeal.close'
                    : 'profile:communityNodeAdvisory.appeal.cancel',
                )
              : t(result ? 'common:actions.close' : 'common:actions.cancel')}
          </Button>
          {!result && canRoute && selectedCandidate && isRightsInfringement && rightsRequestUrl ? (
            <Button asChild>
              <a href={rightsRequestUrl} target='_blank' rel='noreferrer' {...externalLink.linkProps}>
                <ExternalLink className='size-4' aria-hidden='true' />
                {t('report.rightsRequest.open')}
              </a>
            </Button>
          ) : !result &&
            canRoute &&
            selectedCandidate &&
            !isRightsInfringement &&
            selectedCandidate.contact.kind !== 'none' ? (
            <Button
              type='button'
              disabled={submitting || resolving}
              onClick={handleSubmit}
              aria-label={
                appeal ? t('profile:communityNodeAdvisory.appeal.submit') : t('report.submit')
              }
            >
              {selectedCandidate.contact.kind === 'endpoint'
                ? submitting
                  ? appeal
                    ? t('profile:communityNodeAdvisory.appeal.submitting')
                    : t('report.submitting')
                  : appeal
                    ? t('profile:communityNodeAdvisory.appeal.submit')
                    : t('report.submit')
                : t('report.copyContact')}
            </Button>
          ) : null}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

type RightsInfringementPolicyState = {
  status: 'absent' | 'loading' | 'ready' | 'failed';
  title: string;
  body: string;
  version: number | null;
  effectiveDate: string | null;
};

const NO_RIGHTS_POLICY: RightsInfringementPolicyState = {
  status: 'absent',
  title: '',
  body: '',
  version: null,
  effectiveDate: null,
};

/// #1192: 選択中のノードが公開する権利侵害申出ポリシーを、通報画面を開いている間だけ取得する。
/// 公開 policy カタログの読み取りだけで、同意記録・申出送信は行わない(ADR 0033)。
function useRightsInfringementPolicy(input: {
  open: boolean;
  baseUrl: string | null;
  fetchPolicies?: (baseUrl: string, language?: string) => Promise<CommunityNodePoliciesResponse>;
}): RightsInfringementPolicyState {
  const { open, baseUrl, fetchPolicies } = input;
  const { i18n } = useTranslation();
  const language = i18n.resolvedLanguage ?? i18n.language;
  const [state, setState] = useState<RightsInfringementPolicyState>(NO_RIGHTS_POLICY);
  // 呼び出し側が毎描画で新しい関数を渡しても再取得を起こさないよう、参照だけを更新する。
  const fetchRef = useRef(fetchPolicies);
  useEffect(() => {
    fetchRef.current = fetchPolicies;
  });

  useEffect(() => {
    const fetchPolicies = fetchRef.current;
    if (!open || !baseUrl || !fetchPolicies) {
      setState(NO_RIGHTS_POLICY);
      return;
    }
    let active = true;
    setState({ ...NO_RIGHTS_POLICY, status: 'loading' });
    fetchPolicies(baseUrl, language)
      .then((catalog) => {
        if (!active) return;
        const policy = catalog.policies.find(
          (candidate) => candidate.policy_kind === POLICY_KIND_RIGHTS_INFRINGEMENT
        );
        setState(
          policy
            ? {
                status: 'ready',
                title: policy.title,
                body: policy.body_markdown,
                version: policy.policy_version,
                effectiveDate: policy.effective_date ?? null,
              }
            : NO_RIGHTS_POLICY
        );
      })
      .catch(() => {
        if (active) setState({ ...NO_RIGHTS_POLICY, status: 'failed' });
      });
    return () => {
      active = false;
    };
  }, [baseUrl, language, open]);

  return state;
}

/// 申出画面へ進む前に読める対応範囲。折りたたみの既定は閉で、本文は展開時だけ描画する。
function RightsInfringementPolicy({ state }: { state: RightsInfringementPolicyState }) {
  const { t } = useTranslation('shell');
  const [expanded, setExpanded] = useState(false);
  const id = useId();
  const panelId = `${id}-panel`;

  if (state.status === 'absent') {
    return null;
  }
  if (state.status === 'loading') {
    return <Notice aria-live='polite'>{t('report.rightsRequest.policyLoading')}</Notice>;
  }
  if (state.status === 'failed') {
    return <Notice tone='warning'>{t('report.rightsRequest.policyUnavailable')}</Notice>;
  }
  return (
    <section className='report-rights-policy'>
      <h4>
        <button
          type='button'
          aria-expanded={expanded}
          aria-controls={expanded ? panelId : undefined}
          onClick={() => setExpanded((value) => !value)}
        >
          <ChevronRight
            aria-hidden='true'
            className={`size-4 shrink-0 motion-safe:transition-transform ${expanded ? 'rotate-90' : ''}`}
          />
          <span>{state.title}</span>
        </button>
      </h4>
      <p className='report-rights-policy-meta'>
        {[
          state.version != null ? `v${state.version}` : null,
          state.effectiveDate
            ? t('report.rightsRequest.policyEffectiveDate', { date: state.effectiveDate })
            : null,
        ]
          .filter(Boolean)
          .join(' / ')}
      </p>
      {expanded ? (
        <div id={panelId} className='report-rights-policy-body'>
          <MarkdownDocument source={state.body} headingLevel={5} />
        </div>
      ) : null}
    </section>
  );
}
