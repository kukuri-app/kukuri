import { expect, test } from 'vitest';

import {
  MEDIA_FETCH_LEDGER_LIMIT,
  MEDIA_FETCH_MAX_AUTO_ATTEMPTS,
  MEDIA_FETCH_RETRY_DELAYS_MS,
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
