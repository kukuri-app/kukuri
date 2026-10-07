import { act, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, test, vi } from 'vitest';

import { DiscoveryPanel } from './DiscoveryPanel';
import { createDiscoveryPanelFixture } from './fixtures';
import type { DiscoveryPanelView } from './types';

// #1632: Web の build（ADR 0060）は DHT を使えないので、公開コンテンツの発見の切替を出さない。
const runtime = vi.hoisted(() => ({ web: false }));
vi.mock('@/lib/webRuntime', () => ({
  get IS_WEB_RUNTIME() {
    return runtime.web;
  },
}));

afterEach(() => {
  runtime.web = false;
});

function renderToggle(
  view: Partial<DiscoveryPanelView> = {},
  onPublicBlobDiscoveryChange: (enabled: boolean) => Promise<void> = async () => {}
) {
  render(
    <DiscoveryPanel
      view={{ ...createDiscoveryPanelFixture(), ...view }}
      saveDisabled={false}
      resetDisabled={false}
      onSeedPeersChange={() => {}}
      onSave={() => {}}
      onReset={() => {}}
      onPublicBlobDiscoveryChange={onPublicBlobDiscoveryChange}
    />
  );
  return screen.queryByRole('checkbox', { name: /Public content discovery/ });
}

test.each([true, false])('the public content discovery toggle shows the saved value (%s)', (saved) => {
  const toggle = renderToggle({ publicBlobDiscovery: saved });

  expect(toggle).toHaveProperty('checked', saved);
  expect(toggle).toBeEnabled();
  expect(toggle).toHaveAccessibleDescription(/Mainline DHT/);
});

test('switching shows the requested value and ignores the toggle until the switch ends', async () => {
  const user = userEvent.setup();
  let finish = () => {};
  const onChange = vi.fn(() => new Promise<void>((resolve) => { finish = resolve; }));
  const toggle = renderToggle({}, onChange)!;

  await user.click(toggle);
  await user.click(toggle);

  expect(onChange).toHaveBeenCalledOnce();
  expect(onChange).toHaveBeenCalledWith(false);
  expect(toggle).not.toBeChecked();
  expect(toggle).toHaveFocus();
  expect(toggle).toHaveAttribute('aria-disabled', 'true');
  expect(toggle).toHaveAttribute('aria-busy', 'true');
  expect(toggle).toHaveAccessibleName('Public content discovery Switching…');

  // 保存済みの値が変わらないまま終われば（失敗）、元の値へ戻って再び操作できる。
  await act(async () => finish());
  expect(toggle).toBeChecked();
  expect(toggle).not.toHaveAttribute('aria-disabled');
  expect(toggle).toHaveAccessibleName('Public content discovery');
  await user.click(toggle);
  expect(onChange).toHaveBeenCalledTimes(2);
});

test('an environment-locked discovery configuration cannot be switched', () => {
  expect(renderToggle({ envLocked: true })).toBeDisabled();
});

test('the web client does not show the toggle', () => {
  runtime.web = true;

  expect(renderToggle()).toBeNull();
  expect(screen.queryByText(/Mainline DHT/)).not.toBeInTheDocument();
});
