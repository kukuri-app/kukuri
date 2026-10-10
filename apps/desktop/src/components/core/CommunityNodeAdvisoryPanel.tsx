import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { Button } from '@/components/ui/button';
import { Notice } from '@/components/ui/notice';
import type {
  DesktopApi,
  CommunityNodeManifest,
  RelationNeighborsResponse,
  RelationReadResponse,
  TrustBasisEntry,
  TrustUserReadResponse,
} from '@/lib/api';
import { planAppealReportRouting } from '@/lib/api/reportRouting';
import {
  trustRelationErrorMessage,
  trustRelationUnavailableReason,
  type TrustRelationUnavailableReason,
} from '@/lib/api/trustRelationPresentation';

import {
  ReportRoutingDialog,
  type ReportRoutingSubject,
  type ReportSubmitInput,
} from './ReportRoutingDialog';

type ReadState<T> =
  | { status: 'idle' | 'loading'; value: null; reason: null; message: null }
  | { status: 'ready'; value: T; reason: null; message: null }
  | { status: 'unavailable'; value: null; reason: TrustRelationUnavailableReason; message: string };

const IDLE_STATE = { status: 'idle', value: null, reason: null, message: null } as const;
const LOADING_STATE = { status: 'loading', value: null, reason: null, message: null } as const;

type AdvisoryContext = {
  key: string;
  targetPubkey: string;
  baseUrl: string;
  generation: number;
};

type AdvisoryState = {
  context: AdvisoryContext | null;
  trust: ReadState<TrustUserReadResponse>;
  relation: ReadState<RelationReadResponse>;
  neighbors: ReadState<RelationNeighborsResponse>;
  /// basis の続きの読み込み(#1702)。読めた続きは `trust` の basis へ追記する。
  more: ReadState<null>;
};

type AppealSelection = {
  context: AdvisoryContext;
  basis: TrustBasisEntry;
};

const IDLE_ADVISORY_STATE: AdvisoryState = {
  context: null,
  trust: IDLE_STATE,
  relation: IDLE_STATE,
  neighbors: IDLE_STATE,
  more: IDLE_STATE,
};

function advisoryContextKey(targetPubkey: string, baseUrl: string): string {
  return `${targetPubkey}\u0000${baseUrl}`;
}

function resultState<T>(result: PromiseSettledResult<T>): ReadState<T> {
  if (result.status === 'fulfilled') {
    return { status: 'ready', value: result.value, reason: null, message: null };
  }
  return {
    status: 'unavailable',
    value: null,
    reason: trustRelationUnavailableReason(result.reason),
    message: trustRelationErrorMessage(result.reason),
  };
}

/// 根拠から異議申し立ての対象を決める。利用者対象の根拠は、表示中の利用者と一致する場合だけ
/// 対象にする(#699: 別利用者の判定を申し立てない)。投稿・添付対象は #685 / #707 の契約どおり。
function appealSubjectForBasis(
  basis: TrustBasisEntry,
  targetPubkey: string
): ReportRoutingSubject | null {
  switch (basis.target) {
    case 'user_pubkey':
      if (basis.target_id.trim().toLowerCase() !== targetPubkey.trim().toLowerCase()) return null;
      return { kind: 'profile', id: basis.target_id };
    case 'post_id':
      return { kind: 'post', id: basis.target_id };
    case 'blob_cid':
      // 添付由来の判定(#707)。サーバは (blob_cid, media) を受理する。
      return { kind: 'media', id: basis.target_id };
    default:
      return null;
  }
}

function continuous(value: number): string {
  return Number.isFinite(value) ? value.toFixed(3) : String(value);
}

export type CommunityNodeAdvisoryPanelProps = {
  api: Pick<
    DesktopApi,
    | 'readCommunityNodeTrustUser'
    | 'readCommunityNodeRelationUser'
    | 'listCommunityNodeRelationNeighbors'
    | 'submitCommunityNodeReport'
    | 'fetchCommunityNodeManifest'
  >;
  targetPubkey: string;
  /// 適格(認証・必須同意・通信・community_local_trust 提供中)なノードだけを渡す(#705)。
  nodeBaseUrls: string[];
  onOpenCommunityNodeSettings?: () => void;
};

export function CommunityNodeAdvisoryPanel({
  api,
  targetPubkey,
  nodeBaseUrls,
  onOpenCommunityNodeSettings,
}: CommunityNodeAdvisoryPanelProps) {
  const { t } = useTranslation(['profile', 'common']);
  const [baseUrl, setBaseUrl] = useState(nodeBaseUrls[0] ?? '');
  const [advisoryState, setAdvisoryState] = useState<AdvisoryState>(IDLE_ADVISORY_STATE);
  const [appealSelection, setAppealSelection] = useState<AppealSelection | null>(null);
  // 異議申し立てを開いた時に取得した発行元ノードの最新 manifest(#696)。
  const [appealManifest, setAppealManifest] = useState<CommunityNodeManifest | null>(null);
  const [appealResolving, setAppealResolving] = useState(false);
  const [appealResolveError, setAppealResolveError] = useState<string | null>(null);
  const requestGeneration = useRef(0);

  const selectedBaseUrl = nodeBaseUrls.includes(baseUrl) ? baseUrl : (nodeBaseUrls[0] ?? '');
  const currentContextKey = selectedBaseUrl
    ? advisoryContextKey(targetPubkey, selectedBaseUrl)
    : null;
  const currentContextKeyRef = useRef(currentContextKey);
  const visibleAdvisoryState =
    advisoryState.context?.key === currentContextKey ? advisoryState : IDLE_ADVISORY_STATE;
  const activeAppealSelection =
    appealSelection?.context.key === currentContextKey &&
    appealSelection.context.generation === visibleAdvisoryState.context?.generation
      ? appealSelection
      : null;
  const { trust, relation, neighbors, more } = visibleAdvisoryState;
  const basisNextCursor = trust.status === 'ready' ? trust.value.basis_next_cursor : null;
  const appealBasis = activeAppealSelection?.basis ?? null;

  const invalidateAdvisory = useCallback(() => {
    requestGeneration.current += 1;
    setAdvisoryState(IDLE_ADVISORY_STATE);
    setAppealSelection(null);
  }, []);

  // 異議申し立ての受付先は、信頼評価を取得した選択中ノードから開いた時に取得した
  // 最新 manifest だけから決める。取得中・失敗・発行元不一致では候補を作らない(#696)。
  const appealFetchKey = activeAppealSelection
    ? `${activeAppealSelection.context.key}\u0000${activeAppealSelection.basis.signal_id}`
    : null;
  const appealBaseUrl = activeAppealSelection?.context.baseUrl ?? null;
  useEffect(() => {
    setAppealManifest(null);
    setAppealResolveError(null);
    if (!appealFetchKey || !appealBaseUrl) {
      setAppealResolving(false);
      return;
    }
    let active = true;
    setAppealResolving(true);
    api
      .fetchCommunityNodeManifest(appealBaseUrl)
      .then((response) => {
        if (!active) return;
        if (response.status === 'ok' && response.manifest) {
          setAppealManifest(response.manifest);
        } else {
          setAppealResolveError('community node manifest is unavailable');
        }
      })
      .catch((cause) => {
        if (active) setAppealResolveError(cause instanceof Error ? cause.message : String(cause));
      })
      .finally(() => {
        if (active) setAppealResolving(false);
      });
    return () => {
      active = false;
    };
  }, [api, appealBaseUrl, appealFetchKey]);
  const appealPlan = useMemo(
    () =>
      appealBasis && appealBaseUrl
        ? planAppealReportRouting(
            appealBasis.issuer_node_id,
            appealManifest ? { [appealBaseUrl]: appealManifest } : {},
          )
        : null,
    [appealBasis, appealBaseUrl, appealManifest],
  );

  useEffect(() => {
    if (baseUrl !== selectedBaseUrl) setBaseUrl(selectedBaseUrl);
  }, [baseUrl, selectedBaseUrl]);

  useEffect(() => {
    currentContextKeyRef.current = currentContextKey;
    invalidateAdvisory();
  }, [currentContextKey, invalidateAdvisory]);

  async function loadAdvisory({ preserveAppeal = false } = {}) {
    if (!selectedBaseUrl || !currentContextKey) return;
    const generation = ++requestGeneration.current;
    const context: AdvisoryContext = {
      key: currentContextKey,
      targetPubkey,
      baseUrl: selectedBaseUrl,
      generation,
    };
    if (!preserveAppeal) {
      setAppealSelection(null);
      setAdvisoryState({
        context,
        trust: LOADING_STATE,
        relation: LOADING_STATE,
        neighbors: LOADING_STATE,
        more: IDLE_STATE,
      });
    }
    const request = { base_url: context.baseUrl, target_pubkey: context.targetPubkey };
    const [trustResult, relationResult, neighborsResult] = await Promise.allSettled([
      api.readCommunityNodeTrustUser(request),
      api.readCommunityNodeRelationUser(request),
      api.listCommunityNodeRelationNeighbors({ base_url: context.baseUrl, limit: 20 }),
    ]);
    if (
      generation !== requestGeneration.current ||
      context.key !== currentContextKeyRef.current
    ) {
      return;
    }
    if (preserveAppeal) {
      setAppealSelection((selection) =>
        selection?.context.key === context.key ? { ...selection, context } : null
      );
    }
    setAdvisoryState({
      context,
      trust: resultState(trustResult),
      relation: resultState(relationResult),
      neighbors: resultState(neighborsResult),
      more: IDLE_STATE,
    });
  }

  async function loadMoreBasis(cursor: string) {
    const context = visibleAdvisoryState.context;
    if (!context) return;
    setAdvisoryState((state) => (state.context === context ? { ...state, more: LOADING_STATE } : state));
    const [result] = await Promise.allSettled([
      api.readCommunityNodeTrustUser({
        base_url: context.baseUrl,
        target_pubkey: context.targetPubkey,
        cursor,
      }),
    ]);
    // 読込み中に取得し直す・ノードを切り替えた場合は捨てる。信頼度と理由は最初のページのまま、basis だけを追記する。
    setAdvisoryState((state) => {
      if (state.context !== context || state.trust.status !== 'ready') return state;
      const page = resultState(result);
      if (page.status !== 'ready') return { ...state, more: page };
      const { value } = state.trust;
      return {
        ...state,
        trust: {
          ...state.trust,
          value: {
            ...value,
            basis: [...value.basis, ...page.value.basis],
            basis_next_cursor: page.value.basis_next_cursor,
          },
        },
        more: IDLE_STATE,
      };
    });
  }

  function unavailableCopy(state: Extract<ReadState<unknown>, { status: 'unavailable' }>) {
    if (state.reason === 'other') return state.message;
    return t(`profile:communityNodeAdvisory.errors.${state.reason}`);
  }

  async function submitAppeal(input: ReportSubmitInput) {
    const selection = activeAppealSelection;
    const subject = selection ? appealSubjectForBasis(selection.basis, targetPubkey) : null;
    if (
      !selection ||
      selection.context.key !== currentContextKeyRef.current ||
      selection.context.generation !== requestGeneration.current ||
      !input.appeal ||
      input.candidate.contact.kind !== 'endpoint' ||
      !subject
    ) {
      throw new Error(t('profile:communityNodeAdvisory.appeal.unresolvedBody'));
    }
    return api.submitCommunityNodeReport({
      node_base_url: input.candidate.target.nodeBaseUrl,
      report_endpoint: input.candidate.contact.value,
      subject_kind: subject.kind,
      subject_id: subject.id,
      capability: input.candidate.target.capability,
      reason: 'other',
      details: input.details.trim() || null,
      reporter_contact: null,
      appeal: input.appeal,
    });
  }

  return (
    <section className='w-full min-w-0 max-w-full space-y-4 overflow-hidden border-t border-[var(--border-subtle)] pt-4'>
      <div className='space-y-1'>
        <h3 className='text-base font-semibold'>{t('profile:communityNodeAdvisory.title')}</h3>
        <p className='text-sm text-[var(--muted-foreground)]'>
          {t('profile:communityNodeAdvisory.nodeLocalNotice')}
        </p>
      </div>

      {nodeBaseUrls.length === 0 ? (
        <Notice>
          <div className='flex flex-wrap items-center justify-between gap-3'>
            <span>{t('profile:communityNodeAdvisory.noNodes')}</span>
            {onOpenCommunityNodeSettings ? (
              <Button variant='secondary' type='button' onClick={onOpenCommunityNodeSettings}>
                {t('profile:communityNodeAdvisory.openSettings')}
              </Button>
            ) : null}
          </div>
        </Notice>
      ) : (
        <div className='flex min-w-0 flex-wrap items-end gap-3'>
          <label className='min-w-0 flex-1 space-y-1 text-sm font-medium'>
            <span>{t('profile:communityNodeAdvisory.nodeLabel')}</span>
            <select
              className='input min-h-10 w-full min-w-0 max-w-full'
              value={selectedBaseUrl}
              onChange={(event) => {
                invalidateAdvisory();
                setBaseUrl(event.currentTarget.value);
              }}
            >
              {nodeBaseUrls.map((node) => (
                <option key={node} value={node}>
                  {node}
                </option>
              ))}
            </select>
          </label>
          <Button type='button' onClick={() => void loadAdvisory()} disabled={trust.status === 'loading'}>
            {trust.status === 'loading'
              ? t('profile:communityNodeAdvisory.loading')
              : t('profile:communityNodeAdvisory.load')}
          </Button>
        </div>
      )}

      {trust.status === 'ready' ? (
        <section className='w-full min-w-0 max-w-full space-y-3 overflow-hidden rounded-[16px] bg-[var(--surface-panel-soft)] p-4'>
          <div>
            <h4 className='font-semibold'>{t('profile:communityNodeAdvisory.trust.title')}</h4>
            <p className='text-sm text-[var(--muted-foreground)]'>
              {t('profile:communityNodeAdvisory.trust.continuousNotice')}
            </p>
          </div>
          {/* #1061: trust はノードが合算した「あなたから見た信頼度」。内訳から再計算しない。 */}
          <dl className='space-y-1 text-sm'>
            <div>
              <dt>{t('profile:communityNodeAdvisory.trust.trust')}</dt>
              <dd className='text-base font-semibold'>{continuous(trust.value.trust)}</dd>
            </div>
            {trust.value.evaluation && trust.value.evaluation.reasons.length > 0 ? (
              <div>
                <dt>{t('profile:communityNodeAdvisory.trust.reasonsLabel')}</dt>
                <dd>
                  {trust.value.evaluation.reasons
                    .map((reason) => t(`profile:communityNodeAdvisory.trust.reasons.${reason}`))
                    .join(' / ')}
                </dd>
              </div>
            ) : null}
          </dl>
          {trust.value.evaluation?.hide_recommended ? (
            <Notice>{t('profile:communityNodeAdvisory.trust.hideRecommended')}</Notice>
          ) : null}
          <div className='space-y-1'>
            <h5 className='text-sm font-medium'>
              {t('profile:communityNodeAdvisory.trust.breakdownTitle')}
            </h5>
            <p className='text-xs text-[var(--muted-foreground)]'>
              {t('profile:communityNodeAdvisory.trust.breakdownNotice')}
            </p>
            <dl
              className='grid gap-2 text-sm'
              style={{ gridTemplateColumns: 'repeat(3, minmax(0, 1fr))' }}
            >
              <div><dt>{t('profile:communityNodeAdvisory.trust.absolute')}</dt><dd>{continuous(trust.value.absolute)}</dd></div>
              <div><dt>{t('profile:communityNodeAdvisory.trust.relative')}</dt><dd>{continuous(trust.value.relative)}</dd></div>
              <div><dt>{t('profile:communityNodeAdvisory.trust.weight')}</dt><dd>{continuous(trust.value.w_abs_applied)}</dd></div>
            </dl>
          </div>
          {trust.value.basis.length === 0 ? (
            <Notice>{t('profile:communityNodeAdvisory.trust.noBasis')}</Notice>
          ) : (
            <div className='space-y-2'>
              {trust.value.basis.map((basis) => (
                <details key={basis.signal_id} className='rounded-xl border border-[var(--border-subtle)] p-3 text-sm'>
                  <summary className='cursor-pointer font-medium'>
                    {basis.issuer_node_id} · {basis.category} · {basis.severity}
                  </summary>
                  <dl className='mt-3 space-y-1 break-all text-[var(--muted-foreground)]'>
                    <div><dt className='inline font-medium'>{t('profile:communityNodeAdvisory.basisLabels.basis')}: </dt><dd className='inline'>{basis.basis}</dd></div>
                    <div><dt className='inline font-medium'>{t('profile:communityNodeAdvisory.basisLabels.confidence')}: </dt><dd className='inline'>{basis.confidence == null ? '—' : continuous(basis.confidence)}</dd></div>
                    <div><dt className='inline font-medium'>{t('profile:communityNodeAdvisory.basisLabels.visibility')}: </dt><dd className='inline'>{basis.visibility}</dd></div>
                    <div><dt className='inline font-medium'>{t('profile:communityNodeAdvisory.basisLabels.appealStatus')}: </dt><dd className='inline'>{t(`profile:communityNodeAdvisory.appeal.status.${basis.appeal_status}`)}</dd></div>
                    <div><dt className='inline font-medium'>{t('profile:communityNodeAdvisory.basisLabels.target')}: </dt><dd className='inline'>{t(`profile:communityNodeAdvisory.appeal.target.${basis.target}`)} · {basis.target_id}</dd></div>
                    <div><dt className='inline font-medium'>{t('profile:communityNodeAdvisory.basisLabels.expiresAt')}: </dt><dd className='inline'>{basis.expires_at ?? '—'}</dd></div>
                    <div><dt className='inline font-medium'>{t('profile:communityNodeAdvisory.basisLabels.decayFactor')}: </dt><dd className='inline'>{continuous(basis.decay_factor)}</dd></div>
                    <div><dt className='inline font-medium'>{t('profile:communityNodeAdvisory.basisLabels.relationWeight')}: </dt><dd className='inline'>{continuous(basis.relation_weight)}</dd></div>
                    <div><dt className='inline font-medium'>{t('profile:communityNodeAdvisory.basisLabels.contribution')}: </dt><dd className='inline'>{continuous(basis.contribution)}</dd></div>
                  </dl>
                  <div className='mt-3 space-y-2'>
                    {basis.appeal_status === 'none' ? (
                      <>
                        <p className='text-xs text-[var(--muted-foreground)]'>
                          {t('profile:communityNodeAdvisory.appeal.noneContribution')}
                        </p>
                        {appealSubjectForBasis(basis, targetPubkey) ? (
                          <Button
                            type='button'
                            variant='secondary'
                            onClick={() => {
                              if (visibleAdvisoryState.context) {
                                setAppealSelection({
                                  context: visibleAdvisoryState.context,
                                  basis,
                                });
                              }
                            }}
                          >
                            {t('profile:communityNodeAdvisory.appeal.action')}
                          </Button>
                        ) : (
                          <Notice>{t('profile:communityNodeAdvisory.appeal.unsupportedTarget')}</Notice>
                        )}
                      </>
                    ) : basis.appeal_status === 'disputed' ? (
                      <Notice>{t('profile:communityNodeAdvisory.appeal.disputedContribution')}</Notice>
                    ) : (
                      <Notice tone='accent'>
                        {t('profile:communityNodeAdvisory.appeal.clearedContribution')}
                      </Notice>
                    )}
                  </div>
                </details>
              ))}
              {basisNextCursor ? (
                <Button
                  type='button'
                  variant='secondary'
                  disabled={more.status === 'loading'}
                  onClick={() => void loadMoreBasis(basisNextCursor)}
                >
                  {more.status === 'loading'
                    ? t('profile:communityNodeAdvisory.trust.loadingMore')
                    : t('profile:communityNodeAdvisory.trust.showMore')}
                </Button>
              ) : null}
              {more.status === 'unavailable' ? <Notice>{unavailableCopy(more)}</Notice> : null}
            </div>
          )}
        </section>
      ) : trust.status === 'unavailable' ? (
        <Notice>{unavailableCopy(trust)}</Notice>
      ) : null}

      {relation.status === 'ready' ? (
        <section className='w-full min-w-0 max-w-full space-y-2 overflow-hidden rounded-[16px] bg-[var(--surface-panel-soft)] p-4'>
          <h4 className='font-semibold'>{t('profile:communityNodeAdvisory.relation.title')}</h4>
          <p className='text-sm'>{t('profile:communityNodeAdvisory.relation.score', { score: continuous(relation.value.score) })}</p>
          <ul className='space-y-1 text-sm text-[var(--muted-foreground)]'>
            {relation.value.basis.map((basis) => (
              <li key={basis.feature} className='break-words'>{basis.feature}: {continuous(basis.value)} × {continuous(basis.weight)} = {continuous(basis.contribution)}</li>
            ))}
          </ul>
        </section>
      ) : relation.status === 'unavailable' ? (
        <Notice>{unavailableCopy(relation)}</Notice>
      ) : null}

      {neighbors.status === 'ready' ? (
        <section className='w-full min-w-0 max-w-full space-y-2 overflow-hidden rounded-[16px] bg-[var(--surface-panel-soft)] p-4'>
          <h4 className='font-semibold'>{t('profile:communityNodeAdvisory.neighbors.title')}</h4>
          <p className='text-sm text-[var(--muted-foreground)]'>{t('profile:communityNodeAdvisory.neighbors.notice')}</p>
          {neighbors.value.neighbors.length === 0 ? (
            <p className='text-sm'>{t('profile:communityNodeAdvisory.neighbors.empty')}</p>
          ) : (
            <ul className='space-y-1 font-mono text-xs'>
              {neighbors.value.neighbors.map((pubkey) => <li key={pubkey} className='break-all'>{pubkey}</li>)}
            </ul>
          )}
        </section>
      ) : neighbors.status === 'unavailable' ? (
        <Notice>{unavailableCopy(neighbors)}</Notice>
      ) : null}

      {appealBasis && appealPlan && appealSubjectForBasis(appealBasis, targetPubkey) ? (
        <ReportRoutingDialog
          open
          onOpenChange={(open) => {
            if (!open) setAppealSelection(null);
          }}
          subject={appealSubjectForBasis(appealBasis, targetPubkey)!}
          plan={appealPlan}
          appeal={{
            riskSignalId: appealBasis.signal_id,
            issuerNodeId: appealBasis.issuer_node_id,
          }}
          onSubmit={submitAppeal}
          resolving={appealResolving}
          resolveError={appealResolveError}
          onSubmitted={async () => {
            await loadAdvisory({ preserveAppeal: true });
          }}
        />
      ) : null}
    </section>
  );
}
