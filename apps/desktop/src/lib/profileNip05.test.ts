import { afterEach, expect, test, vi } from 'vitest';

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

const PUBKEY = 'a'.repeat(64);

vi.mock('./api/invoke/desktop', () => ({ invokeDesktop: vi.fn() }));

/// 保持と待ちは module ごとなので、test ごとに読み直す。
async function load() {
  vi.resetModules();
  return import('./profileNip05');
}

function document(names: Record<string, string>): Response {
  return new Response(JSON.stringify({ names }));
}

afterEach(() => {
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

// #1670 AC-2.1・2.4: Web は cookie・Referer を送らず転送をエラーにする fetch で照会し、同じ (公開鍵, 識別子) は保持中に再送しない。
test('verifies with a cookie-less fetch and keeps a match for 10 minutes', async () => {
  vi.useFakeTimers({ toFake: ['Date'] });
  const fetch = vi.fn(async () => document({ alice: PUBKEY }));
  vi.stubGlobal('fetch', fetch);
  const { verifyProfileNip05 } = await load();

  expect(await verifyProfileNip05(PUBKEY, ' Alice@Example.com ')).toBe('example.com');
  expect(fetch).toHaveBeenCalledWith(
    'https://example.com/.well-known/nostr.json?name=alice',
    expect.objectContaining({ credentials: 'omit', redirect: 'error', referrerPolicy: 'no-referrer' })
  );
  vi.advanceTimersByTime(10 * 60 * 1000 - 1);
  expect(await verifyProfileNip05(PUBKEY, 'alice@example.com')).toBe('example.com');
  expect(fetch).toHaveBeenCalledTimes(1);
  vi.advanceTimersByTime(1);
  await verifyProfileNip05(PUBKEY, 'alice@example.com');
  expect(fetch).toHaveBeenCalledTimes(2);
});

// #1670 AC-2.1・2.2・2.4: 別の鍵・取得の失敗(CORS など)・非 2xx・512 KiB を超える応答は確認できず、60 秒後に照会し直す。
test.each([
  ['another key', async () => document({ alice: 'b'.repeat(64) })],
  ['a fetch failure', async () => Promise.reject(new TypeError('Failed to fetch'))],
  ['an HTTP error', async () => new Response('', { status: 404 })],
  ['an oversized document', async () => new Response(`{"names":{"alice":"${PUBKEY}"}}${' '.repeat(512 * 1024)}`)],
])('does not verify %s and asks again after 60 seconds', async (_label, respond) => {
  vi.useFakeTimers({ toFake: ['Date'] });
  const fetch = vi.fn(respond);
  vi.stubGlobal('fetch', fetch);
  const { verifyProfileNip05 } = await load();

  expect(await verifyProfileNip05(PUBKEY, 'alice@example.com')).toBeNull();
  vi.advanceTimersByTime(60 * 1000 - 1);
  expect(await verifyProfileNip05(PUBKEY, 'alice@example.com')).toBeNull();
  expect(fetch).toHaveBeenCalledTimes(1);
  vi.advanceTimersByTime(1);
  await verifyProfileNip05(PUBKEY, 'alice@example.com');
  expect(fetch).toHaveBeenCalledTimes(2);
});

test('does not ask for an identifier that is not name@domain', async () => {
  const fetch = vi.fn();
  vi.stubGlobal('fetch', fetch);
  const { verifyProfileNip05 } = await load();
  for (const value of ['alice', 'alice@localhost', 'alice@192.0.2.1']) {
    expect(await verifyProfileNip05(PUBKEY, value)).toBeNull();
  }
  expect(fetch).not.toHaveBeenCalled();
});

// #1670 AC-2.2: 同時 4 件・待ち 32 件まで。超えた分は照会せず結果も持たないので、次の表示で照会する。
test('runs 4 at a time with 32 waiting and drops the rest without keeping it', async () => {
  const pending: Array<() => void> = [];
  const fetch = vi.fn(
    (url: string) =>
      new Promise<Response>((resolve) =>
        pending.push(() => resolve(document({ [new URL(url).searchParams.get('name')!]: PUBKEY })))
      )
  );
  vi.stubGlobal('fetch', fetch);
  const { verifyProfileNip05 } = await load();

  const results = Array.from({ length: 37 }, (_, index) =>
    verifyProfileNip05(PUBKEY, `user${index}@example.com`)
  );
  expect(await results[36]).toBeNull();
  await vi.waitFor(() => expect(fetch).toHaveBeenCalledTimes(4));
  while (fetch.mock.calls.length < 36 || pending.length > 0) {
    pending.shift()?.();
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(pending.length).toBeLessThanOrEqual(4);
  }
  expect(await Promise.all(results.slice(0, 36))).toEqual(Array(36).fill('example.com'));
  expect(fetch).toHaveBeenCalledTimes(36);
  const retried = verifyProfileNip05(PUBKEY, 'user36@example.com');
  await vi.waitFor(() => expect(fetch).toHaveBeenCalledTimes(37));
  pending.shift()?.();
  expect(await retried).toBe('example.com');
});

// #1670 AC-2.3: デスクトップは Rust の command で照会し、ブラウザから取得しない。
test('the desktop asks the native command instead of fetching', async () => {
  const fetch = vi.fn();
  vi.stubGlobal('fetch', fetch);
  vi.stubGlobal('__TAURI_INTERNALS__', {});
  const { verifyProfileNip05 } = await load();
  const { invokeDesktop } = await import('./api/invoke/desktop');
  vi.mocked(invokeDesktop).mockResolvedValue(true);

  expect(await verifyProfileNip05(PUBKEY, 'Alice@Example.com')).toBe('example.com');
  expect(invokeDesktop).toHaveBeenCalledWith('verify_profile_domain', {
    pubkey: PUBKEY,
    identifier: 'alice@example.com',
  });
  expect(fetch).not.toHaveBeenCalled();
});
