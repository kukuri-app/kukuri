import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, test, vi } from 'vitest';

import type { CommunityNodeManifest } from '@/lib/api';

import { PostCard } from './PostCard';
import { createView } from './PostCard.testHelpers';

// PostCard の通報経路(投稿・画像添付・動画添付、最新 manifest 取得)の画面試験。#666 / #684 / #696 / #697
test('report action refreshes the observed node manifest when the dialog opens', async () => {
  const user = userEvent.setup();
  const manifest: CommunityNodeManifest = {
    node_id: 'node-1',
    node_name: 'node.example',
    node_role: 'community-node',
    server_name: 'node.example',
    manifest_version: 'v2',
    capability_scope: { available_enabled: ['community_index'], planned_enabled: [] },
    authority_scope: {
      applies_to: ['this_node', 'communities_indexed_by_this_node'],
      does_not_apply_to: ['user_identity'],
    },
    p2p_boundary: {
      identity_authority: false,
      profile_canonical_store: false,
      social_graph_canonical_store: false,
      content_truth_source: false,
      network_wide_authority: false,
    },
    abuse_contact: 'abuse@node.example',
    report_endpoint: 'https://node.example/v1/report',
    terms_url: '',
    privacy_url: '',
    moderation_policy_url: '',
  };
  const onFetchReportManifest = vi.fn().mockResolvedValue({ status: 'ok', manifest });

  render(
    <PostCard
      view={createView({
        provenance: {
          canonicalSource: 'author_docs',
          observedVia: [
            {
              nodeBaseUrl: 'https://node.example',
              capability: 'community_index',
              observedAt: 123,
            },
          ],
          responsibleReportTargets: [],
        },
      })}
      onOpenAuthor={() => undefined}
      onOpenThread={() => undefined}
      onReply={() => undefined}
      onSubmitReport={vi.fn()}
      onFetchReportManifest={onFetchReportManifest}
    />,
  );

  await user.click(screen.getByRole('button', { name: 'Report' }));

  await waitFor(() =>
    expect(onFetchReportManifest).toHaveBeenCalledWith('https://node.example'),
  );
  expect(await screen.findByText('node.example')).toBeInTheDocument();
});

test('report action does not fetch a manifest or create a candidate without provenance', async () => {
  const user = userEvent.setup();
  const onFetchReportManifest = vi.fn();

  render(
    <PostCard
      view={createView()}
      onOpenAuthor={() => undefined}
      onOpenThread={() => undefined}
      onReply={() => undefined}
      onSubmitReport={vi.fn()}
      onFetchReportManifest={onFetchReportManifest}
    />
  );

  await user.click(screen.getByRole('button', { name: 'Report' }));

  expect(screen.getByRole('dialog')).toBeInTheDocument();
  expect(onFetchReportManifest).not.toHaveBeenCalled();
  expect(screen.getByText('Cannot determine a report target')).toBeInTheDocument();
  expect(screen.getByText(/block, mute, or hide this locally/i)).toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Send report' })).not.toBeInTheDocument();
});

test('attachment report resolves the selected blob provenance instead of the post provenance', async () => {
  const user = userEvent.setup();
  const manifest = (nodeId: string): CommunityNodeManifest => ({
    node_id: nodeId,
    node_name: nodeId,
    node_role: 'community-node',
    server_name: nodeId,
    manifest_version: 'v1',
    capability_scope: { available_enabled: ['community_index'], planned_enabled: [] },
    authority_scope: {
      applies_to: ['this_node', 'communities_indexed_by_this_node'],
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
    report_endpoint: `https://${nodeId}/v1/report`,
    terms_url: '',
    privacy_url: '',
    moderation_policy_url: '',
  });
  const onFetchReportManifest = vi.fn(async (baseUrl: string) => ({
    status: 'ok' as const,
    manifest: manifest(new URL(baseUrl).hostname),
  }));
  const onSubmitReport = vi
    .fn()
    .mockResolvedValue({ status: 'submitted', reference_id: 'media-report-1' });

  render(
    <PostCard
      view={createView({
        provenance: {
          canonicalSource: 'author_docs',
          observedVia: [
            { nodeBaseUrl: 'https://post-node.example', capability: 'community_index' },
          ],
          responsibleReportTargets: [],
        },
        media: {
          ...createView().media,
          kind: 'image',
          imageGalleryItems: [
            {
              hash: 'attachment-hash',
              src: 'blob:attachment-preview',
              mime: 'image/png',
              provenance: {
                canonicalSource: 'blob',
                observedVia: [
                  { nodeBaseUrl: 'https://media-node.example', capability: 'community_index' },
                ],
                responsibleReportTargets: [],
              },
            },
          ],
        },
      })}
      onOpenAuthor={() => undefined}
      onOpenThread={() => undefined}
      onReply={() => undefined}
      onSubmitReport={onSubmitReport}
      onFetchReportManifest={onFetchReportManifest}
    />
  );

  await user.click(screen.getByRole('button', { name: 'image attachment' }));
  await user.click(within(screen.getByRole('dialog')).getByRole('button', { name: 'Report' }));

  await waitFor(() =>
    expect(onFetchReportManifest).toHaveBeenCalledWith('https://media-node.example')
  );
  expect(onFetchReportManifest).not.toHaveBeenCalledWith('https://post-node.example');
  expect(await screen.findByText('media-node.example')).toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Send report' }));
  await waitFor(() => expect(onSubmitReport).toHaveBeenCalledTimes(1));
  expect(onSubmitReport).toHaveBeenCalledWith(
    expect.objectContaining({
      node_base_url: 'https://media-node.example',
      subject_kind: 'media',
      subject_id: 'attachment-hash',
    })
  );
});

// #696: 通報先は開いた時に取得成功した最新 manifest だけから作る。
function reportManifest(nodeId: string, overrides: Partial<CommunityNodeManifest> = {}): CommunityNodeManifest {
  return {
    node_id: nodeId,
    node_name: nodeId,
    node_role: 'community-node',
    server_name: nodeId,
    manifest_version: 'v1',
    capability_scope: { available_enabled: ['community_index'], planned_enabled: [] },
    authority_scope: {
      applies_to: ['this_node', 'communities_indexed_by_this_node'],
      does_not_apply_to: [],
    },
    p2p_boundary: {
      identity_authority: false,
      profile_canonical_store: false,
      social_graph_canonical_store: false,
      content_truth_source: false,
      network_wide_authority: false,
    },
    abuse_contact: 'abuse@node.example',
    report_endpoint: `https://${nodeId}/v1/report`,
    terms_url: '',
    privacy_url: '',
    moderation_policy_url: '',
    ...overrides,
  };
}

function observedView() {
  return createView({
    provenance: {
      canonicalSource: 'author_docs',
      observedVia: [{ nodeBaseUrl: 'https://node.example', capability: 'community_index' }],
      responsibleReportTargets: [],
    },
  });
}

const OBSERVED_UNRESOLVED_TITLE = 'No reachable report target';

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => {
    resolve = r;
  });
  return { promise, resolve };
}

test('report dialog offers no target and no send action while the latest manifest is resolving', async () => {
  const user = userEvent.setup();
  const pending = deferred<{ status: 'ok'; manifest: CommunityNodeManifest }>();
  const onFetchReportManifest = vi.fn().mockReturnValue(pending.promise);
  const onSubmitReport = vi.fn();

  render(
    <PostCard
      view={observedView()}
      onOpenAuthor={() => undefined}
      onOpenThread={() => undefined}
      onReply={() => undefined}
      onSubmitReport={onSubmitReport}
      onFetchReportManifest={onFetchReportManifest}
    />
  );

  await user.click(screen.getByRole('button', { name: 'Report' }));
  await waitFor(() => expect(onFetchReportManifest).toHaveBeenCalledTimes(1));
  expect(screen.getByText('Checking the latest report targets…')).toBeInTheDocument();
  expect(screen.queryByText('node.example')).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Send report' })).not.toBeInTheDocument();

  pending.resolve({ status: 'ok', manifest: reportManifest('node.example') });
  expect(await screen.findByText('node.example')).toBeInTheDocument();
  expect(await screen.findByRole('button', { name: 'Send report' })).toBeEnabled();
  expect(onSubmitReport).not.toHaveBeenCalled();
});

test('a failed latest manifest fetch does not fall back to a target fetched by a previous open', async () => {
  const user = userEvent.setup();
  const onFetchReportManifest = vi
    .fn()
    .mockResolvedValueOnce({ status: 'ok', manifest: reportManifest('node.example') })
    .mockResolvedValueOnce({ status: 'absent', manifest: null });
  const onSubmitReport = vi.fn();

  render(
    <PostCard
      view={observedView()}
      onOpenAuthor={() => undefined}
      onOpenThread={() => undefined}
      onReply={() => undefined}
      onSubmitReport={onSubmitReport}
      onFetchReportManifest={onFetchReportManifest}
    />
  );

  await user.click(screen.getByRole('button', { name: 'Report' }));
  expect(await screen.findByText('node.example')).toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Cancel' }));
  await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());

  await user.click(screen.getByRole('button', { name: 'Report' }));
  await waitFor(() => expect(onFetchReportManifest).toHaveBeenCalledTimes(2));
  expect(await screen.findByText(/Could not refresh report targets/)).toBeInTheDocument();
  expect(screen.queryByText('node.example')).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Send report' })).not.toBeInTheDocument();
  expect(onSubmitReport).not.toHaveBeenCalled();
});

test('a node that withdrew its report endpoint and contact is not offered as a target', async () => {
  const user = userEvent.setup();
  const onFetchReportManifest = vi.fn().mockResolvedValue({
    status: 'ok',
    manifest: reportManifest('node.example', { report_endpoint: '', abuse_contact: '' }),
  });

  render(
    <PostCard
      view={observedView()}
      onOpenAuthor={() => undefined}
      onOpenThread={() => undefined}
      onReply={() => undefined}
      onSubmitReport={vi.fn()}
      onFetchReportManifest={onFetchReportManifest}
    />
  );

  await user.click(screen.getByRole('button', { name: 'Report' }));
  await waitFor(() => expect(onFetchReportManifest).toHaveBeenCalledTimes(1));
  expect(await screen.findByText(OBSERVED_UNRESOLVED_TITLE)).toBeInTheDocument();
  expect(screen.queryByText('node.example')).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Send report' })).not.toBeInTheDocument();
});

test('a manifest response that arrives after the dialog was closed does not seed the next open', async () => {
  const user = userEvent.setup();
  const first = deferred<{ status: 'ok'; manifest: CommunityNodeManifest }>();
  const second = deferred<{ status: 'ok'; manifest: CommunityNodeManifest }>();
  const onFetchReportManifest = vi
    .fn()
    .mockReturnValueOnce(first.promise)
    .mockReturnValueOnce(second.promise);

  render(
    <PostCard
      view={observedView()}
      onOpenAuthor={() => undefined}
      onOpenThread={() => undefined}
      onReply={() => undefined}
      onSubmitReport={vi.fn()}
      onFetchReportManifest={onFetchReportManifest}
    />
  );

  await user.click(screen.getByRole('button', { name: 'Report' }));
  await waitFor(() => expect(onFetchReportManifest).toHaveBeenCalledTimes(1));
  await user.click(screen.getByRole('button', { name: 'Cancel' }));
  await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());

  await user.click(screen.getByRole('button', { name: 'Report' }));
  await waitFor(() => expect(onFetchReportManifest).toHaveBeenCalledTimes(2));
  first.resolve({ status: 'ok', manifest: reportManifest('node.example') });
  await new Promise((r) => setTimeout(r, 0));

  expect(screen.getByText('Checking the latest report targets…')).toBeInTheDocument();
  expect(screen.queryByText('node.example')).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Send report' })).not.toBeInTheDocument();
});

// #697: 動画添付そのものを media として、動画のハッシュと親投稿から引き継いだ観測元で通報できる。
test('video attachment report uses the video hash as media subject with the inherited provenance', async () => {
  const user = userEvent.setup();
  const onFetchReportManifest = vi.fn(async (baseUrl: string) => ({
    status: 'ok' as const,
    manifest: reportManifest(new URL(baseUrl).hostname),
  }));
  const onSubmitReport = vi
    .fn()
    .mockResolvedValue({ status: 'submitted', reference_id: 'video-report-1' });

  render(
    <PostCard
      view={createView({
        provenance: {
          canonicalSource: 'author_docs',
          observedVia: [{ nodeBaseUrl: 'https://post-node.example', capability: 'community_index' }],
          responsibleReportTargets: [],
        },
        media: {
          ...createView().media,
          kind: 'video',
          videoPlaybackSrc: 'blob:video-playback',
          videoReportHash: 'video-manifest-hash',
          provenance: {
            canonicalSource: 'blob',
            observedVia: [
              { nodeBaseUrl: 'https://video-node.example', capability: 'community_index' },
            ],
            responsibleReportTargets: [],
          },
        },
      })}
      onOpenAuthor={() => undefined}
      onOpenThread={() => undefined}
      onReply={() => undefined}
      onSubmitReport={onSubmitReport}
      onFetchReportManifest={onFetchReportManifest}
    />
  );

  await user.click(screen.getByRole('button', { name: 'Report video attachment' }));
  const dialog = screen.getByRole('dialog');
  expect(within(dialog).getByText(/^Media/)).toBeInTheDocument();
  await waitFor(() =>
    expect(onFetchReportManifest).toHaveBeenCalledWith('https://video-node.example')
  );
  expect(onFetchReportManifest).not.toHaveBeenCalledWith('https://post-node.example');
  expect(await screen.findByText('video-node.example')).toBeInTheDocument();
  await user.click(await screen.findByRole('button', { name: 'Send report' }));
  await waitFor(() => expect(onSubmitReport).toHaveBeenCalledTimes(1));
  expect(onSubmitReport).toHaveBeenCalledWith(
    expect.objectContaining({
      node_base_url: 'https://video-node.example',
      subject_kind: 'media',
      subject_id: 'video-manifest-hash',
    })
  );
});

test('the post-level report keeps the post subject even when the post carries a video', async () => {
  const user = userEvent.setup();
  const onFetchReportManifest = vi
    .fn()
    .mockResolvedValue({ status: 'ok', manifest: reportManifest('node.example') });
  const onSubmitReport = vi.fn().mockResolvedValue({ status: 'submitted', reference_id: 'r-1' });

  render(
    <PostCard
      view={createView({
        provenance: {
          canonicalSource: 'author_docs',
          observedVia: [{ nodeBaseUrl: 'https://node.example', capability: 'community_index' }],
          responsibleReportTargets: [],
        },
        media: {
          ...createView().media,
          kind: 'video',
          videoPlaybackSrc: 'blob:video-playback',
          videoReportHash: 'video-manifest-hash',
        },
      })}
      onOpenAuthor={() => undefined}
      onOpenThread={() => undefined}
      onReply={() => undefined}
      onSubmitReport={onSubmitReport}
      onFetchReportManifest={onFetchReportManifest}
    />
  );

  // 投稿操作列の通報は投稿全体(post)。動画の通報ボタンとは別に存在する。
  expect(screen.getByRole('button', { name: 'Report video attachment' })).toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Report' }));
  expect(within(screen.getByRole('dialog')).getByText(/^Post/)).toBeInTheDocument();
  await user.click(await screen.findByRole('button', { name: 'Send report' }));
  await waitFor(() => expect(onSubmitReport).toHaveBeenCalledTimes(1));
  expect(onSubmitReport).toHaveBeenCalledWith(
    expect.objectContaining({ subject_kind: 'post', subject_id: createView().post.object_id })
  );
});

test('an explicitly reportable read-only result submits its declared subject kind', async () => {
  const user = userEvent.setup();
  const onFetchReportManifest = vi
    .fn()
    .mockResolvedValue({ status: 'ok', manifest: reportManifest('node.example') });
  const onSubmitReport = vi.fn().mockResolvedValue({ status: 'submitted', reference_id: 'r-2' });

  render(
    <PostCard
      view={createView({
        reportSubjectKind: 'search_result',
        allowReadOnlyReport: true,
        provenance: {
          canonicalSource: 'unknown',
          observedVia: [
            { nodeBaseUrl: 'https://node.example', capability: 'community_index' },
          ],
          responsibleReportTargets: [],
        },
      })}
      readOnly
      onOpenAuthor={() => undefined}
      onOpenThread={() => undefined}
      onReply={() => undefined}
      onSubmitReport={onSubmitReport}
      onFetchReportManifest={onFetchReportManifest}
    />
  );

  await user.click(screen.getByRole('button', { name: 'Report' }));
  await user.click(await screen.findByRole('button', { name: 'Send report' }));
  await waitFor(() => expect(onSubmitReport).toHaveBeenCalledTimes(1));
  expect(onSubmitReport).toHaveBeenCalledWith(
    expect.objectContaining({
      subject_kind: 'search_result',
      subject_id: createView().post.object_id,
      capability: 'community_index',
    })
  );
});

test('read-only cards remain non-reportable unless the view opts in', () => {
  render(
    <PostCard
      view={createView()}
      readOnly
      onOpenAuthor={() => undefined}
      onOpenThread={() => undefined}
      onReply={() => undefined}
      onSubmitReport={vi.fn()}
    />
  );

  expect(screen.queryByRole('button', { name: 'Report' })).not.toBeInTheDocument();
});
