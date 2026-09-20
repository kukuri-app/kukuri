import { useState } from 'react';

import type { Meta, StoryObj } from '@storybook/react-vite';

import { type ReportRoutingPlan } from '@/lib/api/reportRouting';

import { Button } from '@/components/ui/button';

import { ReportRoutingDialog, type ReportRoutingSubject } from './ReportRoutingDialog';

const meta = {
  title: 'Core/ReportRoutingDialog',
  parameters: {
    layout: 'centered',
  },
} satisfies Meta;

export default meta;

type Story = StoryObj<typeof meta>;

const subject: ReportRoutingSubject = {
  kind: 'post',
  id: 'post-1',
  label: 'alice',
};

const endpointPlan: ReportRoutingPlan = {
  provenanceUnknown: false,
  observedButUnresolved: false,
  localActionsOnly: false,
  candidates: [
    {
      target: {
        nodeBaseUrl: 'https://index.kukuri.example',
        nodeId: 'index-node',
        capability: 'community_index',
        reportEndpoint: 'https://index.kukuri.example/v1/report',
        rightsRequestUrl: 'https://index.kukuri.example/rights-requests/new',
        rightsRequestPolicyUrl: 'https://index.kukuri.example/rights-infringement-policy',
        abuseContact: 'abuse@index.kukuri.example',
        policyUrl: 'https://index.kukuri.example/moderation',
        authorityScope: ['this_node'],
      },
      contact: { kind: 'endpoint', value: 'https://index.kukuri.example/v1/report' },
    },
    {
      target: {
        nodeBaseUrl: 'https://cache.kukuri.example',
        capability: 'media_cache',
        abuseContact: 'abuse@cache.kukuri.example',
        authorityScope: ['this_node'],
      },
      contact: { kind: 'contact', value: 'abuse@cache.kukuri.example' },
    },
  ],
};

const unknownPlan: ReportRoutingPlan = {
  provenanceUnknown: true,
  observedButUnresolved: false,
  localActionsOnly: true,
  candidates: [],
};

const observedUnresolvedPlan: ReportRoutingPlan = {
  provenanceUnknown: false,
  observedButUnresolved: true,
  localActionsOnly: true,
  candidates: [],
};

// #1192: 権利侵害申出ポリシーは同意一覧ではなくこの画面で提示する。
const RIGHTS_POLICY_BODY = `## このノードが取り得る措置

- このノードが索引した投稿・メディアの索引除外
- このノードが発行したリスク判定の取り下げ

## このノードが取り得ない措置

- 投稿正本の削除、他ノードの索引や配信の停止
- 既に第三者端末へ届いたデータの回収

申出の受付は権利侵害の認定や requested action の保証ではありません。`;

const rightsPolicyCatalog = {
  policies: [
    {
      policy_slug: 'terms_of_service',
      policy_version: 1,
      title: 'Community Node 利用規約',
      body_markdown: '利用規約の本文。',
      required: true,
      policy_kind: 'terms',
      is_current: true,
      reference_translation: false,
      fallback: false,
      material_change: false,
      requires_reconsent: false,
    },
    {
      policy_slug: 'rights_infringement',
      policy_version: 2,
      title: 'Community Node 権利侵害申出ポリシー',
      body_markdown: RIGHTS_POLICY_BODY,
      required: false,
      policy_kind: 'rights_infringement',
      effective_date: '2026-09-02',
      language: 'ja',
      is_current: true,
      reference_translation: false,
      fallback: false,
      material_change: false,
      requires_reconsent: false,
    },
  ],
};

function DialogHarness({
  plan,
  label,
  appeal,
  fetchPolicies,
}: {
  plan: ReportRoutingPlan;
  label: string;
  appeal?: { riskSignalId: string; issuerNodeId: string };
  fetchPolicies?: () => Promise<typeof rightsPolicyCatalog>;
}) {
  const [open, setOpen] = useState(false);
  return (
    <div className='flex flex-col items-center gap-4'>
      <Button type='button' onClick={() => setOpen(true)}>
        {label}
      </Button>
      <ReportRoutingDialog
        open={open}
        onOpenChange={setOpen}
        subject={subject}
        plan={plan}
        appeal={appeal}
        onSubmit={async ({ candidate, reason, appeal: appealInput }) => ({
          status: 'submitted',
          reference_id: `story-${candidate.target.capability}-${reason}`,
          disputed_risk_signal_id: appealInput?.risk_signal_id ?? null,
        })}
        onCopyContact={() => {}}
        onFetchNodePolicies={fetchPolicies}
        localActions={
          <Button type='button' variant='secondary'>
            Mute author
          </Button>
        }
      />
    </div>
  );
}

export const ResolvedTargets: Story = {
  render: () => <DialogHarness plan={endpointPlan} label='Report (resolved targets)' />,
};

export const UnknownProvenance: Story = {
  render: () => <DialogHarness plan={unknownPlan} label='Report (unknown provenance)' />,
};

export const ObservedButUnresolved: Story = {
  render: () => <DialogHarness plan={observedUnresolvedPlan} label='Report (observed, unresolved)' />,
};

export const RightsInfringementPolicyShown: Story = {
  render: () => (
    <DialogHarness
      plan={endpointPlan}
      label='Report (rights infringement)'
      fetchPolicies={async () => rightsPolicyCatalog}
    />
  ),
};

export const RightsInfringementPolicyUnavailable: Story = {
  render: () => (
    <DialogHarness
      plan={endpointPlan}
      label='Report (rights policy unavailable)'
      fetchPolicies={async () => {
        throw new Error('offline');
      }}
    />
  ),
};

const appealPlan: ReportRoutingPlan = {
  provenanceUnknown: false,
  observedButUnresolved: false,
  localActionsOnly: false,
  candidates: [
    {
      target: {
        nodeBaseUrl: 'https://index.kukuri.example',
        nodeId: 'index-node',
        capability: 'trust_signal',
        reportEndpoint: 'https://index.kukuri.example/v1/report',
        policyUrl: 'https://index.kukuri.example/moderation',
        authorityScope: ['this_node'],
      },
      contact: { kind: 'endpoint', value: 'https://index.kukuri.example/v1/report' },
    },
  ],
};

export const Appeal: Story = {
  render: () => (
    <DialogHarness
      plan={appealPlan}
      label='異議申し立てを開く'
      appeal={{ riskSignalId: 'signal-1', issuerNodeId: 'index-node' }}
    />
  ),
};
