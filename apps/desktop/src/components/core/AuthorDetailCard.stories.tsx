import type { Meta, StoryObj } from '@storybook/react-vite';
import { userEvent, within } from 'storybook/test';

import {
  createStoryAuthorDetailView,
  STORY_EMPTY_AUTHOR_DETAIL_VIEW,
} from '@/components/storyFixtures';

import { AuthorDetailCard } from './AuthorDetailCard';
import { CommunityNodeAdvisoryPanel } from './CommunityNodeAdvisoryPanel';
import { createDesktopMockApi } from '@/mocks/desktopApiMock';

const authorDetailView = createStoryAuthorDetailView();
const advisoryApi = {
  ...createDesktopMockApi(),
  // 異議申し立てを開いた時に取得する発行元ノードの最新 manifest(#696)。
  async fetchCommunityNodeManifest(baseUrl: string) {
    const manifest = advisoryManifests[baseUrl as keyof typeof advisoryManifests];
    return manifest
      ? { status: 'ok' as const, manifest }
      : { status: 'absent' as const, manifest: null };
  },
  async readCommunityNodeTrustUser(request: { target_pubkey: string; cursor?: string | null }) {
    const view = {
      viewer_pubkey: 'story-viewer',
      target_id: request.target_pubkey,
      absolute: 0.35,
      relative: 0.62,
      trust: 0.485,
      w_abs_applied: 0.5,
      computed_at: '2026-08-14T00:00:00Z',
      basis: [
        {
          signal_id: 'story-signal',
          issuer_node_id: 'community.example.com',
          target: 'user_pubkey' as const,
          target_id: request.target_pubkey,
          component: 'relative' as const,
          category: 'spam' as const,
          severity: 'low' as const,
          basis: 'provider_verdict' as const,
          confidence: 0.78,
          visibility: 'subscribed_nodes' as const,
          appeal_status: 'none' as const,
          expires_at: null,
          raw_contribution: 0.2,
          decay_factor: 0.9,
          relation_weight: 1,
          contribution: 0.18,
        },
        // 添付(blob_cid)由来の判定。media として異議申し立てできる(#707)。
        {
          signal_id: 'story-attachment-signal',
          issuer_node_id: 'community.example.com',
          target: 'blob_cid' as const,
          target_id: 'f'.repeat(64),
          component: 'absolute' as const,
          category: 'nsfw' as const,
          severity: 'high' as const,
          basis: 'provider_verdict' as const,
          confidence: 0.9,
          visibility: 'local' as const,
          appeal_status: 'none' as const,
          expires_at: null,
          raw_contribution: -0.3,
          decay_factor: 1,
          relation_weight: 1,
          contribution: -0.3,
        },
      ],
      // basis は 1 ページずつ届き、続きは「さらに表示」で読む(#1702)。
      basis_next_cursor: 'story-basis-page-2',
    };
    if (!request.cursor) return view;
    return {
      ...view,
      basis: [
        {
          ...view.basis[0],
          signal_id: 'story-older-signal',
          category: 'phishing' as const,
          severity: 'medium' as const,
          decay_factor: 0.4,
          contribution: 0.08,
        },
      ],
      basis_next_cursor: null,
    };
  },
  async listCommunityNodeRelationNeighbors() {
    return { viewer_pubkey: 'story-viewer', neighbors: ['c'.repeat(64)] };
  },
  async submitCommunityNodeReport(request: { appeal?: { risk_signal_id: string } | null }) {
    return {
      status: 'submitted' as const,
      reference_id: 'story-report',
      disputed_risk_signal_id: request.appeal?.risk_signal_id ?? null,
    };
  },
};

const clearedPostAdvisoryApi = {
  ...advisoryApi,
  async readCommunityNodeTrustUser(request: { target_pubkey: string }) {
    const view = await advisoryApi.readCommunityNodeTrustUser(request);
    return {
      ...view,
      absolute: 0,
      relative: 0,
      trust: 0,
      basis: [
        {
          ...view.basis[0],
          signal_id: 'story-cleared-post-signal',
          target: 'post_id' as const,
          target_id: 'post-cleared-by-operator',
          appeal_status: 'cleared' as const,
          contribution: 0,
        },
      ],
      basis_next_cursor: null,
    };
  },
};

const advisoryManifests = {
  'https://community.example.com': {
    node_id: 'community.example.com',
    node_name: 'コミュニティノード',
    node_role: 'community-node',
    server_name: 'community.example.com',
    manifest_version: 'v1',
    capability_scope: { available_enabled: ['community_local_trust'], planned_enabled: [] },
    authority_scope: {
      applies_to: ['this_node', 'trust_signals_issued_by_this_node'],
      does_not_apply_to: [],
    },
    p2p_boundary: {
      identity_authority: false,
      profile_canonical_store: false,
      social_graph_canonical_store: false,
      content_truth_source: false,
      network_wide_authority: false,
    },
    abuse_contact: '',
    report_endpoint: 'https://community.example.com/v1/report',
    terms_url: '',
    privacy_url: '',
    moderation_policy_url: '',
  },
};

const meta = {
  title: 'Core/AuthorDetailCard',
  component: AuthorDetailCard,
  parameters: { layout: 'padded' },
  render: (args) => (
    <div style={{ maxWidth: '420px' }}>
      <AuthorDetailCard {...args} />
    </div>
  ),
  args: {
    view: authorDetailView,
    localAuthorPubkey: 'f'.repeat(64),
    onToggleRelationship: () => undefined,
    onToggleMute: () => undefined,
  },
} satisfies Meta<typeof AuthorDetailCard>;

export default meta;

type Story = StoryObj<typeof meta>;

export const Selected: Story = {};

export const CommunityNodeAdvisory: Story = {
  args: {
    communityNodeAdvisory: (
      <CommunityNodeAdvisoryPanel
        api={advisoryApi}
        targetPubkey={authorDetailView.author?.author_pubkey ?? 'a'.repeat(64)}
        nodeBaseUrls={['https://community.example.com']}
      />
    ),
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(canvas.getByRole('button', { name: 'Load advisory' }));
    await userEvent.click(await canvas.findByText(/community\.example\.com · spam · low/));
    canvasElement.ownerDocument.defaultView?.scrollTo(0, 0);
  },
};

export const CommunityNodeClearedPostAdvisory: Story = {
  args: {
    communityNodeAdvisory: (
      <CommunityNodeAdvisoryPanel
        api={clearedPostAdvisoryApi}
        targetPubkey={authorDetailView.author?.author_pubkey ?? 'a'.repeat(64)}
        nodeBaseUrls={['https://community.example.com']}
      />
    ),
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(canvas.getByRole('button', { name: 'Load advisory' }));
    await userEvent.click(await canvas.findByText(/community\.example\.com · spam · low/));
    canvasElement.ownerDocument.defaultView?.scrollTo(0, 0);
  },
};

// #992: ブロック中はフォローとメッセージが無効になり、理由は tooltip で出る。
export const Blocked: Story = {
  args: {
    view: {
      ...authorDetailView,
      author: { ...authorDetailView.author!, following: false, blocking: true },
      summary: {
        ...authorDetailView.summary!,
        following: false,
        blocking: true,
        followActionLabel: 'Follow',
        blockActionLabel: 'Unblock',
      },
      canMessage: true,
    },
    onOpenDirectMessage: () => undefined,
    onToggleBlock: () => undefined,
  },
};

export const BlockedWhileFollowing: Story = {
  args: {
    view: {
      ...authorDetailView,
      author: { ...authorDetailView.author!, blocking: true },
      summary: { ...authorDetailView.summary!, blocking: true, blockActionLabel: 'Unblock' },
    },
    onToggleBlock: () => undefined,
  },
};

export const Empty: Story = {
  args: {
    view: STORY_EMPTY_AUTHOR_DETAIL_VIEW,
  },
};
