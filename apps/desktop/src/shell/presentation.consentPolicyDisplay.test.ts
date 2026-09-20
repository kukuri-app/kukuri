import { describe, expect, it } from 'vitest';

import type { CommunityNodeNodeStatus } from '@/lib/api';

import { communityNodeConsentView } from './presentation';

// #1192: 一括同意の一覧は 利用規約 → プライバシーポリシー → 残り の順で、権利侵害申出
// ポリシーは含めない（権利侵害申請モーダルで提示する）。
function status(): CommunityNodeNodeStatus {
  return {
    base_url: 'https://node.example',
    auth_state: { authenticated: true },
    local_consent: { records: [], withdrawn_at: null },
    invite_code_saved: false,
    restart_required: false,
  } as CommunityNodeNodeStatus;
}

function catalog(entries: { slug: string; kind?: string; required?: boolean }[]) {
  return {
    status: 'ok' as const,
    policies: entries.map(({ slug, kind, required }) => ({
      policy_slug: slug,
      policy_version: 1,
      title: slug,
      body_markdown: 'Body',
      required: required ?? false,
      policy_kind: kind,
      is_current: true,
      reference_translation: false,
      fallback: false,
      material_change: false,
      requires_reconsent: false,
    })),
  };
}

describe('communityNodeConsentView (#1192)', () => {
  it('puts the terms first and the privacy policy second, keeping the rest in catalog order', () => {
    const view = communityNodeConsentView(
      status(),
      catalog([
        { slug: 'abuse_policy', kind: 'abuse_policy' },
        { slug: 'privacy_policy', kind: 'privacy', required: true },
        { slug: 'data_retention', kind: 'data_retention' },
        { slug: 'terms_of_service', kind: 'terms', required: true },
      ])
    );

    expect(view.policies.map((policy) => policy.policySlug)).toEqual([
      'terms_of_service',
      'privacy_policy',
      'abuse_policy',
      'data_retention',
    ]);
    expect(view.policies[0].policyKind).toBe('terms');
  });

  it('hides the rights infringement policy from the bulk consent list', () => {
    const view = communityNodeConsentView(
      status(),
      catalog([
        { slug: 'terms_of_service', kind: 'terms', required: true },
        { slug: 'rights_infringement', kind: 'rights_infringement' },
        { slug: 'trust_observation_sharing', kind: 'trust_observation_sharing' },
      ])
    );

    expect(view.policies.map((policy) => policy.policySlug)).toEqual(['terms_of_service']);
  });

  // 必須文書を隠すと allRequiredAccepted が到達不能になり node へ接続できなくなる。
  it('keeps a required rights infringement policy in the list', () => {
    const view = communityNodeConsentView(
      status(),
      catalog([
        { slug: 'terms_of_service', kind: 'terms', required: true },
        { slug: 'rights_infringement', kind: 'rights_infringement', required: true },
      ])
    );

    expect(view.policies.map((policy) => policy.policySlug)).toEqual([
      'terms_of_service',
      'rights_infringement',
    ]);
  });

  // policy_kind を持たない文書（現行 operator config に無い退役 slug 等）は末尾に残す。
  it('keeps a policy without a kind in the list, after the ordered ones', () => {
    const view = communityNodeConsentView(
      status(),
      catalog([
        { slug: 'legacy_notice' },
        { slug: 'terms_of_service', kind: 'terms', required: true },
      ])
    );

    expect(view.policies.map((policy) => policy.policySlug)).toEqual([
      'terms_of_service',
      'legacy_notice',
    ]);
  });
});
