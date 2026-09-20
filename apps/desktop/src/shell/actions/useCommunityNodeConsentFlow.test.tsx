import { act, renderHook, waitFor } from '@testing-library/react';
import { expect, test, vi } from 'vitest';
import i18n from '@/i18n';
import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import { useCommunityNodeConsentFlow } from './useCommunityNodeConsentFlow';
import { createDeferred } from '@/shell/DesktopShellPage.testHelpers';
import type { CommunityNodePoliciesResponse } from '@/lib/api';

const A = 'https://first.example';
const B = 'https://second.example';

async function setup() {
  const api = createDesktopMockApi();
  await api.setCommunityNodeConfig([{ base_url: A }, { base_url: B }]);
  return api;
}

test('policy failure retries within the same node and closing never accepts', async () => {
  const api = await setup();
  const fetch = vi.spyOn(api, 'fetchCommunityNodePolicies').mockRejectedValueOnce(new Error('offline'));
  const accept = vi.spyOn(api, 'acceptCommunityNodeConsents');
  const { result } = renderHook(() => useCommunityNodeConsentFlow({ api, configuredBaseUrls: [A, B] }));
  act(() => result.current.open(A));
  await waitFor(() => expect(result.current.dialog?.consent.loadError).toBe('offline'));
  act(() => result.current.dialog!.onRetry());
  await waitFor(() => expect(result.current.dialog?.consent.loaded).toBe(true));
  expect(fetch).toHaveBeenNthCalledWith(2, A, 'en');
  act(() => result.current.close());
  expect(result.current.dialog).toBeNull();
  expect(accept).not.toHaveBeenCalled();
});

test('late policy response cannot replace the newly selected node or language', async () => {
  const api = await setup();
  const original = await api.fetchCommunityNodePolicies(A);
  const old = createDeferred<CommunityNodePoliciesResponse>();
  vi.spyOn(api, 'fetchCommunityNodePolicies').mockReturnValueOnce(old.promise).mockImplementation(async (baseUrl, language) => ({
    policies: original.policies.map((policy) => ({ ...policy, body_markdown: `${baseUrl} ${language}` })),
  }));
  const { result } = renderHook(() => useCommunityNodeConsentFlow({ api, configuredBaseUrls: [A, B] }));
  act(() => result.current.open(A));
  act(() => result.current.open(B));
  await waitFor(() => expect(result.current.dialog?.consent.policies[0]?.body).toBe(`${B} en`));
  await act(async () => { await i18n.changeLanguage('ja'); });
  await waitFor(() => expect(result.current.dialog?.consent.policies[0]?.body).toBe(`${B} ja`));
  await act(async () => old.resolve(original));
  expect(result.current.dialog?.baseUrl).toBe(B);
  expect(result.current.dialog?.consent.policies[0]?.body).toBe(`${B} ja`);
});

test('submits the displayed snapshot once and keeps a failed acceptance in the dialog', async () => {
  const api = await setup();
  const original = await api.fetchCommunityNodePolicies(A);
  vi.spyOn(api, 'fetchCommunityNodePolicies').mockResolvedValue({ policies: original.policies.map((policy) => ({
    ...policy, policy_snapshot_revision: 'shown-revision',
  })) });
  const save = createDeferred<void>();
  const accept = vi.fn().mockReturnValueOnce(save.promise).mockResolvedValue(undefined);
  const { result } = renderHook(() => useCommunityNodeConsentFlow({ api, configuredBaseUrls: [A, B], acceptConsents: accept }));
  act(() => result.current.open(A));
  await waitFor(() => expect(result.current.dialog?.consent.loaded).toBe(true));
  act(() => { result.current.dialog!.onAccept(); result.current.dialog!.onAccept(); });
  await waitFor(() => expect(accept).toHaveBeenCalledTimes(1));
  expect(accept).toHaveBeenCalledWith(A, original.policies.map((policy) => ({
    policy_slug: policy.policy_slug, policy_version: policy.policy_version, policy_snapshot_revision: 'shown-revision',
  })), 'en');
  await act(async () => save.reject(new Error('storage unavailable')));
  expect(result.current.dialog?.error).toContain('Consent could not be completed');
  act(() => result.current.dialog!.onAccept());
  await waitFor(() => expect(result.current.dialog).toBeNull());
  expect(accept).toHaveBeenCalledTimes(2);
});

test('a node removed while its policies load cannot be accepted', async () => {
  const api = await setup();
  const original = await api.fetchCommunityNodePolicies(A);
  const deferred = createDeferred<CommunityNodePoliciesResponse>();
  vi.spyOn(api, 'fetchCommunityNodePolicies').mockReturnValue(deferred.promise);
  const accept = vi.spyOn(api, 'acceptCommunityNodeConsents');
  const { result, rerender } = renderHook(({ nodes }) => useCommunityNodeConsentFlow({ api, configuredBaseUrls: nodes }), {
    initialProps: { nodes: [A, B] },
  });
  act(() => result.current.open(A));
  rerender({ nodes: [B] });
  await act(async () => deferred.resolve(original));
  expect(result.current.dialog).toBeNull();
  expect(accept).not.toHaveBeenCalled();
});

test('a configuration change before submit blocks acceptance even before shell refresh', async () => {
  const api = await setup();
  const accept = vi.spyOn(api, 'acceptCommunityNodeConsents');
  const { result } = renderHook(() => useCommunityNodeConsentFlow({ api, configuredBaseUrls: [A, B] }));
  act(() => result.current.open(A));
  await waitFor(() => expect(result.current.dialog?.consent.loaded).toBe(true));
  await api.setCommunityNodeConfig([{ base_url: B }]);
  await act(async () => result.current.dialog!.onAccept());
  expect(accept).not.toHaveBeenCalled();
});

// #1192: 表示しない文書を同意記録にしない。観測提供(#1061)と権利侵害申請ポリシーは
// 一覧から外れるため、受諾 payload にも入ってはならない。
test('acceptance submits only the documents the dialog displayed', async () => {
  const api = await setup();
  const original = await api.fetchCommunityNodePolicies(A);
  const hidden = [
    { ...original.policies[0], policy_slug: 'trust_observation_sharing', policy_kind: 'trust_observation_sharing', title: 'Observation Sharing', required: false },
    { ...original.policies[0], policy_slug: 'rights_infringement', policy_kind: 'rights_infringement', title: 'Rights Request Policy', required: false },
  ];
  vi.spyOn(api, 'fetchCommunityNodePolicies').mockResolvedValue({
    policies: [
      ...original.policies.map((policy, index) => ({
        ...policy, policy_kind: index === 0 ? 'terms' : 'privacy',
      })),
      ...hidden,
    ],
  });
  const accept = vi.fn().mockResolvedValue(undefined);
  const { result } = renderHook(() => useCommunityNodeConsentFlow({ api, configuredBaseUrls: [A, B], acceptConsents: accept }));
  act(() => result.current.open(A));
  await waitFor(() => expect(result.current.dialog?.consent.loaded).toBe(true));
  const displayed = result.current.dialog!.consent.policies.map((policy) => policy.policySlug);
  expect(displayed).toEqual(['terms_of_service', 'privacy_policy']);
  await act(async () => result.current.dialog!.onAccept());
  expect(accept).toHaveBeenCalledWith(A, displayed.map((slug) => expect.objectContaining({ policy_slug: slug })), 'en');
  expect(accept.mock.calls[0][1]).toHaveLength(2);
});
