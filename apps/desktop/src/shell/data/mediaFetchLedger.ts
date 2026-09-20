/// #1207: メディア取得の試行台帳。hash 単位で自動取得の回数と間隔を管理する。
///
/// 1 試行 = `getBlobMediaPayload` 1 回(Rust 側では peer 走査 1 系列)。台帳は shell の data 層に
/// 1 つだけ置き、component の mount や state の参照の変化とは無関係に回数を数える。
/// リセットするのは、利用者の明示再試行、attachment の status が `Available` へ変わったとき、
/// 成人向け gate の切替、api の差し替え(=別の backend)だけとする。

/// 自動取得の最大試行数(初回 + 再試行 2 回)。
export const MEDIA_FETCH_MAX_AUTO_ATTEMPTS = 3;
/// n 回目の失敗から次の試行までの待ち時間。
export const MEDIA_FETCH_RETRY_DELAYS_MS: readonly number[] = [5_000, 30_000];
/// 実際に参照する待ち時間。表示の結合 test が実時間を待たずに上限到達を再現するためだけに差し替える。
export const mediaFetchRetryPolicy: { retryDelaysMs: readonly number[] } = {
  retryDelaysMs: MEDIA_FETCH_RETRY_DELAYS_MS,
};
/// 利用者の明示再試行は 1 回だけ試し、結果をすぐ返す。
export const MEDIA_FETCH_MANUAL_ATTEMPTS = 1;
/// 台帳の上限。超えた分は、取得中でない古い項目から捨てる。
export const MEDIA_FETCH_LEDGER_LIMIT = 2_000;

type LedgerEntry = {
  attempts: number;
  maxAttempts: number;
  inFlight: boolean;
  nextRetryAt: number | null;
  exhausted: boolean;
  lastStatus: string | null;
};

export type MediaFetchDecision =
  | { kind: 'fetch'; attempt: number }
  | { kind: 'wait'; retryAt: number }
  | { kind: 'skip' };

export type MediaFetchFailureOutcome =
  | { kind: 'retry'; retryAt: number }
  | { kind: 'exhausted' };

export class MediaFetchLedger {
  private readonly entries = new Map<string, LedgerEntry>();

  /// この hash を今取得してよいかを決める。`fetch` を返したときは試行を 1 回消費し、取得中にする。
  decide(hash: string, status: string | null, now: number): MediaFetchDecision {
    let entry = this.entries.get(hash);
    if (entry && !entry.inFlight && status === 'Available' && entry.lastStatus !== 'Available') {
      // backend がローカルに揃ったと報告した。失敗の記録を引き継がずに取り直す。
      this.entries.delete(hash);
      entry = undefined;
    }
    if (!entry) {
      entry = {
        attempts: 0,
        maxAttempts: MEDIA_FETCH_MAX_AUTO_ATTEMPTS,
        inFlight: false,
        nextRetryAt: null,
        exhausted: false,
        lastStatus: status,
      };
      this.entries.set(hash, entry);
      this.prune();
    }
    entry.lastStatus = status;
    if (entry.inFlight || entry.exhausted) {
      return { kind: 'skip' };
    }
    if (entry.nextRetryAt !== null && entry.nextRetryAt > now) {
      return { kind: 'wait', retryAt: entry.nextRetryAt };
    }
    entry.attempts += 1;
    entry.inFlight = true;
    entry.nextRetryAt = null;
    // 挿入順を「最後に試行した順」に保つ(上限超過時に古いものから捨てるため)。
    this.entries.delete(hash);
    this.entries.set(hash, entry);
    return { kind: 'fetch', attempt: entry.attempts };
  }

  succeed(hash: string): void {
    this.entries.delete(hash);
  }

  fail(hash: string, now: number): MediaFetchFailureOutcome {
    const entry = this.entries.get(hash);
    if (!entry) {
      return { kind: 'exhausted' };
    }
    entry.inFlight = false;
    if (entry.attempts >= entry.maxAttempts) {
      entry.exhausted = true;
      entry.nextRetryAt = null;
      return { kind: 'exhausted' };
    }
    const delays = mediaFetchRetryPolicy.retryDelaysMs;
    const delay = delays[Math.min(entry.attempts - 1, delays.length - 1)] ?? 0;
    entry.nextRetryAt = now + delay;
    return { kind: 'retry', retryAt: entry.nextRetryAt };
  }

  /// 利用者の明示再試行。取得中なら何もしない(重複実行を防ぐ)。
  requestManualRetry(hash: string): boolean {
    const entry = this.entries.get(hash);
    if (entry?.inFlight) {
      return false;
    }
    this.entries.set(hash, {
      attempts: 0,
      maxAttempts: MEDIA_FETCH_MANUAL_ATTEMPTS,
      inFlight: false,
      nextRetryAt: null,
      exhausted: false,
      lastStatus: entry?.lastStatus ?? null,
    });
    return true;
  }

  /// gate の切替などで、この hash の記録を捨てる。取得中の結果は呼出元が epoch で無効にする。
  forget(hash: string): void {
    this.entries.delete(hash);
  }

  clear(): void {
    this.entries.clear();
  }

  isInFlight(hash: string): boolean {
    return this.entries.get(hash)?.inFlight ?? false;
  }

  isExhausted(hash: string): boolean {
    return this.entries.get(hash)?.exhausted ?? false;
  }

  get size(): number {
    return this.entries.size;
  }

  private prune(): void {
    if (this.entries.size <= MEDIA_FETCH_LEDGER_LIMIT) {
      return;
    }
    for (const [hash, entry] of this.entries) {
      if (this.entries.size <= MEDIA_FETCH_LEDGER_LIMIT) {
        return;
      }
      if (!entry.inFlight) {
        this.entries.delete(hash);
      }
    }
  }
}
