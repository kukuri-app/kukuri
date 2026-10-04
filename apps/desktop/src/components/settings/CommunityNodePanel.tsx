import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { Button } from '@/components/ui/button';
import { Card, CardHeader } from '@/components/ui/card';
import { Input } from '@/components/ui/input';
import { Notice } from '@/components/ui/notice';
import type { RelationOptoutResponse } from '@/lib/api';
import type { CommunityIndexNodePreference } from '@/lib/api/communityIndex';
import {
  trustRelationUnavailableReason,
  type TrustRelationUnavailableReason,
} from '@/lib/api/trustRelationPresentation';

import { CommunityNodeAdvisoryAdoptionField } from './CommunityNodeAdvisoryAdoptionField';
import { CommunityNodeConsentDialog } from './CommunityNodeConsentDialog';
import { CommunityNodeTrustPriorityField } from './CommunityNodeTrustPriorityField';
import {
  CommunityNodeObservationSharingField,
  type CommunityNodeObservationSharingHandlers,
} from './CommunityNodeObservationSharingField';
import { useCommunityNodePolicyDialog, type FetchCommunityNodePolicyView, type AcceptCommunityNodePolicyView } from '@/shell/actions/useCommunityNodePolicyDialog';
import { SettingsActionRow } from './SettingsActionRow';
import { SettingsDiagnosticList } from './SettingsDiagnosticList';
import { SettingsEditorField } from './SettingsEditorField';
import { type CommunityNodePanelView } from './types';

type CommunityNodePanelProps = {
  view: CommunityNodePanelView;
  saveDisabled: boolean;
  resetDisabled: boolean;
  clearDisabled: boolean;
  nodeActionsDisabled?: boolean;
  onAddNode: () => void;
  onNodeBaseUrlChange: (id: string, value: string) => void;
  onRemoveNode: (id: string) => void;
  /// #1056: node ごとの content advisory 採用の切替(保存は「ノードを保存」で確定する)。
  onNodeContentAdvisoryChange?: (id: string, enabled: boolean) => void;
  onSaveNodes: () => void;
  onReset: () => void;
  onClearNodes: () => void;
  onAuthenticate: (baseUrl: string) => void;
  onFetchConsents: FetchCommunityNodePolicyView;
  onAcceptConsents: AcceptCommunityNodePolicyView;
  onWithdrawConsents?: (baseUrl: string) => void | Promise<void>;
  onRefresh: (baseUrl: string) => boolean | void | Promise<boolean | void>;
  onClearToken: (baseUrl: string) => void;
  onSubmitInviteCode: (baseUrl: string, inviteCode: string) => Promise<void>;
  /// #1061: 信頼値の採用順位。未指定なら順位の設定を出さない。
  trustNodePriority?: readonly string[];
  onTrustNodePriorityChange?: (priority: string[]) => void;
  /// #1061: ブロック / ミュート観測の提供（CN の任意文書への同意）。未指定なら選択肢を出さない。
  observationSharing?: CommunityNodeObservationSharingHandlers;
  onGetRelationOptout?: (baseUrl: string) => Promise<RelationOptoutResponse>;
  onSetRelationOptout?: (baseUrl: string) => Promise<RelationOptoutResponse>;
  onClearRelationOptout?: (baseUrl: string) => Promise<RelationOptoutResponse>;
  indexNodePreference?: CommunityIndexNodePreference;
  eligibleIndexNodeBaseUrls?: readonly string[];
  onIndexNodePreferenceChange?: (preference: CommunityIndexNodePreference) => void;
  showDiagnostics?: boolean;
};

export function CommunityNodePanel({
  view,
  saveDisabled,
  resetDisabled,
  clearDisabled,
  nodeActionsDisabled = false,
  onAddNode,
  onNodeBaseUrlChange,
  onRemoveNode,
  onNodeContentAdvisoryChange,
  onSaveNodes,
  onReset,
  onClearNodes,
  onAuthenticate,
  onFetchConsents,
  onAcceptConsents,
  onWithdrawConsents = () => {},
  onRefresh,
  onClearToken,
  onSubmitInviteCode,
  trustNodePriority,
  onTrustNodePriorityChange,
  observationSharing,
  onGetRelationOptout,
  onSetRelationOptout,
  onClearRelationOptout,
  indexNodePreference = { mode: 'auto' },
  eligibleIndexNodeBaseUrls = [],
  onIndexNodePreferenceChange = () => {},
  showDiagnostics = true,
}: CommunityNodePanelProps) {
  const { t, i18n } = useTranslation(['common', 'settings']);
  const consentFlow = useCommunityNodePolicyDialog({
    nodes: view.nodes, language: i18n.resolvedLanguage ?? i18n.language,
    fetchPolicies: onFetchConsents, acceptPolicies: onAcceptConsents, withdraw: onWithdrawConsents,
  });
  const [relationOptoutByNode, setRelationOptoutByNode] = useState<
    Record<string, { busy: boolean; value: RelationOptoutResponse | null; error: string | null }>
  >({});
  const [inviteCodeByNode, setInviteCodeByNode] = useState<Record<string, string>>({});
  const [inviteBusyByNode, setInviteBusyByNode] = useState<Record<string, boolean>>({});

  const relationOptoutAvailable = Boolean(
    onGetRelationOptout && onSetRelationOptout && onClearRelationOptout
  );
  const configuredIndexNodes = view.nodes.filter((node) => node.saved && node.baseUrl.trim());
  const selectedIndexNodeValue =
    indexNodePreference.mode === 'manual' ? indexNodePreference.baseUrl : 'auto';
  const selectedIndexNodeUnavailable =
    indexNodePreference.mode === 'manual' &&
    !eligibleIndexNodeBaseUrls.includes(indexNodePreference.baseUrl);

  // 距離利用停止欄の縮退案内(#712)。未提供・失効・認証・同意を安定コードで判別し、
  // 索引画面と同じ文言で案内する。判別できないときだけ従来どおり生メッセージを表示する。
  const DISTANCE_OPTOUT_ERROR_KEYS: Partial<Record<TrustRelationUnavailableReason, string>> = {
    relation_visibility_not_configured: 'notConfigured',
    relation_visibility_not_activated: 'notActivated',
    auth_required: 'authRequired',
    consent_required: 'consentRequired',
  };

  function distanceOptoutErrorMessage(error: unknown): string {
    const key = DISTANCE_OPTOUT_ERROR_KEYS[trustRelationUnavailableReason(error)];
    if (key) return t(`settings:communityNode.distanceOptout.errors.${key}`);
    return error instanceof Error ? error.message : String(error);
  }

  async function updateRelationOptout(
    baseUrl: string,
    operation: () => Promise<RelationOptoutResponse>
  ) {
    setRelationOptoutByNode((current) => ({
      ...current,
      [baseUrl]: { busy: true, value: current[baseUrl]?.value ?? null, error: null },
    }));
    try {
      const value = await operation();
      setRelationOptoutByNode((current) => ({
        ...current,
        [baseUrl]: { busy: false, value, error: null },
      }));
    } catch (error) {
      setRelationOptoutByNode((current) => ({
        ...current,
        [baseUrl]: {
          busy: false,
          value: current[baseUrl]?.value ?? null,
          error: distanceOptoutErrorMessage(error),
        },
      }));
    }
  }

  async function refreshCommunityNode(node: CommunityNodePanelView['nodes'][number]) {
    const knownConsentRequired =
      !node.consent.hasLocalConsent ||
      node.consent.withdrawn ||
      node.consent.hasPendingUpdate ||
      (node.consent.loaded && !node.consent.allRequiredAccepted);
    if (knownConsentRequired) {
      consentFlow.open(node.baseUrl);
      return;
    }
    const consentRequired = await onRefresh(node.baseUrl);
    if (consentRequired) {
      consentFlow.open(node.baseUrl);
    }
  }

  async function submitInviteCode(baseUrl: string) {
    const inviteCode = inviteCodeByNode[baseUrl]?.trim() ?? '';
    if (!inviteCode) {
      return;
    }
    setInviteBusyByNode((current) => ({ ...current, [baseUrl]: true }));
    try {
      await onSubmitInviteCode(baseUrl, inviteCode);
      setInviteCodeByNode((current) => ({ ...current, [baseUrl]: '' }));
    } finally {
      setInviteBusyByNode((current) => ({ ...current, [baseUrl]: false }));
    }
  }

  return (
    <Card className='min-w-0 space-y-4'>
      <CardHeader>
        <h3>{t('settings:communityNode.title')}</h3>
        <small>{view.summaryLabel}</small>
      </CardHeader>

      {view.status === 'loading' ? <Notice>{t('settings:communityNode.loading')}</Notice> : null}
      {view.panelError ? <Notice tone='destructive'>{view.panelError}</Notice> : null}

      <SettingsEditorField
        as='div'
        label={t('settings:communityNode.nodesLabel')}
        hint={t('settings:communityNode.nodesHint')}
        message={view.editorMessage}
        tone={view.editorMessageTone}
      >
        <SettingsActionRow>
          <Button variant='secondary' onClick={onAddNode}>
            {t('settings:communityNode.actions.addNode')}
          </Button>
        </SettingsActionRow>
      </SettingsEditorField>

      <SettingsActionRow>
        <Button variant='secondary' disabled={saveDisabled} onClick={onSaveNodes}>
          {t('settings:communityNode.actions.saveNodes')}
        </Button>
        <Button variant='secondary' disabled={resetDisabled} onClick={onReset}>
          {t('common:actions.reset')}
        </Button>
        <Button variant='secondary' disabled={clearDisabled} onClick={onClearNodes}>
          {t('settings:communityNode.actions.clearNodes')}
        </Button>
      </SettingsActionRow>

      {trustNodePriority && onTrustNodePriorityChange ? (
        <CommunityNodeTrustPriorityField
          configuredBaseUrls={view.nodes
            .filter((node) => node.saved && node.baseUrl.trim())
            .map((node) => node.baseUrl)}
          priority={trustNodePriority}
          disabled={nodeActionsDisabled}
          onChange={onTrustNodePriorityChange}
        />
      ) : null}

      <SettingsEditorField
        label={t('settings:communityNode.indexNode.label')}
        hint={t('settings:communityNode.indexNode.hint')}
      >
        <select
          aria-label={t('settings:communityNode.indexNode.label')}
          className='min-h-11 w-full rounded-[var(--radius-control)] border border-[var(--border-subtle)] bg-[var(--surface-panel)] px-3 text-sm text-foreground'
          value={selectedIndexNodeValue}
          onChange={(event) => {
            const baseUrl = event.currentTarget.value;
            onIndexNodePreferenceChange(
              baseUrl === 'auto' ? { mode: 'auto' } : { mode: 'manual', baseUrl }
            );
          }}
        >
          <option value='auto'>{t('settings:communityNode.indexNode.automatic')}</option>
          {configuredIndexNodes.map((node) => {
            const available = eligibleIndexNodeBaseUrls.includes(node.baseUrl);
            return (
              <option key={node.id} value={node.baseUrl}>
                {available
                  ? node.baseUrl
                  : t('settings:communityNode.indexNode.optionUnavailable', {
                      baseUrl: node.baseUrl,
                    })}
              </option>
            );
          })}
        </select>
        {selectedIndexNodeUnavailable ? (
          <Notice tone='warning'>{t('settings:communityNode.indexNode.unavailable')}</Notice>
        ) : null}
      </SettingsEditorField>

      {view.nodes.length === 0 ? <Notice>{t('settings:communityNode.noNodes')}</Notice> : null}

      <div className='min-w-0 space-y-3'>
        {view.nodes.map((node) => {
          const relationOptout = relationOptoutByNode[node.baseUrl];
          const admissionCode = node.admissionRejectionCode;
          const inviteActionAvailable = admissionCode == null || admissionCode.startsWith('INVITE_');
          const inviteCode = inviteCodeByNode[node.baseUrl] ?? '';
          const inviteBusy = inviteBusyByNode[node.baseUrl] ?? false;
          const inviteHelpId = `community-node-invite-help-${node.id}`;
          return (
          <section
            key={node.id}
            className='min-w-0 rounded-[20px] border border-[var(--border-subtle)] bg-[var(--surface-panel-soft)] p-4 shadow-[var(--shadow-dropdown)]'
          >
            <div className='flex flex-wrap items-start justify-between gap-3'>
              <div className='min-w-0 flex-1 space-y-3'>
                <h4 className='break-all text-base font-semibold text-foreground'>
                  {node.baseUrl.trim() || t('settings:communityNode.baseUrlsPlaceholder')}
                </h4>
                <div className='space-y-2'>
                  <label className='block text-sm font-medium text-foreground'>
                    {t('settings:communityNode.baseUrlLabel')}
                  </label>
                  <Input
                    aria-label={t('settings:communityNode.baseUrlLabel')}
                    value={node.baseUrl}
                    onChange={(event) => onNodeBaseUrlChange(node.id, event.target.value)}
                    placeholder={t('settings:communityNode.baseUrlsPlaceholder')}
                    className='font-mono text-[0.8rem]'
                  />
                </div>
                <p className='text-sm text-[var(--muted-foreground)]'>
                  {node.saved
                    ? t('settings:communityNode.nodeSummary')
                    : t('settings:communityNode.unsavedNodeSummary')}
                </p>
                {admissionCode ? (
                  <Notice tone={admissionCode === 'BANNED' ? 'destructive' : 'warning'}>
                    <span className='block font-semibold'>
                      {t(`settings:communityNode.admission.reasons.${admissionCode}`)}
                    </span>
                    <span className='block'>
                      {t(`settings:communityNode.admission.nextSteps.${admissionCode}`)}
                    </span>
                  </Notice>
                ) : null}
                {inviteActionAvailable ? (
                  <div className='space-y-2'>
                    <label className='block text-sm font-medium text-foreground'>
                      {t('settings:communityNode.admission.inviteCodeLabel')}
                    </label>
                    <Input
                      type='password'
                      autoComplete='off'
                      spellCheck={false}
                      aria-label={t('settings:communityNode.admission.inviteCodeLabel')}
                      aria-describedby={inviteHelpId}
                      value={inviteCode}
                      onChange={(event) =>
                        setInviteCodeByNode((current) => ({
                          ...current,
                          [node.baseUrl]: event.target.value,
                        }))
                      }
                      placeholder={t('settings:communityNode.admission.inviteCodePlaceholder')}
                    />
                    <p id={inviteHelpId} className='text-sm text-[var(--muted-foreground)]'>
                      {node.inviteCodeSaved
                        ? t('settings:communityNode.admission.inviteCodeSaved')
                        : t('settings:communityNode.admission.inviteCodeHint')}
                    </p>
                    <SettingsActionRow>
                      <Button
                        variant='secondary'
                        disabled={
                          nodeActionsDisabled ||
                          !node.saved ||
                          !node.baseUrl.trim() ||
                          !inviteCode.trim() ||
                          inviteBusy
                        }
                        onClick={() => void submitInviteCode(node.baseUrl)}
                      >
                        {inviteBusy
                          ? t('settings:communityNode.admission.savingInviteCode')
                          : t('settings:communityNode.admission.saveInviteCode')}
                      </Button>
                    </SettingsActionRow>
                  </div>
                ) : null}
              </div>
              <Button variant='secondary' onClick={() => onRemoveNode(node.id)}>
                {t('common:actions.remove')}
              </Button>
            </div>

            {showDiagnostics ? (
              <>
                <div className='mt-4'>
                  <SettingsDiagnosticList items={node.diagnostics} columns={2} />
                </div>

                <div className='mt-4 space-y-2'>
                  <h5 className='text-sm font-semibold text-foreground'>
                    {t('settings:communityNode.dependency.heading')}
                  </h5>
                  <SettingsDiagnosticList items={node.dependency.diagnostics} columns={2} />
                  {node.dependency.manifestError ? (
                    <Notice tone='destructive'>{node.dependency.manifestError}</Notice>
                  ) : null}
                  {node.dependency.boundaryNotes.map((note) => (
                    <p key={note} className='text-sm text-[var(--muted-foreground)]'>
                      {note}
                    </p>
                  ))}
                </div>
              </>
            ) : null}

            <div className='mt-4'>
              <SettingsActionRow>
                <Button
                  variant='secondary'
                  disabled={nodeActionsDisabled || !node.saved || !node.baseUrl.trim()}
                  onClick={() => onAuthenticate(node.baseUrl)}
                >
                  {t('common:actions.authenticate')}
                </Button>
                <Button
                  variant='secondary'
                  disabled={nodeActionsDisabled || !node.saved || !node.baseUrl.trim()}
                  onClick={() => consentFlow.open(node.baseUrl)}
                >
                  {t('common:actions.consents')}
                </Button>
                <Button
                  variant='secondary'
                  disabled={nodeActionsDisabled || !node.saved || !node.baseUrl.trim()}
                  onClick={() => void refreshCommunityNode(node)}
                >
                  {t('common:actions.refresh')}
                </Button>
                <Button
                  variant='secondary'
                  disabled={nodeActionsDisabled || !node.saved || !node.baseUrl.trim()}
                  onClick={() => onClearToken(node.baseUrl)}
                >
                  {t('settings:communityNode.actions.clearToken')}
                </Button>
              </SettingsActionRow>
            </div>
            {onNodeContentAdvisoryChange ? (
              <CommunityNodeAdvisoryAdoptionField
                nodeId={node.id}
                enabled={node.contentAdvisoryEnabled !== false}
                onChange={(enabled) => onNodeContentAdvisoryChange(node.id, enabled)}
              />
            ) : null}
            {observationSharing && node.saved && node.baseUrl.trim() ? (
              <CommunityNodeObservationSharingField
                nodeId={node.id}
                baseUrl={node.baseUrl}
                disabled={nodeActionsDisabled}
                language={i18n.resolvedLanguage ?? i18n.language}
                {...observationSharing}
              />
            ) : null}
            {relationOptoutAvailable ? (
              <section className='mt-4 space-y-3 rounded-[16px] border border-[var(--border-subtle)] p-4'>
                <div className='space-y-1'>
                  <h5 className='text-sm font-semibold text-foreground'>
                    {t('settings:communityNode.distanceOptout.title')}
                  </h5>
                  <p className='text-sm text-[var(--muted-foreground)]'>
                    {t('settings:communityNode.distanceOptout.description')}
                  </p>
                  <p className='text-sm text-[var(--muted-foreground)]'>
                    {t('settings:communityNode.distanceOptout.notPrivacy')}
                  </p>
                </div>
                {relationOptout?.value ? (
                  <Notice tone={relationOptout.value.opted_out ? 'warning' : 'neutral'}>
                    {relationOptout.value.opted_out
                      ? t('settings:communityNode.distanceOptout.enabled', {
                          distance: relationOptout.value.min_proximity,
                        })
                      : t('settings:communityNode.distanceOptout.disabled', {
                          distance: relationOptout.value.min_proximity,
                        })}
                  </Notice>
                ) : (
                  <p className='text-sm text-[var(--muted-foreground)]'>
                    {t('settings:communityNode.distanceOptout.notLoaded')}
                  </p>
                )}
                {!node.distanceOptoutEligible ? (
                  <Notice tone='warning'>{t('settings:communityNode.distanceOptout.notEligible')}</Notice>
                ) : null}
                {relationOptout?.error ? <Notice tone='destructive'>{relationOptout.error}</Notice> : null}
                <SettingsActionRow>
                  <Button
                    variant='secondary'
                    disabled={
                      nodeActionsDisabled ||
                      !node.saved ||
                      !node.baseUrl.trim() ||
                      !node.distanceOptoutEligible ||
                      relationOptout?.busy
                    }
                    onClick={() =>
                      void updateRelationOptout(node.baseUrl, () => onGetRelationOptout!(node.baseUrl))
                    }
                  >
                    {t('settings:communityNode.distanceOptout.load')}
                  </Button>
                  {relationOptout?.value?.opted_out ? (
                    <Button
                      variant='secondary'
                      disabled={relationOptout.busy || !node.distanceOptoutEligible}
                      onClick={() =>
                        void updateRelationOptout(node.baseUrl, () => onClearRelationOptout!(node.baseUrl))
                      }
                    >
                      {t('settings:communityNode.distanceOptout.clear')}
                    </Button>
                  ) : (
                    <Button
                      variant='secondary'
                      disabled={!relationOptout?.value || relationOptout.busy || !node.distanceOptoutEligible}
                      onClick={() =>
                        void updateRelationOptout(node.baseUrl, () => onSetRelationOptout!(node.baseUrl))
                      }
                    >
                      {t('settings:communityNode.distanceOptout.enable')}
                    </Button>
                  )}
                </SettingsActionRow>
              </section>
            ) : null}
          </section>
          );
        })}
      </div>

      {consentFlow.dialog ? <CommunityNodeConsentDialog {...consentFlow.dialog} /> : null}
    </Card>
  );
}
