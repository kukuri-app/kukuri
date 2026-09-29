import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, test, vi } from 'vitest';

import { createDesktopMockApi } from '@/mocks/desktopApiMock';
import { App } from '@/App';

import { buildImagePost, openSettingsSection, setViewportWidth } from './DesktopShellPage.testHelpers';

// #1056 / TR-10: 設定で採用を OFF にして保存すると、その node の採用設定が保存され、
// 以後そのノードへは照会しない。

beforeEach(() => {
  setViewportWidth(1024);
  window.history.replaceState(null, '', '/');
});

// #1416: 起動時の設定の読込は、CN 状態の取得・反映の完了を待たずに始める。
test('startup requests the node config while the node statuses are still pending', async () => {
  const api = createDesktopMockApi();
  const statuses = vi.spyOn(api, 'getCommunityNodeStatuses').mockReturnValue(new Promise(() => undefined));
  const config = vi.spyOn(api, 'getCommunityNodeConfig').mockReturnValue(new Promise(() => undefined));

  render(<App api={api} />);

  await waitFor(() => {
    expect(statuses).toHaveBeenCalled();
    expect(config).toHaveBeenCalled();
  });
});

test('turning adoption off and saving stores the setting and stops lookups', async () => {
  const user = userEvent.setup();
  const api = createDesktopMockApi({
    seedPosts: {
      'kukuri:topic:general': [
        buildImagePost({ content: 'caption', content_status: 'Available' }),
      ],
    },
  });
  const setCommunityNodeConfig = vi.spyOn(api, 'setCommunityNodeConfig');
  const lookup = vi.spyOn(api, 'lookupCommunityNodeContentAdvisories');

  render(<App api={api} />);
  await waitFor(() => {
    expect(lookup).toHaveBeenCalled();
  });

  await openSettingsSection(user, 'community-node');
  const drawer = screen.getByRole('dialog', { name: 'Settings' });
  const toggle = within(drawer).getByRole('checkbox', {
    name: "Use this node's estimates of adult material",
  });
  expect(toggle).toBeChecked();
  await user.click(toggle);
  expect(toggle).not.toBeChecked();
  await user.click(within(drawer).getByRole('button', { name: 'Save Nodes' }));

  await waitFor(() => {
    expect(setCommunityNodeConfig).toHaveBeenCalledWith([
      { base_url: 'https://api.kukuri.app', content_advisory_enabled: false },
    ]);
  });
  const config = await api.getCommunityNodeConfig();
  expect(config.nodes[0].content_advisory_enabled).toBe(false);

  const callsAfterSave = lookup.mock.calls.length;
  await new Promise((resolve) => setTimeout(resolve, 600));
  expect(lookup).toHaveBeenCalledTimes(callsAfterSave);
  expect(
    within(drawer).getByRole('checkbox', { name: "Use this node's estimates of adult material" })
  ).not.toBeChecked();
});
