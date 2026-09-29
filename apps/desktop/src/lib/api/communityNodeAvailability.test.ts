import { beforeEach, describe, expect, test } from 'vitest';
import type { CommunityNodeNodeStatus } from './types';
import { communityIndexAvailability, firstUnconsentedCommunityNode, reconsentPendingCommunityNodes } from './communityNodeAvailability';
import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import type { CommunityIndexManifestEntry, CommunityIndexNodePreference } from './communityIndex';

const A = 'https://first.example';
const B = 'https://second.example';
let ready: CommunityNodeNodeStatus;
let pending: CommunityNodeNodeStatus;
let manifests: Record<string, CommunityIndexManifestEntry>;
const config = { nodes: [{ base_url: A }, { base_url: B }] };

beforeEach(async () => {
  const api = createDesktopMockApi();
  await api.setCommunityNodeConfig(config.nodes);
  pending = (await api.getCommunityNodeStatuses())[0];
  ready = await api.acceptCommunityNodeConsents(A, (await api.fetchCommunityNodePolicies(A)).policies, 'en');
  manifests = { [A]: { status: 'ok', manifest: (await api.fetchCommunityNodeManifest(A)).manifest! } };
});

describe('first community node explanation', () => {
  test('uses configured index zero even when status order differs or the node has no index', () => {
    expect(firstUnconsentedCommunityNode(config, [{ ...pending, base_url: B }, pending], true)).toBe(A);
    expect(firstUnconsentedCommunityNode({ nodes: [...config.nodes].reverse() }, [pending, { ...pending, base_url: B }], true)).toBe(B);
  });
  test('waits for every local consent and does not confuse an unavailable accepted node with no consent', () => {
    const second = { ...pending, base_url: B };
    expect(firstUnconsentedCommunityNode(config, [pending], true)).toBeNull();
    expect(firstUnconsentedCommunityNode(config, [pending, second], false)).toBeNull();
    expect(firstUnconsentedCommunityNode({ nodes: [] }, [pending], true)).toBeNull();
    expect(firstUnconsentedCommunityNode(config, [{ ...ready, last_error: 'offline', consent_update_pending: true }, second], true)).toBeNull();
    expect(firstUnconsentedCommunityNode(config, [{ ...pending, local_consent: undefined }, second], true)).toBeNull();
    expect(firstUnconsentedCommunityNode(config, [pending, { ...ready, base_url: 'https://removed.example' }, second], true)).toBe(A);
  });
  test('withdrawn records are not active consent; a stored token is not consent', () => {
    expect(firstUnconsentedCommunityNode({ nodes: [{ base_url: A }] }, [{
      ...ready, local_consent: { ...ready.local_consent!, withdrawn_at: 42 },
    }], true)).toBe(A);
    expect(firstUnconsentedCommunityNode({ nodes: [{ base_url: A }] }, [{ ...pending, auth_state: ready.auth_state }], true)).toBe(A);
  });
});

// #1420: 再同意の自動提示は同意済みNodeだけ。未同意・撤回済みは初回案内・手動の同意の対象。
test('policy updates are prompted only for nodes with active consent, in list order', () => {
  const updated = { ...ready, consent_update_pending: true };
  expect(reconsentPendingCommunityNodes(config, [{ ...updated, base_url: B }, updated])).toEqual([A, B]);
  expect(reconsentPendingCommunityNodes(config, [
    { ...pending, consent_update_pending: true },
    { ...updated, base_url: B, local_consent: { ...ready.local_consent!, withdrawn_at: 42 } },
  ])).toEqual([]);
  expect(reconsentPendingCommunityNodes({ nodes: [{ base_url: A }] }, [ready, { ...updated, base_url: B }])).toEqual([]);
});

function availability(status: CommunityNodeNodeStatus, preference: CommunityIndexNodePreference = { mode: 'auto' }) {
  return communityIndexAvailability({ config, statuses: [status], manifests, preference,
    configLoaded: true, statusesLoaded: true, statusError: false });
}

describe('index availability reasons', () => {
  test('keeps automatic and manual state separate without sending or selecting another node', () => {
    expect(availability(ready)).toMatchObject({ reason: 'ready', baseUrl: A, manual: false });
    expect(availability(pending, { mode: 'manual', baseUrl: B })).toMatchObject({
      reason: 'checking', baseUrl: B, manual: true,
    });
  });
  test.each([
    [{}, 'consentRequired', 'consent'],
    [{ consent_update_pending: true }, 'reconsentRequired', 'consent'],
  ] as const)('distinguishes missing and updated consent %j', (patch, reason, recovery) => {
    expect(availability({ ...pending, ...patch })).toMatchObject({ reason, recovery });
  });
  test.each(['connecting', 'authenticating', 'accepting', 'refreshing'] as const)(
    'shows %s as preparation rather than failure', (session_phase) => {
      expect(availability({ ...ready, auth_state: pending.auth_state, session_phase })).toMatchObject({ reason: 'connecting' });
    }
  );
  test('shows saved consent while retrying and preserves the server deadline', () => {
    expect(availability({ ...ready, last_error: 'offline', session_phase: 'retrying', retry_after: 12345 })).toMatchObject({
      reason: 'retrying', retryAfter: 12345, recovery: 'metadata',
    });
  });
  test.each(['INVITE_REQUIRED', 'INVITE_EXPIRED', 'BANNED', 'NOT_ALLOWLISTED'] as const)(
    'classifies admission %s without treating it as consent', (code) => {
      expect(availability({ ...ready, auth_state: pending.auth_state,
        admission_rejection: { code, message: 'private server diagnostic' }, session_phase: 'awaiting_admission',
      }).reason).toBe(code.startsWith('INVITE_') ? 'inviteRequired' : 'admissionDenied');
    }
  );
  test.each(['loading', 'absent', 'error'] as const)('reports manifest %s', (status) => {
    manifests[A] = status === 'error' ? { status, error: 'offline' } : { status };
    expect(availability(ready).reason).toBe({ loading: 'checking', absent: 'manifestAbsent', error: 'manifestError' }[status]);
  });
  test('does not promise search when the node only assists connectivity', () => {
    const entry = manifests[A];
    if (entry.status !== 'ok') throw new Error('fixture');
    entry.manifest.capability_scope.available_enabled = ['iroh_relay'];
    expect(availability(ready)).toMatchObject({ reason: 'indexNotProvided', recovery: 'settings' });
  });
  test('does not present unknown or failed local status as empty results or no nodes', () => {
    expect(communityIndexAvailability({ config: { nodes: [] }, statuses: [], manifests: {}, preference: { mode: 'auto' },
      configLoaded: false, statusesLoaded: false, statusError: false }).reason).toBe('checking');
    expect(communityIndexAvailability({ config, statuses: [ready], manifests, preference: { mode: 'auto' },
      configLoaded: true, statusesLoaded: true, statusError: true }).reason).toBe('statusUnavailable');
  });
});
