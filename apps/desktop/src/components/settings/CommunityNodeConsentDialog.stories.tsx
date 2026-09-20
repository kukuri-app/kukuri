import type { Meta, StoryObj } from '@storybook/react-vite';
import { expect, userEvent, within } from 'storybook/test';
import { CommunityNodeConsentDialog } from './CommunityNodeConsentDialog';
import { communityNodePanelFixture } from './fixtures';
import type { CommunityNodeConsentPolicyView } from './types';

// #1106: cn-operator が生成する本文と同じ構文(見出し・箇条書き・引用・表・コード・リンク)。
const abuseBody = [
  '# Abuse ポリシー',
  '',
  '- 運営者: Example Operator',
  '- 連絡先: abuse@example-kukuri.net',
  '',
  '> 注記: この文書は operator config から自動生成された下書きであり、法的助言ではありません。',
  '> 最終的な内容・適法性の判断は、運営者自身および必要に応じて専門家への確認が必要です。',
  '',
  '## 対応範囲',
  '',
  '通報エンドポイント `POST https://example-kukuri.net/v1/report` を提供します。詳細は [運営者の案内](https://example-kukuri.net/abuse) を参照してください。',
  '',
  '| データ | 保存先 | 性質 |',
  '|---|---|---|',
  '| 認証・同意・通報 | Postgres | 管理系の永続データ（同意は撤回まで、通報は保持方針に従う） |',
  '| ランデブー / presence | Valkey | TTL 付きの揮発データ（短期で自動失効） |',
  '',
  '```text',
  'client',
  '  +-- relay (cn-iroh-relay)',
  '```',
  '',
  '<script>alert("raw html is shown as text")</script>',
].join('\n');

const basePolicies = communityNodePanelFixture.nodes[0].consent.policies.map((policy) => ({
  ...policy, acceptedAtLabel: null,
}));
const longPolicies: CommunityNodeConsentPolicyView[] = [
  ...basePolicies,
  // #1192: 一覧は 利用規約 → プライバシー → 残り の順。権利侵害申出ポリシーは
  // 権利侵害申請モーダルで提示するため、この一覧には現れない。
  ...[
    ['privacy_policy', 'プライバシーポリシー', 'privacy', true],
    ['abuse_policy', 'Abuse ポリシー', 'abuse_policy', true],
    ['external_transmission', '外部送信に関する公表事項', 'external_transmission', true],
    ['moderation_policy', 'モデレーションポリシー', 'moderation_policy', true],
    ['data_retention', 'データ保持ポリシー', 'data_retention', false],
  ].map(([policySlug, title, policyKind, required]) => ({
    ...basePolicies[0],
    policySlug: policySlug as string,
    title: title as string,
    policyKind: policyKind as string,
    required: required as boolean,
    body: policySlug === 'abuse_policy' ? abuseBody : basePolicies[0].body,
    effectiveDate: '2026-09-17',
    language: 'ja',
  })),
];
const consent = {
  ...communityNodePanelFixture.nodes[0].consent,
  allRequiredAccepted: false, hasLocalConsent: false,
  policies: basePolicies,
};
const meta = {
  title: 'Settings/CommunityNodeConsentDialog',
  component: CommunityNodeConsentDialog,
  args: {
    open: true, baseUrl: 'https://api.kukuri.app', consent, busy: false,
    onOpenChange: () => {}, onAccept: () => {}, onRetry: () => {},
  },
} satisfies Meta<typeof CommunityNodeConsentDialog>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Review: Story = {};
export const Loading: Story = { args: { consent: { ...consent, loaded: false, loading: true, policies: [] } } };
export const LoadError: Story = { args: { consent: { ...consent, loaded: false, loadError: 'The connection is unavailable.', policies: [] } } };
export const Empty: Story = { args: { consent: { ...consent, policies: [] } } };
export const Accepting: Story = { args: { busy: true } };
export const AcceptError: Story = { args: { error: 'Consent could not be completed. Check the displayed terms and try again.' } };
export const Accepted: Story = { args: { consent: communityNodePanelFixture.nodes[0].consent } };
export const Updated: Story = { args: { consent: { ...consent, hasLocalConsent: true, hasPendingUpdate: true,
  policies: consent.policies.map((policy) => ({ ...policy, updated: true, policyVersion: 2, previouslyAcceptedVersion: 1 })),
} } };
// 版は同じで snapshot だけが変わった再同意。
export const ContentUpdated: Story = { args: { consent: { ...consent, hasLocalConsent: true, hasPendingUpdate: true,
  policies: longPolicies.map((policy, index) => ({
    ...policy, updated: index % 2 === 0, previouslyAcceptedVersion: index % 2 === 0 ? 1 : null,
  })),
} } };
export const ManyDocuments: Story = { args: { consent: { ...consent, policies: longPolicies } } };
export const ExpandedMarkdown: Story = {
  args: { consent: { ...consent, policies: longPolicies } },
  play: async ({ canvasElement }) => {
    const dialog = within(canvasElement.ownerDocument.body).getByRole('dialog');
    const toggle = within(dialog).getByRole('button', { name: 'Abuse ポリシー' });
    await userEvent.click(toggle);
    await expect(toggle).toHaveAttribute('aria-expanded', 'true');
    await expect(within(dialog).getByRole('heading', { name: '対応範囲' })).toBeInTheDocument();
  },
};
export const Withdrawn: Story = { args: { consent: { ...consent, withdrawn: true } } };
