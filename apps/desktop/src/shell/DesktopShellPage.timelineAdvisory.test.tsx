import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, test, vi } from 'vitest';

import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import { App } from '@/App';
import type {
  CommunityNodeContentAdvisoryLookupResult,
  ContentAdvisory,
  DesktopApi,
  PostView,
} from '@/lib/api';

import {
  buildImagePost,
  buildNotification,
  createDeferred,
  getActiveColumn,
  openSettingsSection,
  renderAtHash,
  setViewportWidth,
} from './DesktopShellPage.testHelpers';

// #1056 / ADR 0046 §6.3: タイムライン系の表示経路で、採用 node の content advisory を成人向けゲートへ
// 合成する。照会中はメディアを取得せずスケルトン、確定後は見つけると同じ代替表示 + 説明 + 申し立て。

// 照会の確定後の表示は、起動時の再描画の連鎖と照会の debounce が終わった後に出る。待ち時間は起動全体の
// CPU 量で決まり runner の速さに比例するので、既定の 1 秒ではなくこの上限で待つ。
const LOOKUP_WAIT = { timeout: 10_000 };
const IMAGE_HASH = 'a'.repeat(64);
const NODE_BASE_URL = 'https://api.kukuri.app';
const ISSUER_NODE_ID = 'd'.repeat(64);

function blobAdvisory(overrides?: Partial<ContentAdvisory>): ContentAdvisory {
  return {
    issuer_node_id: ISSUER_NODE_ID,
    subject_kind: 'blob_cid',
    subject_id: IMAGE_HASH,
    category: 'nsfw',
    label: 'adult',
    confidence: 84,
    signal_id: 'signal-timeline-1',
    basis: 'classifier_score',
    ...overrides,
  };
}

function lookupResult(advisories: ContentAdvisory[]): CommunityNodeContentAdvisoryLookupResult {
  return {
    nodes: [{ base_url: NODE_BASE_URL, node_id: ISSUER_NODE_ID, advisories, error: null }],
  };
}

function timelinePost(overrides?: Partial<PostView>): PostView {
  return buildImagePost({
    object_id: 'timeline-image-post',
    content: 'timeline image caption',
    content_status: 'Available',
    root_id: 'timeline-image-post',
    ...overrides,
  });
}

function createTimelineApi(post: PostView): DesktopApi {
  return createDesktopMockApi({ seedPosts: { 'kukuri:topic:general': [post] } });
}

function blobRequests(api: { getBlobMediaPayload: ReturnType<typeof vi.fn> }) {
  return api.getBlobMediaPayload.mock.calls.filter(([hash]) => hash === IMAGE_HASH);
}

beforeEach(() => {
  setViewportWidth(1024);
  window.history.replaceState(null, '', '/');
});

// AC-3 / AC-4 / INVAR-2: advisory 付き投稿は代替表示になり、表示設定 OFF の間 bytes を要求しない。
// #1108 AC-3: 詳細 dialog を開いても bytes を要求しない。
test('advisory-labeled timeline posts stay gated and never request their media', async () => {
  const user = userEvent.setup();
  const api = createTimelineApi(timelinePost());
  const lookup = vi
    .spyOn(api, 'lookupCommunityNodeContentAdvisories')
    .mockResolvedValue(lookupResult([blobAdvisory()]));
  const getBlobMediaPayload = vi.fn(api.getBlobMediaPayload);
  api.getBlobMediaPayload = getBlobMediaPayload;

  render(<App api={api} />);

  const column = getActiveColumn('Timeline');
  const placeholder = await within(column).findByTestId(
    'media-adult-gated-timeline-image-post',
    {},
    LOOKUP_WAIT
  );
  expect(within(column).queryByRole('button', { name: 'Reload post' })).not.toBeInTheDocument();
  expect(within(column).queryByTestId('post-advisory-gated-timeline-image-post')).not.toBeInTheDocument();
  expect(screen.queryByText('timeline image caption')).not.toBeInTheDocument();
  expect(blobRequests({ getBlobMediaPayload })).toHaveLength(0);

  await user.click(placeholder);
  const dialog = await screen.findByTestId('post-advisory-dialog-timeline-image-post');
  expect(within(dialog).getByTestId('post-advisory-gated-timeline-image-post')).toBeInTheDocument();
  expect(within(dialog).getByTestId('post-advisory-appeal-timeline-image-post')).toBeInTheDocument();
  expect(within(dialog).queryByRole('img')).not.toBeInTheDocument();
  expect(screen.queryByText('timeline image caption')).not.toBeInTheDocument();
  expect(blobRequests({ getBlobMediaPayload })).toHaveLength(0);

  // INVAR-1: 照会は可視の post id / blob hash だけを送る。
  const requested = lookup.mock.calls.flatMap(([request]) => [
    ...request.post_ids,
    ...request.blob_hashes,
  ]);
  expect(requested).toContain('timeline-image-post');
  expect(requested).toContain(IMAGE_HASH);
  for (const [request] of lookup.mock.calls) {
    expect(Object.keys(request).sort()).toEqual(['blob_hashes', 'post_ids']);
  }
});

// 照会中はスケルトン(取得しない)、確定後は代替表示。両者の表示を分ける。
test('pending lookups show a skeleton without fetching until the estimate is settled', async () => {
  const api = createTimelineApi(timelinePost());
  const pending = createDeferred<CommunityNodeContentAdvisoryLookupResult>();
  vi.spyOn(api, 'lookupCommunityNodeContentAdvisories').mockReturnValue(pending.promise);
  const getBlobMediaPayload = vi.fn(api.getBlobMediaPayload);
  api.getBlobMediaPayload = getBlobMediaPayload;

  render(<App api={api} />);

  const column = getActiveColumn('Timeline');
  expect(
    await within(column).findByTestId('media-advisory-pending-timeline-image-post')
  ).toBeInTheDocument();
  expect(within(column).queryByTestId('media-adult-gated-timeline-image-post')).not.toBeInTheDocument();
  // 本文は bytes 取得を伴わないため照会中も表示する。
  expect(within(column).getByText('timeline image caption')).toBeInTheDocument();
  await new Promise((resolve) => setTimeout(resolve, 500));
  expect(blobRequests({ getBlobMediaPayload })).toHaveLength(0);

  pending.resolve(lookupResult([blobAdvisory()]));

  expect(
    await within(column).findByTestId('media-adult-gated-timeline-image-post')
  ).toBeInTheDocument();
  expect(within(column).queryByTestId('media-advisory-pending-timeline-image-post')).not.toBeInTheDocument();
  expect(blobRequests({ getBlobMediaPayload })).toHaveLength(0);
});

// TR-12: 照会に失敗したら advisory 無しとして通常表示へ戻る(fail-open)。
test('a failed lookup settles to the normal display and resumes fetching', async () => {
  const api = createTimelineApi(timelinePost());
  vi.spyOn(api, 'lookupCommunityNodeContentAdvisories').mockRejectedValue(
    new Error('node unreachable')
  );
  const getBlobMediaPayload = vi.fn(api.getBlobMediaPayload);
  api.getBlobMediaPayload = getBlobMediaPayload;

  render(<App api={api} />);

  await waitFor(() => {
    expect(blobRequests({ getBlobMediaPayload }).length).toBeGreaterThan(0);
  });
  expect(screen.queryByTestId('media-advisory-pending-timeline-image-post')).not.toBeInTheDocument();
  expect(screen.queryByTestId('media-adult-gated-timeline-image-post')).not.toBeInTheDocument();
});

// AC-4: 返信プレビューの親投稿(post_id)への advisory でも、カード全体を代替表示にする。
test('an advisory on a reply preview parent gates the enclosing card', async () => {
  const post = timelinePost({
    object_id: 'advisory-reply-host',
    content: 'safe-looking reply body',
    attachments: [],
    reply_to: 'advisory-reply-parent',
    reply_preview: {
      object_id: 'advisory-reply-parent',
      topic: 'kukuri:topic:general',
      author: { pubkey: 'b'.repeat(64), name: 'parent', display_name: null, picture_asset: null },
      content: 'advisory reply preview body',
      content_status: 'Available',
      attachments: [],
      content_labels: [],
      root_id: 'advisory-reply-parent',
      reply_to: null,
    },
  });
  const api = createTimelineApi(post);
  vi.spyOn(api, 'lookupCommunityNodeContentAdvisories').mockResolvedValue(
    lookupResult([
      blobAdvisory({
        subject_kind: 'post_id',
        subject_id: 'advisory-reply-parent',
        category: 'objectionable',
        label: 'sensitive',
      }),
    ])
  );

  render(<App api={api} />);

  // #1108 AC-4: メディア枠が無い投稿は、本文欄に詳細を開く操作だけを出す。
  const trigger = await within(getActiveColumn('Timeline')).findByTestId(
    'post-advisory-details-trigger-advisory-reply-host'
  );
  expect(screen.queryByTestId('post-advisory-gated-advisory-reply-host')).not.toBeInTheDocument();
  await userEvent.setup().click(trigger);
  expect(await screen.findByTestId('post-advisory-gated-advisory-reply-host')).toBeInTheDocument();
  expect(screen.queryByText('safe-looking reply body')).not.toBeInTheDocument();
  expect(screen.queryByText('advisory reply preview body')).not.toBeInTheDocument();
});

// AC-4: 申し立ては発行元の risk signal を対象に既存の通報ダイアログで始める。
test('the appeal action opens the report dialog for the issuing signal', async () => {
  const user = userEvent.setup();
  const api = createTimelineApi(timelinePost());
  vi.spyOn(api, 'lookupCommunityNodeContentAdvisories').mockResolvedValue(
    lookupResult([blobAdvisory()])
  );

  render(<App api={api} />);

  await user.click(
    await within(getActiveColumn('Timeline')).findByTestId('media-adult-gated-timeline-image-post')
  );
  await user.click(await screen.findByTestId('post-advisory-appeal-timeline-image-post'));
  expect(await screen.findByRole('dialog')).toBeInTheDocument();
});

// TR-3 / TR-7: 採用 node が無ければ照会せず、通常どおり取得する。
test('nodes with adoption disabled are never asked and media loads normally', async () => {
  const api = createTimelineApi(timelinePost());
  await api.setCommunityNodeConfig([
    { base_url: NODE_BASE_URL, content_advisory_enabled: false },
  ]);
  const lookup = vi.spyOn(api, 'lookupCommunityNodeContentAdvisories');
  const getBlobMediaPayload = vi.fn(api.getBlobMediaPayload);
  api.getBlobMediaPayload = getBlobMediaPayload;

  render(<App api={api} />);

  await waitFor(() => {
    expect(blobRequests({ getBlobMediaPayload }).length).toBeGreaterThan(0);
  });
  expect(lookup).not.toHaveBeenCalled();
  expect(screen.queryByTestId('media-advisory-pending-timeline-image-post')).not.toBeInTheDocument();
});

// TR-5: 表示設定 ON で advisory 付きメディアを取得し、OFF へ戻すと取得を止めて代替表示へ戻す。
test('enabling adult display renders advisory timeline media and disabling clears it again', async () => {
  const user = userEvent.setup();
  const api = createTimelineApi(timelinePost());
  vi.spyOn(api, 'lookupCommunityNodeContentAdvisories').mockResolvedValue(
    lookupResult([blobAdvisory()])
  );
  const getBlobMediaPayload = vi.fn(api.getBlobMediaPayload);
  api.getBlobMediaPayload = getBlobMediaPayload;
  const setAdultContentDisplayEnabled = vi.fn(api.setAdultContentDisplayEnabled);
  api.setAdultContentDisplayEnabled = setAdultContentDisplayEnabled;

  render(<App api={api} />);
  expect(
    await within(getActiveColumn('Timeline')).findByTestId('media-adult-gated-timeline-image-post')
  ).toBeInTheDocument();
  expect(blobRequests({ getBlobMediaPayload })).toHaveLength(0);

  await openSettingsSection(user, 'safety');
  await user.click(screen.getByTestId('adult-content-display-toggle'));
  await waitFor(() => {
    expect(setAdultContentDisplayEnabled).toHaveBeenCalledWith(true);
  });
  await waitFor(() => {
    expect(blobRequests({ getBlobMediaPayload }).length).toBeGreaterThan(0);
  });
  await waitFor(() => {
    expect(screen.queryByTestId('media-adult-gated-timeline-image-post')).not.toBeInTheDocument();
  });

  const callsBeforeDisable = blobRequests({ getBlobMediaPayload }).length;
  await user.click(screen.getByTestId('adult-content-display-toggle'));
  await waitFor(() => {
    expect(setAdultContentDisplayEnabled).toHaveBeenCalledWith(false);
  });
  expect(
    await within(getActiveColumn('Timeline')).findByTestId('media-adult-gated-timeline-image-post')
  ).toBeInTheDocument();
  expect(blobRequests({ getBlobMediaPayload })).toHaveLength(callsBeforeDisable);
});

// AC-4: object-backed 通知でも、対象投稿に advisory があればプレビューを伏せる。
test('object notifications hide their preview when the target has an advisory', async () => {
  const api = createDesktopMockApi({
    notifications: [
      buildNotification({
        notification_id: 'notification-advisory-preview',
        object_id: 'advisory-notification-target',
        preview_text: 'advisory notification raw preview',
        content_labels: [],
      }),
    ],
  });
  vi.spyOn(api, 'lookupCommunityNodeContentAdvisories').mockResolvedValue(
    lookupResult([
      blobAdvisory({ subject_kind: 'post_id', subject_id: 'advisory-notification-target' }),
    ])
  );

  renderAtHash('#/notifications?topic=kukuri%3Atopic%3Ageneral', api);

  expect(
    await screen.findByText(/Community Node you configured estimates this post/, {}, LOOKUP_WAIT)
  ).toBeInTheDocument();
  expect(screen.queryByText('advisory notification raw preview')).not.toBeInTheDocument();
});
