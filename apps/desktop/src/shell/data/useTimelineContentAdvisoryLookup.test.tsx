import { act, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, test, vi } from 'vitest';

import type {
  CommunityNodeConfig,
  CommunityNodeContentAdvisoryLookupRequest,
  CommunityNodeContentAdvisoryLookupResult,
  CommunityNodeNodeStatus,
  PostView,
} from '@/lib/api';
import { createShellHookHarness } from '@/shell/testSupport/renderShellHook';

import {
  TIMELINE_ADVISORY_LOOKUP_DEBOUNCE_MS,
  TIMELINE_ADVISORY_LOOKUP_SETTLE_TIMEOUT_MS,
  adoptingContentAdvisoryNodes,
  useTimelineContentAdvisoryLookup,
  type AdoptingContentAdvisoryNodes,
} from './useTimelineContentAdvisoryLookup';

const HASH = 'a'.repeat(64);
const NODE = 'https://node.example';
const ADOPTING: AdoptingContentAdvisoryNodes = { baseUrls: [NODE], undetermined: false };

function status(overrides: Partial<CommunityNodeNodeStatus> = {}): CommunityNodeNodeStatus {
  return {
    base_url: NODE,
    auth_state: { authenticated: true, expires_at: null },
    consent_state: { all_required_accepted: true, items: [] },
    last_error: null,
    ...overrides,
  } as CommunityNodeNodeStatus;
}

function post(id: string, withMedia = true): PostView {
  return {
    object_id: id,
    attachments: withMedia
      ? [{ hash: HASH, mime: 'image/png', bytes: 1, role: 'image_original', status: 'Available' }]
      : [],
  } as unknown as PostView;
}

describe('adoptingContentAdvisoryNodes', () => {
  const config: CommunityNodeConfig = { nodes: [{ base_url: NODE }] };

  test('is undetermined until the config and statuses are loaded', () => {
    expect(
      adoptingContentAdvisoryNodes(config, [], { configLoaded: false, statusesLoaded: false })
    ).toEqual({ baseUrls: [], undetermined: true });
    expect(
      adoptingContentAdvisoryNodes(config, [], { configLoaded: true, statusesLoaded: false })
    ).toEqual({ baseUrls: [], undetermined: true });
  });

  test('adopts authenticated, consented nodes unless adoption is disabled', () => {
    expect(adoptingContentAdvisoryNodes(config, [status()])).toEqual(ADOPTING);
    expect(
      adoptingContentAdvisoryNodes(
        { nodes: [{ base_url: NODE, content_advisory_enabled: false }] },
        [status()]
      )
    ).toEqual({ baseUrls: [], undetermined: false });
    expect(
      adoptingContentAdvisoryNodes(config, [
        status({ consent_state: { all_required_accepted: false, items: [] } }),
      ])
    ).toEqual({ baseUrls: [], undetermined: false });
    expect(adoptingContentAdvisoryNodes({ nodes: [] }, [])).toEqual({
      baseUrls: [],
      undetermined: false,
    });
  });
});

type Lookup = (
  request: CommunityNodeContentAdvisoryLookupRequest
) => Promise<CommunityNodeContentAdvisoryLookupResult>;

describe('useTimelineContentAdvisoryLookup', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  function mount(nodes: AdoptingContentAdvisoryNodes, lookup: Lookup, posts = [post('post-1')]) {
    const harness = createShellHookHarness();
    const api = { lookupCommunityNodeContentAdvisories: vi.fn(lookup) };
    const hook = renderHook(
      (props: { nodes: AdoptingContentAdvisoryNodes; posts: PostView[] }) =>
        useTimelineContentAdvisoryLookup({
          api,
          posts: props.posts,
          notifications: [],
          adoptingNodes: props.nodes,
        }),
      { wrapper: harness.wrapper, initialProps: { nodes, posts } }
    );
    return { harness, api, hook };
  }

  async function advance(ms: number) {
    await act(async () => {
      await vi.advanceTimersByTimeAsync(ms);
    });
  }

  test('does not look up and is inactive without adopting nodes', async () => {
    const { harness, api } = mount({ baseUrls: [], undetermined: false }, async () => ({
      nodes: [],
    }));
    await advance(TIMELINE_ADVISORY_LOOKUP_DEBOUNCE_MS * 2);
    expect(api.lookupCommunityNodeContentAdvisories).not.toHaveBeenCalled();
    expect(harness.store.getState().timelineAdvisoryLookup.active).toBe(false);
  });

  test('stays active without sending while adopting nodes are undetermined', async () => {
    const { harness, api } = mount({ baseUrls: [], undetermined: true }, async () => ({
      nodes: [],
    }));
    await advance(TIMELINE_ADVISORY_LOOKUP_DEBOUNCE_MS * 2);
    expect(api.lookupCommunityNodeContentAdvisories).not.toHaveBeenCalled();
    expect(harness.store.getState().timelineAdvisoryLookup).toEqual({
      active: true,
      settled: {},
    });
  });

  test('batches visible identifiers once and records advisories and settled subjects', async () => {
    const { harness, api, hook } = mount(ADOPTING, async () => ({
      nodes: [
        {
          base_url: NODE,
          node_id: 'd'.repeat(64),
          error: null,
          advisories: [
            {
              issuer_node_id: 'd'.repeat(64),
              subject_kind: 'blob_cid',
              subject_id: HASH,
              category: 'nsfw',
              label: 'adult',
              confidence: 84,
              signal_id: 'signal-1',
              basis: 'classifier_score',
            },
          ],
        },
      ],
    }));
    await advance(TIMELINE_ADVISORY_LOOKUP_DEBOUNCE_MS);
    expect(api.lookupCommunityNodeContentAdvisories).toHaveBeenCalledTimes(1);
    expect(api.lookupCommunityNodeContentAdvisories).toHaveBeenCalledWith({
      post_ids: ['post-1'],
      blob_hashes: [HASH],
    });
    const state = harness.store.getState();
    expect(state.timelineContentAdvisories[`blob_cid:${HASH}`]).toHaveLength(1);
    expect(state.timelineAdvisoryLookup.settled).toEqual({
      'post_id:post-1': true,
      [`blob_cid:${HASH}`]: true,
    });

    // 同じ subject は再送しない。新しい投稿の分だけ送る。
    hook.rerender({ nodes: ADOPTING, posts: [post('post-1'), post('post-2', false)] });
    await advance(TIMELINE_ADVISORY_LOOKUP_DEBOUNCE_MS);
    expect(api.lookupCommunityNodeContentAdvisories).toHaveBeenCalledTimes(2);
    expect(api.lookupCommunityNodeContentAdvisories).toHaveBeenLastCalledWith({
      post_ids: ['post-2'],
      blob_hashes: [],
    });
  });

  test('settles subjects when the lookup fails', async () => {
    const { harness } = mount(ADOPTING, async () => {
      throw new Error('unreachable');
    });
    await advance(TIMELINE_ADVISORY_LOOKUP_DEBOUNCE_MS);
    expect(harness.store.getState().timelineAdvisoryLookup.settled['post_id:post-1']).toBe(true);
  });

  test('settles subjects when the lookup never answers', async () => {
    const { harness } = mount(ADOPTING, () => new Promise(() => {}));
    await advance(TIMELINE_ADVISORY_LOOKUP_DEBOUNCE_MS);
    expect(
      harness.store.getState().timelineAdvisoryLookup.settled['post_id:post-1']
    ).toBeUndefined();
    await advance(TIMELINE_ADVISORY_LOOKUP_SETTLE_TIMEOUT_MS);
    expect(harness.store.getState().timelineAdvisoryLookup.settled['post_id:post-1']).toBe(true);
  });

  test('discards results and asks again when the adopting nodes change', async () => {
    const { harness, api, hook } = mount(ADOPTING, async () => ({ nodes: [] }));
    await advance(TIMELINE_ADVISORY_LOOKUP_DEBOUNCE_MS);
    expect(api.lookupCommunityNodeContentAdvisories).toHaveBeenCalledTimes(1);

    hook.rerender({ nodes: { baseUrls: [], undetermined: false }, posts: [post('post-1')] });
    await advance(TIMELINE_ADVISORY_LOOKUP_DEBOUNCE_MS);
    expect(harness.store.getState().timelineAdvisoryLookup).toEqual({
      active: false,
      settled: {},
    });

    hook.rerender({
      nodes: { baseUrls: [NODE, 'https://other.example'], undetermined: false },
      posts: [post('post-1')],
    });
    await advance(TIMELINE_ADVISORY_LOOKUP_DEBOUNCE_MS);
    expect(api.lookupCommunityNodeContentAdvisories).toHaveBeenCalledTimes(2);
  });

  // 監査 non-blocker 2: active のまま採用 node の組だけが変わっても、同じ投稿を照会し直す。
  test('asks again when only the set of adopting nodes changes', async () => {
    // 投稿配列は同じ参照のまま(照会対象は変わらない)。
    const posts = [post('post-1')];
    const { harness, api, hook } = mount(ADOPTING, async () => ({ nodes: [] }), posts);
    await advance(TIMELINE_ADVISORY_LOOKUP_DEBOUNCE_MS);
    expect(api.lookupCommunityNodeContentAdvisories).toHaveBeenCalledTimes(1);

    hook.rerender({
      nodes: { baseUrls: [NODE, 'https://other.example'], undetermined: false },
      posts,
    });
    await advance(TIMELINE_ADVISORY_LOOKUP_DEBOUNCE_MS);
    expect(api.lookupCommunityNodeContentAdvisories).toHaveBeenCalledTimes(2);
    expect(harness.store.getState().timelineAdvisoryLookup.settled['post_id:post-1']).toBe(true);
  });

  test('releases subjects outside the display window and ignores their late response', async () => {
    let resolve!: (value: CommunityNodeContentAdvisoryLookupResult) => void;
    const response = new Promise<CommunityNodeContentAdvisoryLookupResult>((done) => { resolve = done; });
    const { harness, api, hook } = mount(ADOPTING, () => response);
    await advance(TIMELINE_ADVISORY_LOOKUP_DEBOUNCE_MS);

    hook.rerender({ nodes: ADOPTING, posts: [] });
    expect(harness.store.getState().timelineAdvisoryLookup.settled).toEqual({});
    await act(async () => {
      resolve({ nodes: [{
        base_url: NODE,
        node_id: 'd'.repeat(64),
        error: null,
        advisories: [{
          issuer_node_id: 'd'.repeat(64), subject_kind: 'blob_cid', subject_id: HASH,
          category: 'nsfw', label: 'adult', confidence: 84,
          signal_id: 'signal-1', basis: 'classifier_score',
        }],
      }] });
    });
    expect(harness.store.getState().timelineContentAdvisories).toEqual({});
    expect(harness.store.getState().timelineAdvisoryLookup.settled).toEqual({});

    hook.rerender({ nodes: ADOPTING, posts: [post('post-1')] });
    await advance(TIMELINE_ADVISORY_LOOKUP_DEBOUNCE_MS);
    expect(api.lookupCommunityNodeContentAdvisories).toHaveBeenCalledTimes(2);
    expect(harness.store.getState().timelineContentAdvisories[`blob_cid:${HASH}`]).toHaveLength(1);
  });
});
