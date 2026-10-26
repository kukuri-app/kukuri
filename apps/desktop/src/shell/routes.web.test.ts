import { expect, test, vi } from 'vitest';

import { primarySectionHidden } from './routes';

vi.mock('@/lib/webRuntime', () => ({ IS_WEB_RUNTIME: true }));

// WebでもLiveは通常機能とし、gameは未対応のまま維持する。
test('the web runtime shows live regardless of developer mode and never shows game', () => {
  expect(primarySectionHidden('live', false)).toBe(false);
  expect(primarySectionHidden('live', true)).toBe(false);
  expect(primarySectionHidden('game', true)).toBe(true);
  expect(primarySectionHidden('timeline', false)).toBe(false);
});
