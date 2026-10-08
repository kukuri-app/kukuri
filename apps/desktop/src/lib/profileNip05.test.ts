import { expect, test } from 'vitest';

import { parseProfileNip05 } from './profileNip05';

// #1670: crates/core の `normalize_profile_nip05` の test と同じ例で、同じ形だけを受け付ける。
test('normalizes and splits an identifier', () => {
  expect(parseProfileNip05('  Alice_1.x-y@Sub.Example.COM ')).toEqual({
    identifier: 'alice_1.x-y@sub.example.com',
    name: 'alice_1.x-y',
    domain: 'sub.example.com',
  });
  expect(parseProfileNip05('_@xn--wgv71a119e.jp')?.domain).toBe('xn--wgv71a119e.jp');
});

test.each([
  '',
  'alice',
  '@example.com',
  'alice@',
  'alice@localhost',
  'a b@example.com',
  'alice+tag@example.com',
  'alice@192.0.2.1',
  'alice@example.com.',
  'alice@-example.com',
  'alice@exa_mple.com',
  'alice@例え.jp',
  'alice@bob@example.com',
  `${'a'.repeat(65)}@example.com`,
  `a@${Array(4).fill('a'.repeat(63)).join('.')}.com`,
])('rejects %s', (value) => {
  expect(parseProfileNip05(value)).toBeNull();
});
