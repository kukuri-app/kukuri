import { expect, test, vi } from 'vitest';

import { primarySectionHidden } from './routes';

vi.mock('@/lib/webRuntime', () => ({ IS_WEB_RUNTIME: true }));

// #1879: Web でも live は開発者モードの間だけ出し、game は出さない(ADR 0060 §3)。
test('the web runtime shows live only in developer mode and never shows game', () => {
  expect(primarySectionHidden('live', false)).toBe(true);
  expect(primarySectionHidden('live', true)).toBe(false);
  expect(primarySectionHidden('game', true)).toBe(true);
  expect(primarySectionHidden('timeline', false)).toBe(false);
});
