import { act, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, test, vi } from 'vitest';
import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import { upsertCommunityNodeStatus } from '@/shell/presentation';
import { createDesktopShellStore, DesktopShellStoreContext } from '@/shell/store';
import { CommunityNodeOnboarding } from './CommunityNodeOnboarding';

const A = 'https://first.example';
const B = 'https://second.example';

// #1420: 同意済みの 2 Node。公開 catalog は版が 1 つ上がっている(規約の更新)。
async function setup() {
  const api = createDesktopMockApi();
  await api.setCommunityNodeConfig([{ base_url: A }, { base_url: B }]);
  const policies = (await api.fetchCommunityNodePolicies(A)).policies;
  const statuses = [
    await api.acceptCommunityNodeConsents(A, policies, 'en'),
    await api.acceptCommunityNodeConsents(B, policies, 'en'),
  ];
  vi.spyOn(api, 'fetchCommunityNodePolicies').mockImplementation(async () => ({
    policies: policies.map((policy) => ({ ...policy, policy_version: policy.policy_version + 1 })),
  }));
  const store = createDesktopShellStore();
  store.getState().patchState({
    communityNodeConfig: await api.getCommunityNodeConfig(), communityNodeConfigLoaded: true,
    communityNodeStatuses: statuses, communityNodeStatusesLoaded: true,
    syncStatus: { ...store.getState().syncStatus, local_author_pubkey: 'a'.repeat(64) },
  });
  // 実際の受諾処理と同じく、受諾後の status を store へ反映する。
  const onAccept = vi.fn(async (baseUrl: string, documents: Parameters<typeof api.acceptCommunityNodeConsents>[1], language: string) => {
    const status = await api.acceptCommunityNodeConsents(baseUrl, documents, language);
    store.getState().patchState({ communityNodeStatuses: upsertCommunityNodeStatus(store.getState().communityNodeStatuses, status) });
  });
  const setPending = (...baseUrls: string[]) => act(() => {
    store.getState().patchState({
      communityNodeStatuses: store.getState().communityNodeStatuses.map((status) => ({
        ...status, consent_update_pending: baseUrls.includes(status.base_url),
      })),
    });
  });
  render(
    <DesktopShellStoreContext.Provider value={store}>
      <CommunityNodeOnboarding api={api} onAccept={onAccept} onOpenSettings={() => {}} onRetry={async () => {}} />
    </DesktopShellStoreContext.Provider>
  );
  return { onAccept, setPending, user: userEvent.setup() };
}

const policiesDialog = () => screen.findByRole('dialog', { name: 'Community node policies' });
const settle = () => act(async () => { await new Promise((resolve) => setTimeout(resolve, 20)); });

test('opens the policies dialog when an accepted node reports a policy update', async () => {
  const { onAccept, setPending } = await setup();
  await settle();
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  setPending(A);
  const dialog = await policiesDialog();
  expect(within(dialog).getByText(A)).toBeInTheDocument();
  expect(await within(dialog).findByText(/This node updated its policies/)).toBeInTheDocument();
  expect(onAccept).not.toHaveBeenCalled();
});

test('waits until another dialog closes', async () => {
  const { setPending } = await setup();
  const other = document.createElement('div');
  other.setAttribute('role', 'dialog');
  document.body.append(other);
  setPending(A);
  await settle();
  expect(screen.queryByRole('dialog', { name: 'Community node policies' })).not.toBeInTheDocument();
  act(() => other.remove());
  expect(within(await policiesDialog()).getByText(A)).toBeInTheDocument();
});

test('a closed prompt stays closed for the same update and returns for the next one', async () => {
  const { onAccept, setPending, user } = await setup();
  setPending(A);
  await user.click(within(await policiesDialog()).getByRole('button', { name: 'Not now' }));
  await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
  setPending(A);
  await settle();
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  expect(onAccept).not.toHaveBeenCalled();
  setPending();
  setPending(A);
  expect(within(await policiesDialog()).getByText(A)).toBeInTheDocument();
});

test('accepting passes the displayed documents once and closes the prompt', async () => {
  const { onAccept, setPending, user } = await setup();
  setPending(A);
  const accept = within(await policiesDialog()).getByRole('button', { name: 'Accept' });
  await waitFor(() => expect(accept).toBeEnabled());
  await user.click(accept);
  await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
  await settle();
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  expect(onAccept).toHaveBeenCalledExactlyOnceWith(A, expect.any(Array), 'en');
  const documents = onAccept.mock.calls[0][1];
  expect(documents.length).toBeGreaterThan(0);
  expect(documents.every((document) => document.policy_version === 2)).toBe(true);
});

test('several updated nodes are prompted one at a time in list order', async () => {
  const { setPending, user } = await setup();
  setPending(A, B);
  const first = await policiesDialog();
  expect(within(first).getByText(A)).toBeInTheDocument();
  expect(screen.getAllByRole('dialog')).toHaveLength(1);
  await user.click(within(first).getByRole('button', { name: 'Not now' }));
  await waitFor(() => expect(within(screen.getByRole('dialog', { name: 'Community node policies' })).getByText(B)).toBeInTheDocument());
});
