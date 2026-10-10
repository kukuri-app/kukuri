import { expect, test, vi } from 'vitest';

const activeListeners = vi.hoisted(() => new Set<object>());
vi.mock('@tauri-apps/api/app', () => ({
  onBackButtonPress: vi.fn(async () => {
    const listener = { unregister: vi.fn(async () => { activeListeners.delete(listener); }) };
    activeListeners.add(listener);
    return listener;
  }),
}));

import { listenBackButton } from './androidBackButton';

test('ending each document releases its back subscription before the next document registers', async () => {
  for (let document = 0; document < 2; document += 1) {
    await listenBackButton();
    expect(activeListeners.size).toBe(1);
    window.dispatchEvent(new Event('pagehide'));
    expect(activeListeners.size).toBe(0);
  }
});
