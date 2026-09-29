import { expect, test } from 'vitest';

import {
  MEDIA_FETCH_LEDGER_LIMIT,
  MEDIA_MEMORY_BUDGET_BYTES,
  MEDIA_FETCH_MAX_AUTO_ATTEMPTS,
  MEDIA_FETCH_RETRY_DELAYS_MS,
  MEDIA_HIDDEN_RETAIN_BYTES,
  MEDIA_HIDDEN_RETAIN_LIMIT,
  MediaFetchLedger,
} from './mediaFetchLedger';

function exhaust(
  ledger: MediaFetchLedger,
  hash: string,
  startAt = 0,
  status = 'Missing'
): number {
  let now = startAt;
  for (;;) {
    const decision = ledger.decide(hash, status, now);
    if (decision.kind !== 'fetch') {
      throw new Error(`expected a fetch decision, got ${decision.kind}`);
    }
    const outcome = ledger.fail(hash, now);
    if (outcome.kind === 'exhausted') {
      return now;
    }
    now = outcome.retryAt;
  }
}

test('automatic attempts follow the fixed delays and stop at the limit', () => {
  const ledger = new MediaFetchLedger();
  const retryTimes: number[] = [];
  let now = 1_000;
  for (let attempt = 1; attempt <= MEDIA_FETCH_MAX_AUTO_ATTEMPTS; attempt += 1) {
    expect(ledger.decide('hash-a', 'Missing', now)).toEqual({ kind: 'fetch', attempt });
    const outcome = ledger.fail('hash-a', now);
    if (outcome.kind === 'retry') {
      retryTimes.push(outcome.retryAt - now);
      // 待ち時間の途中では取得しない。
      expect(ledger.decide('hash-a', 'Missing', outcome.retryAt - 1)).toEqual({
        kind: 'wait',
        retryAt: outcome.retryAt,
      });
      now = outcome.retryAt;
    }
  }
  expect(retryTimes).toEqual([...MEDIA_FETCH_RETRY_DELAYS_MS]);
  expect(ledger.isExhausted('hash-a')).toBe(true);
  expect(ledger.decide('hash-a', 'Missing', now + 3_600_000)).toEqual({ kind: 'skip' });
});

test('a hash in flight is never requested twice', () => {
  const ledger = new MediaFetchLedger();
  expect(ledger.decide('hash-a', 'Missing', 0).kind).toBe('fetch');
  expect(ledger.decide('hash-a', 'Missing', 0)).toEqual({ kind: 'skip' });
  expect(ledger.decide('hash-a', 'Available', 0)).toEqual({ kind: 'skip' });
});

test('an attachment that becomes available resets the exhausted record once', () => {
  const ledger = new MediaFetchLedger();
  const now = exhaust(ledger, 'hash-a');
  expect(ledger.decide('hash-a', 'Available', now)).toEqual({ kind: 'fetch', attempt: 1 });
  // 同じ `Available` のままでは、再び上限に達した後に取り直さない。
  const retry = ledger.fail('hash-a', now);
  const resumeAt = retry.kind === 'retry' ? retry.retryAt : now;
  const exhaustedAt = exhaust(ledger, 'hash-a', resumeAt, 'Available');
  expect(ledger.decide('hash-a', 'Available', exhaustedAt)).toEqual({ kind: 'skip' });
});

test('manual retry allows exactly one more attempt and is ignored while in flight', () => {
  const ledger = new MediaFetchLedger();
  const now = exhaust(ledger, 'hash-a');
  expect(ledger.requestManualRetry('hash-a')).toBe(true);
  expect(ledger.decide('hash-a', 'Missing', now)).toEqual({ kind: 'fetch', attempt: 1 });
  expect(ledger.requestManualRetry('hash-a')).toBe(false);
  expect(ledger.fail('hash-a', now)).toEqual({ kind: 'exhausted' });
  expect(ledger.decide('hash-a', 'Missing', now)).toEqual({ kind: 'skip' });
});

test('success forgets the record', () => {
  const ledger = new MediaFetchLedger();
  ledger.decide('hash-a', 'Missing', 0);
  ledger.succeed('hash-a');
  expect(ledger.size).toBe(0);
});

test('the ledger stays bounded and keeps entries in flight', () => {
  const ledger = new MediaFetchLedger();
  ledger.decide('in-flight', 'Missing', 0);
  for (let index = 0; index < MEDIA_FETCH_LEDGER_LIMIT + 50; index += 1) {
    const hash = `hash-${index}`;
    ledger.decide(hash, 'Missing', 0);
    ledger.fail(hash, 0);
  }
  expect(ledger.size).toBeLessThanOrEqual(MEDIA_FETCH_LEDGER_LIMIT);
  expect(ledger.isInFlight('in-flight')).toBe(true);
});

test('a full active ledger defers another hash and rejects oversized keys', () => {
  const ledger = new MediaFetchLedger();
  for (let index = 0; index < MEDIA_FETCH_LEDGER_LIMIT; index += 1) {
    expect(ledger.decide(`hash-${index}`, 'Missing', 0).kind).toBe('fetch');
  }
  expect(ledger.decide('new-hash', 'Missing', 0)).toEqual({ kind: 'skip' });
  expect(ledger.requestManualRetry('new-hash')).toBe(false);
  expect(ledger.size).toBe(MEDIA_FETCH_LEDGER_LIMIT);
  expect(ledger.decide('あ'.repeat(100), 'Missing', 0)).toEqual({ kind: 'skip' });
});

test('display demand owns reservations, cancellation, and URL release within 128 MiB', () => {
  const ledger = new MediaFetchLedger();
  const eightyMiB = 80 * 1024 * 1024;
  expect(ledger.decide('visible-a', 'Missing', 0, eightyMiB).kind).toBe('fetch');
  expect(ledger.decide('visible-b', 'Missing', 0, eightyMiB)).toEqual({ kind: 'skip' });
  let cancelled = 0;
  ledger.trackCancel('visible-a', () => { cancelled += 1; });
  ledger.forget('visible-a');
  expect(cancelled).toBe(1);
  expect(ledger.memoryBytes).toBe(0);

  expect(ledger.decide('visible-b', 'Missing', 0, eightyMiB).kind).toBe('fetch');
  let released = 0;
  ledger.succeed('visible-b', {
    url: 'blob:visible-b',
    memoryBytes: eightyMiB,
    release: () => { released += 1; },
  });
  expect(ledger.memoryBytes).toBe(eightyMiB);
  expect(ledger.decide('too-large', 'Missing', 0, MEDIA_MEMORY_BUDGET_BYTES - eightyMiB + 1)).toEqual({ kind: 'skip' });
  ledger.clear();
  expect(released).toBe(1);
  expect(ledger.memoryBytes).toBe(0);
});

// #1419: 画面外へ出た取得済みの画像は、最後に表示の対象だった順に上限まで残し、取得中は止める。
test('hidden releases keep the most recently shown images within the count and byte bounds', () => {
  const ledger = new MediaFetchLedger();
  const show = (hash: string, memoryBytes = 0) => {
    expect(ledger.decide(hash, 'Missing', 0).kind).toBe('fetch');
    ledger.succeed(hash, { url: `blob:${hash}`, memoryBytes, release: () => undefined });
  };
  const release = (visible: string[]) => {
    const released = ledger.hiddenReleases(new Set(visible));
    released.forEach((hash) => ledger.forget(hash));
    return released;
  };
  const hashes = Array.from({ length: MEDIA_HIDDEN_RETAIN_LIMIT + 2 }, (_, index) => `shown-${index}`);
  hashes.forEach((hash) => show(hash));
  expect(release(hashes)).toEqual([]);
  // 最初の 1 件は最後まで表示の対象だったので、画面外の中で最も新しい扱いになり、次に古い 1 件が解放される。
  expect(release([hashes[0]])).toEqual([hashes[1]]);
  expect(ledger.decide('pending', 'Missing', 0).kind).toBe('fetch');
  expect(release([])).toEqual(['pending', hashes[2]]);
  expect(ledger.decide(hashes[0], 'Missing', 0)).toEqual({ kind: 'skip' });

  const bounded = new MediaFetchLedger();
  const large = MEDIA_HIDDEN_RETAIN_BYTES / 2 + 1;
  expect(bounded.decide('old', 'Missing', 0).kind).toBe('fetch');
  bounded.succeed('old', { url: 'blob:old', memoryBytes: large, release: () => undefined });
  expect(bounded.decide('new', 'Missing', 0).kind).toBe('fetch');
  bounded.succeed('new', { url: 'blob:new', memoryBytes: large, release: () => undefined });
  expect(bounded.hiddenReleases(new Set())).toEqual(['old']);
});
