import { act, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';

import type {
  AuthorTrustGateResult,
  CommunityNodeConfig,
  CommunityNodeNodeStatus,
  PostView,
} from '@/lib/api';
import { createShellHookHarness } from '@/shell/testSupport/renderShellHook';

import {
  AUTHOR_TRUST_GATE_LOOKUP_DEBOUNCE_MS,
  AUTHOR_TRUST_GATE_LOOKUP_STALE_GRACE_MS,
  AUTHOR_TRUST_GATE_LOOKUP_SWEEP_MS,
  useAuthorTrustGateLookup,
} from './useAuthorTrustGateLookup';

// #1061 / ADR 0026 §8.4: 判断は評価の期限まで使い、期限切れ・同意取消では折りたたみを続けない。

const NODE = 'https://node.example';
const AUTHOR = 'b'.repeat(64);

function config(priority: string[] = [NODE]): CommunityNodeConfig {
  return { nodes: [{ base_url: NODE }], trust_node_priority: priority };
}

function status(overrides: Partial<CommunityNodeNodeStatus> = {}): CommunityNodeNodeStatus {
  return {
    base_url: NODE,
    auth_state: { authenticated: true, expires_at: null },
    consent_state: { all_required_accepted: true, items: [] },
    last_error: null,
    ...overrides,
  } as CommunityNodeNodeStatus;
}

function post(authorPubkey = AUTHOR): PostView {
  return { object_id: 'post-1', author_pubkey: authorPubkey } as unknown as PostView;
}

function hiddenGate(expiresAt: string | null): AuthorTrustGateResult {
  return {
    gates: [
      {
        author_pubkey: AUTHOR,
        hidden: true,
        node_base_url: NODE,
        reasons: ['related_users_block_or_mute'],
        expires_at: expiresAt,
        always_visible: false,
      },
    ],
  };
}

function visibleGate(expiresAt: string | null): AuthorTrustGateResult {
  return {
    gates: [
      {
        author_pubkey: AUTHOR,
        hidden: false,
        node_base_url: NODE,
        reasons: [],
        expires_at: expiresAt,
        always_visible: false,
      },
    ],
  };
}

type MountProps = {
  config: CommunityNodeConfig;
  posts: PostView[];
  statuses: CommunityNodeNodeStatus[];
  statusesLoaded: boolean;
};

function mount(result: () => AuthorTrustGateResult, initial?: Partial<MountProps>) {
  const harness = createShellHookHarness();
  const api = { evaluateAuthorTrustGates: vi.fn(async () => result()) };
  const hook = renderHook(
    (props: MountProps) =>
      useAuthorTrustGateLookup({
        api,
        posts: props.posts,
        config: props.config,
        statuses: props.statuses,
        statusesLoaded: props.statusesLoaded,
      }),
    {
      wrapper: harness.wrapper,
      initialProps: {
        config: config(),
        posts: [post()],
        statuses: [status()],
        statusesLoaded: true,
        ...initial,
      },
    }
  );
  return { harness, api, hook };
}

async function advance(ms: number) {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
}

beforeEach(() => {
  vi.useFakeTimers();
});
afterEach(() => {
  vi.useRealTimers();
});

test('no node is queried without an adopted priority', async () => {
  const { harness, api } = mount(() => hiddenGate(null), { config: config([]) });

  await advance(AUTHOR_TRUST_GATE_LOOKUP_DEBOUNCE_MS * 2);

  expect(api.evaluateAuthorTrustGates).not.toHaveBeenCalled();
  expect(harness.store.getState().authorTrustGates).toEqual({});
});

test('an expired evaluation is looked up again without opening the post first', async () => {
  const expired = new Date(Date.now() - 1_000).toISOString();
  const fresh = new Date(Date.now() + 600_000).toISOString();
  let expiresAt = expired;
  let hidden = true;
  const { harness, api } = mount(() => (hidden ? hiddenGate(expiresAt) : visibleGate(fresh)));

  await advance(AUTHOR_TRUST_GATE_LOOKUP_DEBOUNCE_MS);
  expect(harness.store.getState().authorTrustGates[AUTHOR]?.hidden).toBe(true);

  // 期限を過ぎたら照会し直す（応答を待つあいだの扱いは猶予の test で確認する）。
  expiresAt = fresh;
  await advance(AUTHOR_TRUST_GATE_LOOKUP_SWEEP_MS);
  expect(harness.store.getState().authorTrustGates[AUTHOR]?.hidden).toBe(true);
  await advance(AUTHOR_TRUST_GATE_LOOKUP_DEBOUNCE_MS);
  expect(api.evaluateAuthorTrustGates).toHaveBeenCalledTimes(2);

  // 期限内の判断は作り直さない。
  await advance(AUTHOR_TRUST_GATE_LOOKUP_SWEEP_MS * 2);
  expect(api.evaluateAuthorTrustGates).toHaveBeenCalledTimes(2);

  // 評価が変わったら、次の作り直しで折りたたみをやめる。
  hidden = false;
  expiresAt = new Date(Date.now() - 1_000).toISOString();
  harness.store.getState().setField('authorTrustGates', (current) => ({
    ...current,
    [AUTHOR]: { ...current[AUTHOR], expires_at: expiresAt },
  }));
  await advance(600_000 + AUTHOR_TRUST_GATE_LOOKUP_DEBOUNCE_MS);
  expect(harness.store.getState().authorTrustGates[AUTHOR]?.hidden).toBe(false);
});

test('a failed lookup stops collapsing the post (fail-open)', async () => {
  const fresh = new Date(Date.now() + 600_000).toISOString();
  const { harness, api } = mount(() => hiddenGate(fresh));

  await advance(AUTHOR_TRUST_GATE_LOOKUP_DEBOUNCE_MS);
  expect(harness.store.getState().authorTrustGates[AUTHOR]?.hidden).toBe(true);

  api.evaluateAuthorTrustGates.mockRejectedValue(new Error('unreachable'));
  await advance(600_000 + AUTHOR_TRUST_GATE_LOOKUP_DEBOUNCE_MS);
  expect(harness.store.getState().authorTrustGates).toEqual({});
});

test('a lookup that never answers stops collapsing the post after the grace period', async () => {
  const fresh = new Date(Date.now() + 600_000).toISOString();
  const { harness, api } = mount(() => hiddenGate(fresh));

  await advance(AUTHOR_TRUST_GATE_LOOKUP_DEBOUNCE_MS);
  expect(harness.store.getState().authorTrustGates[AUTHOR]?.hidden).toBe(true);

  // 応答が返らない CN。期限を過ぎた判断は猶予のあいだだけ使う。
  api.evaluateAuthorTrustGates.mockImplementation(
    () => new Promise<AuthorTrustGateResult>(() => {})
  );
  await advance(600_000 + AUTHOR_TRUST_GATE_LOOKUP_DEBOUNCE_MS);
  expect(harness.store.getState().authorTrustGates[AUTHOR]?.hidden).toBe(true);

  // 猶予を過ぎたら捨てる（古い判断で折りたたみ続けない）。
  await advance(AUTHOR_TRUST_GATE_LOOKUP_STALE_GRACE_MS + AUTHOR_TRUST_GATE_LOOKUP_SWEEP_MS);
  expect(harness.store.getState().authorTrustGates).toEqual({});
});

test('nothing is queried until the node statuses are loaded', async () => {
  const fresh = new Date(Date.now() + 600_000).toISOString();
  const { api, hook } = mount(() => hiddenGate(fresh), { statusesLoaded: false });

  await advance(AUTHOR_TRUST_GATE_LOOKUP_DEBOUNCE_MS * 2);
  expect(api.evaluateAuthorTrustGates).not.toHaveBeenCalled();

  hook.rerender({
    config: config(),
    posts: [post()],
    statuses: [status()],
    statusesLoaded: true,
  });
  await advance(AUTHOR_TRUST_GATE_LOOKUP_DEBOUNCE_MS);
  expect(api.evaluateAuthorTrustGates).toHaveBeenCalledTimes(1);
});

test('withdrawing the required consents drops the collapse and stops using it', async () => {
  const fresh = new Date(Date.now() + 600_000).toISOString();
  const { harness, api, hook } = mount(() => hiddenGate(fresh));

  await advance(AUTHOR_TRUST_GATE_LOOKUP_DEBOUNCE_MS);
  expect(harness.store.getState().authorTrustGates[AUTHOR]?.hidden).toBe(true);

  api.evaluateAuthorTrustGates.mockImplementation(async () => ({ gates: [] }));
  hook.rerender({
    config: config(),
    posts: [post()],
    statuses: [status({ consent_state: { all_required_accepted: false, items: [] } })],
    statusesLoaded: true,
  });

  // 同意が外れた時点で古い判断を捨てる(照会の応答を待たずに折りたたみをやめる)。
  expect(harness.store.getState().authorTrustGates).toEqual({});
  await advance(AUTHOR_TRUST_GATE_LOOKUP_DEBOUNCE_MS);
  expect(harness.store.getState().authorTrustGates).toEqual({});
});

test('a re-render during the debounce does not drop the queued lookup', async () => {
  const fresh = new Date(Date.now() + 600_000).toISOString();
  const { api, hook } = mount(() => hiddenGate(fresh));

  // 待ち時間の途中で表示が更新されると、以前は timer が破棄されたまま再設定されなかった。
  await advance(AUTHOR_TRUST_GATE_LOOKUP_DEBOUNCE_MS / 2);
  hook.rerender({ config: config(), posts: [post()], statuses: [status()], statusesLoaded: true });
  await advance(AUTHOR_TRUST_GATE_LOOKUP_DEBOUNCE_MS * 2);

  expect(api.evaluateAuthorTrustGates).toHaveBeenCalledTimes(1);
});

test('a previous display visit cannot restore its gate after the author returns', async () => {
  const resolves: Array<(value: AuthorTrustGateResult) => void> = [];
  const { harness, api, hook } = mount(() => hiddenGate(null));
  api.evaluateAuthorTrustGates.mockImplementation(() =>
    new Promise<AuthorTrustGateResult>((done) => { resolves.push(done); }));
  await advance(AUTHOR_TRUST_GATE_LOOKUP_DEBOUNCE_MS);

  hook.rerender({ config: config(), posts: [], statuses: [status()], statusesLoaded: true });
  expect(harness.store.getState().authorTrustGates).toEqual({});
  hook.rerender({ config: config(), posts: [post()], statuses: [status()], statusesLoaded: true });
  await advance(AUTHOR_TRUST_GATE_LOOKUP_DEBOUNCE_MS);
  expect(api.evaluateAuthorTrustGates).toHaveBeenCalledTimes(2);

  await act(async () => { resolves[0](hiddenGate(null)); });
  expect(harness.store.getState().authorTrustGates).toEqual({});
  await act(async () => { resolves[1](visibleGate(null)); });
  expect(harness.store.getState().authorTrustGates[AUTHOR]?.hidden).toBe(false);
});
