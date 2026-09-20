import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';

import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import { App } from '@/App';
import { openControlCenter, setViewportWidth } from './DesktopShellPage.testHelpers';

const DEFAULT_NODE = 'https://api.kukuri.app';
const SECOND_NODE = 'https://second.example';

beforeEach(() => {
  setViewportWidth(1024);
  window.history.replaceState(null, '', '/');
});

afterEach(() => {
  vi.useRealTimers();
});

/// 既定 mock は 1 ノードだけ同意済み。2 ノード目も受諾して初回起動の案内を出さない状態にする。
async function withTwoConsentedNodes() {
  const api = createDesktopMockApi();
  await api.setCommunityNodeConfig([{ base_url: DEFAULT_NODE }, { base_url: SECOND_NODE }]);
  const catalog = await api.fetchCommunityNodePolicies(SECOND_NODE, 'en');
  await api.acceptCommunityNodeConsents(
    SECOND_NODE,
    catalog.policies.map((policy) => ({
      policy_slug: policy.policy_slug,
      policy_version: policy.policy_version,
      policy_snapshot_revision: policy.policy_snapshot_revision ?? null,
    })),
    'en'
  );
  return api;
}

// #1192: 設定画面へ行かずに、コントロールセンターのコミュニティノード欄から
// 各ノードの規約同意モーダルを開ける。開く対象は行ごとに固定する。
test('Control Center lists each Community Node with its consent state and opens its policies', async () => {
  const user = userEvent.setup();
  const api = await withTwoConsentedNodes();
  const fetchPolicies = vi.spyOn(api, 'fetchCommunityNodePolicies');
  render(<App api={api} />);

  const controlCenter = await openControlCenter(user);
  const list = within(controlCenter).getByRole('list', { name: 'Configured Community Nodes' });
  const rows = within(list).getAllByRole('listitem');
  expect(rows).toHaveLength(2);
  expect(within(rows[0]).getByText(DEFAULT_NODE)).toBeInTheDocument();
  expect(within(rows[0]).getByText('accepted')).toBeInTheDocument();
  expect(within(rows[1]).getByText(SECOND_NODE)).toBeInTheDocument();

  await user.click(
    within(list).getByRole('button', {
      name: `Open policies and consent state for ${SECOND_NODE}`,
    })
  );

  const dialog = await screen.findByRole('dialog', { name: 'Community node policies' });
  expect(within(dialog).getByText(SECOND_NODE)).toBeInTheDocument();
  await waitFor(() => expect(fetchPolicies).toHaveBeenCalledWith(SECOND_NODE, 'en'));
  // 行ごとに対象を固定するため、他ノードのカタログは取得しない。
  expect(fetchPolicies).not.toHaveBeenCalledWith(DEFAULT_NODE, expect.anything());
});

test('Control Center says so when no Community Node is configured', async () => {
  const user = userEvent.setup();
  const api = createDesktopMockApi();
  await api.setCommunityNodeConfig([]);
  render(<App api={api} />);

  const controlCenter = await openControlCenter(user);
  expect(within(controlCenter).getByText('No Community Node is configured.')).toBeInTheDocument();
  expect(
    within(controlCenter).queryByRole('list', { name: 'Configured Community Nodes' })
  ).not.toBeInTheDocument();
});

// 規約モーダルを閉じたら、開いた行のボタンへ focus が戻る(#1108 と同じ扱い)。
test('closing the policies dialog returns focus to the Community Node row that opened it', async () => {
  const user = userEvent.setup();
  const api = await withTwoConsentedNodes();
  render(<App api={api} />);

  const controlCenter = await openControlCenter(user);
  const open = within(controlCenter).getByRole('button', {
    name: `Open policies and consent state for ${SECOND_NODE}`,
  });
  await user.click(open);

  const dialog = await screen.findByRole('dialog', { name: 'Community node policies' });
  await user.click(within(dialog).getByRole('button', { name: 'Close' }));

  await waitFor(() => expect(open).toHaveFocus());
});

// Escape は手前のモーダルだけを閉じる。コントロールセンターまで畳むと、規約を読み終えた
// 位置へ戻れなくなる。
test('Escape closes the policies dialog without also closing the Control Center', async () => {
  const user = userEvent.setup();
  const api = await withTwoConsentedNodes();
  render(<App api={api} />);

  const controlCenter = await openControlCenter(user);
  const open = within(controlCenter).getByRole('button', {
    name: `Open policies and consent state for ${SECOND_NODE}`,
  });
  await user.click(open);
  await screen.findByRole('dialog', { name: 'Community node policies' });

  await user.keyboard('{Escape}');

  await waitFor(() =>
    expect(screen.queryByRole('dialog', { name: 'Community node policies' })).not.toBeInTheDocument()
  );
  expect(screen.getByRole('complementary', { name: 'Control Center' })).toBeVisible();
  await waitFor(() => expect(open).toHaveFocus());
});
